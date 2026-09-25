//! Service configuration. Queue length and request frequency stay bounded;
//! repository size and job runtime are deliberately unbounded (issue #97).

use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;

use crate::naming::{NamerKind, DEFAULT_MODEL};
use crate::pipeline::PruneVariant;

#[derive(Clone, Debug)]
pub struct Limits {
    /// Total clone cache budget; eviction never rejects the active clone.
    /// Env: `TOLMAP_CLONE_CACHE_BYTES`.
    pub clone_cache_bytes: u64,
    /// Maximum number of blocking index bodies running at once. One fits
    /// the production 1 GB machine; raising this needs a memory measurement.
    /// Env: `TOLMAP_MAX_CONCURRENT_JOBS`.
    pub max_concurrent_jobs: usize,
    /// Pending jobs only, excluding running jobs. Sixteen keeps admission
    /// bounded on the 1 GB machine; with one worker and a 900 s job budget,
    /// pending requests still need a bounded queue on one worker.
    /// Env: `TOLMAP_MAX_QUEUED_JOBS`.
    pub max_queued_jobs: usize,
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
            clone_cache_bytes: 2 * 1024 * 1024 * 1024,
            max_concurrent_jobs: 1,
            max_queued_jobs: 16,
            rate_limit_per_ip: 30,
            rate_limit_window_seconds: 60,
            rate_limit_per_repo: 3,
            rate_limit_per_repo_window_seconds: 300,
        }
    }
}

