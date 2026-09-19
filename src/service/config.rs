//! Configuration for `tolmap serve`, and the limits a public-facing clone-
//! and-index service needs (docs/ARCHITECTURE.md's "Limits" section).
//!
//! Every field has a documented default rather than a magic number sitting
//! in `jobs.rs`/`clone.rs` -- the brief is explicit that this belongs in one
//! place. Defaults are picked to comfortably admit django (851 files, the
//! largest fixture) with headroom: the corpus is the floor, not the ceiling.
//!
//! **Every default here is also overridable by an environment variable**
//! (issue #23 gap 4: these were defaults in code with no way to change one
//! without a rebuild). `Limits::from_env` is the one place that reads
//! `TOLMAP_MAX_*`/`TOLMAP_RATE_LIMIT_*` -- request-path code
//! (`clone.rs`/`jobs.rs`/`http.rs`) only ever reads `Limits` fields, never
//! `std::env::var` itself, so this stays the single source of truth rather
//! than env lookups scattering across the request path.

use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Clone, Debug)]
pub struct Limits {
    /// Reject a repository with more source files than this, after
    /// detection has picked a language and root. django is 851; the
    /// default leaves ~6x headroom for a larger monorepo before it is
    /// worth raising deliberately rather than by surprise.
    ///
    /// Env: `TOLMAP_MAX_FILES`.
    pub max_files: usize,
    /// Reject a clone whose working tree + `.git` exceeds this many bytes.
    /// Checked with `du -sb` on the materialised clone before indexing
    /// starts, so an oversized repo fails as `repo_too_large`, not partway
    /// through a long extract.
    ///
    /// Env: `TOLMAP_MAX_CLONE_BYTES`.
    pub max_clone_bytes: u64,
    /// Cap on how many commits `git log` is asked to walk for co-change
    /// (mirrors `extract.rs`'s existing internal 4000, but enforced here
    /// too as a pre-check on `git rev-list --count`, so a repository with
    /// an enormous history is rejected as `repo_too_large` before cloning
    /// rather than discovered to be slow only after `extract::build` is
    /// already running).
    ///
    /// Env: `TOLMAP_MAX_HISTORY_COMMITS`.
    pub max_history_commits: usize,
    /// Wall-clock budget for one job (clone + detect + index), enforced
    /// with `tokio::time::timeout` around the whole job so a stuck job
    /// fails as `index_failed` with a clear timeout message instead of
    /// hanging a worker forever.
    ///
    /// Env: `TOLMAP_MAX_JOB_SECONDS`.
    pub max_job_seconds: u64,
    /// Requests per window, per source IP, across all endpoints under
    /// `/api/`. A generous default -- this is a floor against accidental
    /// hammering (a retry loop, a misconfigured client), not a product
    /// throttle.
    ///
    /// Env: `TOLMAP_RATE_LIMIT_PER_IP` / `TOLMAP_RATE_LIMIT_WINDOW_SECONDS`.
    pub rate_limit_per_ip: u32,
    pub rate_limit_window_seconds: u64,
    /// `POST /api/index` requests for the same slug within the window.
    /// Tighter than the per-IP limit: re-requesting the same repo
    /// repeatedly is the specific abuse pattern this guards (each request
    /// that misses cache burns a full clone + index), independent of who
    /// is asking.
    ///
    /// Env: `TOLMAP_RATE_LIMIT_PER_REPO` / `TOLMAP_RATE_LIMIT_PER_REPO_WINDOW_SECONDS`.
    pub rate_limit_per_repo: u32,
    pub rate_limit_per_repo_window_seconds: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_files: 5_000,
            max_clone_bytes: 2 * 1024 * 1024 * 1024, // 2 GiB
            max_history_commits: 20_000,
            max_job_seconds: 900, // 15 minutes
            rate_limit_per_ip: 30,
            rate_limit_window_seconds: 60,
            rate_limit_per_repo: 3,
            rate_limit_per_repo_window_seconds: 300,
        }
    }
}

impl Limits {
    /// Overlays `TOLMAP_MAX_*`/`TOLMAP_RATE_LIMIT_*` on top of
    /// [`Limits::default`] -- an unset or unparsable variable falls back to
    /// the default rather than failing startup, since a typo'd env var
    /// should not take the whole service down when a working default
    /// exists. This is the one place any of these are read from the
    /// environment; everything downstream takes a `Limits` value.
    fn from_env() -> Self {
        let default = Limits::default();
        Limits {
            max_files: env_var_or("TOLMAP_MAX_FILES", default.max_files),
            max_clone_bytes: env_var_or("TOLMAP_MAX_CLONE_BYTES", default.max_clone_bytes),
            max_history_commits: env_var_or(
                "TOLMAP_MAX_HISTORY_COMMITS",
                default.max_history_commits,
            ),
            max_job_seconds: env_var_or("TOLMAP_MAX_JOB_SECONDS", default.max_job_seconds),
            rate_limit_per_ip: env_var_or("TOLMAP_RATE_LIMIT_PER_IP", default.rate_limit_per_ip),
            rate_limit_window_seconds: env_var_or(
                "TOLMAP_RATE_LIMIT_WINDOW_SECONDS",
                default.rate_limit_window_seconds,
            ),
            rate_limit_per_repo: env_var_or(
                "TOLMAP_RATE_LIMIT_PER_REPO",
                default.rate_limit_per_repo,
            ),
            rate_limit_per_repo_window_seconds: env_var_or(
                "TOLMAP_RATE_LIMIT_PER_REPO_WINDOW_SECONDS",
                default.rate_limit_per_repo_window_seconds,
            ),
        }
    }
}

