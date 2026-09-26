//! The executor (docs/WORKER_TIER.md §2): everything between "this job is
//! ready to run" and "the job child has exited". It materialises the clone
//! in this host's own cache, copies it into a fresh job directory, hardens
//! that directory, stages the inputs, spawns the job child with a v1
//! `WorkerSpec`, relays its events, answers `install_request` with the
//! local sandbox, kills the process group on cancel and reaps the child
//! with `wait_with_peak`.
//!
//! **Why its inputs are what they are.** In local mode the master calls
//! [`execute`] in-process, between `jobs::prepare` and `jobs::register`. In
//! remote mode (#97 phase 1, later PRs) an agent on a worker host calls the
//! same function, and a worker host never holds the database (§1, §5.2).
//! So [`execute`] takes no `AppState`, no `Store` and no `ServeConfig`: a
//! job description ([`JobSpec`]), the job's inputs ([`JobInputs`]: a names
//! cache by value and previous-map files by path), this host's environment
//! ([`ExecEnv`], every field named), and two narrow callbacks: an
//! [`EventSink`] for what the child reports and a [`CancelProbe`] for
//! whether to stop. Nothing reachable from those arguments can open the
//! store, which is the compile-level guard: a change that needs the store
//! in here has to change this signature first, and that is the review
//! point. Store reads belong in `jobs::prepare`, store writes in
//! `jobs::register`.
//!
//! The job child itself does not change (§2, "The executor moves; the job
//! child does not change"): it gets the same `WorkerSpec` on stdin and
//! writes the same v1 events on stdout as before this module existed.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::progress::StageId;
use crate::service::clone::{self, RepoRef, RepoSource};
use crate::service::error::{ApiError, ErrorBody};
use crate::service::worker_result;
use crate::worker::{JobSpec, LocalInputs, PreviousMap, WorkerEvent, WorkerSpec};

/// This host's side of a job: where its caches and job directories live,
/// which binary is the job child and which uid it drops to. The master
/// builds one from its `ServeConfig` (`jobs::exec_env`); an agent will
/// build one from its own flags. None of it names the store.
pub struct ExecEnv {
    /// Root of this host's shared clone cache; clones live under
    /// `<clone_cache>/repos/<owner>/<repo>` (`clone::materialize_with_progress`).
    pub clone_cache: PathBuf,
    /// The clone cache's LRU budget (`TOLMAP_CLONE_CACHE_BYTES` on the
    /// master). Also sent to the child in its `WorkerSpec`, as before.
    pub clone_cache_bytes: u64,
    /// Per-job directories are made at `<job_root>/<owner>/<repo>/<job id>`.
    pub job_root: PathBuf,
    /// Dependency-install scratch directories are made at
    /// `<install_root>/<job id>`, outside the job directory the worker's uid
    /// owns (see [`ServiceInstall`]).
    pub install_root: PathBuf,
    /// The `tolmap` binary to run as `tolmap worker`.
    pub worker_exe: PathBuf,
    /// The uid/gid the child drops to when this process is root.
    pub worker_uid: u32,
    pub worker_gid: u32,
    /// Whether `OPENROUTER_API_KEY` joins the child's environment allowlist
    /// -- see [`WorkerHardening::allow_openrouter_key`].
    pub allow_openrouter_key: bool,
}

/// A warm-start candidate: a stored map the job may start from. In local
/// mode `path` is the master's own store file, read here at the master's
/// uid and copied into the job directory before the child gets it (issue
/// #141, [`stage_previous_map`]); the child never sees `path` itself.
#[derive(Clone, Debug)]
pub struct PreviousMapInput {
    pub commit: String,
    pub branch: Option<String>,
    pub path: PathBuf,
}

/// What `jobs::prepare` read from the store for this job, handed over as
/// data rather than as a way to read more.
pub struct JobInputs {
    /// The slug's district names (CLAUDE.md: never rename a district
    /// without the previous name in hand).
    pub names: crate::naming::NameCache,
    /// Newest first, as `Store::warm_start_candidates` returns them.
    pub previous_maps: Vec<PreviousMapInput>,
}

/// What the child reported and the executor observed, for
/// `jobs::register`. `job_dir` still exists: the caller removes it once the
/// result is registered (or refused), because registration reads
/// `output_dir` inside it.
pub struct Executed {
    pub job_dir: PathBuf,
    pub output_dir: PathBuf,
    /// The executor's own record of what it handed the child: the commit
    /// and branch the result is stored under come from here, not from the
    /// child, whose copy of the checkout is its to change.
    pub checkout: clone::Materialized,
    pub output: WorkerOutput,
}

