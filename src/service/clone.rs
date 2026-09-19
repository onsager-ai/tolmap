//! Repository identity, cloning and the on-disk cache.
//!
//! Clone strategy is `--filter=blob:none`, not `--depth 1` (docs/ARCHITECTURE.md):
//! `git_cochange` in `extract.rs` walks up to 4000 commits, so co-change
//! needs the full commit graph. A blobless clone gets that graph without
//! downloading file contents for history it will not read, and `git
//! clone`/`git fetch` then materialise the working tree of the default
//! branch on top of it -- one command each, not a separate checkout step.
//!
//! The cache lives under a configured directory (default: the system temp
//! dir, never inside a mapped repository -- nothing derived belongs there,
//! per docs/ARCHITECTURE.md's "what lives where").

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::{bail, ensure, Context, Result};

use crate::service::config::Limits;
use crate::service::error::ApiError;

#[derive(Clone, Debug)]
pub enum RepoSource {
    /// `git clone`/`fetch`-able URL, already normalised to end in `.git`
    /// where that matters (GitHub accepts both; kept as given otherwise).
    Remote(String),
    /// An already-materialised local directory -- used for fixtures and
    /// tests, so end-to-end verification does not require network access.
    /// Never cloned, cached or evicted; read in place.
    Local(PathBuf),
}

#[derive(Clone, Debug)]
pub struct RepoRef {
    pub slug: String,
    pub owner: String,
    pub repo: String,
    pub source: RepoSource,
}

/// The body of `POST /api/index` names a repository three ways -- see
/// docs/API.md. Exactly one of `repo`/`path` is expected; the caller
/// (`service::http`) is responsible for that validation since it is a
/// request-shape concern, not a repo-identity one.
pub fn resolve(repo: Option<&str>, path: Option<&str>) -> Result<RepoRef, ApiError> {
    match (repo, path) {
        (Some(_), Some(_)) => Err(ApiError::invalid_request(
            "pass either \"repo\" or \"path\", not both",
        )),
        (None, None) => Err(ApiError::invalid_request(
            "pass \"repo\" (owner/name or an https url) or \"path\" (a local directory)",
        )),
        (None, Some(path)) => {
            let path = Path::new(path);
            ensure_local(path)?;
            let repo_name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
                .ok_or_else(|| ApiError::invalid_request("path has no final component to name the repository after"))?;
            Ok(RepoRef {
                slug: format!("local/{repo_name}"),
                owner: "local".to_owned(),
                repo: repo_name,
                source: RepoSource::Local(path.to_owned()),
            })
        }
        (Some(spec), None) => parse_repo_spec(spec),
    }
}

fn ensure_local(path: &Path) -> Result<(), ApiError> {
    if !path.is_dir() {
        return Err(ApiError::invalid_request(format!(
            "{} is not a directory",
            path.display()
        )));
    }
    Ok(())
}

fn parse_repo_spec(spec: &str) -> Result<RepoRef, ApiError> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err(ApiError::invalid_request("\"repo\" is empty"));
    }
    let (owner, repo, url) = if trimmed.starts_with("https://") || trimmed.starts_with("http://")
    {
        let without_git = trimmed.strip_suffix(".git").unwrap_or(trimmed);
        let mut segments = without_git
            .rsplit('/')
            .filter(|segment| !segment.is_empty());
        let repo = segments
            .next()
            .ok_or_else(|| ApiError::invalid_request("could not parse a repo name out of the url"))?;
        let owner = segments
            .next()
            .ok_or_else(|| ApiError::invalid_request("could not parse an owner out of the url"))?;
        (owner.to_owned(), repo.to_owned(), trimmed.to_owned())
    } else {
        let mut parts = trimmed.splitn(2, '/');
        let owner = parts.next().filter(|s| !s.is_empty());
        let repo = parts.next().filter(|s| !s.is_empty());
        match (owner, repo) {
            (Some(owner), Some(repo)) => (
                owner.to_owned(),
                repo.to_owned(),
                format!("https://github.com/{owner}/{repo}.git"),
            ),
            _ => {
                return Err(ApiError::invalid_request(
                    "\"repo\" must be \"owner/name\" or an https url",
                ))
            }
        }
    };
    Ok(RepoRef {
        slug: format!("{owner}/{repo}"),
        owner,
        repo,
        source: RepoSource::Remote(url),
    })
}

