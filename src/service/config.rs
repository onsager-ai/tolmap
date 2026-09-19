//! Configuration for `tolmap serve`, and the limits a public-facing clone-
//! and-index service needs (docs/ARCHITECTURE.md's "Limits" section).
//!
//! Every field has a documented default rather than a magic number sitting
//! in `jobs.rs`/`clone.rs` -- the brief is explicit that this belongs in one
//! place. Defaults are picked to comfortably admit django (851 files, the
//! largest fixture) with headroom: the corpus is the floor, not the ceiling.

use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Limits {
    /// Reject a repository with more source files than this, after
    /// detection has picked a language and root. django is 851; the
    /// default leaves ~6x headroom for a larger monorepo before it is
    /// worth raising deliberately rather than by surprise.
    pub max_files: usize,
    /// Reject a clone whose working tree + `.git` exceeds this many bytes.
    /// Checked with `du -sb` on the materialised clone before indexing
    /// starts, so an oversized repo fails as `repo_too_large`, not partway
    /// through a long extract.
    pub max_clone_bytes: u64,
    /// Cap on how many commits `git log` is asked to walk for co-change
    /// (mirrors `extract.rs`'s existing internal 4000, but enforced here
    /// too as a pre-check on `git rev-list --count`, so a repository with
    /// an enormous history is rejected as `repo_too_large` before cloning
    /// rather than discovered to be slow only after `extract::build` is
    /// already running).
    pub max_history_commits: usize,
    /// Wall-clock budget for one job (clone + detect + index), enforced
    /// with `tokio::time::timeout` around the whole job so a stuck job
    /// fails as `index_failed` with a clear timeout message instead of
    /// hanging a worker forever.
    pub max_job_seconds: u64,
    /// Requests per window, per source IP, across all endpoints under
    /// `/api/`. A generous default -- this is a floor against accidental
    /// hammering (a retry loop, a misconfigured client), not a product
    /// throttle.
    pub rate_limit_per_ip: u32,
    pub rate_limit_window_seconds: u64,
    /// `POST /api/index` requests for the same slug within the window.
    /// Tighter than the per-IP limit: re-requesting the same repo
    /// repeatedly is the specific abuse pattern this guards (each request
    /// that misses cache burns a full clone + index), independent of who
    /// is asking.
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
}

impl ServeConfig {
    /// Reads overridable settings from the environment
    /// (`TOLMAP_PORT`, `TOLMAP_DB_PATH`, `TOLMAP_CACHE_DIR`); everything
    /// else uses `Limits::default()`. No config file yet -- the job
    /// service has no equivalent of `.tolmap/config.toml` to read limits
    /// from, and env vars are enough for the one thing an operator needs
    /// to move today (where the cache lives, e.g. off a small root disk).
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
        ServeConfig {
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            db_path,
            cache_dir,
            limits: Limits::default(),
        }
    }
}