/// Where everything the child reports goes. The master implements it
/// against the job's snapshot and the registry (`jobs::SnapshotSink`),
/// exactly as `process_worker_exe` updated them before the split; an agent
/// will implement it by forwarding each event to the master.
pub trait EventSink {
    /// The executor's own clone into this host's cache has started. It runs
    /// before the child exists, so no child event covers it.
    fn clone_started(&mut self);
    /// ... and finished, after `duration_s`.
    fn clone_finished(&mut self, duration_s: f64, success: bool);
    /// One v1 event from the child, in the order it was written. Every
    /// event is delivered, including an `install_request` (after the
    /// executor has answered it), a `result` and an `error`.
    fn event(&mut self, event: WorkerEvent);
    /// Called repeatedly while a dependency install blocks the child, so
    /// the job's elapsed time keeps moving.
    fn install_tick(&self);
    /// The child's own peak RSS, from `wait_with_peak`, on every path that
    /// reaps it -- success, failure and cancel alike.
    fn peak_rss(&mut self, bytes: u64);
}

/// Whether to stop, and which process to kill when told to. In local mode
/// the registry decides (`jobs::RegistryProbe`) and kills the child's
/// process group itself on cancel; `child_started` must therefore kill at
/// once if the job was cancelled before the child existed.
pub trait CancelProbe {
    fn is_cancelled(&self) -> bool;
    fn child_started(&self, pid: u32);
    fn child_finished(&self);
}

/// Runs one job on this host: clone, job directory, inputs, child, events.
/// On `Err` the job directory has already been removed; on `Ok` the caller
/// removes [`Executed::job_dir`] once it has registered the result.
///
/// Local mode keeps today's commit behaviour: the checkout is whatever the
/// clone resolves HEAD to, and `job.commit` (pinned at admission) is not
/// consulted. Remote mode will check out `job.commit` instead
/// (docs/WORKER_TIER.md §3.3).
pub fn execute(
    env: &ExecEnv,
    job_id: Uuid,
    job: &JobSpec,
    inputs: &JobInputs,
    sink: &mut dyn EventSink,
    probe: &dyn CancelProbe,
) -> Result<Executed, ErrorBody> {
    let internal = |error: &dyn std::fmt::Display| ApiError::internal(error.to_string()).body;
    let repo_ref = RepoRef {
        slug: job.slug.clone(),
        owner: job.owner.clone(),
        repo: job.repo.clone(),
        source: if job.local {
            RepoSource::Local(PathBuf::from(&job.source))
        } else {
            RepoSource::Remote(job.source.clone())
        },
    };
    // Per-job directory, not the shared clone cache -- see
    // `harden_job_dir`'s doc comment. `output/` is where the worker writes
    // its build artifacts; `repo/` is the fresh, real (non-hardlinked)
    // local-clone checkout materialised below, the *only* copy of the
    // repository the worker ever sees; `cache/` is created for
    // wire-protocol/structural consistency with the rest of this per-job
    // layout but is otherwise unused by the worker in this flow (see the
    // `WorkerSpec` comment below).
    let job_dir = env
        .job_root
        .join(&repo_ref.owner)
        .join(&repo_ref.repo)
        .join(job_id.to_string());
    let output_dir = job_dir.join("output");
    let worker_cache_dir = job_dir.join("cache");
    let job_repo_dir = job_dir.join("repo");
    if let Err(error) = std::fs::create_dir_all(&output_dir) {
        return Err(internal(&error));
    }
    let fail = |error: ErrorBody| -> Result<Executed, ErrorBody> {
        let _ = std::fs::remove_dir_all(&job_dir);
        Err(error)
    };
    if let Err(error) = std::fs::create_dir_all(&worker_cache_dir) {
        return fail(internal(&error));
    }
    // See `materialize_job_repo`'s doc comment for the full design (why
    // this runs here rather than in the worker, the `--no-hardlinks`
    // reasoning, and the cancellation-during-clone gap this doesn't cover).
    let checkout = match materialize_job_repo(
        &env.clone_cache,
        &repo_ref,
        env.clone_cache_bytes,
        &job_repo_dir,
        sink,
    ) {
        Ok(checkout) => checkout,
        Err(error) => return fail(error),
    };
    let names_input = output_dir.join("worker-names-input.json");
    if let Err(error) = crate::naming::save_cache(&names_input, &inputs.names) {
        return fail(internal(&error));
    }
    // Like the names cache above, and before the same chown: the worker's
    // uid cannot read the map store (see `stage_previous_map`).
    let previous_maps = stage_previous_map(
        &job_dir,
        &inputs.previous_maps,
        checkout.branch.as_deref(),
        &|line: String| eprintln!("job {job_id}: {line}"),
    );
    // Chown + lock down the job's directory *before* the child that will
    // run inside it is spawned -- see `harden_job_dir`. This covers the
    // fresh `repo/` checkout made above too: a real, non-hardlinked copy,
    // so chowning it never touches the shared cache's own objects. A no-op
    // (aside from the 0700 permission bits, harmless either way) unless
    // this process is root, which only the runtime image is.
    if let Err(error) = harden_job_dir(&job_dir, env.worker_uid, env.worker_gid) {
        return fail(internal(&error));
    }
    // Issue #110 P1c: a SCIP job may install TypeScript dependencies. The
    // worker cannot start the sandbox (it is unprivileged), so it asks this
    // process, which can (root in the runtime image), over the protocol;
    // see `ServiceInstall`. Anywhere the sandbox cannot start, the answer
    // is a recorded fallback, never a failed job.
    let install = (job.install.as_deref() == Some("sandbox")).then(|| ServiceInstall {
        repo: job_repo_dir.clone(),
        scratch: env.install_root.join(job_id.to_string()),
        settings: crate::indexers::InstallSettings::from_env(env.worker_uid, env.worker_gid),
    });
    // Always a local, already-materialised checkout -- see
    // `clone::materialize_with_progress`'s `RepoSource::Local` branch: no
    // clone, no cache_dir use, no network, nothing left for the worker to do
    // in its own "Clone or fetch" stage but resolve HEAD. True whether the
    // job named a remote URL or a local fixture path; both go through the
    // same materialize + local-clone above, and the worker itself is never
    // told which one it was. `cache_dir` below is consequently unused by the
    // worker in this flow, kept only for structural consistency with the
    // rest of `WorkerSpec`.
    let checkout_job = JobSpec {
        source: job_repo_dir.to_string_lossy().into_owned(),
        local: true,
        install: install.as_ref().map(|_| "sandbox".to_owned()),
        ..job.clone()
    };
    let spec = checkout_job.to_worker_spec(LocalInputs {
        cache_dir: worker_cache_dir.to_string_lossy().into_owned(),
        output_dir: output_dir.to_string_lossy().into_owned(),
        clone_cache_bytes: env.clone_cache_bytes,
        previous_maps,
        names_cache: Some(names_input.to_string_lossy().into_owned()),
    });
    let hardening = WorkerHardening {
        job_dir: job_dir.clone(),
        uid: env.worker_uid,
        gid: env.worker_gid,
        allow_openrouter_key: env.allow_openrouter_key,
    };
    let output = run_child(
        &env.worker_exe,
        spec,
        job_id,
        &hardening,
        install.as_ref(),
        sink,
        probe,
    );
    if let Some(install) = &install {
        let _ = std::fs::remove_dir_all(&install.scratch);
    }
    match output {
        Ok(output) => Ok(Executed {
            job_dir,
            output_dir,
            checkout,
            output,
        }),
        Err(error) => fail(error),
    }
}