impl Limits {
    /// Overlays queue, cache and rate settings on top of
    /// [`Limits::default`] -- an unset or unparsable variable falls back to
    /// the default rather than failing startup, since a typo'd env var
    /// should not take the whole service down when a working default
    /// exists. This is the one place any of these are read from the
    /// environment; everything downstream takes a `Limits` value.
    fn from_env() -> Self {
        let default = Limits::default();
        Limits {
            clone_cache_bytes: env_var_or("TOLMAP_CLONE_CACHE_BYTES", default.clone_cache_bytes),
            max_concurrent_jobs: env_var_or(
                "TOLMAP_MAX_CONCURRENT_JOBS",
                default.max_concurrent_jobs,
            )
            .max(1),
            max_queued_jobs: env_var_or("TOLMAP_MAX_QUEUED_JOBS", default.max_queued_jobs),
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
    /// Select the blend/prune route for maps built by the job service.
    /// Node-relative is the owner-approved default; every measured route
    /// remains available for controlled comparisons.
    ///
    /// Env: `TOLMAP_PRUNE_VARIANT` (`absolute`, `percentile`,
    /// `node-relative`, or `pre-rescale`).
    pub prune_variant: PruneVariant,
    /// `TOLMAP_NAMER` defaults to IDF; hosting does not enable model naming.
    pub namer: NamerKind,
    pub namer_model: String,
    /// Where job maps take their references from (issue #110): `hand` (the
    /// default) or `scip`. `scip` needs the indexers on the worker's `PATH`
    /// (P1b's image); without them every language records a fallback to
    /// the hand-written graph rather than failing.
    ///
    /// Env: `TOLMAP_REFS`.
    pub refs: crate::extract::RefsMode,
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
    /// Uid/gid the worker child (`service::jobs::process_worker_exe`) is
    /// dropped to when the service itself is running as root -- the
    /// Dockerfile runtime stage's case, and the only one where dropping
    /// privilege is even possible (`CommandExt::uid()/gid()` fails if the
    /// caller isn't root). Baked into the image as `TOLMAP_WORKER_UID` /
    /// `TOLMAP_WORKER_GID` `ENV`, the same image-layout-constant pattern as
    /// `TOLMAP_STATIC_DIR` -- not a deploy-time tunable, since the
    /// uid is fixed at image build time (`useradd --uid 10001` in the
    /// Dockerfile) and there is nothing an operator would ever want to
    /// retune per-deployment. Defaults below (10001/10001) only matter for
    /// local dev/CI, where the image's `ENV` is absent and the service is
    /// not root anyway, so no uid drop is attempted regardless of what
    /// these default to.
    ///
    /// Env: `TOLMAP_WORKER_UID` / `TOLMAP_WORKER_GID`.
    pub worker_uid: u32,
    pub worker_gid: u32,
}

impl ServeConfig {
    /// Reads overridable settings from the environment
    /// (`TOLMAP_PORT`, `TOLMAP_BIND_ADDR`, `TOLMAP_DB_PATH`,
    /// `TOLMAP_CACHE_DIR`, `TOLMAP_STATIC_DIR`,
    /// `TOLMAP_PRUNE_VARIANT`, `TOLMAP_RETAIN_COMMITS_PER_REPO`, plus
    /// `Limits::from_env`'s
    /// `TOLMAP_CLONE_CACHE_BYTES`/queue/rate variables); anything left unset uses its
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
        // queue/rate variable).
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
        let prune_variant = env_var_or("TOLMAP_PRUNE_VARIANT", PruneVariant::default());
        let namer = env_var_or("TOLMAP_NAMER", NamerKind::Idf);
        let namer_model =
            env::var("TOLMAP_NAMER_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_owned());
        let refs = env_var_or("TOLMAP_REFS", crate::extract::RefsMode::Hand);
        // 20 is generous for a debugging/time-travel window (which commit
        // looked like what) while still being a bound instead of the
        // unbounded growth issue #23 gap 2 reported -- see store::prune.
        let retain_commits_per_repo = env_var_or("TOLMAP_RETAIN_COMMITS_PER_REPO", 20usize);
        // 10001 is arbitrary but fixed -- it just has to not collide with
        // anything else the runtime image's base (debian:bookworm-slim)
        // creates. See the Dockerfile's `useradd` line, which is where the
        // value that actually matters in production is baked in.
        let worker_uid = env_var_or("TOLMAP_WORKER_UID", 10001u32);
        let worker_gid = env_var_or("TOLMAP_WORKER_GID", 10001u32);
        ServeConfig {
            bind: SocketAddr::new(bind_ip, port),
            db_path,
            cache_dir,
            static_dir,
            prune_variant,
            namer,
            namer_model,
            refs,
            limits: Limits::from_env(),
            retain_commits_per_repo,
            worker_uid,
            worker_gid,
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
            "TOLMAP_CLONE_CACHE_BYTES",
            "TOLMAP_MAX_CONCURRENT_JOBS",
            "TOLMAP_MAX_QUEUED_JOBS",
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
        assert_eq!(from_env.clone_cache_bytes, defaults.clone_cache_bytes);
        assert_eq!(from_env.max_concurrent_jobs, 1);
        assert_eq!(from_env.max_queued_jobs, 16);
        assert_eq!(from_env.rate_limit_per_repo, defaults.rate_limit_per_repo);

        // Set: the environment value wins, without a rebuild.
        env::set_var("TOLMAP_CLONE_CACHE_BYTES", "1024");
        env::set_var("TOLMAP_RATE_LIMIT_PER_IP", "5");
        env::set_var("TOLMAP_MAX_CONCURRENT_JOBS", "2");
        env::set_var("TOLMAP_MAX_QUEUED_JOBS", "4");
        let overridden = Limits::from_env();
        assert_eq!(overridden.clone_cache_bytes, 1024);
        assert_eq!(overridden.rate_limit_per_ip, 5);
        assert_eq!(overridden.max_concurrent_jobs, 2);
        assert_eq!(overridden.max_queued_jobs, 4);

        // Garbage: falls back to the default instead of panicking the
        // service on startup or silently coercing to 0.
        env::set_var("TOLMAP_CLONE_CACHE_BYTES", "not-a-number");
        env::set_var("TOLMAP_MAX_CONCURRENT_JOBS", "0");
        env::set_var("TOLMAP_MAX_QUEUED_JOBS", "bad");
        let garbage = Limits::from_env();
        assert_eq!(garbage.clone_cache_bytes, defaults.clone_cache_bytes);
        assert_eq!(garbage.max_concurrent_jobs, 1);
        assert_eq!(garbage.max_queued_jobs, defaults.max_queued_jobs);

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
    fn prune_variant_defaults_to_node_relative_and_accepts_every_route() {
        let _guard = lock_env();
        env::remove_var("TOLMAP_PRUNE_VARIANT");
        assert_eq!(
            ServeConfig::from_env().prune_variant,
            PruneVariant::NodeRelative
        );

        for (value, expected) in [
            ("absolute", PruneVariant::Absolute),
            ("percentile", PruneVariant::Percentile),
            ("node-relative", PruneVariant::NodeRelative),
            ("pre-rescale", PruneVariant::PreRescale),
        ] {
            env::set_var("TOLMAP_PRUNE_VARIANT", value);
            assert_eq!(ServeConfig::from_env().prune_variant, expected);
        }

        env::set_var("TOLMAP_PRUNE_VARIANT", "invalid");
        assert_eq!(
            ServeConfig::from_env().prune_variant,
            PruneVariant::NodeRelative
        );
        env::remove_var("TOLMAP_PRUNE_VARIANT");
    }
}