pub struct Materialized {
    pub path: PathBuf,
    pub commit: String,
    pub branch: Option<String>,
}

/// Cheap HEAD resolution used by `POST /api/index`'s cache-hit fast path
/// (docs/API.md): answering "is this commit already cached" must not
/// require a full clone. `git ls-remote` talks to the remote without
/// fetching anything into a local repo; a local path is just `rev-parse`.
pub fn resolve_head(source: &RepoSource) -> Result<String> {
    match source {
        RepoSource::Local(path) => rev_parse(path, "HEAD"),
        RepoSource::Remote(url) => {
            let output = Command::new("git")
                .args(["ls-remote", url, "HEAD"])
                .output()
                .context("run git ls-remote")?;
            ensure!(
                output.status.success(),
                "git ls-remote failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            let sha = stdout
                .split_whitespace()
                .next()
                .ok_or_else(|| anyhow::anyhow!("git ls-remote returned no output"))?;
            Ok(sha.to_owned())
        }
    }
}

/// Clones (first time) or fetches + fast-forwards (subsequent times) a
/// remote into `<cache_dir>/repos/<owner>/<repo>`, or simply resolves HEAD
/// for a local path with no copy. Runs LRU eviction against
/// `limits.max_clone_bytes` after a remote materialisation, and enforces
/// the clone-size and history-depth caps before returning -- both checks
/// that must happen before the (much slower) indexing stage per
/// docs/API.md's "must not present as a timeout".
pub fn materialize(
    cache_dir: &Path,
    repo_ref: &RepoRef,
    limits: &Limits,
) -> Result<Materialized, ApiError> {
    match &repo_ref.source {
        RepoSource::Local(path) => {
            let commit = rev_parse(path, "HEAD").map_err(|e| ApiError::clone_failed(e.to_string()))?;
            let branch = current_branch(path);
            check_history_depth(path, limits)?;
            Ok(Materialized {
                path: path.clone(),
                commit,
                branch,
            })
        }
        RepoSource::Remote(url) => {
            let dest = cache_dir
                .join("repos")
                .join(&repo_ref.owner)
                .join(&repo_ref.repo);
            if dest.join(".git").is_dir() {
                fetch_and_fast_forward(&dest).map_err(|e| ApiError::clone_failed(e.to_string()))?;
            } else {
                std::fs::create_dir_all(dest.parent().unwrap())
                    .map_err(|e| ApiError::internal(e.to_string()))?;
                clone_blobless(url, &dest).map_err(|e| ApiError::clone_failed(e.to_string()))?;
            }
            touch(&dest);
            let size = directory_size(&dest).unwrap_or(0);
            if size > limits.max_clone_bytes {
                // Over budget as soon as we can measure it -- do not run
                // extraction over a repo we are about to reject anyway.
                return Err(ApiError::repo_too_large(format!(
                    "clone size {size} bytes exceeds the configured limit of {} bytes",
                    limits.max_clone_bytes
                )));
            }
            if let Err(err) = evict_lru(&cache_dir.join("repos"), limits.max_clone_bytes, &dest) {
                // Eviction is best-effort: failing to reclaim space for the
                // *next* job is not a reason to fail *this* one.
                eprintln!("cache eviction warning: {err:#}");
            }
            check_history_depth(&dest, limits)?;
            let commit = rev_parse(&dest, "HEAD").map_err(|e| ApiError::clone_failed(e.to_string()))?;
            let branch = current_branch(&dest);
            Ok(Materialized {
                path: dest,
                commit,
                branch,
            })
        }
    }
}

fn check_history_depth(repo: &Path, limits: &Limits) -> Result<(), ApiError> {
    let output = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "rev-list", "--count", "HEAD"])
        .output()
        .map_err(|e| ApiError::internal(format!("run git rev-list: {e}")))?;
    if !output.status.success() {
        // Not fatal to the request -- an unborn/empty repo (no commits
        // yet) fails `rev-list` too, and that is a legitimate (if useless)
        // input, not an oversized one. Let indexing itself reject it.
        return Ok(());
    }
    let count: usize = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    if count > limits.max_history_commits {
        return Err(ApiError::repo_too_large(format!(
            "history depth {count} commits exceeds the configured limit of {}",
            limits.max_history_commits
        )));
    }
    Ok(())
}