/// The error a cancelled job fails with, whether the registry cancelled it
/// while queued or the executor noticed the cancel after killing the child.
pub(crate) fn cancelled_error() -> ErrorBody {
    ErrorBody {
        error: "cancelled".to_owned(),
        message: "job cancelled".to_owned(),
    }
}

#[cfg(unix)]
pub(crate) fn kill_worker_group(pid: u32) {
    // CommandExt::process_group(0) made the worker the group leader. Git
    // children inherit that group, so a single signal terminates the tree.
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    if let Ok(pid) = i32::try_from(pid) {
        unsafe {
            kill(-pid, 9);
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn kill_worker_group(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .status();
}

/// Reaps the job child and returns its own peak RSS in bytes, alongside its
/// exit status -- `run_child`'s replacement for a plain `child.wait()`.
///
/// **Why not `getrusage(RUSAGE_CHILDREN)`**, which docs/WORKER_TIER.md #2.1
/// and #8 said to use before #149: that call reports the maximum over
/// *every* child this process has ever reaped, not this one. A small job
/// reaped after a large one would report the large one's peak, and this
/// process also reaps its own git clone/fetch children (`service::clone`)
/// and, when it runs as root, the nsjail dependency install
/// (`ServiceInstall::run`) -- both would leak into the figure. `wait4`
/// reports only the named child's own usage, which on Linux also covers
/// whatever grandchildren *it* reaps itself (the SCIP indexers a `--refs
/// scip` job spawns as its own children), and excludes everything else
/// this process runs.
///
/// After this returns `Ok`, `child` has been reaped. Nothing may call
/// `child.wait()` or `child.try_wait()` on it again -- `std` would see no
/// such process and return `ECHILD`. `run_child`'s two call sites are the
/// only places this child is ever reaped.
#[cfg(unix)]
fn wait_with_peak(
    child: &mut std::process::Child,
) -> std::io::Result<(std::process::ExitStatus, Option<u64>)> {
    use std::os::unix::process::ExitStatusExt;
    let pid = child.id() as libc::pid_t;
    let mut status: libc::c_int = 0;
    let mut rusage: libc::rusage = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `pid` names this process's own child, spawned above and
        // not yet reaped by anything else (see the doc comment: this is the
        // one and only reap of this child); `status` and `rusage` are
        // correctly-typed, writable out-parameters valid for the call.
        let reaped = unsafe { libc::wait4(pid, &mut status, 0, &mut rusage) };
        if reaped == -1 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        break;
    }
    // `ru_maxrss` is kibibytes on Linux and bytes on macOS -- there is no
    // portable unit, so the conversion is a `cfg`, per the platform's own
    // `getrusage(2)`/`wait4(2)` manual page.
    #[cfg(target_os = "macos")]
    let peak_bytes = rusage.ru_maxrss as u64;
    #[cfg(not(target_os = "macos"))]
    let peak_bytes = (rusage.ru_maxrss as u64).saturating_mul(1024);
    let peak_bytes = (peak_bytes > 0).then_some(peak_bytes);
    // SAFETY: `status` was just filled in by the successful `wait4` above,
    // in the same encoding `libc::waitpid`/`std::process` itself produces.
    Ok((std::process::ExitStatus::from_raw(status), peak_bytes))
}

#[cfg(not(unix))]
fn wait_with_peak(
    child: &mut std::process::Child,
) -> std::io::Result<(std::process::ExitStatus, Option<u64>)> {
    Ok((child.wait()?, None))
}

/// Materializes the shared cache for `repo_ref` and copies it into
/// `job_repo_dir` as a fast, non-hardlinked local clone -- everything
/// [`execute`] needs before it can build a `WorkerSpec` and spawn the
/// worker. Split out into its own function so this cache-reuse behaviour
/// (a second call for the same `owner`/`repo` fetches/fast-forwards the
/// existing `cache_dir/repos/<owner>/<repo>` clone rather than re-cloning
/// it) is directly unit-testable without spawning a real `tolmap worker`
/// child process -- see `jobs.rs`'s
/// `materialize_job_repo_reuses_the_shared_clone_cache_on_a_second_call`
/// test. The end-to-end path through a real worker binary is
/// `tests/service_byte_identity.rs`.
///
/// The clone/fetch against the shared LRU cache
/// (`cache_dir/repos/<owner>/<repo>`) runs here, in the executor, still at
/// its own uid -- not inside the worker, and not dropped afterward. This
/// keeps real cache reuse across jobs for the same repo (the whole reason
/// `clone::materialize_with_progress`'s cache/eviction machinery exists)
/// without handing the worker access to it: `git clone`/`fetch
/// --filter=blob:none` run no repository-controlled code
/// (docs/SCIP_SANDBOX.md's threat table: "none by default"), so doing this
/// step before the uid drop, in the trusted process, is not a security
/// regression -- it is exactly where that doc's own §4.1 puts "the clone is
/// copied ... into it, so the shared clone cache is never writable from a
/// jail," just without the jail (that's `local_clone_into`, next).
/// `Progress::silent()`: see `jobs::mark_clone_running`'s comment on the
/// resulting observability trade-off.
///
/// Known gap, not fixed here: the git child processes spawned inside
/// `clone::materialize_with_progress` are not registered with the
/// [`CancelProbe`] the way the worker child is (`child_started`/
/// `kill_worker_group`), so a cancellation that arrives while this function
/// is still running does not kill them -- they run to completion before
/// [`execute`] can notice the cancellation and return. Fixing that
/// properly means threading a cancellation hook through
/// `run_git_with_progress` (called from `clone_blobless` and three separate
/// call sites inside `fetch_and_fast_forward`) and every existing caller of
/// `materialize`/`materialize_with_progress` (`worker.rs`, two test
/// suites) -- a real shape change to `clone.rs`'s git-invocation plumbing,
/// not a small addition, and not verifiable without a local `cargo build`
/// this repo's rules don't allow here. Left as a documented gap rather than
/// forced through unverified (docs/WORKER_TIER.md §6 carries it over).
pub(crate) fn materialize_job_repo(
    cache_dir: &Path,
    repo_ref: &RepoRef,
    clone_cache_bytes: u64,
    job_repo_dir: &Path,
    sink: &mut dyn EventSink,
) -> Result<clone::Materialized, ErrorBody> {
    let clone_started = Instant::now();
    sink.clone_started();
    // `materialize_with_progress` reads only `clone_cache_bytes` from its
    // limits; the rest are the service's admission limits, which the
    // executor has no business holding.
    let limits = crate::service::config::Limits {
        clone_cache_bytes,
        ..crate::service::config::Limits::default()
    };
    let materialized = match clone::materialize_with_progress(
        cache_dir,
        repo_ref,
        &limits,
        &crate::progress::Progress::silent(),
    ) {
        Ok(materialized) => materialized,
        Err(error) => {
            sink.clone_finished(clone_started.elapsed().as_secs_f64(), false);
            return Err(error.body);
        }
    };
    // Fast local copy of that just-materialised clone into the job's own
    // directory -- real object copies, not hardlinks; see
    // `clone::local_clone_into`'s doc comment for why. This is the only
    // copy of the repository the worker's process is ever handed; the
    // shared cache directory itself is never chowned, hardened or passed to
    // the worker.
    if let Err(error) = clone::local_clone_into(&materialized.path, job_repo_dir) {
        sink.clone_finished(clone_started.elapsed().as_secs_f64(), false);
        return Err(ApiError::clone_failed(error.to_string()).body);
    }
    sink.clone_finished(clone_started.elapsed().as_secs_f64(), true);
    Ok(materialized)
}

/// Issue #141: the map store (`cache_dir/maps/<owner>/<repo>`) is the
/// service's own and `0700` (`harden_persistent_dir`), and in the runtime
/// image the worker runs at another uid ([`run_child`]), so a store path
/// handed to the worker cannot be read. The worker treated that as "no
/// previous map", and every re-index cold-started: finding 4's warm start,
/// 88% district retention against 46% cold, silently off.
///
/// So the executor picks the previous map the worker would have picked --
/// the newest on the checked-out branch, else the newest overall
/// (`candidates` is newest first) -- and copies it into
/// `job_dir/previous/<commit>.json` while `job_dir` is still its own, before
/// `harden_job_dir` hands the whole directory to the worker's uid. The store
/// keeps its owner and mode. Only that one map is copied: the worker never
/// uses more than one, and a repository's retained maps can be large.
///
/// A map that cannot be copied is a cold start, as an unreadable one always
/// was, but logged, not silent.
pub(crate) fn stage_previous_map(
    job_dir: &Path,
    candidates: &[PreviousMapInput],
    branch: Option<&str>,
    log: &dyn Fn(String),
) -> Vec<PreviousMap> {
    let Some(row) = candidates
        .iter()
        .find(|row| row.branch.as_deref() == branch)
        .or_else(|| candidates.first())
    else {
        return Vec::new();
    };
    // The commit names the copy, and the worker logs the name it warm
    // started from; the store only ever records a checked object id.
    if !worker_result::is_object_id(&row.commit) {
        log(format!(
            "cold start: stored commit {:?} is not an object id",
            row.commit
        ));
        return Vec::new();
    }
    let dir = job_dir.join("previous");
    let copy = dir.join(format!("{}.json", row.commit));
    let copied = worker_result::create_private_dir(&dir)
        .and_then(|()| worker_result::copy_for_worker(&row.path, &copy));
    if let Err(error) = copied {
        log(format!(
            "cold start: previous map {} not copied for the worker: {error}",
            row.commit
        ));
        return Vec::new();
    }
    vec![PreviousMap {
        branch: row.branch.clone(),
        path: copy.to_string_lossy().into_owned(),
    }]
}

/// A TypeScript dependency install the executor runs for its worker (issue
/// #110 P1c) when the worker sends `WorkerEvent::InstallRequest`. The
/// executor holds this, not the worker: it names the job's checkout and a
/// scratch directory under [`ExecEnv::install_root`], which is root's and
/// outside the job directory the worker's uid owns, so nothing the worker
/// (or the repository) can write decides where root creates the jail's
/// home, the proxy socket or the log. `indexers::install` re-reads the
/// install policy from the checkout itself; the request carries nothing.
pub(crate) struct ServiceInstall {
    pub(crate) repo: PathBuf,
    pub(crate) scratch: PathBuf,
    pub(crate) settings: crate::indexers::InstallSettings,
}

impl ServiceInstall {
    fn run(
        &self,
        id: Uuid,
        tick: &dyn Fn(),
        cancelled: &dyn Fn() -> bool,
    ) -> crate::schema::InstallCoverage {
        let log = |line: String| eprintln!("job {id}: {line}");
        // The checkout must still be the plain directory the executor made:
        // nsjail, as root, bind-mounts whatever this path resolves to.
        let is_plain_directory =
            std::fs::symlink_metadata(&self.repo).is_ok_and(|meta| meta.file_type().is_dir());
        let parent = self.scratch.parent().map(Path::to_path_buf);
        if !is_plain_directory
            || parent
                .as_deref()
                .is_some_and(|parent| std::fs::create_dir_all(parent).is_err())
        {
            log(
                "install sandbox unavailable: the job checkout or scratch directory is not usable"
                    .to_owned(),
            );
            return crate::indexers::fell_back("sandbox_unavailable", None);
        }
        if let Some(parent) = parent {
            let _ = harden_persistent_dir(&parent);
        }
        crate::indexers::install(
            &self.repo,
            &self.scratch,
            &self.settings,
            tick,
            cancelled,
            &log,
        )
    }
}

/// A worker's `result` event. Its `branch` is not kept: the stored branch is
/// the one the executor's own checkout resolved (`jobs::store_worker_result`).
pub struct WorkerOutput {
    pub map_path: String,
    pub symbols_path: String,
    pub symbols_dir: String,
    pub names_cache: String,
    pub commit: String,
    pub lang: String,
    pub files: usize,
    pub districts: usize,
    pub modularity: f64,
}

/// Everything [`run_child`] needs to lock the child down, gathered in one
/// place so the spawn call itself stays readable. Not part of `WorkerSpec`
/// -- that struct crosses the stdin wire to the child and is
/// serialised/logged; none of this belongs in it, and
/// `allow_openrouter_key` in particular controls what the *parent* puts in
/// the child's own environment, which has nothing to do with the spec.
pub(crate) struct WorkerHardening {
    /// The job's own per-job directory (see `harden_job_dir`) -- used as
    /// both `HOME` and `TMPDIR` for the child, never the parent's own.
    pub(crate) job_dir: PathBuf,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    /// True only when `state.config.namer == NamerKind::Model` -- see the
    /// PR body's "OPENROUTER_API_KEY residual exposure" note. Operator
    /// config, set once at service startup, never attacker/request-
    /// controlled; a no-op (key withheld) for today's production default
    /// (`NamerKind::Idf`).
    pub(crate) allow_openrouter_key: bool,
}

#[cfg(test)]
impl WorkerHardening {
    /// Test helper: uses the *current* process's own uid/gid, so
    /// [`run_child`]'s uid/gid drop is exercised only when the test itself
    /// runs as root (in which case dropping to itself is a no-op) and is
    /// otherwise correctly skipped via the same `is_root()` check
    /// production goes through -- no separate test-only code path.
    pub(crate) fn for_test(job_dir: &Path) -> Self {
        WorkerHardening {
            job_dir: job_dir.to_path_buf(),
            uid: current_uid(),
            gid: current_gid(),
            allow_openrouter_key: false,
        }
    }
}

#[cfg(unix)]
pub(crate) fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
pub(crate) fn current_gid() -> u32 {
    unsafe { libc::getegid() }
}

#[cfg(not(unix))]
pub(crate) fn current_uid() -> u32 {
    0
}

#[cfg(not(unix))]
pub(crate) fn current_gid() -> u32 {
    0
}

#[cfg(unix)]
pub(crate) fn is_root() -> bool {
    current_uid() == 0
}

#[cfg(not(unix))]
pub(crate) fn is_root() -> bool {
    false
}

/// Logged at most once per process: dropping the worker child to a
/// dedicated uid (see [`run_child`]) only works when this process itself is
/// root, which it is not on a GitHub Actions runner or a developer's own
/// machine -- only the Fly.io runtime image (no `USER` in the Dockerfile,
/// deliberately) runs the service as root so this drop can happen. Not a
/// per-job warning: every job in a given process hits the same euid, so
/// repeating it per job would just be noise.
#[cfg(unix)]
fn warn_unprivileged_once() {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!(
            "worker hardening: service is not running as root (euid != 0); \
             spawning the worker unprivileged as the current user instead of \
             dropping to a dedicated uid. Expected in CI and local dev; the \
             Fly.io runtime image runs the service as root specifically so \
             this drop can happen -- see the Dockerfile's runtime stage."
        );
    });
}