/// Parses an environment variable of any numeric `Limits` field type,
/// falling back to `default` when the variable is unset *or* fails to
/// parse -- see [`Limits::from_env`].
fn env_var_or<T: FromStr>(key: &str, default: T) -> T {
    env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[derive(Clone, Debug)]
pub struct ServeConfig {
    /// Loopback only, by design -- see docs/API.md and the milestone brief.
    /// Not configurable via environment or flag: making this service
    /// reachable off-box is a separate, later decision this change does
    /// not make even as an opt-in.
    pub bind: SocketAddr,
    pub db_path: PathBuf,
    pub cache_dir: PathBuf,
    pub limits: Limits,
    /// Store retention policy (issue #23 gap 2): the number of most-recently-
    /// indexed commits kept per repository slug; older `(slug, commit_sha)`
    /// rows and their map files are pruned once a newer one lands. The
    /// single newest row for a slug is never pruned regardless of this
    /// value -- see `store::Store::prune` and docs/FINDINGS.md finding 4,
    /// which is exactly what would regress if it were.
    ///
    /// Env: `TOLMAP_RETAIN_COMMITS_PER_REPO`.
    pub retain_commits_per_repo: usize,
}

impl ServeConfig {
    /// Reads overridable settings from the environment
    /// (`TOLMAP_PORT`, `TOLMAP_DB_PATH`, `TOLMAP_CACHE_DIR`,
    /// `TOLMAP_RETAIN_COMMITS_PER_REPO`, plus `Limits::from_env`'s
    /// `TOLMAP_MAX_*`/`TOLMAP_RATE_LIMIT_*`); anything left unset uses its
    /// documented default. No config file yet -- the job service has no
    /// equivalent of `.tolmap/config.toml` to read limits from, and env
    /// vars are enough for what an operator needs to move today.
    pub fn from_env() -> Self {
        // 8787 is fixed by the frontend contract: its dev proxy defaults to
        // 127.0.0.1:8787 (see docs/API.md). Still overridable via
        // TOLMAP_PORT for a deployment that puts something else in front.
        let port: u16 = env::var("TOLMAP_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(8787);
        let cache_dir = env::var("TOLMAP_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| env::temp_dir().join("tolmap-cache"));
        let db_path = env::var("TOLMAP_DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| cache_dir.join("tolmap.sqlite3"));
        // 20 is generous for a debugging/time-travel window (which commit
        // looked like what) while still being a bound instead of the
        // unbounded growth issue #23 gap 2 reported -- see store::prune.
        let retain_commits_per_repo = env_var_or("TOLMAP_RETAIN_COMMITS_PER_REPO", 20usize);
        ServeConfig {
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            db_path,
            cache_dir,
            limits: Limits::from_env(),
            retain_commits_per_repo,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // std::env is process-global and cargo test runs test functions
    // concurrently within one process, so two tests touching the same
    // TOLMAP_* key could race. Everything these tests set, they clear
    // before returning, and neither test's key set overlaps the other's.
    #[test]
    fn limits_from_env_overrides_defaults_and_falls_back_on_garbage() {
        let keys = [
            "TOLMAP_MAX_FILES",
            "TOLMAP_MAX_CLONE_BYTES",
            "TOLMAP_MAX_HISTORY_COMMITS",
            "TOLMAP_MAX_JOB_SECONDS",
            "TOLMAP_RATE_LIMIT_PER_IP",
            "TOLMAP_RATE_LIMIT_WINDOW_SECONDS",
            "TOLMAP_RATE_LIMIT_PER_REPO",
            "TOLMAP_RATE_LIMIT_PER_REPO_WINDOW_SECONDS",
        ];
        for key in keys {
            env::remove_var(key);
        }

        // Nothing set: every field matches the documented default.
        let defaults = Limits::default();
        let from_env = Limits::from_env();
        assert_eq!(from_env.max_files, defaults.max_files);
        assert_eq!(from_env.max_clone_bytes, defaults.max_clone_bytes);
        assert_eq!(from_env.max_history_commits, defaults.max_history_commits);
        assert_eq!(from_env.rate_limit_per_repo, defaults.rate_limit_per_repo);

        // Set: the environment value wins, without a rebuild.
        env::set_var("TOLMAP_MAX_FILES", "10");
        env::set_var("TOLMAP_MAX_CLONE_BYTES", "1024");
        env::set_var("TOLMAP_RATE_LIMIT_PER_IP", "5");
        let overridden = Limits::from_env();
        assert_eq!(overridden.max_files, 10);
        assert_eq!(overridden.max_clone_bytes, 1024);
        assert_eq!(overridden.rate_limit_per_ip, 5);
        // A field with no override still reads its default.
        assert_eq!(overridden.max_history_commits, defaults.max_history_commits);

        // Garbage: falls back to the default instead of panicking the
        // service on startup or silently coercing to 0.
        env::set_var("TOLMAP_MAX_FILES", "not-a-number");
        let garbage = Limits::from_env();
        assert_eq!(garbage.max_files, defaults.max_files);

        for key in keys {
            env::remove_var(key);
        }
    }

    #[test]
    fn retain_commits_per_repo_env_override() {
        env::remove_var("TOLMAP_RETAIN_COMMITS_PER_REPO");
        assert_eq!(env_var_or("TOLMAP_RETAIN_COMMITS_PER_REPO", 20usize), 20);
        env::set_var("TOLMAP_RETAIN_COMMITS_PER_REPO", "3");
        assert_eq!(env_var_or("TOLMAP_RETAIN_COMMITS_PER_REPO", 20usize), 3);
        env::remove_var("TOLMAP_RETAIN_COMMITS_PER_REPO");
    }
}
