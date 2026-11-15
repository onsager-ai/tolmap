//! Headless access to the local service's committed snapshot store.
use super::*;
use crate::query_contract::{
    MapIdentity, QueryCoverage, QueryEnvelope, ReferenceMode, QUERY_SCHEMA,
};
use clap::Args;
use sha2::{Digest, Sha256};

#[derive(Debug, Args)]
pub struct StatusArgs {
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    /// Share an existing `tolmap local --data` store.
    #[arg(long)]
    pub data: Option<PathBuf>,
    #[arg(long, default_value = "json", value_parser = ["json", "text"])]
    pub format: String,
}

#[derive(Debug, Args)]
pub struct RefreshArgs {
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    #[arg(long)]
    pub data: Option<PathBuf>,
    #[arg(long, conflicts_with_all = ["pkg", "lang"])]
    pub all_sources: bool,
    #[arg(long)]
    pub pkg: Vec<String>,
    #[arg(long, value_parser = ["py", "go", "ts", "rs"])]
    pub lang: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, ts_rs::TS)]
pub struct Status {
    pub checkout_commit: String,
    pub indexed_commit: Option<String>,
    pub requested_commit: Option<String>,
    pub freshness: String,
    pub commits_behind: Option<u64>,
    pub error: Option<String>,
    pub dirty: bool,
    pub changed_files: usize,
    pub sources: Vec<(String, String)>,
    pub all_sources: Option<bool>,
    #[ts(type = "unknown | null")]
    pub extraction_cache: Option<serde_json::Value>,
    pub previous_map_reused: Option<bool>,
    pub map_path: Option<PathBuf>,
}

pub type StatusResponse = QueryEnvelope<Status, Option<MapIdentity>>;

#[derive(Debug)]
struct InputError(String);
impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for InputError {}

fn paths(repo: &Path, data: Option<&Path>) -> Result<(PathBuf, PathBuf)> {
    let output = checked(command(repo, &["rev-parse", "--show-toplevel"]))?;
    let checkout = PathBuf::from(String::from_utf8(output)?.trim()).canonicalize()?;
    checked(command(&checkout, &["rev-parse", "HEAD^{commit}"]))?;
    let data = match data {
        Some(path) => path.to_owned(),
        None => {
            let base = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
                .context("set HOME, XDG_DATA_HOME or --data")?;
            base.join("tolmap/local").join(format!(
                "{:x}",
                Sha256::digest(checkout.as_os_str().as_encoded_bytes())
            ))
        }
    };
    let data = if data.exists() {
        data.canonicalize()?
    } else if data.is_relative() {
        std::env::current_dir()?.join(data)
    } else {
        data
    };
    Ok((checkout, data))
}

fn read_optional<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn state(checkout: PathBuf, data: PathBuf, create: bool) -> Result<Local> {
    if create {
        private(&data)?;
        ensure!(
            !data.canonicalize()?.starts_with(&checkout),
            "private storage must be outside the checkout"
        );
        private(&data.join("snapshots"))?;
    }
    let identity = read_optional::<Identity>(&data.join("identity.json"))?;
    let repository = if let Some(identity) = identity {
        ensure!(
            identity.checkout == checkout,
            InputError("storage belongs to another repository".into())
        );
        identity.repository
    } else {
        let repository = Uuid::new_v4().to_string();
        if create {
            crate::incremental::atomic_write(
                &data.join("identity.json"),
                &serde_json::to_vec(&Identity {
                    checkout: checkout.clone(),
                    repository: repository.clone(),
                })?,
            )?;
        }
        repository
    };
    let attempt = read_optional(&data.join("attempt.json"))?;
    Ok(Local {
        checkout,
        data,
        repository,
        assets: PathBuf::new(),
        origin: String::new(),
        prepared: Mutex::new(None),
        job: Mutex::new(None),
        notice: Mutex::new(String::new()),
        attempt: Mutex::new(attempt),
    })
}