fn clone_blobless(url: &str, dest: &Path) -> Result<()> {
    let output = Command::new("git")
        .args([
            "clone",
            "--filter=blob:none",
            url,
            &dest.to_string_lossy(),
        ])
        .output()
        .context("run git clone")?;
    if !output.status.success() {
        bail!(
            "git clone {url} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn fetch_and_fast_forward(dest: &Path) -> Result<()> {
    let dest_str = dest.to_string_lossy();
    let fetch = Command::new("git")
        .args(["-C", &dest_str, "fetch", "--filter=blob:none", "--prune", "origin"])
        .output()
        .context("run git fetch")?;
    ensure!(
        fetch.status.success(),
        "git fetch failed: {}",
        String::from_utf8_lossy(&fetch.stderr).trim()
    );
    // `git clone` records the remote's default branch at
    // refs/remotes/origin/HEAD; reuse it so an update tracks the same
    // branch the initial clone checked out, without us having to remember
    // it separately.
    let symbolic = Command::new("git")
        .args([
            "-C",
            &dest_str,
            "symbolic-ref",
            "--short",
            "refs/remotes/origin/HEAD",
        ])
        .output()
        .context("run git symbolic-ref")?;
    ensure!(
        symbolic.status.success(),
        "git symbolic-ref failed: {}",
        String::from_utf8_lossy(&symbolic.stderr).trim()
    );
    // "origin/main" -> "main"
    let branch = String::from_utf8_lossy(&symbolic.stdout)
        .trim()
        .split('/')
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("unexpected symbolic-ref output"))?
        .to_owned();
    let checkout = Command::new("git")
        .args(["-C", &dest_str, "checkout", &branch])
        .output()
        .context("run git checkout")?;
    ensure!(
        checkout.status.success(),
        "git checkout {branch} failed: {}",
        String::from_utf8_lossy(&checkout.stderr).trim()
    );
    let reset = Command::new("git")
        .args(["-C", &dest_str, "reset", "--hard", &format!("origin/{branch}")])
        .output()
        .context("run git reset")?;
    ensure!(
        reset.status.success(),
        "git reset --hard failed: {}",
        String::from_utf8_lossy(&reset.stderr).trim()
    );
    Ok(())
}

fn rev_parse(repo: &Path, rev: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "rev-parse", rev])
        .output()
        .context("run git rev-parse")?;
    ensure!(
        output.status.success(),
        "git rev-parse {rev} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// `None` for a detached HEAD (`git rev-parse --abbrev-ref HEAD` prints
/// literally "HEAD" in that case) rather than treating "HEAD" as a branch
/// name.
fn current_branch(repo: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (name != "HEAD" && !name.is_empty()).then_some(name)
}

fn touch(dir: &Path) {
    let _ = std::fs::write(dir.join(".tolmap-last-used"), b"");
}

fn last_used(dir: &Path) -> SystemTime {
    std::fs::metadata(dir.join(".tolmap-last-used"))
        .and_then(|meta| meta.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Sums file sizes under `root` (following the working tree and `.git`
/// alike -- both count against the clone-size budget). A plain recursive
/// walk rather than shelling out to `du`: one less external-tool
/// dependency to assume is on `PATH`, and the size this needs is "bytes
/// this directory occupies", not `du`'s block-rounded view.
fn directory_size(root: &Path) -> Result<u64> {
    let mut total = 0u64;
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).with_context(|| format!("read_dir {}", dir.display()))? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                total += entry.metadata()?.len();
            }
            // Symlinks (rare in a git working tree, but possible) are not
            // followed -- counting a target outside `root` would make this
            // budget check meaningless.
        }
    }
    Ok(total)
}

