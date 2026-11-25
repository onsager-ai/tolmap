//! Shared personal repository library used by CLI and future web management.
use crate::service::{
    application_config::ApplicationConfig,
    store::{PersonalSnapshot, Repository, Store},
};
use anyhow::{bail, ensure, Context, Result};
use clap::Args;
use serde_json::json;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Args)]
pub struct AddArgs {
    #[arg(required_unless_present = "scan", conflicts_with = "scan")]
    pub source: Option<String>,
    #[arg(long, conflicts_with = "name")]
    pub scan: Option<PathBuf>,
    #[arg(long)]
    pub name: Option<String>,
    #[command(flatten)]
    pub sources: SourceArgs,
}
#[derive(Clone, Debug, Default, Args, serde::Serialize)]
pub struct SourceArgs {
    #[arg(long, conflicts_with_all=["pkg","lang"])]
    pub all_sources: bool,
    #[arg(long)]
    pub pkg: Vec<String>,
    #[arg(long,value_parser=["py","go","ts","rs"])]
    pub lang: Vec<String>,
}
#[derive(Debug, Args)]
pub struct RefreshArgs {
    #[arg(conflicts_with = "repo")]
    pub name: Option<String>,
    #[arg(long)]
    pub repo: Option<String>,
    #[arg(long)]
    pub commit: Option<String>,
    #[command(flatten)]
    pub sources: SourceArgs,
}
#[derive(Debug, Args)]
pub struct StatusArgs {
    #[arg(long)]
    pub repo: Option<String>,
    #[arg(long,default_value="json",value_parser=["json","text"])]
    pub format: String,
}

pub struct Library {
    pub config: ApplicationConfig,
    pub store: Store,
}

pub(crate) fn private(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    ensure!(
        std::fs::symlink_metadata(path)?.is_dir(),
        "real storage directory required"
    );
    Ok(())
}

/// Nonblocking locks live outside removable repo directories: deleting a locked
/// inode would let another process lock a different inode for the same repo.
fn lock(path: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    ensure!(
        try_lock(&file),
        "repository or job capacity is busy; retry after the current operation"
    );
    Ok(file)
}
fn try_lock(file: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }) == 0
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    // Only the operation name is diagnostic; arguments and stderr may contain
    // repository data or network/helper secrets and must not enter job errors.
    let operation = args.first().copied().unwrap_or("unknown");
    let out = crate::source::git(repo)
        .args(args)
        .output()
        .with_context(|| format!("could not execute Git {operation} in {}", repo.display()))?;
    ensure!(
        out.status.success(),
        "Git {operation} failed in {} (exit {})",
        repo.display(),
        out.status
            .code()
            .map_or_else(|| "signal".into(), |code| code.to_string())
    );
    Ok(String::from_utf8(out.stdout)?.trim().into())
}
fn checkout(path: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(git(path, &["rev-parse", "--show-toplevel"])?).canonicalize()?)
}

/// Accepted forms share one identity; remote URLs never carry credentials.
pub fn github_identity(input: &str) -> Result<String> {
    let name = input
        .strip_prefix("https://github.com/")
        .or_else(|| input.strip_prefix("http://github.com/"))
        .or_else(|| input.strip_prefix("git@github.com:"))
        .unwrap_or(input);
    let name = name
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(name.trim_end_matches('/'));
    let parts: Vec<_> = name.split('/').collect();
    ensure!(
        parts.len() == 2
            && parts.iter().all(|p| !p.is_empty()
                && *p != "."
                && *p != ".."
                && p.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))),
        "expected a checkout path or GitHub owner/name without credentials"
    );
    Ok(format!("github.com/{}", name.to_ascii_lowercase()))
}