pub(super) fn report(s: &Local) -> Result<StatusResponse> {
    let checkout_commit = String::from_utf8(checked(command(
        &s.checkout,
        &["rev-parse", "HEAD^{commit}"],
    ))?)?
    .trim()
    .to_owned();
    let dirty = checked(command(
        &s.checkout,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    ))?;
    // Rename/copy entries have a second NUL-delimited path, not a second file.
    let mut records = dirty.split(|b| *b == 0).filter(|r| !r.is_empty());
    let mut changed_files = 0;
    while let Some(record) = records.next() {
        changed_files += 1;
        if record.iter().take(2).any(|b| matches!(b, b'R' | b'C')) {
            records.next();
        }
    }
    let latest = if s.data.join("snapshots").exists() {
        snapshots(s)?.pop()
    } else {
        None
    };
    let mut attempt =
        read_optional::<crate::schema::SnapshotFreshness>(&s.data.join("attempt.json"))?;
    if let Some(attempt) = attempt.as_mut() {
        if matches!(
            attempt.state,
            crate::schema::FreshnessState::Queued | crate::schema::FreshnessState::Indexing
        ) && writer_stopped(&s.data)?
        {
            attempt.state = crate::schema::FreshnessState::Failed;
            attempt.failure_reason = Some("Indexing was interrupted; retry refresh.".into());
        }
    }
    let indexed_commit = latest.as_ref().map(|p| p.commit.clone());
    let requested_commit = attempt
        .as_ref()
        .and_then(|a| a.requested_commit.clone())
        .or_else(|| indexed_commit.clone());
    let freshness = match attempt.as_ref().map(|a| a.state) {
        Some(crate::schema::FreshnessState::Queued | crate::schema::FreshnessState::Indexing) => {
            "indexing"
        }
        Some(crate::schema::FreshnessState::Failed) => "failed",
        _ if indexed_commit.as_deref() == Some(&checkout_commit) => "fresh",
        _ if latest.is_some() => "stale",
        _ => "none",
    };
    let commits_behind = indexed_commit
        .as_ref()
        .filter(|c| *c != &checkout_commit)
        .and_then(|commit| {
            let ancestor = command(
                &s.checkout,
                &["merge-base", "--is-ancestor", commit, &checkout_commit],
            )
            .status()
            .ok()?;
            if !ancestor.success() {
                return None;
            }
            String::from_utf8(
                checked(command(
                    &s.checkout,
                    &[
                        "rev-list",
                        "--count",
                        &format!("{commit}..{checkout_commit}"),
                    ],
                ))
                .ok()?,
            )
            .ok()?
            .trim()
            .parse()
            .ok()
        });
    let mut map = None;
    let mut coverage = QueryCoverage {
        refs: ReferenceMode::Hand,
        zero_edge_files: None,
        calls_total: None,
        calls_resolved: None,
        unresolved: None,
        lower_bound: true,
    };
    let mut sources = vec![];
    let mut all_sources = None;
    let mut extraction_cache = None;
    let mut previous_map_reused = None;
    let map_path = latest.as_ref().map(|p| {
        s.data
            .join("snapshots")
            .join(p.id.to_string())
            .join("map.json")
    });
    if let Some(path) = &map_path {
        let snapshot = crate::query::Snapshot::load(path)?;
        let overview = snapshot.overview(0);
        coverage = overview.coverage;
        map = Some(snapshot.identity);
        let metadata =
            read_optional::<serde_json::Value>(&path.parent().unwrap().join("index.json"))?;
        if let Some(metadata) = metadata {
            extraction_cache = metadata.get("extraction").cloned();
            previous_map_reused = metadata.get("prior_snapshot").map(|v| !v.is_null());
            sources =
                serde_json::from_value(metadata.get("sources").cloned().unwrap_or(json!([])))?;
            all_sources = metadata.get("all_sources").and_then(|v| v.as_bool());
            // Old index records predate explicit source pairs; observations still name their scope.
            if sources.is_empty() {
                if let Some(rows) = extraction_cache.as_ref().and_then(|v| v.as_array()) {
                    sources = rows
                        .iter()
                        .filter_map(|r| {
                            Some((
                                r.get("source")?.as_str()?.into(),
                                r.get("language")?.as_str()?.into(),
                            ))
                        })
                        .collect();
                }
            }
        }
    }
    Ok(QueryEnvelope {
        schema: QUERY_SCHEMA.into(),
        query: crate::query_contract::Query::Status {},
        map,
        coverage,
        limit: 1,
        total: 1,
        truncated: false,
        results: vec![Status {
            checkout_commit,
            indexed_commit,
            requested_commit,
            freshness: freshness.into(),
            commits_behind,
            error: attempt.and_then(|a| a.failure_reason),
            dirty: changed_files > 0,
            changed_files,
            sources,
            all_sources,
            extraction_cache,
            previous_map_reused,
            map_path,
        }],
    })
}

