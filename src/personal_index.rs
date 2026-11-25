//! Committed snapshot capture and isolated indexing for the personal library.
use crate::{
    service::{
        executor::{self, CancelProbe, EventSink, ExecEnv, JobInputs, PreviousMapInput},
        worker_result,
    },
    source,
    worker::{JobSpec, WorkerEvent},
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
};
use uuid::Uuid;
#[derive(Clone, Serialize, Deserialize)]
struct Snapshot {
    id: Uuid,
    repository: String,
    commit: String,
    prepared: Uuid,
    sequence: u64,
}
struct Prepared {
    id: Uuid,
    commit: String,
}
struct Job {
    id: Uuid,
    stage: String,
}
struct SnapshotIndex {
    checkout: PathBuf,
    data: PathBuf,
    repository: String,
    owner: String,
    name: String,
    slug: String,
    prepared: Mutex<Option<Prepared>>,
    job: Mutex<Option<Job>>,
    attempt: Mutex<Option<crate::schema::SnapshotFreshness>>,
}
type PersonalEvent = dyn Fn(&WorkerEvent) + Send + Sync;

fn safe_file_stem(name: &str) -> String {
    name.replace(['/', '\\'], "_")
}

impl SnapshotIndex {
    fn worker_repo(&self) -> String {
        safe_file_stem(&self.name)
    }
}
pub(crate) struct PersonalControl {
    probe: Arc<Probe>,
    pub event: Arc<PersonalEvent>,
}
impl PersonalControl {
    pub fn new(event: impl Fn(&WorkerEvent) + Send + Sync + 'static) -> Self {
        Self {
            probe: Arc::new(Probe {
                stopped: AtomicBool::new(false),
                pid: AtomicU32::new(0),
            }),
            event: Arc::new(event),
        }
    }
    pub fn stop(&self) {
        self.probe.stop();
    }
    pub fn cancelled(&self) -> bool {
        self.probe.is_cancelled()
    }
}
struct PersonalSink<'a> {
    local: Sink,
    control: &'a PersonalControl,
}
impl EventSink for PersonalSink<'_> {
    fn clone_started(&mut self) {
        self.local.clone_started();
    }
    fn clone_finished(&mut self, elapsed: f64, cached: bool) {
        self.local.clone_finished(elapsed, cached);
    }
    fn event(&mut self, event: WorkerEvent) {
        (self.control.event)(&event);
        self.local.event(event);
    }
    fn install_tick(&self) {
        self.local.install_tick();
    }
    fn peak_rss(&mut self, peak: u64) {
        self.local.peak_rss(peak);
    }
}
struct Probe {
    stopped: AtomicBool,
    pid: AtomicU32,
}
struct PreviewWorktree {
    git_dir: PathBuf,
    path: PathBuf,
}
impl Drop for PreviewWorktree {
    fn drop(&mut self) {
        let _ = source::git(&self.git_dir)
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .output();
    }
}
impl Probe {
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let pid = self.pid.load(Ordering::SeqCst);
        if pid > 0 {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}
impl CancelProbe for Probe {
    fn is_cancelled(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
    fn child_started(&self, pid: u32) {
        self.pid.store(pid, Ordering::SeqCst);
        if self.is_cancelled() {
            self.stop();
        }
    }
    fn child_finished(&self) {
        self.pid.store(0, Ordering::SeqCst);
    }
}
struct Sink(Arc<SnapshotIndex>);
impl EventSink for Sink {
    fn clone_started(&mut self) {
        self.stage("Preparing committed source");
    }
    fn clone_finished(&mut self, _: f64, _: bool) {}
    fn event(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::StageStarted { stage, .. } => self.stage(&format!("{stage:?}")),
            WorkerEvent::Progress { value, .. } => self.stage(&value.label),
            WorkerEvent::Log { message, .. } => eprintln!("{message}"),
            _ => {}
        }
    }
    fn install_tick(&self) {}
    fn peak_rss(&mut self, _: u64) {}
}
impl Sink {
    fn stage(&self, text: &str) {
        eprintln!("{text}");
        if let Some(j) = self.0.job.lock().unwrap().as_mut() {
            j.stage = text.into();
        }
        if let Some(attempt) = self.0.attempt.lock().unwrap().as_mut() {
            attempt.state = crate::schema::FreshnessState::Indexing;
        }
    }
}
fn private(path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    ensure!(
        std::fs::symlink_metadata(path)?.is_dir(),
        "real storage directory required"
    );
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn checked(mut command: std::process::Command) -> Result<Vec<u8>> {
    let o = command.output()?;
    ensure!(
        o.status.success(),
        "local Git operation failed; keep the checkout available and retry"
    );
    Ok(o.stdout)
}
fn comparison_preview(
    git_dir: &Path,
    preview_root: &Path,
    commit: &str,
) -> Result<PreviewWorktree> {
    ensure!(source::valid_commit(commit), "snapshot commit is invalid");
    private(preview_root)?;
    let preview = PreviewWorktree {
        git_dir: git_dir.to_owned(),
        path: preview_root.join(Uuid::new_v4().to_string()),
    };
    let mut command = source::git(git_dir);
    command
        .args(["worktree", "add", "--detach"])
        .arg(&preview.path)
        .arg(commit);
    checked(command)?;
    Ok(preview)
}
fn command(repo: &Path, args: &[&str]) -> std::process::Command {
    let mut c = source::git(repo);
    c.args(args);
    c
}
fn prepare_revision(s: &SnapshotIndex, revision: &str) -> Result<()> {
    ensure!(s.job.lock().unwrap().is_none(), "indexing is still running");
    let commit = String::from_utf8(checked(command(
        &s.checkout,
        &["rev-parse", &format!("{revision}^{{commit}}")],
    ))?)?
    .trim()
    .to_owned();
    ensure!(
        worker_result::is_object_id(&commit),
        "committed HEAD required"
    );
    let id = Uuid::new_v4();
    let root = s.data.join("prepared").join(id.to_string());
    private(&root)?;
    let bare = root.join("source.git");
    let mut c = source::git(&s.checkout);
    c.args(["clone", "--bare", "--no-local", "--"])
        .arg(&s.checkout)
        .arg(&bare);
    checked(c)?;
    // A historical commit may be reachable only by its object ID, rather than
    // a branch copied by clone. Pin it in the independent source before GC.
    let mut capture = source::git(&bare);
    capture
        .args(["fetch", "--no-tags", "--"])
        .arg(&s.checkout)
        .arg(format!("{commit}:refs/tolmap/snapshot"));
    checked(capture)?;
    // Independent objects, no alternates/hardlinks: moving/GC'ing the original
    // checkout cannot substitute or expire source belonging to an old map.
    let preview = root.join("preview");
    let mut c = source::git(&root);
    c.args(["clone", "--no-local", "--no-checkout", "--"])
        .arg(&bare)
        .arg(&preview);
    checked(c)?;
    // Clone copies branches only. A GitHub refresh selects origin/HEAD, which
    // the checkout's local branch never follows, so fetch the pinned ref.
    checked(command(
        &preview,
        &["fetch", "--no-tags", "origin", "refs/tolmap/snapshot"],
    ))?;
    checked(command(&preview, &["checkout", "--detach", &commit]))?;
    *s.prepared.lock().unwrap() = Some(Prepared { id, commit });
    std::fs::remove_dir_all(preview)?;
    Ok(())
}
fn snapshots(s: &SnapshotIndex) -> Result<Vec<Snapshot>> {
    let mut result = Vec::new();
    for e in std::fs::read_dir(s.data.join("snapshots"))? {
        let p = e?.path().join("snapshot.json");
        if p.is_file() {
            result.push(serde_json::from_slice::<Snapshot>(&std::fs::read(p)?)?);
        }
    }
    result.sort_by_key(|x| (x.sequence, x.id));
    Ok(result)
}

/// The local snapshot metadata is its one visibility switch. Hold the job
/// lock through the durable write so cancellation and stale retries cannot
/// race a completed snapshot into view.
fn publish_snapshot(
    s: &SnapshotIndex,
    id: Uuid,
    prepared: Uuid,
    commit: &str,
    sequence: u64,
    cancel: &Probe,
) -> Result<()> {
    let job = s.job.lock().unwrap();
    ensure!(
        !cancel.is_cancelled() && job.as_ref().is_some_and(|job| job.id == id),
        "cancelled_or_stale_attempt"
    );
    let snap = Snapshot {
        id,
        repository: s.repository.clone(),
        commit: commit.to_owned(),
        prepared,
        sequence,
    };
    crate::incremental::atomic_write(
        &s.data
            .join("snapshots")
            .join(id.to_string())
            .join("snapshot.json"),
        &serde_json::to_vec(&snap)?,
    )?;
    drop(job);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_configured(
    s: Arc<SnapshotIndex>,
    prepared: Uuid,
    commit: String,
    all: bool,
    local_sources: Option<Vec<(String, String)>>,
    id: Uuid,
    cancel: Arc<Probe>,
    config: Option<&crate::service::config::ServeConfig>,
    control: Option<&PersonalControl>,
) -> Result<()> {
    let source = s
        .data
        .join("prepared")
        .join(prepared.to_string())
        .join("source.git");
    let previous = snapshots(&s)?.last().cloned();
    let mut inputs = JobInputs {
        names: Default::default(),
        previous_maps: vec![],
    };
    let sequence = previous.as_ref().map(|p| p.sequence + 1).unwrap_or(1);
    if let Some(p) = previous {
        let dir = s.data.join("snapshots").join(p.id.to_string());
        inputs.names = crate::naming::load_cache(&dir.join("names.json"));
        inputs.previous_maps.push(PreviousMapInput {
            commit: p.commit,
            branch: None,
            path: dir.join("map.json"),
        });
    }
    let file_stem = s.worker_repo();
    let spec = JobSpec {
        slug: s.slug.clone(),
        owner: s.owner.clone(),
        repo: file_stem.clone(),
        source: source.to_string_lossy().into(),
        local: true,
        commit: commit.clone(),
        all_sources: all,
        prune_variant: config
            .map(|c| c.prune_variant.to_string())
            .unwrap_or_else(|| "node-relative".into()),
        namer: config
            .map(|c| c.namer.to_string())
            .unwrap_or_else(|| "idf".into()),
        namer_model: config.map(|c| c.namer_model.clone()).unwrap_or_default(),
        refs: Some(
            config
                .map(|c| c.refs.to_string())
                .unwrap_or_else(|| "hand".into()),
        ),
        install: config.filter(|c| c.scip_install).map(|_| "sandbox".into()),
    };
    // Workers run as a separate uid in the image and cannot traverse the
    // root-owned 0700 library. Only disposable job files live in this scratch
    // directory; the job itself is hardened to the worker uid before spawning.
    let scratch = tempfile::Builder::new().prefix("tolmap-job-").tempdir()?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o711))?;
    let env = ExecEnv {
        clone_cache: config
            .map(|c| c.cache_dir.join(&s.repository))
            .unwrap_or_else(|| s.data.join("cache")),
        clone_cache_bytes: config
            .map(|c| c.limits.clone_cache_bytes)
            .unwrap_or(2 * 1024 * 1024 * 1024),
        job_root: scratch.path().join("jobs"),
        install_root: scratch.path().join("install"),
        worker_exe: std::env::current_exe()?,
        worker_uid: config.map(|c| c.worker_uid).unwrap_or(10001),
        worker_gid: config.map(|c| c.worker_gid).unwrap_or(10001),
        allow_openrouter_key: false,
    };
    let mut local_sink = Sink(s.clone());
    let mut personal_sink;
    let sink: &mut dyn EventSink = if let Some(control) = control {
        personal_sink = PersonalSink {
            local: Sink(s.clone()),
            control,
        };
        &mut personal_sink
    } else {
        &mut local_sink
    };
    let result = if let Some(sources) = local_sources {
        executor::execute_local_sources(&env, id, &spec, &inputs, sink, cancel.as_ref(), &sources)
    } else {
        executor::execute(&env, id, &spec, &inputs, sink, cancel.as_ref())
    }
    .map_err(|e| anyhow::anyhow!("personal executor stopped: {}: {}", e.error, e.message))?;
    let work = result.job_dir.clone();
    let publish = (|| -> Result<()> {
        ensure!(
            !cancel.is_cancelled()
                && result.checkout.commit == commit
                && result.output.commit == commit,
            "cancelled or commit mismatch"
        );
        let dir = s.data.join("snapshots").join(id.to_string());
        private(&dir)?;
        let o = &result.output;
        // Adoption first takes custody on the scratch filesystem. Only checked
        // artifacts are copied into the private retained store, which may be on
        // a different mounted filesystem (rename cannot cross that boundary).
        let staging = scratch.path().join("adopted");
        private(&staging)?;
        let adopted = worker_result::adopt(
            &result.output_dir,
            &staging,
            &file_stem,
            &worker_result::Reported {
                map_path: &o.map_path,
                symbols_path: &o.symbols_path,
                symbols_dir: &o.symbols_dir,
                names_cache: &o.names_cache,
                files: o.files,
                districts: o.districts,
            },
            None,
        )
        .map_err(|e| anyhow::anyhow!("invalid worker artifacts: {e}"))?;
        // Unclaimed worker output is not part of the checked artifact set.
        std::fs::remove_dir_all(staging.join("output"))?;
        copy_artifacts(&staging, &dir)?;
        crate::naming::save_cache(&dir.join("names.json"), &adopted.names)?;
        publish_snapshot(&s, id, prepared, &commit, sequence, cancel.as_ref())
    })();
    let _ = std::fs::remove_dir_all(work);
    publish
}

// The source is already in parent-owned custody after worker_result::adopt.
fn copy_artifacts(source: &Path, destination: &Path) -> Result<()> {
    private(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_artifacts(&entry.path(), &target)?;
        } else {
            ensure!(kind.is_file(), "artifact must be a regular file");
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

pub(crate) struct RefreshArgs {
    pub all_sources: bool,
    pub pkg: Vec<String>,
    pub lang: Vec<String>,
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

fn refresh_revision(
    s: Arc<SnapshotIndex>,
    args: &RefreshArgs,
    revision: &str,
    config: Option<&crate::service::config::ServeConfig>,
    control: Option<&PersonalControl>,
) -> Result<()> {
    eprintln!("Preparing committed HEAD; dirty files are excluded; no upload");
    let commit = String::from_utf8(checked(command(
        &s.checkout,
        &["rev-parse", &format!("{revision}^{{commit}}")],
    ))?)?
    .trim()
    .to_owned();
    let id = Uuid::new_v4();
    let cancel = control.map(|c| c.probe.clone()).unwrap_or_else(|| {
        Arc::new(Probe {
            stopped: AtomicBool::new(false),
            pid: AtomicU32::new(0),
        })
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
    let result = (|| -> Result<()> {
        prepare_revision(&s, &commit)?;
        let (prepared, prepared_commit) = {
            let p = s.prepared.lock().unwrap();
            let p = p.as_ref().context("source preparation failed")?;
            (p.id, p.commit.clone())
        };
        ensure!(
            prepared_commit == commit,
            "checkout HEAD moved while preparing; retry refresh"
        );
        let bare = s
            .data
            .join("prepared")
            .join(prepared.to_string())
            .join("source.git");
        let preview = comparison_preview(&bare, &s.data.join("previews"), &commit)?;
        let sources = selected_sources(&preview.path.canonicalize()?, args)?;
        drop(preview);
        *s.job.lock().unwrap() = Some(Job {
            id,
            stage: "Indexing".into(),
        });
        // Default selection retains the worker's confidence admission rule;
        // explicit CLI sources are owner intent, without an HTML token.
        run_configured(
            s.clone(),
            prepared,
            commit,
            args.all_sources,
            sources,
            id,
            cancel,
            config,
            control,
        )
    })();
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

pub(crate) fn personal_refresh(
    checkout: PathBuf,
    data: PathBuf,
    repository: &crate::service::store::Repository,
    args: &RefreshArgs,
    revision: &str,
    config: &crate::service::config::ServeConfig,
    control: Option<&PersonalControl>,
) -> Result<crate::service::store::PersonalSnapshot> {
    private(&data)?;
    private(&data.join("snapshots"))?;
    ensure!(
        !data.canonicalize()?.starts_with(checkout.canonicalize()?),
        "private storage must be outside the checkout"
    );
    let s = Arc::new(SnapshotIndex {
        checkout,
        data,
        repository: repository.id.clone(),
        owner: if repository.kind == "github" {
            repository
                .location
                .strip_prefix("github.com/")
                .and_then(|v| v.split_once('/'))
                .map(|(owner, _)| owner.to_owned())
                .unwrap_or_else(|| "local".into())
        } else {
            "local".into()
        },
        name: if repository.kind == "github" {
            repository
                .location
                .rsplit('/')
                .next()
                .unwrap_or(&repository.name)
                .to_owned()
        } else {
            repository.name.clone()
        },
        slug: if repository.kind == "github" {
            repository
                .location
                .strip_prefix("github.com/")
                .unwrap_or(&repository.name)
                .to_owned()
        } else {
            format!("local/{}", repository.name)
        },
        prepared: Mutex::new(None),
        job: Mutex::new(None),
        attempt: Mutex::new(None),
    });
    refresh_revision(s.clone(), args, revision, Some(config), control)?;
    let snapshot = snapshots(&s)?
        .pop()
        .context("indexing produced no complete snapshot")?;
    Ok(crate::service::store::PersonalSnapshot {
        id: snapshot.id.to_string(),
        repository: repository.id.clone(),
        commit: snapshot.commit,
        map_path: s
            .data
            .join("snapshots")
            .join(snapshot.id.to_string())
            .join("map.json"),
        source_path: s
            .data
            .join("prepared")
            .join(snapshot.prepared.to_string())
            .join("source.git"),
        indexed_at: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    /// A GitHub refresh indexes origin/HEAD while the checkout's own branch
    /// stays at the first indexed commit; preparing it must still succeed.
    #[test]
    fn prepares_a_commit_that_no_local_branch_points_at() {
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        git(&checkout, &["init", "-q"]);
        std::fs::write(checkout.join("a.txt"), "one\n").unwrap();
        git(&checkout, &["add", "a.txt"]);
        git(&checkout, &["commit", "-qm", "one"]);
        std::fs::write(checkout.join("a.txt"), "two\n").unwrap();
        git(&checkout, &["commit", "-qam", "two"]);
        let upstream = git(&checkout, &["rev-parse", "HEAD"]);
        git(
            &checkout,
            &["update-ref", "refs/remotes/origin/main", &upstream],
        );
        git(&checkout, &["reset", "-q", "--hard", "HEAD~1"]);
        let s = SnapshotIndex {
            checkout,
            data: dir.path().join("data"),
            repository: "repo".into(),
            owner: "local".into(),
            name: "repo".into(),
            slug: "local/repo".into(),
            prepared: Mutex::new(None),
            job: Mutex::new(None),
            attempt: Mutex::new(None),
        };
        prepare_revision(&s, &upstream).unwrap();
        assert_eq!(
            s.prepared.lock().unwrap().as_ref().unwrap().commit,
            upstream
        );
    }

    #[test]
    fn worker_repo_preserves_labels_and_sanitizes_path_separators() {
        let mut index = SnapshotIndex {
            checkout: PathBuf::new(),
            data: PathBuf::new(),
            repository: "id".into(),
            owner: "owner".into(),
            name: "my_repo.js".into(),
            slug: "owner/my_repo.js".into(),
            prepared: Mutex::new(None),
            job: Mutex::new(None),
            attempt: Mutex::new(None),
        };
        assert_eq!(index.worker_repo(), "my_repo.js");
        index.name = "acme/widgets".into();
        assert_eq!(index.worker_repo(), "acme_widgets");
        index.name = "acme\\widgets".into();
        assert_eq!(index.worker_repo(), "acme_widgets");
    }
}