/// Chowns `job_dir` (recursively -- it may already hold the names-cache
/// input file the executor wrote before calling this) to the worker's uid
/// and locks it to `0700`, before the worker that will run as that uid
/// ever sees it. A no-op chown when this process is not root -- see
/// `is_root`'s callers -- since a single shared uid already owns
/// everything in that case and `chown` to a *different* uid you don't have
/// privilege for would just fail. The `0700` still applies either way; it
/// is harmless when it is also a no-op (the directory is already
/// exclusively this process's).
#[cfg(unix)]
pub(crate) fn harden_job_dir(job_dir: &Path, uid: u32, gid: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if is_root() {
        chown_recursive(job_dir, uid, gid)?;
    }
    std::fs::set_permissions(job_dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
pub(crate) fn harden_job_dir(_job_dir: &Path, _uid: u32, _gid: u32) -> std::io::Result<()> {
    Ok(())
}

/// Never follows a symlink: the tree holds a checkout of the repository, and
/// a committed symlink names whatever its author chose. `symlink_metadata`
/// decides whether to descend and `lchown` changes the link itself, so the
/// walk stays inside `path`. Children are done before their directory, so
/// every directory whose entries are being walked is still this process's
/// own and nothing else can change them meanwhile.
#[cfg(unix)]
pub(crate) fn chown_recursive(path: &Path, uid: u32, gid: u32) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path)?.file_type().is_dir() {
        for entry in std::fs::read_dir(path)? {
            chown_recursive(&entry?.path(), uid, gid)?;
        }
    }
    std::os::unix::fs::lchown(path, Some(uid), Some(gid))
}