/// Evicts least-recently-used `<owner>/<repo>` directories under
/// `repos_root` (by `.tolmap-last-used` mtime -- see `touch`) until the
/// total is back under `budget_bytes`. `keep` (the directory this call is
/// servicing) is never evicted, even if this is its first use and nothing
/// else makes that obvious yet.
fn evict_lru(repos_root: &Path, budget_bytes: u64, keep: &Path) -> Result<()> {
    let mut candidates = Vec::new();
    if !repos_root.is_dir() {
        return Ok(());
    }
    for owner_entry in std::fs::read_dir(repos_root)? {
        let owner_dir = owner_entry?.path();
        if !owner_dir.is_dir() {
            continue;
        }
        for repo_entry in std::fs::read_dir(&owner_dir)? {
            let repo_dir = repo_entry?.path();
            if !repo_dir.is_dir() || repo_dir == keep {
                continue;
            }
            let size = directory_size(&repo_dir).unwrap_or(0);
            candidates.push((last_used(&repo_dir), size, repo_dir));
        }
    }
    candidates.sort_by_key(|(when, _, _)| *when);
    let mut total: u64 = candidates.iter().map(|(_, size, _)| size).sum::<u64>()
        + directory_size(keep).unwrap_or(0);
    for (_, size, dir) in candidates {
        if total <= budget_bytes {
            break;
        }
        if std::fs::remove_dir_all(&dir).is_ok() {
            total = total.saturating_sub(size);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_slash_name_resolves_to_github() {
        let repo_ref = parse_repo_spec("pallets/flask").unwrap();
        assert_eq!(repo_ref.slug, "pallets/flask");
        assert_eq!(repo_ref.owner, "pallets");
        assert_eq!(repo_ref.repo, "flask");
        match repo_ref.source {
            RepoSource::Remote(url) => assert_eq!(url, "https://github.com/pallets/flask.git"),
            RepoSource::Local(_) => panic!("expected a remote source"),
        }
    }

    #[test]
    fn https_url_parses_owner_and_repo_from_the_path() {
        let repo_ref = parse_repo_spec("https://github.com/django/django.git").unwrap();
        assert_eq!(repo_ref.slug, "django/django");
        assert_eq!(repo_ref.owner, "django");
        assert_eq!(repo_ref.repo, "django");
    }

    #[test]
    fn https_url_without_dot_git_suffix_also_parses() {
        let repo_ref = parse_repo_spec("https://github.com/psf/requests").unwrap();
        assert_eq!(repo_ref.slug, "psf/requests");
    }

    #[test]
    fn a_local_path_slugs_as_local_basename() {
        let dir = tempfile::tempdir().unwrap();
        let repo_ref = resolve(None, Some(dir.path().to_str().unwrap())).unwrap();
        assert_eq!(repo_ref.owner, "local");
        assert!(repo_ref.slug.starts_with("local/"));
    }

    #[test]
    fn both_repo_and_path_is_rejected() {
        let err = resolve(Some("a/b"), Some("/tmp")).unwrap_err();
        assert_eq!(err.body.error, "invalid_request");
    }

    #[test]
    fn neither_repo_nor_path_is_rejected() {
        let err = resolve(None, None).unwrap_err();
        assert_eq!(err.body.error, "invalid_request");
    }
}