pub fn status(args: StatusArgs) -> i32 {
    let (checkout, data) = match paths(&args.repo, args.data.as_deref()) {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("{error:#}");
            return 2;
        }
    };
    match state(checkout, data, false).and_then(|s| report(&s)) {
        Ok(response) => {
            if args.format == "text" {
                let value = &response.results[0];
                println!("{}: HEAD={} indexed={} requested={} dirty={} changed_files={} behind={}\nmap={}\nsources={} all_sources={}\nextraction_cache={} previous_map_reused={}\nerror={}", value.freshness, value.checkout_commit, value.indexed_commit.as_deref().unwrap_or("none"), value.requested_commit.as_deref().unwrap_or("none"), value.dirty, value.changed_files, value.commits_behind.map(|n| n.to_string()).unwrap_or("unknown".into()), value.map_path.as_ref().map(|p| p.display().to_string()).unwrap_or("none".into()), serde_json::to_string(&value.sources).unwrap(), serde_json::to_string(&value.all_sources).unwrap(), serde_json::to_string(&value.extraction_cache).unwrap(), serde_json::to_string(&value.previous_map_reused).unwrap(), value.error.as_deref().unwrap_or("none"));
            } else {
                println!("{}", serde_json::to_string(&response).unwrap());
            }
            0
        }
        Err(error) => {
            eprintln!("{error:#}");
            if error.downcast_ref::<InputError>().is_some() {
                2
            } else {
                3
            }
        }
    }
}

/// Advisory process lock shared by the CLI and loopback server. The kernel
/// releases it on interruption; stale lock files do not prevent a retry.
pub(super) fn lock_store(data: &Path) -> Result<std::fs::File> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(data.join("writer.lock"))?;
    ensure!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "local store is busy; stop the local server or wait for refresh"
    );
    Ok(file)
}

fn writer_stopped(data: &Path) -> Result<bool> {
    use std::os::fd::AsRawFd;
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(data.join("writer.lock"))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let stopped = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    if stopped {
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    }
    Ok(stopped)
}

fn selected_sources(repo: &Path, args: &RefreshArgs) -> Result<Option<Vec<(String, String)>>> {
    if args.all_sources {
        return Ok(None);
    }
    if args.pkg.is_empty() && args.lang.is_empty() {
        return Ok(None);
    }
    ensure!(
        args.pkg.len() <= 1 && args.lang.len() <= 1 || args.pkg.len() == args.lang.len(),
        "--pkg and --lang must be paired"
    );
    let pairs = if args.pkg.len() > 1 || args.lang.len() > 1 {
        args.pkg
            .iter()
            .cloned()
            .zip(args.lang.iter().cloned())
            .collect()
    } else {
        let (pkg, lang) = match (args.pkg.first(), args.lang.first()) {
            (Some(pkg), Some(lang)) => (pkg.clone(), lang.clone()),
            (Some(pkg), None) => (
                pkg.clone(),
                crate::detect::detect(repo)?.chosen.language.as_str().into(),
            ),
            (None, Some(lang)) => {
                let language = crate::extract::LanguageKind::parse(lang)?;
                let candidate = crate::detect::detect_language(repo, language)?
                    .context("no source found for --lang; pass --pkg")?;
                (candidate.pkg, lang.clone())
            }
            _ => unreachable!(),
        };
        vec![(pkg, lang)]
    };
    for (pkg, lang) in &pairs {
        crate::extract::LanguageKind::parse(lang)?;
        let root = repo
            .join(pkg)
            .canonicalize()
            .context("source root does not exist")?;
        ensure!(
            root.is_dir() && root.starts_with(repo),
            "source root must be a directory inside the repository"
        );
    }
    Ok(Some(pairs))
}

fn refresh_snapshot(s: Arc<Local>, args: &RefreshArgs) -> Result<()> {
    eprintln!("Preparing committed HEAD; dirty files are excluded; no upload");
    prepare(&s)?;
    let p = s.prepared.lock().unwrap();
    let p = p.as_ref().context("source preparation failed")?;
    let (prepared, commit) = (p.id, p.commit.clone());
    let bare = s
        .data
        .join("prepared")
        .join(prepared.to_string())
        .join("source.git");
    let preview = comparison_preview(&bare, &s.data.join("previews"), &commit)?;
    let sources = selected_sources(&preview.path.canonicalize()?, args)
        .map_err(|error| InputError(format!("{error:#}")))?;
    // Default detection retains the service's low-confidence admission rule.
    // An explicit source selection is owner intent and needs no HTML token.
    drop(preview);
    let id = Uuid::new_v4();
    let cancel = Arc::new(Probe {
        stopped: AtomicBool::new(false),
        pid: AtomicU32::new(0),
    });
    *s.job.lock().unwrap() = Some(Job {
        id,
        stage: "Queued".into(),
        cancel: cancel.clone(),
    });
    let mut attempt = crate::schema::SnapshotFreshness {
        indexed_commit: snapshots(&s)?.last().map(|p| p.commit.clone()),
        requested_commit: Some(commit.clone()),
        state: crate::schema::FreshnessState::Indexing,
        failure_reason: None,
        failure_code: None,
        retryable: false,
        job_id: Some(id.to_string()),
    };
    crate::incremental::atomic_write(&s.data.join("attempt.json"), &serde_json::to_vec(&attempt)?)?;
    *s.attempt.lock().unwrap() = Some(attempt.clone());
    let result = run(
        s.clone(),
        prepared,
        commit,
        args.all_sources,
        None,
        sources,
        id,
        cancel,
    );
    match &result {
        Ok(()) => {
            attempt.state = crate::schema::FreshnessState::Complete;
            attempt.indexed_commit = attempt.requested_commit.clone();
        }
        Err(error) => {
            attempt.state = crate::schema::FreshnessState::Failed;
            attempt.failure_reason = Some(format!("{error:#}"));
            attempt.failure_code = Some("local_index_failed_or_cancelled".into());
            attempt.retryable = true;
        }
    }
    crate::incremental::atomic_write(&s.data.join("attempt.json"), &serde_json::to_vec(&attempt)?)?;
    *s.attempt.lock().unwrap() = Some(attempt);
    *s.job.lock().unwrap() = None;
    result
}