impl Library {
    fn config(data: Option<&Path>) -> Result<ApplicationConfig> {
        let mut config = ApplicationConfig::load(data)?;
        if config.data.is_relative() {
            config.data = std::env::current_dir()?.join(&config.data);
        }
        if config.service.db_path.is_relative() {
            config.service.db_path = std::env::current_dir()?.join(&config.service.db_path);
        }
        if config.service.cache_dir.is_relative() {
            config.service.cache_dir = std::env::current_dir()?.join(&config.service.cache_dir);
        }
        Ok(config)
    }

    pub fn open_for_query(data: Option<&Path>) -> Result<Self> {
        let config = Self::config(data)?;
        let store = Store::open_for_query(&config.service.db_path)?;
        Ok(Self { config, store })
    }

    pub fn open(data: Option<&Path>) -> Result<Self> {
        let config = Self::config(data)?;
        private(&config.data)?;
        private(&config.service.db_path.with_extension("locks"))?;
        private(&config.data.join("repositories"))?;
        if let Some(p) = config.service.db_path.parent().filter(|p| !p.exists()) {
            private(p)?;
        }
        let store = Store::open(&config.service.db_path)?;
        Ok(Self { config, store })
    }
    pub fn resolve(&self, name: Option<&str>) -> Result<Repository> {
        resolve_repository(&self.store.repositories()?, name, &std::env::current_dir()?)
    }
    fn locks(&self) -> PathBuf {
        self.config.service.db_path.with_extension("locks")
    }
    fn repo_dir(&self, repo: &Repository) -> PathBuf {
        self.config.data.join("repositories").join(&repo.id)
    }
    fn repo_lock_path(&self, repo: &Repository) -> PathBuf {
        self.locks().join(format!("{}.lock", repo.id))
    }
    fn repo_lock(&self, repo: &Repository) -> Result<std::fs::File> {
        lock(&self.repo_lock_path(repo))
    }
    fn writer_interrupted(&self, repo: &Repository) -> bool {
        match std::fs::File::open(self.repo_lock_path(repo)) {
            Ok(file) => try_lock(&file),
            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        }
    }
    fn slot(&self) -> Result<std::fs::File> {
        for slot in 0..self.config.service.limits.max_concurrent_jobs {
            if let Ok(guard) = lock(&self.locks().join(format!("slot-{slot}.lock"))) {
                return Ok(guard);
            }
        }
        bail!("job capacity is busy; retry after the current operation")
    }
    pub fn snapshot(
        &self,
        repo: &Repository,
        commit: Option<&str>,
        snapshot_id: Option<&str>,
    ) -> Result<PersonalSnapshot> {
        let history = self.store.personal_history(&repo.id)?;
        let matches: Vec<_> = history
            .into_iter()
            .filter(|s| {
                commit.is_none_or(|c| s.commit.starts_with(c))
                    && snapshot_id.is_none_or(|id| s.id == id)
            })
            .collect();
        ensure!(
            !matches.is_empty(),
            "no complete snapshot for {}; run tolmap refresh {}",
            repo.name,
            repo.name
        );
        if snapshot_id.is_some() && matches.len() > 1 {
            bail!(
                "ambiguous snapshot: {}",
                matches
                    .iter()
                    .map(|s| format!("{} ({})", s.id, s.commit))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Ok(matches.into_iter().next().unwrap())
    }
    pub fn add(&self, input: &str, name: Option<&str>, sources: &SourceArgs) -> Result<Repository> {
        let repo = self.register(input, name)?;
        self.refresh(&repo, None, sources)?;
        Ok(repo)
    }
    pub(crate) fn validate_source(&self, input: &str) -> Result<(&'static str, String)> {
        let local = input.starts_with(['.', '/'])
            || (Path::new(input).is_dir() && checkout(Path::new(input)).is_ok());
        let (kind, location) = if local {
            (
                "local",
                checkout(Path::new(input))?.to_string_lossy().into_owned(),
            )
        } else {
            ("github", github_identity(input)?)
        };
        if kind == "local" {
            ensure!(
                !self
                    .config
                    .data
                    .canonicalize()?
                    .starts_with(Path::new(&location)),
                "personal data root must be outside mapped checkouts"
            );
        }
        Ok((kind, location))
    }
    pub(crate) fn register(&self, input: &str, name: Option<&str>) -> Result<Repository> {
        // Registration/name allocation needs a short global lock; indexing does not.
        let registry = lock(&self.locks().join("registry.lock"))?;
        let (kind, location) = self.validate_source(input)?;
        let repos = self.store.repositories()?;
        if let Some(existing) = repos
            .iter()
            .find(|r| r.kind == kind && r.location == location)
        {
            ensure!(
                name.is_none_or(|n| n == existing.name),
                "repository is already registered as {}",
                existing.name
            );
            let existing = existing.clone();
            drop(registry);
            return Ok(existing);
        }
        if let Some(existing) = name.and_then(|n| repos.iter().find(|r| r.name == n)) {
            let _guard = self.repo_lock(existing)?;
            ensure!(
                kind == "local"
                    && existing.kind == "local"
                    && !Path::new(&existing.location).exists(),
                "name {} belongs to {}; cannot relink to {}",
                existing.name,
                existing.location,
                location
            );
            let history = self.store.personal_history(&existing.id)?;
            let last = history.first().with_context(|| {
                format!(
                    "cannot relink {} to {}: registration {} has no successful snapshot to verify",
                    existing.location, location, existing.name
                )
            })?;
            ensure!(git(Path::new(&location), &["merge-base","--is-ancestor",&last.commit,"HEAD"]).is_ok(),
                "cannot relink {} to {}: last indexed commit {} is not an ancestor of new checkout HEAD",existing.location,location,last.commit);
            self.store.relink(&existing.id, &location)?;
            let mut repo = existing.clone();
            repo.location = location;
            drop(_guard);
            drop(registry);
            return Ok(repo);
        }
        let default = if kind == "local" {
            Path::new(&location)
                .file_name()
                .context("checkout has no basename; pass --name")?
                .to_string_lossy()
                .into_owned()
        } else {
            location.strip_prefix("github.com/").unwrap().into()
        };
        let mut chosen = name.unwrap_or(&default).to_string();
        ensure!(!chosen.is_empty(), "name must not be empty");
        if repos.iter().any(|r| r.name == chosen) {
            use sha2::{Digest, Sha256};
            chosen = format!("{default}-{:x}", Sha256::digest(location.as_bytes()));
        }
        let repo = Repository {
            id: Uuid::new_v4().to_string(),
            name: chosen,
            kind: kind.into(),
            location,
            observed_head: None,
            observed_at: None,
        };
        self.store.register(&repo)?;
        drop(registry);
        Ok(repo)
    }
    fn acquire(&self, repo: &Repository, fetch: bool) -> Result<PathBuf> {
        if repo.kind == "local" {
            ensure!(
                Path::new(&repo.location).is_dir(),
                "checkout {} is missing; relink with tolmap add <new-path> --name {}",
                repo.location,
                repo.name
            );
            return Ok(PathBuf::from(&repo.location));
        }
        let root = self.repo_dir(repo);
        private(&root)?;
        let path = root.join("checkout");
        if !fetch {
            ensure!(
                path.is_dir(),
                "remote source is missing; refresh without --commit to acquire it"
            );
            return Ok(path);
        }
        if path.exists() && git(&path, &["rev-parse", "--git-dir"]).is_err() {
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
        crate::personal_github::acquire(&root, &path, &repo.location)?;
        Ok(path)
    }
    pub fn refresh(
        &self,
        repo: &Repository,
        commit: Option<&str>,
        sources: &SourceArgs,
    ) -> Result<PersonalSnapshot> {
        self.refresh_controlled(repo, commit, sources, None)
    }
    pub(crate) fn refresh_controlled(
        &self,
        repo: &Repository,
        commit: Option<&str>,
        sources: &SourceArgs,
        control: Option<&crate::personal_index::PersonalControl>,
    ) -> Result<PersonalSnapshot> {
        let _guard = self.repo_lock(repo)?;
        let current = self
            .store
            .repositories()?
            .into_iter()
            .find(|r| r.id == repo.id)
            .context("repository was removed")?;
        let repo = &current;
        let _slot = self.slot()?;
        let attempt = Uuid::new_v4().to_string();
        self.store.personal_begin(&repo.id, &attempt)?;
        let result = (|| {
            ensure!(!control.is_some_and(|c| c.cancelled()), "cancelled");
            let mut checkout = self.acquire(repo, commit.is_none())?;
            ensure!(!control.is_some_and(|c| c.cancelled()), "cancelled");
            let revision = if repo.kind == "github" && commit.is_none() {
                "refs/remotes/origin/HEAD^{commit}"
            } else {
                "HEAD^{commit}"
            };
            let observed = git(&checkout, &["rev-parse", revision])?;
            if commit.is_none() {
                self.store.observe(&repo.id, &observed)?;
            }
            let selected = if let Some(commit) = commit {
                ensure!(
                    !commit.is_empty() && commit.bytes().all(|b| b.is_ascii_hexdigit()),
                    "--commit must identify an available immutable revision"
                );
                let revision = format!("{commit}^{{commit}}");
                if git(&checkout, &["rev-parse", "--verify", &revision]).is_err() {
                    let retained = self
                        .store
                        .personal_history(&repo.id)?
                        .into_iter()
                        .find(|s| s.commit.starts_with(commit))
                        .context("revision is unavailable in checkout and retained source")?;
                    ensure!(retained.source_path.is_dir(), "snapshot source is missing");
                    checkout = retained.source_path;
                }
                git(&checkout, &["rev-parse", "--verify", &revision])?
            } else {
                observed
            };
            let selection = serde_json::to_value(sources)?;
            for snapshot in self.store.personal_history(&repo.id)? {
                if snapshot.commit == selected
                    && snapshot.map_path.is_file()
                    && snapshot.source_path.is_dir()
                {
                    let path = snapshot.map_path.with_file_name("selection.json");
                    if path.is_file()
                        && serde_json::from_slice::<serde_json::Value>(&std::fs::read(path)?)?
                            == selection
                    {
                        return Ok(snapshot);
                    }
                }
            }
            let args = crate::personal_index::RefreshArgs {
                all_sources: sources.all_sources,
                pkg: sources.pkg.clone(),
                lang: sources.lang.clone(),
            };
            let snapshot = crate::personal_index::personal_refresh(
                checkout,
                self.repo_dir(repo),
                repo,
                &args,
                &selected,
                &self.config.service,
                control,
            )?;
            crate::incremental::atomic_write(
                &snapshot.map_path.with_file_name("selection.json"),
                &serde_json::to_vec(&selection)?,
            )?;
            Ok(snapshot)
        })();
        match &result {
            Ok(snapshot) => self.store.personal_finish(&attempt, Ok(snapshot))?,
            Err(error) => self
                .store
                .personal_finish(&attempt, Err(&format!("{error:#}")))?,
        }
        result
    }
    pub fn remove(&self, repo: &Repository) -> Result<()> {
        let _registry = lock(&self.locks().join("registry.lock"))?;
        let _guard = self.repo_lock(repo)?;
        self.store.remove_repository(&repo.id)?;
        let dir = self.repo_dir(repo);
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        let cache = self.config.service.cache_dir.join(&repo.id);
        if cache.exists() {
            std::fs::remove_dir_all(cache)?;
        }
        Ok(())
    }
    pub fn status(&self, repo: &Repository) -> Result<serde_json::Value> {
        let latest = self.store.personal_history(&repo.id)?.into_iter().next();
        let mut attempt = self.store.personal_attempt(&repo.id)?;
        if let Some((state, error)) = attempt.as_mut() {
            if state == "running" && self.writer_interrupted(repo) {
                *state = "failed".into();
                *error = Some("interrupted writer; retry refresh".into());
            }
        }
        let (head, dirty) = if repo.kind == "local" && Path::new(&repo.location).exists() {
            (
                git(Path::new(&repo.location), &["rev-parse", "HEAD^{commit}"]).ok(),
                git(
                    Path::new(&repo.location),
                    &[
                        "--no-optional-locks",
                        "status",
                        "--porcelain=v1",
                        "--untracked-files=all",
                    ],
                )
                .ok()
                .map(|s| !s.is_empty()),
            )
        } else {
            (repo.observed_head.clone(), None)
        };
        let freshness = if repo.kind == "local" && !Path::new(&repo.location).exists() {
            "missing"
        } else if latest.is_none() {
            "none"
        } else if latest.as_ref().map(|s| &s.commit) == head.as_ref() {
            "fresh"
        } else {
            "stale"
        };
        Ok(
            json!({"repository":repo,"snapshot":latest,"checkout_commit":head,"dirty":dirty,"freshness":freshness,"attempt":attempt}),
        )
    }
}

pub fn resolve_repository(
    repos: &[Repository],
    name: Option<&str>,
    cwd: &Path,
) -> Result<Repository> {
    if let Some(name) = name {
        if let Some(repo) = repos.iter().find(|r| r.name == name) {
            return Ok(repo.clone());
        }
        let candidates: Vec<_> = repos
            .iter()
            .filter(|r| r.id == name || r.name.rsplit('/').next() == Some(name))
            .collect();
        ensure!(
            candidates.len() == 1,
            "repository {name:?} is missing or ambiguous; candidates: {}",
            candidates
                .iter()
                .map(|r| format!("{} ({})", r.name, r.id))
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Ok(candidates[0].clone());
    }
    let cwd = cwd.canonicalize()?;
    repos.iter().filter(|r|r.kind=="local" && cwd.starts_with(&r.location)).max_by_key(|r|Path::new(&r.location).components().count()).cloned().context("no registered checkout contains the current directory; pass --repo NAME or tolmap add <path>")
}

pub fn resolve_map(
    map: Option<&Path>,
    repo: Option<&str>,
    commit: Option<&str>,
    snapshot: Option<&str>,
    data: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(map) = map {
        ensure!(
            repo.is_none() && commit.is_none() && snapshot.is_none(),
            "--map conflicts with database snapshot selectors"
        );
        return Ok(map.to_owned());
    }
    let library = Library::open_for_query(data)?;
    let repo = library.resolve(repo)?;
    let selected = library.snapshot(&repo, commit, snapshot)?;
    let status = library.status(&repo)?;
    eprintln!(
        "{}: snapshot {} at {}; freshness={} dirty={}",
        repo.name, selected.id, selected.commit, status["freshness"], status["dirty"]
    );
    Ok(selected.map_path)
}

pub fn scan(folder: &Path) -> Result<Vec<PathBuf>> {
    fn visit(path: &Path, results: &mut Vec<PathBuf>) -> Result<()> {
        if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Ok(());
        }
        if path.join(".git").exists() {
            results.push(checkout(path)?);
            return Ok(());
        }
        let mut entries = std::fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if entry.file_type()?.is_dir() {
                visit(&entry.path(), results)?;
            }
        }
        Ok(())
    }
    let mut roots = Vec::new();
    visit(folder, &mut roots)?;
    Ok(roots)
}

pub fn run_add(args: AddArgs, data: Option<&Path>) -> Result<()> {
    let library = Library::open(data)?;
    if let Some(folder) = args.scan {
        let mut failed = false;
        for root in scan(&folder)? {
            match library.add(&root.to_string_lossy(), None, &args.sources) {
                Ok(repo) => println!("{}", serde_json::to_string(&repo)?),
                Err(error) => {
                    failed = true;
                    eprintln!("{}: {error:#}", root.display());
                }
            }
        }
        ensure!(
            !failed,
            "one or more checkouts failed; successful registrations were retained"
        );
    } else {
        let repo = library.add(
            args.source.as_deref().unwrap(),
            args.name.as_deref(),
            &args.sources,
        )?;
        println!("{}", serde_json::to_string(&repo)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repo(name: &str, path: &str) -> Repository {
        Repository {
            id: name.into(),
            name: name.into(),
            kind: "local".into(),
            location: path.into(),
            observed_head: None,
            observed_at: None,
        }
    }
    #[test]
    fn failed_first_index_keeps_local_registration() {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        for args in [
            vec!["init", "--quiet"],
            vec![
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
        ] {
            assert!(crate::source::git(&checkout)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let data = root.path().join("data");
        let library = Library::open(Some(&data)).unwrap();
        let sources = SourceArgs {
            pkg: vec!["missing".into()],
            lang: vec!["ts".into()],
            ..SourceArgs::default()
        };
        assert!(library
            .add(checkout.to_str().unwrap(), Some("fixture"), &sources)
            .is_err());
        let registered = library.resolve(Some("fixture")).unwrap();
        assert_eq!(registered.location, checkout.to_string_lossy());
        assert!(library
            .store
            .personal_history(&registered.id)
            .unwrap()
            .is_empty());
        assert_eq!(
            library
                .store
                .personal_attempt(&registered.id)
                .unwrap()
                .unwrap()
                .0,
            "failed"
        );
    }

    #[test]
    fn git_errors_name_the_operation_without_stderr() {
        let directory = tempfile::tempdir().unwrap();
        let error = git(directory.path(), &["rev-parse", "--show-toplevel"]).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("Git rev-parse failed"), "{message}");
        assert!(!message.contains("fatal:"), "{message}");
    }

    #[test]
    fn github_spellings_and_credentials() {
        for spelling in [
            "Owner/Name",
            "https://github.com/OWNER/name.git",
            "git@github.com:owner/name",
        ] {
            assert_eq!(github_identity(spelling).unwrap(), "github.com/owner/name");
        }
        for bad in [
            "https://token@github.com/owner/name",
            "https://gitlab.com/owner/name",
            "../name",
            "owner/name?token=x",
        ] {
            assert!(github_identity(bad).is_err());
        }
    }
    #[test]
    fn names_resolve_exact_then_unique_short_and_nearest_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let repos = vec![
            repo("owner/app", temp.path().to_str().unwrap()),
            repo("other/app", nested.to_str().unwrap()),
        ];
        assert!(resolve_repository(&repos, Some("app"), temp.path()).is_err());
        assert_eq!(
            resolve_repository(&repos, Some("owner/app"), temp.path())
                .unwrap()
                .id,
            "owner/app"
        );
        assert_eq!(
            resolve_repository(&repos, None, &nested).unwrap().id,
            "other/app"
        );
    }
    #[test]
    fn process_locks_are_busy_and_release_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("lock");
        let guard = lock(&path).unwrap();
        assert!(lock(&path).is_err());
        drop(guard);
        assert!(lock(&path).is_ok());
    }
}

/// Resolve a check's base through the same registry as read-only queries.
/// Explicit checkout + base retains the independent build/check workflow.
#[allow(clippy::too_many_arguments)]
pub fn resolve_check(
    checkout_path: Option<PathBuf>,
    name: Option<&str>,
    commit: Option<&str>,
    snapshot_id: Option<&str>,
    base: Option<String>,
    map: Option<PathBuf>,
    head: Option<&str>,
    data: Option<&Path>,
) -> Result<(PathBuf, String, Option<PathBuf>)> {
    if let Some(checkout) = checkout_path {
        ensure!(
            name.is_none() && commit.is_none() && snapshot_id.is_none(),
            "explicit checkout conflicts with snapshot selectors"
        );
        return Ok((
            checkout,
            base.context("explicit checkout requires --base")?,
            map,
        ));
    }
    if let Some(map) = map {
        ensure!(
            name.is_none() && commit.is_none() && snapshot_id.is_none(),
            "--map conflicts with snapshot selectors"
        );
        return Ok((
            std::env::current_dir()?,
            base.context("--map requires --base")?,
            Some(map),
        ));
    }
    let library = Library::open_for_query(data)?;
    let repo = library.resolve(name)?;
    let snapshot = library.snapshot(&repo, commit, snapshot_id)?;
    let path = if repo.kind == "local" {
        ensure!(
            Path::new(&repo.location).is_dir(),
            "checkout {} is missing",
            repo.location
        );
        PathBuf::from(&repo.location)
    } else {
        let head = head.with_context(|| remote_check_guidance(&snapshot.commit))?;
        ensure!(
            snapshot.source_path.is_dir(),
            "retained snapshot source is missing; use a local checkout: tolmap check . --base {} --report-only (omit --repo)",
            snapshot.commit
        );
        // check creates temporary detached worktrees from this retained bare source.
        ensure!(!head.starts_with('-'), "invalid --head");
        snapshot.source_path.clone()
    };
    if let Some(base) = base {
        let resolved = git(&path, &["rev-parse", &format!("{base}^{{commit}}")])?;
        ensure!(
            resolved == snapshot.commit,
            "--base must agree with the selected snapshot ({})",
            snapshot.commit
        );
    }
    Ok((path, snapshot.commit, Some(snapshot.map_path)))
}

fn remote_check_guidance(base: &str) -> String {
    format!(
        "GitHub library entry has no working-tree head; pass --head <retained-commit> to check retained source, or from your clone/worktree run: tolmap check . --base {base} --report-only (omit --repo)"
    )
}

#[cfg(test)]
mod check_resolution_tests {
    #[test]
    fn github_entry_without_head_names_both_supported_workflows() {
        let message = super::remote_check_guidance("abc123");
        assert!(message.contains("no working-tree head"));
        assert!(message.contains("--head <retained-commit>"));
        assert!(message.contains("tolmap check . --base abc123 --report-only"));
        assert!(message.contains("omit --repo"));
    }
}

/// Detached view of retained objects for build/detect. Its lifetime owns cleanup.
pub struct SourceView {
    pub path: PathBuf,
    source: PathBuf,
    _guard: std::fs::File,
}
impl SourceView {
    pub fn resolve(
        name: Option<&str>,
        commit: Option<&str>,
        snapshot: Option<&str>,
        data: Option<&Path>,
    ) -> Result<Self> {
        let library = Library::open(data)?;
        let repo = library.resolve(name)?;
        let selected = library.snapshot(&repo, commit, snapshot)?;
        ensure!(selected.source_path.is_dir(), "snapshot source is missing");
        let root = library.config.data.join("work");
        private(&root)?;
        let path = root.join(&selected.id);
        let guard = lock(&library.locks().join(format!("view-{}.lock", selected.id)))?;
        if path.exists() {
            std::fs::remove_dir_all(&path)?;
        }
        git(
            &selected.source_path,
            &["worktree", "prune", "--expire", "now"],
        )?;
        git(
            &selected.source_path,
            &[
                "worktree",
                "add",
                "--detach",
                path.to_str().context("work path must be UTF-8")?,
                &selected.commit,
            ],
        )?;
        Ok(Self {
            path,
            source: selected.source_path,
            _guard: guard,
        })
    }
}
impl Drop for SourceView {
    fn drop(&mut self) {
        if let Some(path) = self.path.to_str() {
            let _ = git(&self.source, &["worktree", "remove", "--force", path]);
        }
    }
}
