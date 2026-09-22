//! Configuration for `tolmap serve`, and the limits a public-facing clone-
//! and-index service needs (docs/ARCHITECTURE.md's "Limits" section).
//!
//! Every field has a documented default rather than a magic number sitting
//! in `jobs.rs`/`clone.rs` -- the brief is explicit that this belongs in one
//! place. Defaults are picked to comfortably admit django (851 files, the
//! largest fixture) with headroom: the corpus is the floor, not the ceiling.
//! `max_history_commits` is the one field this did not actually hold for
//! until 2026-09-20 -- see its own doc comment below.
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

use crate::geometry::TerrainMode;

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
    /// Reject a repository whose `HEAD` history (`git rev-list --count
    /// HEAD`) has more commits than this.
    ///
    /// **This does not bound indexing cost** -- correcting a false claim an
    /// earlier version of this comment made. `extract.rs`'s `git_history`
    /// call is hardcoded to walk only the most recent 4000 commits for
    /// co-change no matter how deep `HEAD`'s history is (see
    /// `service::clone`'s module doc comment), so a repository with 200,000
    /// commits costs `extract::build` exactly what one with 4,000 does.
    ///
    /// What it actually bounds depends on which of
    /// `service::clone::materialize`'s two paths a request takes -- an
    /// earlier version of *this* correction also overstated it, by
    /// describing only one of the two paths as if it were both:
    ///
    /// - `RepoSource::Local` (fixtures and tests; nothing is cloned):
    ///   `check_history_depth` runs first, before anything else, so it
    ///   genuinely pre-empts work -- a repository over the limit never
    ///   reaches `detect`/`extract::build`.
    /// - `RepoSource::Remote`: `clone_blobless`/`fetch_and_fast_forward`
    ///   run *first*, so the clone -- network transfer and disk writes
    ///   alike -- has already happened by the time this check runs.
    ///   `check_clone_size` has also already measured the real,
    ///   already-materialised size with `du -sb` and rejected an
    ///   oversized clone on its own. So on this path
    ///   `check_history_depth` is a **post-clone refusal to index**, not a
    ///   pre-clone gate: it cannot save the clone cost, because that cost
    ///   is already paid by the time it runs. What it still catches that
    ///   `check_clone_size` alone cannot is a repository with an enormous
    ///   commit *count* but a small byte footprint (many tiny or
    ///   near-empty commits can keep total `.git` bytes modest while
    ///   `git rev-list`-style traversals over that history stay expensive
    ///   in wall-clock terms, independent of size) -- a real but narrow
    ///   case, not a restatement of the size check. Whether that narrow
    ///   case earns a second, independent knob here rather than just
    ///   tightening `max_clone_bytes` is an open question this change
    ///   does not resolve -- see the PR that added this correction.
    ///
    /// The default used to be 20,000, on the belief that this "comfortably
    /// admits django ... with headroom" (this file's module doc comment).
    /// That was false and untested: django's real history is 34,942
    /// commits (measured 2026-09-20 via `TOLMAP_FIXTURE_REPOS`, `git -C
    /// django rev-list --count HEAD`), so the old default rejected the
    /// fixture the comment claimed it admitted. 200,000 restores roughly
    /// the same ~5.9x headroom over django's real depth that `max_files`
    /// keeps over its file count (5,000 / 851 ~= 5.87), rather than a
    /// number picked to sound safe. That part of this change stands
    /// regardless of the open question above -- a repository this deep
    /// should be admitted either way.
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
            max_history_commits: 200_000, // see field doc comment -- was 20_000, rejected django
            max_job_seconds: 900,         // 15 minutes
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
    /// Loopback by default -- see docs/API.md. Until 2026-09-20 this was
    /// hardcoded and undocumented as ever changing ("Not configurable via
    /// environment or flag: making this service reachable off-box is a
    /// separate, later decision this change does not make even as an
    /// opt-in", is what this comment used to say). That later decision has
    /// now been made: the Fly.io deployment needs the service reachable
    /// from outside the container, so it is configurable via
    /// `TOLMAP_BIND_ADDR` -- but the default is unchanged, so a developer
    /// who sets nothing still gets today's loopback-only behaviour. An
    /// unparsable value falls back to the default rather than panicking,
    /// matching `Limits::from_env`'s existing handling of bad input (see
    /// `env_var_or`).
    ///
    /// Env: `TOLMAP_BIND_ADDR`.
    pub bind: SocketAddr,
    pub db_path: PathBuf,
    pub cache_dir: PathBuf,
    /// Serve the built web bundle (and fall back unmatched non-`/api`
    /// paths to its `index.html`, for the client-side router) alongside the
    /// API when set -- see `service::http::router`. `None` (the default)
    /// keeps the router exactly as it was before this existed: `/api/*`
    /// only, nothing else registered. Local development runs the Vite dev
    /// server separately and proxies `/api` to this service
    /// (`web/vite.config.ts`), so this must stay opt-in rather than
    /// defaulting to some in-repo path that would silently start competing
    /// with that proxy.
    ///
    /// Env: `TOLMAP_STATIC_DIR`.
    pub static_dir: Option<PathBuf>,
    /// Select terrain-aware subdivision for maps built by the job service.
    /// The service stays off when unset so each hosted environment must make
    /// its rollout choice explicitly.
    ///
    /// Env: `TOLMAP_TERRAIN` (`false`, `auto`, or `true`).
    pub terrain: TerrainMode,
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
    /// (`TOLMAP_PORT`, `TOLMAP_BIND_ADDR`, `TOLMAP_DB_PATH`,
    /// `TOLMAP_CACHE_DIR`, `TOLMAP_STATIC_DIR`, `TOLMAP_TERRAIN`,
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
        // Loopback unless told otherwise -- see the `bind` field's doc
        // comment. `env_var_or` falls back to the default on anything that
        // does not parse as an `IpAddr`, so a typo'd TOLMAP_BIND_ADDR can't
        // take the service down at startup (same handling as every
        // TOLMAP_MAX_*/TOLMAP_RATE_LIMIT_* variable).
        let bind_ip = env_var_or("TOLMAP_BIND_ADDR", IpAddr::V4(Ipv4Addr::LOCALHOST));
        let cache_dir = env::var("TOLMAP_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| env::temp_dir().join("tolmap-cache"));
        let db_path = env::var("TOLMAP_DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| cache_dir.join("tolmap.sqlite3"));
        // Unset (the default) keeps the router exactly as it is without
        // this -- see the `static_dir` field's doc comment.
        let static_dir = env::var("TOLMAP_STATIC_DIR").ok().map(PathBuf::from);
        // Unlike the CLI's Auto default, the hosted service stays Off unless
        // an operator explicitly chooses Auto or On for that environment.
        let terrain = env_var_or("TOLMAP_TERRAIN", TerrainMode::Off);
        // 20 is generous for a debugging/time-travel window (which commit
        // looked like what) while still being a bound instead of the
        // unbounded growth issue #23 gap 2 reported -- see store::prune.
        let retain_commits_per_repo = env_var_or("TOLMAP_RETAIN_COMMITS_PER_REPO", 20usize);
        ServeConfig {
            bind: SocketAddr::new(bind_ip, port),
            db_path,
            cache_dir,
            static_dir,
            terrain,
            limits: Limits::from_env(),
            retain_commits_per_repo,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // std::env is process-global and cargo test runs test functions
    // concurrently within one process (on separate threads by default), so
    // two tests that touch overlapping TOLMAP_* keys race each other
    // directly. This used to be "solved" by an assumption -- "neither
    // test's key set overlaps the other's" -- which held only by accident
    // and broke the moment bind_addr_unset_still_binds_loopback and
    // bind_addr_env_override_and_garbage_fallback were added: both touch
    // TOLMAP_PORT, one via `remove_var` expecting 8787, the other via
    // `set_var("...", "9999")`, and interleaved runs produced the wrong
    // port in whichever test read it mid-mutation (measured: ~15-17% of
    // `cargo test --release --lib service::config` runs at default
    // parallelism). Proving every test's key set disjoint from every
    // other's by inspection is exactly the kind of thing that quietly
    // stops being true the next time someone adds a test, so it is not
    // the fix -- serialising is. Every test in this module that sets or
    // removes a `TOLMAP_*` var takes this lock for its whole body (acquired
    // once, at the top, not re-acquired around each env call, so the whole
    // read-mutate-assert-restore sequence is atomic with respect to every
    // other such test). `lock_env` recovers from a poisoned lock rather
    // than propagating one test's panic into every later test's failure,
    // since several of these tests deliberately assert on bad input.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn limits_from_env_overrides_defaults_and_falls_back_on_garbage() {
        let _guard = lock_env();
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
        let _guard = lock_env();
        env::remove_var("TOLMAP_RETAIN_COMMITS_PER_REPO");
        assert_eq!(env_var_or("TOLMAP_RETAIN_COMMITS_PER_REPO", 20usize), 20);
        env::set_var("TOLMAP_RETAIN_COMMITS_PER_REPO", "3");
        assert_eq!(env_var_or("TOLMAP_RETAIN_COMMITS_PER_REPO", 20usize), 3);
        env::remove_var("TOLMAP_RETAIN_COMMITS_PER_REPO");
    }

    // TOLMAP_BIND_ADDR / TOLMAP_STATIC_DIR are read directly in
    // ServeConfig::from_env, not through Limits, so they get their own
    // test rather than living in limits_from_env_overrides_defaults_and_falls_back_on_garbage.
    #[test]
    fn bind_addr_unset_still_binds_loopback() {
        let _guard = lock_env();
        env::remove_var("TOLMAP_BIND_ADDR");
        env::remove_var("TOLMAP_PORT");
        let config = ServeConfig::from_env();
        assert_eq!(
            config.bind,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8787)
        );
    }

    #[test]
    fn bind_addr_env_override_and_garbage_fallback() {
        let _guard = lock_env();
        env::set_var("TOLMAP_PORT", "9999");

        env::set_var("TOLMAP_BIND_ADDR", "0.0.0.0");
        let config = ServeConfig::from_env();
        assert_eq!(
            config.bind,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 9999),
            "an opt-in TOLMAP_BIND_ADDR must be honoured -- that opt-in is the point of this change"
        );

        // Garbage falls back to the loopback default rather than failing
        // startup, matching every other TOLMAP_* variable's handling.
        env::set_var("TOLMAP_BIND_ADDR", "not-an-ip");
        let config = ServeConfig::from_env();
        assert_eq!(config.bind.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));

        env::remove_var("TOLMAP_BIND_ADDR");
        env::remove_var("TOLMAP_PORT");
    }

    #[test]
    fn static_dir_unset_by_default_set_when_configured() {
        let _guard = lock_env();
        env::remove_var("TOLMAP_STATIC_DIR");
        assert_eq!(ServeConfig::from_env().static_dir, None);

        env::set_var("TOLMAP_STATIC_DIR", "/srv/tolmap/web");
        assert_eq!(
            ServeConfig::from_env().static_dir,
            Some(PathBuf::from("/srv/tolmap/web"))
        );
        env::remove_var("TOLMAP_STATIC_DIR");
    }

    #[test]
    fn terrain_is_off_by_default_and_requires_an_explicit_valid_opt_in() {
        let _guard = lock_env();
        env::remove_var("TOLMAP_TERRAIN");
        assert_eq!(ServeConfig::from_env().terrain, TerrainMode::Off);

        env::set_var("TOLMAP_TERRAIN", "true");
        assert_eq!(ServeConfig::from_env().terrain, TerrainMode::On);

        env::set_var("TOLMAP_TERRAIN", "auto");
        assert_eq!(ServeConfig::from_env().terrain, TerrainMode::Auto);

        env::set_var("TOLMAP_TERRAIN", "false");
        assert_eq!(ServeConfig::from_env().terrain, TerrainMode::Off);

        // Match every other typed setting: invalid input falls back to the
        // safe documented default instead of changing startup behaviour.
        env::set_var("TOLMAP_TERRAIN", "not-a-boolean");
        assert_eq!(ServeConfig::from_env().terrain, TerrainMode::Off);
        env::remove_var("TOLMAP_TERRAIN");
    }

    /// The regression this change fixes: `max_history_commits`'s default
    /// used to be 20,000, and django's real history is 34,942 commits (see
    /// the field's doc comment for how that was measured) -- so the old
    /// default rejected the exact fixture the module doc comment claimed
    /// it "comfortably admits ... with headroom". This pins both the fact
    /// (django's real depth) and the headroom this change restores, so a
    /// future change that lowers the default again gets caught here
    /// instead of only when someone actually tries to index django.
    #[test]
    fn default_history_limit_keeps_headroom_over_djangos_real_history() {
        const DJANGO_REAL_HISTORY_COMMITS: usize = 34_942;
        let default = Limits::default().max_history_commits;
        assert!(
            default > DJANGO_REAL_HISTORY_COMMITS,
            "default max_history_commits ({default}) must admit django's real history \
             ({DJANGO_REAL_HISTORY_COMMITS} commits)"
        );
        // Same ~5.9x headroom style as max_files (5_000 / 851 ~= 5.87) --
        // not just "bigger than django", a deliberately chosen margin.
        assert!(
            (default as f64) / (DJANGO_REAL_HISTORY_COMMITS as f64) > 5.0,
            "default should keep roughly max_files's headroom ratio over the real fixture, \
             not just clear it narrowly"
        );
    }
}