pub fn refresh(args: RefreshArgs) -> i32 {
    let (checkout, data) = match paths(&args.repo, args.data.as_deref()) {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("{error:#}");
            return 2;
        }
    };
    if (args.pkg.len() > 1 || args.lang.len() > 1) && args.pkg.len() != args.lang.len() {
        eprintln!("--pkg and --lang must be paired");
        return 2;
    }
    let result = (|| -> Result<i32> {
        private(&data)?;
        let _lock = lock_store(&data)?;
        let s = Arc::new(state(checkout, data, true)?);
        let result = refresh_snapshot(s.clone(), &args);
        println!("{}", serde_json::to_string(&report(&s)?)?);
        if let Err(error) = result {
            eprintln!("{error:#}");
            return Ok(if error.downcast_ref::<InputError>().is_some() {
                2
            } else {
                3
            });
        }
        Ok(0)
    })();
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error:#}");
            if error.downcast_ref::<InputError>().is_some() {
                2
            } else {
                3
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_none_fresh_stale_dirty_and_incomplete_publication() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        super::super::tests::test_git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("module.py"), "def alpha():\n    return 1\n").unwrap();
        super::super::tests::test_git(&repo, &["add", "."]);
        super::super::tests::test_git(&repo, &["commit", "-qm", "first"]);
        let commit = super::super::tests::test_git(&repo, &["rev-parse", "HEAD"]);
        let s = state(repo.clone(), root.path().join("data"), true).unwrap();
        let empty = report(&s).unwrap();
        assert_eq!(empty.results[0].freshness, "none");
        assert!(empty.map.is_none());
        assert!(!empty.results[0].dirty);
        let id = Uuid::new_v4();
        let dir = s.data.join("snapshots").join(id.to_string());
        std::fs::create_dir(&dir).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("data/flask.json"),
            dir.join("map.json"),
        )
        .unwrap();
        assert_eq!(report(&s).unwrap().results[0].freshness, "none");
        let cancel = Probe {
            stopped: AtomicBool::new(false),
            pid: AtomicU32::new(0),
        };
        *s.job.lock().unwrap() = Some(Job {
            id,
            stage: "publish".into(),
            cancel: Arc::new(Probe {
                stopped: AtomicBool::new(false),
                pid: AtomicU32::new(0),
            }),
        });
        publish_snapshot(&s, id, Uuid::new_v4(), &commit, 1, &cancel).unwrap();
        let fresh = report(&s).unwrap();
        assert_eq!(fresh.results[0].freshness, "fresh");
        assert_eq!(
            fresh.results[0].indexed_commit.as_deref(),
            Some(commit.as_str())
        );
        assert_eq!(fresh.map.unwrap().sha256.len(), 64);
        std::fs::write(repo.join("module.py"), "def alpha():\n    return 2\n").unwrap();
        std::fs::write(repo.join("untracked.py"), "x = 1\n").unwrap();
        let dirty = report(&s).unwrap();
        assert_eq!(dirty.results[0].freshness, "fresh");
        assert!(dirty.results[0].dirty);
        assert_eq!(dirty.results[0].changed_files, 2);
        super::super::tests::test_git(&repo, &["add", "."]);
        super::super::tests::test_git(&repo, &["commit", "-qm", "second"]);
        let stale = report(&s).unwrap();
        assert_eq!(stale.results[0].freshness, "stale");
        assert_eq!(stale.results[0].commits_behind, Some(1));
        assert!(!stale.results[0].dirty);
    }
}