/// Floor for the *persistent* parts of `/data` that are safe to lock down
/// this way -- `cache_dir/maps`, every stored map for every repo ever
/// indexed, and `cache_dir/install`: `0700`, so that even a bug that
/// pointed a `tolmap-worker`-owned process at this path would be refused by
/// the mode bits alone, on top of never being handed the path in the first
/// place. Does not chown -- this directory is created and owned by
/// whichever uid the service itself runs as (root in production), which is
/// exactly the owner it should keep; only the *mode* needs tightening from
/// whatever `create_dir_all`'s default (umask-dependent) leaves it at.
///
/// Deliberately **not** used for the sqlite store's own directory (see
/// `store::Store::open`, which locks the *file* down instead): in today's
/// production deployment (`TOLMAP_DB_PATH=/data/tolmap.sqlite3`,
/// `TOLMAP_CACHE_DIR=/data/cache`) the store's directory is `/data`, an
/// *ancestor* of `cache_dir` -- chmod'ing `/data` to `0700` root-owned
/// would deny the dropped-uid worker even `x` (search) permission to reach
/// `/data/cache/work/.../job_dir`, breaking every job, not narrowing what a
/// compromised one can read. `cache_dir/maps` has no such conflict: it is
/// a sibling of `cache_dir/work`, never an ancestor of any path the worker
/// is handed.
#[cfg(unix)]
pub(crate) fn harden_persistent_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
pub(crate) fn harden_persistent_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Spawns `exe worker` on `spec`, relays its v1 events to `sink` in order,
/// answers `install_request` with `install`, kills the process group when
/// `probe` says the job is cancelled or the stream went bad, and reaps the
/// child. A child that exits without a terminal protocol event is a job
/// failure, not a host failure.
pub(crate) fn run_child(
    exe: &Path,
    spec: WorkerSpec,
    id: Uuid,
    hardening: &WorkerHardening,
    install: Option<&ServiceInstall>,
    sink: &mut dyn EventSink,
    probe: &dyn CancelProbe,
) -> Result<WorkerOutput, ErrorBody> {
    let mut command = Command::new(exe);
    command
        .arg("worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Empty environment plus an explicit allowlist -- see
    // docs/SCIP_SANDBOX.md #2/#4.1 point 4. This is every env var
    // `src/worker.rs::run` and everything it calls (geometry.rs, naming.rs,
    // service/clone.rs) actually reads, audited by hand; anything else this
    // process's own environment carries -- including any other Fly secret
    // an operator has set -- never reaches this child at all.
    command.env_clear();
    if let Ok(path) = std::env::var("PATH") {
        // `service::clone` shells out to a bare `Command::new("git")` and
        // needs PATH to resolve it.
        command.env("PATH", path);
    }
    if let Ok(lang) = std::env::var("LANG") {
        command.env("LANG", lang);
    }
    // The job's own directory, not the parent's -- see `WorkerHardening`.
    command.env("HOME", &hardening.job_dir);
    command.env("TMPDIR", &hardening.job_dir);
    for key in [
        "TOLMAP_NAMER_BUDGET_USD",
        "TOLMAP_NAMER_INPUT_USD_PER_TOKEN",
        "TOLMAP_NAMER_OUTPUT_USD_PER_TOKEN",
        "TOLMAP_NAMER_LEDGER",
        // Issue #110: where the worker image puts the SCIP indexers
        // (docs/API.md configuration table). A `--refs scip` job without them would record a
        // fallback for every language instead of indexing.
        "TOLMAP_SCIP_TYPESCRIPT",
        "TOLMAP_SCIP_PYTHON",
        "TOLMAP_SCIP_GO",
    ] {
        // Pricing/budget knobs, a ledger path and indexer locations --
        // operator config set at service startup, not secrets.
        if let Ok(value) = std::env::var(key) {
            command.env(key, value);
        }
    }
    if hardening.allow_openrouter_key {
        if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
            command.env("OPENROUTER_API_KEY", key);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        if is_root() {
            command.uid(hardening.uid);
            command.gid(hardening.gid);
        } else {
            warn_unprivileged_once();
        }
    }
    let mut attempts = 0;
    let mut child = loop {
        match command.spawn() {
            Ok(child) => break child,
            // An executable being replaced during deployment can briefly be busy on Linux.
            Err(error) if error.raw_os_error() == Some(26) && attempts < 5 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                return Err(ErrorBody {
                    error: "worker_crashed".to_owned(),
                    message: format!("could not spawn worker: {error}"),
                })
            }
        }
    };
    probe.child_started(child.id());
    struct Registration<'a>(&'a dyn CancelProbe);
    impl Drop for Registration<'_> {
        fn drop(&mut self) {
            self.0.child_finished();
        }
    }
    let _registration = Registration(probe);
    let stderr = child.stderr.take().expect("piped worker stderr");
    let stderr_reader = std::thread::spawn(move || {
        let mut text = String::new();
        let mut reader = BufReader::new(stderr);
        let mut chunk = [0u8; 4096];
        while let Ok(count) = reader.read(&mut chunk) {
            if count == 0 {
                break;
            }
            if text.len() < 1024 * 1024 {
                text.push_str(&String::from_utf8_lossy(&chunk[..count]));
            }
        }
        text
    });
    // The worker's stdin stays open only when it may ask for an install
    // and needs a channel for the answer; otherwise it closes after the
    // spec, as it always has (a worker may read stdin to its end).
    let answers_installs = spec.install.is_some() && install.is_some();
    let mut worker_stdin = None;
    let write_spec = (|| -> anyhow::Result<()> {
        let mut stdin = child.stdin.take().expect("piped worker stdin");
        serde_json::to_writer(&mut stdin, &spec)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        if answers_installs {
            worker_stdin = Some(stdin);
        }
        Ok(())
    })();
    if let Err(error) = write_spec {
        kill_worker_group(child.id());
        let _ = child.kill();
        // The child never produced a `features` event on this path, so
        // there is nothing informative to log yet, but a peak is still
        // real evidence if the platform reports one -- see
        // `wait_with_peak`'s doc comment. This is also the reap: nothing
        // below may call `child.wait()`/`try_wait()` again.
        if let Ok((_, Some(peak))) = wait_with_peak(&mut child) {
            sink.peak_rss(peak);
        }
        let _ = stderr_reader.join();
        return Err(ErrorBody {
            error: "worker_crashed".to_owned(),
            message: format!("could not send worker spec: {error}"),
        });
    }
    let stdout = child.stdout.take().expect("piped worker stdout");
    let mut outcome = None;
    let mut error = None;
    let mut last_stage = None;
    for line in BufReader::new(stdout).lines() {
        let line = match line {
            Ok(line) => line,
            Err(reason) => {
                error = Some(format!("read worker event: {reason}"));
                break;
            }
        };
        let event: WorkerEvent = match serde_json::from_str(&line) {
            Ok(event) => event,
            Err(reason) => {
                error = Some(format!("invalid worker event: {reason}"));
                break;
            }
        };
        if event.version() != 1 {
            error = Some(format!(
                "unsupported worker event version {}",
                event.version()
            ));
            break;
        }
        match &event {
            WorkerEvent::StageStarted { stage, .. } => last_stage = Some(*stage),
            WorkerEvent::Progress { value, .. } => last_stage = Some(value.stage),
            WorkerEvent::Result {
                map_path,
                symbols_path,
                symbols_dir,
                names_cache,
                commit,
                lang,
                files,
                districts,
                modularity,
                ..
            } => {
                outcome = Some(WorkerOutput {
                    map_path: map_path.clone(),
                    symbols_path: symbols_path.clone(),
                    symbols_dir: symbols_dir.clone(),
                    names_cache: names_cache.clone(),
                    commit: commit.clone(),
                    lang: lang.clone(),
                    files: *files,
                    districts: *districts,
                    modularity: *modularity,
                });
            }
            WorkerEvent::Error { code, message, .. } => {
                error = Some(format!("{code}\n{message}"));
            }
            WorkerEvent::InstallRequest { .. } => {
                // The worker blocks until this answers, so its events wait
                // in the pipe meanwhile; `install_tick` keeps the job's
                // elapsed time moving instead.
                let coverage = match install {
                    Some(install) if worker_stdin.is_some() => {
                        install.run(id, &|| sink.install_tick(), &|| probe.is_cancelled())
                    }
                    // A worker that asks without having been offered
                    // installs gets a fallback, not a hang.
                    _ => crate::indexers::fell_back("sandbox_unavailable", None),
                };
                if let Some(stdin) = worker_stdin.as_mut() {
                    let answer = serde_json::to_vec(&coverage).map(|mut line| {
                        line.push(b'\n');
                        line
                    });
                    if let Ok(line) = answer {
                        // A worker killed meanwhile (cancel, shutdown) makes
                        // this a broken pipe; its exit is handled below.
                        let _ = stdin.write_all(&line).and_then(|()| stdin.flush());
                    }
                }
            }
            WorkerEvent::StageFinished { .. }
            | WorkerEvent::Features { .. }
            | WorkerEvent::Log { .. } => {}
        }
        sink.event(event);
    }
    if error.is_some() || probe.is_cancelled() {
        kill_worker_group(child.id());
    }
    // The reap: record the peak for every path that reaches here, including
    // a failed or a cancelled job (its peak is the out-of-memory evidence,
    // #97 phase 0 point 1). Nothing below may call `child.wait()`/
    // `try_wait()` on `child` again -- see `wait_with_peak`'s doc comment.
    let (status, peak) = wait_with_peak(&mut child).map_err(|reason| ErrorBody {
        error: "worker_crashed".to_owned(),
        message: format!("wait for worker: {reason}"),
    })?;
    if let Some(peak) = peak {
        sink.peak_rss(peak);
    }
    let stderr = stderr_reader.join().unwrap_or_default();
    if probe.is_cancelled() {
        return Err(cancelled_error());
    }
    if let Some(error) = error {
        if let Some((code, message)) = error.split_once('\n') {
            return Err(ErrorBody {
                error: code.to_owned(),
                message: message.to_owned(),
            });
        }
        return Err(ErrorBody {
            error: "worker_crashed".to_owned(),
            message: error,
        });
    }
    if !status.success() || outcome.is_none() {
        return Err(ErrorBody {
            error: "worker_crashed".to_owned(),
            message: format!(
                "worker exited {status} at {}{}",
                last_stage.map_or("before first stage", StageId::label),
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {}", stderr.trim())
                },
            ),
        });
    }
    Ok(outcome.expect("checked above"))
}
