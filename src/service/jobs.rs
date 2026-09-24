//! FIFO admission and snapshots live in the service. Each blocking build
//! runs in a child process; the child never opens the service database.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use ts_rs::TS;
use uuid::Uuid;

use crate::progress::{ProgressValue, StageId};
use crate::service::clone::{self, RepoRef};
use crate::service::error::{ApiError, ErrorBody};
use crate::service::eta::{expected_passes, progress_total, Eta, EtaModel, TimingRow};
use crate::service::store::MapRow;
use crate::service::time::now_rfc3339;
use crate::service::AppState;
use crate::worker::{PreviousMap, RepoFeatures, WorkerEvent, WorkerSpec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Cloning,
    Detecting,
    Indexing,
    Done,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct JobSnapshot {
    #[ts(type = "string")]
    pub job_id: Uuid,
    pub slug: String,
    pub commit: Option<String>,
    pub status: JobStatus,
    pub stage: String,
    /// One-based FIFO position while waiting; null once a worker starts.
    pub queue_position: Option<usize>,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// Human-readable failure text, or null. Flat, not an object: this is
    /// rendered directly by the client. The machine code lives beside it.
    pub error: Option<String>,
    /// Machine-readable failure code (`cancelled`, `detection_failed`,
    /// ...), or null. Clients branch on this rather than pattern-matching the
    /// message text.
    pub error_code: Option<String>,
    pub progress: Option<ProgressValue>,
    #[serde(default)]
    pub eta: Option<Eta>,
    #[serde(default)]
    pub eta_start_s: Option<f64>,
    pub elapsed_s: f64,
    pub stages: Vec<StageSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum StageState {
    Pending,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct StageSnapshot {
    pub id: StageId,
    pub label: String,
    pub state: StageState,
    pub started_at: Option<String>,
    pub duration_s: Option<f64>,
}

impl JobSnapshot {
    /// Splits an [`ErrorBody`] across the two flat fields above.
    pub fn set_error(&mut self, body: ErrorBody) {
        self.error = Some(body.message);
        self.error_code = Some(body.error);
    }
}

type JobKey = (String, String);
type JobRunner = Arc<dyn Fn(Arc<AppState>, RepoRef, watch::Sender<JobSnapshot>) + Send + Sync>;

struct PendingJob {
    id: Uuid,
    key: JobKey,
    repo_ref: RepoRef,
    tx: watch::Sender<JobSnapshot>,
    runner: JobRunner,
}

#[derive(Default)]
struct RegistryInner {
    jobs: HashMap<Uuid, watch::Sender<JobSnapshot>>,
    active: HashMap<JobKey, Uuid>,
    queue: VecDeque<PendingJob>,
    running: usize,
    running_jobs: BTreeMap<Uuid, watch::Sender<JobSnapshot>>,
    children: HashMap<Uuid, u32>,
    cancelled: HashSet<Uuid>,
    features: HashMap<Uuid, RepoFeatures>,
    eta_model: EtaModel,
    /// Set once by [`JobRegistry::shutdown`] and never cleared -- the
    /// process is exiting, not pausing. Checked by `enqueue_job` so no job
    /// is admitted after a shutdown signal starts draining the registry.
    stopping: bool,
}

pub struct JobRegistry(Mutex<RegistryInner>);

impl JobRegistry {
    pub fn load_timings(&self, store: &crate::service::store::Store) -> anyhow::Result<()> {
        let rows = store.recent_timings()?;
        self.0
            .lock()
            .expect("job registry mutex poisoned")
            .eta_model = EtaModel::from_rows(rows);
        Ok(())
    }

    pub fn cancel(&self, id: Uuid) -> Result<JobSnapshot, ApiError> {
        let mut registry = self.0.lock().expect("job registry mutex poisoned");
        let tx = registry
            .jobs
            .get(&id)
            .ok_or_else(|| ApiError::not_found(format!("no job {id}")))?
            .clone();
        if is_terminal(&tx.borrow()) {
            return Ok(tx.borrow().clone());
        }
        if let Some(position) = registry.queue.iter().position(|job| job.id == id) {
            let job = registry.queue.remove(position).expect("position exists");
            registry.active.remove(&job.key);
            finish_failed(&tx, cancelled_error());
            refresh_queue_etas(&mut registry);
        } else {
            registry.cancelled.insert(id);
            registry.active.retain(|_, active_id| *active_id != id);
            finish_failed(&tx, cancelled_error());
            if let Some(&pid) = registry.children.get(&id) {
                kill_worker_group(pid);
            }
            refresh_queue_etas(&mut registry);
        }
        let result = tx.borrow().clone();
        Ok(result)
    }

    fn register_child(&self, id: Uuid, pid: u32) {
        let mut registry = self.0.lock().expect("job registry mutex poisoned");
        registry.children.insert(id, pid);
        if registry.cancelled.contains(&id) {
            kill_worker_group(pid);
        }
    }

    fn unregister_child(&self, id: Uuid) {
        self.0
            .lock()
            .expect("job registry mutex poisoned")
            .children
            .remove(&id);
    }

    fn is_cancelled(&self, id: Uuid) -> bool {
        self.0
            .lock()
            .expect("job registry mutex poisoned")
            .cancelled
            .contains(&id)
    }

    fn set_features(&self, id: Uuid, features: RepoFeatures) {
        self.0
            .lock()
            .expect("job registry mutex poisoned")
            .features
            .insert(id, features);
    }

    fn features(&self, id: Uuid) -> RepoFeatures {
        self.0
            .lock()
            .expect("job registry mutex poisoned")
            .features
            .get(&id)
            .cloned()
            .unwrap_or_default()
    }

    fn estimate(
        &self,
        tx: &watch::Sender<JobSnapshot>,
        running: Option<(StageId, f64, Option<f64>, Option<f64>)>,
        completed_passes: &[usize; StageId::ALL.len()],
    ) {
        let mut registry = self.0.lock().expect("job registry mutex poisoned");
        let snapshot = tx.borrow().clone();
        if is_terminal(&snapshot) {
            return;
        }
        let features = registry
            .features
            .get(&snapshot.job_id)
            .cloned()
            .unwrap_or_default();
        let mut done = [false; StageId::ALL.len()];
        for stage in &snapshot.stages {
            done[stage.id.index() - 1] = stage.state == StageState::Done
                && completed_passes[stage.id.index() - 1] >= expected_passes(stage.id, &features);
        }
        let eta = registry.eta_model.predict(&features, &done, running);
        tx.send_modify(|snapshot| snapshot.eta = Some(eta));
        refresh_queue_etas(&mut registry);
    }
    pub fn subscribe(&self, id: Uuid) -> Option<watch::Receiver<JobSnapshot>> {
        self.0
            .lock()
            .expect("job registry mutex poisoned")
            .jobs
            .get(&id)
            .map(watch::Sender::subscribe)
    }

    /// Stops admission (`enqueue_job` starts rejecting with
    /// `server_stopping`) and fails every job currently queued or running,
    /// with the same error, killing any worker child already spawned. This
    /// is the "drain" half of graceful shutdown -- see `service::serve`'s
    /// `shutdown_signal` for why it must run, and finish, before axum's
    /// `with_graceful_shutdown` future resolves: that ordering is what lets
    /// an open SSE stream deliver the resulting terminal frame before its
    /// connection is torn down.
    ///
    /// One lock acquisition covers the flag flip and both sweeps, so no job
    /// can be dequeued into "running" (see `worker_loop`) or admitted (see
    /// `enqueue_job`) in between: a job is exactly "not yet seen" (still
    /// invisible to this function, and about to be rejected by the flag),
    /// "queued", or "running" at every instant this holds the lock.
    pub fn shutdown(&self) {
        let mut registry = self.0.lock().expect("job registry mutex poisoned");
        registry.stopping = true;
        while let Some(job) = registry.queue.pop_front() {
            registry.active.remove(&job.key);
            finish_failed(&job.tx, server_stopping_error());
        }
        let running: Vec<(Uuid, watch::Sender<JobSnapshot>)> = registry
            .running_jobs
            .iter()
            .map(|(id, tx)| (*id, tx.clone()))
            .collect();
        for (id, tx) in running {
            // Mirrors `cancel`'s running-job branch: mark cancelled (so the
            // worker thread's own exit path attributes the kill correctly
            // rather than reporting `worker_crashed`), fail the job with the
            // shutdown-specific error, then kill the child. `finish_failed`
            // is a no-op if a terminal state already landed, which is what
            // makes the ordering here safe rather than merely convenient.
            registry.cancelled.insert(id);
            registry.active.retain(|_, active_id| *active_id != id);
            finish_failed(&tx, server_stopping_error());
            if let Some(&pid) = registry.children.get(&id) {
                kill_worker_group(pid);
            }
        }
        refresh_queue_etas(&mut registry);
    }
}

pub fn new_registry() -> JobRegistry {
    JobRegistry(Mutex::new(RegistryInner::default()))
}

fn cancelled_error() -> ErrorBody {
    ErrorBody {
        error: "cancelled".to_owned(),
        message: "job cancelled".to_owned(),
    }
}

/// Terminal error for every job still queued or running when the service
/// receives a shutdown signal -- see [`JobRegistry::shutdown`] and
/// docs/API.md. Same shape as `cancelled_error`, distinct code: a client
/// that branches on `error_code` needs to tell "you (or another caller)
/// cancelled this" apart from "the service went away out from under this".
fn server_stopping_error() -> ErrorBody {
    ErrorBody {
        error: "server_stopping".to_owned(),
        message: "the service is shutting down".to_owned(),
    }
}

#[cfg(unix)]
fn kill_worker_group(pid: u32) {
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
fn kill_worker_group(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .status();
}

fn refresh_queue_etas(registry: &mut RegistryInner) {
    let prior =
        registry
            .eta_model
            .predict(&RepoFeatures::default(), &[false; StageId::ALL.len()], None);
    let mut wait_s: f64 = registry
        .running_jobs
        .values()
        .map(|tx| {
            let row = tx.borrow();
            if is_terminal(&row) {
                0.0
            } else {
                row.eta.unwrap_or(prior).midpoint()
            }
        })
        .sum();
    for (index, job) in registry.queue.iter().enumerate() {
        let features = registry.features.get(&job.id).cloned().unwrap_or_default();
        let eta = registry
            .eta_model
            .predict(&features, &[false; StageId::ALL.len()], None);
        job.tx.send_modify(|snapshot| {
            snapshot.queue_position = Some(index + 1);
            snapshot.eta_start_s = Some(wait_s);
            snapshot.eta = Some(eta);
        });
        wait_s += eta.midpoint();
    }
}

/// Queues a job and returns its id immediately; the work happens on a
/// spawned task. `state.jobs` keeps the sending half so `GET
/// /api/jobs/{id}` and the SSE endpoint can each get their own receiver via
/// `.subscribe()`.
pub fn spawn_job(
    state: Arc<AppState>,
    repo_ref: RepoRef,
    commit: String,
) -> Result<Uuid, ApiError> {
    enqueue_job(state, repo_ref, commit, Arc::new(run_blocking))
}

fn enqueue_job(
    state: Arc<AppState>,
    repo_ref: RepoRef,
    commit: String,
    runner: JobRunner,
) -> Result<Uuid, ApiError> {
    let key = (repo_ref.slug.clone(), commit.clone());
    let mut registry = state.jobs.0.lock().expect("job registry mutex poisoned");
    if registry.stopping {
        return Err(ApiError::server_stopping(
            "the service is shutting down and is not accepting new jobs",
        ));
    }
    if let Some(id) = registry.active.get(&key) {
        return Ok(*id);
    }
    let max_running = state.config.limits.max_concurrent_jobs.max(1);
    if registry.running >= max_running
        && registry.queue.len() >= state.config.limits.max_queued_jobs
    {
        return Err(ApiError::busy(
            "the index queue is full; please try again later",
        ));
    }
    let job_id = Uuid::new_v4();
    let snapshot = JobSnapshot {
        job_id,
        slug: repo_ref.slug.clone(),
        commit: Some(commit),
        status: JobStatus::Queued,
        stage: "queued".to_owned(),
        queue_position: None,
        started_at: now_rfc3339(),
        finished_at: None,
        error: None,
        error_code: None,
        progress: None,
        eta: None,
        eta_start_s: None,
        elapsed_s: 0.0,
        stages: StageId::ALL
            .iter()
            .map(|&id| StageSnapshot {
                id,
                label: id.label().to_owned(),
                state: StageState::Pending,
                started_at: None,
                duration_s: None,
            })
            .collect(),
    };
    let (tx, _rx) = watch::channel(snapshot);
    let initial_eta =
        registry
            .eta_model
            .predict(&RepoFeatures::default(), &[false; StageId::ALL.len()], None);
    tx.send_modify(|snapshot| snapshot.eta = Some(initial_eta));
    registry.jobs.insert(job_id, tx.clone());
    registry.active.insert(key.clone(), job_id);
    let job = PendingJob {
        id: job_id,
        key,
        repo_ref,
        tx,
        runner,
    };
    if registry.running < max_running {
        registry.running += 1;
        job.tx
            .send_modify(|snapshot| snapshot.eta_start_s = Some(0.0));
        registry.running_jobs.insert(job_id, job.tx.clone());
        tokio::spawn(worker_loop(state.clone(), job));
    } else {
        job.tx
            .send_modify(|snapshot| snapshot.queue_position = Some(registry.queue.len() + 1));
        registry.queue.push_back(job);
    }
    refresh_queue_etas(&mut registry);
    Ok(job_id)
}

async fn worker_loop(state: Arc<AppState>, first: PendingJob) {
    let mut job = first;
    loop {
        let PendingJob {
            id,
            key,
            repo_ref,
            tx,
            runner,
        } = job;
        tx.send_modify(|snapshot| {
            snapshot.queue_position = None;
            snapshot.eta_start_s = None;
            snapshot.started_at = now_rfc3339();
        });
        let blocking_state = state.clone();
        let blocking_tx = tx.clone();
        let result =
            tokio::task::spawn_blocking(move || runner(blocking_state, repo_ref, blocking_tx))
                .await;
        if let Err(join_error) = result {
            finish_failed(
                &tx,
                ErrorBody {
                    error: "internal_error".to_owned(),
                    message: format!("job task panicked: {join_error}"),
                },
            );
        }
        // The runner normally sets a terminal snapshot itself. A panic or
        // an unexpected return must never leave an accepted job in flight.
        if !is_terminal(&tx.borrow()) {
            finish_failed(
                &tx,
                ErrorBody {
                    error: "internal_error".to_owned(),
                    message: "job exited without a terminal state".to_owned(),
                },
            );
        }
        let mut registry = state.jobs.0.lock().expect("job registry mutex poisoned");
        let snapshot = tx.borrow().clone();
        let row = TimingRow {
            features: registry.features.remove(&id).unwrap_or_default(),
            elapsed_s: snapshot.elapsed_s,
            stage_s: snapshot
                .stages
                .iter()
                .map(|stage| {
                    (snapshot.status == JobStatus::Done && stage.state == StageState::Done)
                        .then_some(stage.duration_s)
                        .flatten()
                })
                .collect(),
        };
        if let Err(error) = state.store.save_timing(&id.to_string(), &row) {
            eprintln!("timing store warning for {id}: {error:#}");
        } else {
            registry.eta_model.record(row);
        }
        registry.running_jobs.remove(&id);
        registry.cancelled.remove(&id);
        if registry.active.get(&key) == Some(&id) {
            registry.active.remove(&key);
        }
        if let Some(next) = registry.queue.pop_front() {
            registry.running_jobs.insert(next.id, next.tx.clone());
            refresh_queue_etas(&mut registry);
            job = next;
        } else {
            registry.running -= 1;
            refresh_queue_etas(&mut registry);
            break;
        }
    }
}

fn is_terminal(snapshot: &JobSnapshot) -> bool {
    matches!(snapshot.status, JobStatus::Done | JobStatus::Failed)
}

fn advance(tx: &watch::Sender<JobSnapshot>, status: JobStatus, stage: &str) {
    tx.send_modify(|snapshot| {
        if is_terminal(snapshot) {
            return;
        }
        snapshot.status = status;
        snapshot.stage = stage.to_owned();
    });
}

fn set_commit(tx: &watch::Sender<JobSnapshot>, commit: &str) {
    tx.send_modify(|snapshot| {
        if !is_terminal(snapshot) {
            snapshot.commit = Some(commit.to_owned());
        }
    });
}

/// Marks the `Clone` stage running for the service's own
/// `clone::materialize_with_progress` call in `run_blocking`, made *before*
/// the worker is spawned now that the clone/fetch against the shared cache
/// happens in the service, not the child. Paired with
/// `mark_clone_finished`. Without this, a client watching the job's
/// `stages`/`status` would see a stale `Queued` for however long the
/// service-side clone takes, then the worker's own (now near-instant, since
/// it always gets a `RepoSource::Local` checkout) version of the same stage
/// flash by -- see the PR body's observability note. This does not forward
/// git's own object/delta/byte counters tick-by-tick the way the in-worker
/// clone used to (that plumbing stays in `clone::run_git_with_progress`,
/// unused here because this call passes `Progress::silent()`); it only
/// brackets the stage as running, then done or failed.
fn mark_clone_running(tx: &watch::Sender<JobSnapshot>, started: Instant) {
    tx.send_modify(|snapshot| {
        if is_terminal(snapshot) {
            return;
        }
        snapshot.status = JobStatus::Cloning;
        snapshot.stage = StageId::Clone.label().to_owned();
        snapshot.progress = None;
        snapshot.elapsed_s = started.elapsed().as_secs_f64();
        let row = &mut snapshot.stages[StageId::Clone.index() - 1];
        row.state = StageState::Running;
        if row.started_at.is_none() {
            row.started_at = Some(now_rfc3339());
        }
    });
}

/// See `mark_clone_running`. `duration_s` accumulates onto the row the same
/// way a `WorkerEvent::StageFinished` does for a multi-pass stage -- the
/// worker's own near-instant `Clone` stage (its `RepoSource::Local` branch
/// just resolves HEAD) adds a second, tiny duration on top of this one
/// rather than replacing it, so the reported total still covers the real
/// clone/fetch time.
fn mark_clone_finished(tx: &watch::Sender<JobSnapshot>, duration_s: f64, success: bool) {
    tx.send_modify(|snapshot| {
        if is_terminal(snapshot) {
            return;
        }
        let row = &mut snapshot.stages[StageId::Clone.index() - 1];
        row.state = if success {
            StageState::Done
        } else {
            StageState::Failed
        };
        row.duration_s = Some(row.duration_s.unwrap_or(0.0) + duration_s);
    });
}

/// Materializes the shared cache for `repo_ref` and copies it into
/// `job_repo_dir` as a fast, non-hardlinked local clone -- everything
/// `run_blocking` needs before it can build a `WorkerSpec` and spawn the
/// worker. Split out into its own function so this cache-reuse behaviour
/// (a second call for the same `owner`/`repo` fetches/fast-forwards the
/// existing `cache_dir/repos/<owner>/<repo>` clone rather than re-cloning
/// it) is directly unit-testable without spawning a real `tolmap worker`
/// child process -- `run_blocking`'s own worker-spawn step needs
/// `std::env::current_exe()` to be an actual `tolmap` binary, which a
/// `cargo test` test binary is not, so no test in this file exercises
/// `run_blocking` end to end. See this file's
/// `materialize_job_repo_reuses_the_shared_clone_cache_on_a_second_call`
/// test.
///
/// The clone/fetch against the shared, service-owned LRU cache
/// (`cache_dir/repos/<owner>/<repo>`) runs here, in the service, still at
/// its own uid -- not inside the worker, and not dropped afterward. This
/// restores real cache reuse across jobs for the same repo (the whole
/// reason `clone::materialize_with_progress`'s cache/eviction machinery
/// exists) without handing the worker access to it: `git
/// clone`/`fetch --filter=blob:none` run no repository-controlled code
/// (docs/SCIP_SANDBOX.md's threat table: "none by default"), so doing this
/// step before the uid drop, in the trusted process, is not a security
/// regression -- it is exactly where that doc's own §4.1 puts "the clone is
/// copied ... into it, so the shared clone cache is never writable from a
/// jail," just without the jail (that's `local_clone_into`, next).
/// `Progress::silent()`: see `mark_clone_running`'s comment on the
/// resulting observability trade-off.
///
/// Known gap, not fixed here: the git child processes spawned inside
/// `clone::materialize_with_progress` are not registered with the job
/// registry the way the worker child is (`register_child`/
/// `kill_worker_group`), so a cancellation that arrives while this function
/// is still running does not kill them -- they run to completion before
/// `run_blocking` can notice the cancellation and return. Fixing that
/// properly means threading a cancellation hook through
/// `run_git_with_progress` (called from `clone_blobless` and three separate
/// call sites inside `fetch_and_fast_forward`) and every existing caller of
/// `materialize`/`materialize_with_progress` (`worker.rs`, two test
/// suites) -- a real shape change to `clone.rs`'s git-invocation plumbing,
/// not a small addition, and not verifiable without a local `cargo build`
/// this repo's rules don't allow here. Left as a documented gap rather than
/// forced through unverified.
fn materialize_job_repo(
    cache_dir: &Path,
    repo_ref: &RepoRef,
    limits: &crate::service::config::Limits,
    job_repo_dir: &Path,
    tx: &watch::Sender<JobSnapshot>,
    started: Instant,
) -> Result<(), ErrorBody> {
    let clone_started = Instant::now();
    mark_clone_running(tx, started);
    let materialized = match clone::materialize_with_progress(
        cache_dir,
        repo_ref,
        limits,
        &crate::progress::Progress::silent(),
    ) {
        Ok(materialized) => materialized,
        Err(error) => {
            mark_clone_finished(tx, clone_started.elapsed().as_secs_f64(), false);
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
        mark_clone_finished(tx, clone_started.elapsed().as_secs_f64(), false);
        return Err(ApiError::clone_failed(error.to_string()).body);
    }
    mark_clone_finished(tx, clone_started.elapsed().as_secs_f64(), true);
    Ok(())
}

fn finish_done(tx: &watch::Sender<JobSnapshot>) {
    tx.send_modify(|snapshot| {
        if is_terminal(snapshot) {
            return;
        }
        snapshot.status = JobStatus::Done;
        snapshot.stage = "done".to_owned();
        snapshot.finished_at = Some(now_rfc3339());
        snapshot.error = None;
        snapshot.error_code = None;
        snapshot.eta = None;
        snapshot.eta_start_s = None;
    });
}

fn finish_failed(tx: &watch::Sender<JobSnapshot>, error: ErrorBody) {
    tx.send_modify(|snapshot| {
        if is_terminal(snapshot) {
            return;
        }
        snapshot.status = JobStatus::Failed;
        snapshot.stage = "failed".to_owned();
        snapshot.finished_at = Some(now_rfc3339());
        snapshot.set_error(error);
        snapshot.eta = None;
        snapshot.eta_start_s = None;
        for stage in &mut snapshot.stages {
            if matches!(stage.state, StageState::Running) {
                stage.state = StageState::Failed;
            }
        }
    });
}

/// Run the whole blocking pipeline outside the API process. A child that
/// exits without a terminal protocol event is a job failure, not a service
/// failure; the queue loop retains the slot until this function returns.
fn run_blocking(state: Arc<AppState>, repo_ref: RepoRef, tx: watch::Sender<JobSnapshot>) {
    let started = Instant::now();
    if state.jobs.is_cancelled(tx.borrow().job_id) {
        return;
    }
    let previous_maps = match state.store.warm_start_candidates(&repo_ref.slug) {
        Ok(rows) => rows
            .into_iter()
            .map(|row| PreviousMap {
                branch: row.branch,
                path: row.map_path.to_string_lossy().into_owned(),
            })
            .collect(),
        Err(error) => return finish_failed(&tx, ApiError::internal(error.to_string()).body),
    };
    // Per-job directory, not the shared `cache_dir` -- see
    // `harden_job_dir`'s doc comment. `output/` is where the worker writes
    // its build artifacts (unchanged from before this change); `repo/` is
    // the fresh, real (non-hardlinked) local-clone checkout materialised
    // below, the *only* copy of the repository the worker ever sees;
    // `cache/` is created for wire-protocol/structural consistency with the
    // rest of this per-job layout but is otherwise unused by the worker in
    // this flow (see the `WorkerSpec` comment below).
    let job_dir = state
        .config
        .cache_dir
        .join("work")
        .join(&repo_ref.owner)
        .join(&repo_ref.repo)
        .join(tx.borrow().job_id.to_string());
    let output_dir = job_dir.join("output");
    let worker_cache_dir = job_dir.join("cache");
    let job_repo_dir = job_dir.join("repo");
    if let Err(error) = std::fs::create_dir_all(&output_dir) {
        return finish_failed(&tx, ApiError::internal(error.to_string()).body);
    }
    if let Err(error) = std::fs::create_dir_all(&worker_cache_dir) {
        let _ = std::fs::remove_dir_all(&job_dir);
        return finish_failed(&tx, ApiError::internal(error.to_string()).body);
    }
    // See `materialize_job_repo`'s doc comment for the full design (why
    // this runs in the service rather than the worker, the `--no-hardlinks`
    // reasoning, and the cancellation-during-clone gap this doesn't cover).
    if let Err(error) = materialize_job_repo(
        &state.config.cache_dir,
        &repo_ref,
        &state.config.limits,
        &job_repo_dir,
        &tx,
        started,
    ) {
        let _ = std::fs::remove_dir_all(&job_dir);
        return finish_failed(&tx, error);
    }
    let names_input = output_dir.join("worker-names-input.json");
    let names = match state.store.load_names(&repo_ref.slug) {
        Ok(names) => names,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&job_dir);
            return finish_failed(&tx, ApiError::internal(error.to_string()).body);
        }
    };
    if let Err(error) = crate::naming::save_cache(&names_input, &names) {
        let _ = std::fs::remove_dir_all(&job_dir);
        return finish_failed(&tx, ApiError::internal(error.to_string()).body);
    }
    // Chown + lock down the job's directory *before* the child that will
    // run inside it is spawned -- see `harden_job_dir`. This now covers the
    // fresh `repo/` checkout made above too: a real, non-hardlinked copy,
    // so chowning it never touches the shared cache's own objects. A no-op
    // (aside from the 0700 permission bits, harmless either way) unless the
    // service itself is root, which only the Fly.io runtime image is.
    if let Err(error) = harden_job_dir(&job_dir, state.config.worker_uid, state.config.worker_gid) {
        let _ = std::fs::remove_dir_all(&job_dir);
        return finish_failed(&tx, ApiError::internal(error.to_string()).body);
    }
    // Always a local, already-materialised checkout now -- see
    // `clone::materialize_with_progress`'s `RepoSource::Local` branch: no
    // clone, no cache_dir use, no network, nothing left for the worker to do
    // in its own "Clone or fetch" stage but resolve HEAD. True whether the
    // original request named a remote URL or a local fixture path; both go
    // through the same service-side materialize + local-clone above, and
    // the worker itself is never told which one it was. `cache_dir` below
    // is consequently unused by the worker in this flow, kept only for
    // structural consistency with the rest of `WorkerSpec`.
    let spec = WorkerSpec {
        v: 1,
        slug: repo_ref.slug.clone(),
        owner: repo_ref.owner.clone(),
        repo: repo_ref.repo.clone(),
        source: job_repo_dir.to_string_lossy().into_owned(),
        local: true,
        all_sources: false,
        cache_dir: worker_cache_dir.to_string_lossy().into_owned(),
        output_dir: output_dir.to_string_lossy().into_owned(),
        clone_cache_bytes: state.config.limits.clone_cache_bytes,
        prune_variant: state.config.prune_variant.to_string(),
        namer: state.config.namer.to_string(),
        namer_model: state.config.namer_model.clone(),
        previous_maps,
        names_cache: Some(names_input.to_string_lossy().into_owned()),
        refs: Some(state.config.refs.to_string()),
    };
    let output = match process_worker(&state, &tx, spec, started, &job_dir) {
        Ok(output) => output,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&job_dir);
            return finish_failed(&tx, error);
        }
    };
    if state.jobs.is_cancelled(tx.borrow().job_id) {
        let _ = std::fs::remove_dir_all(&job_dir);
        return;
    }
    set_commit(&tx, &output.commit);
    let built_path = PathBuf::from(&output.map_path);
    let final_path = state
        .config
        .cache_dir
        .join("maps")
        .join(&repo_ref.owner)
        .join(&repo_ref.repo)
        .join(format!("{}.json", output.commit));
    let save = (|| -> anyhow::Result<()> {
        let final_parent = final_path.parent().expect("map parent");
        std::fs::create_dir_all(final_parent)?;
        // Persistent store, not a per-job scratch dir: owned by the
        // service's own uid (root in production), not `tolmap-worker` --
        // even a bug that pointed the worker at this path could not read
        // every stored map for every repo ever indexed. See the PR body's
        // "at minimum" floor.
        harden_persistent_dir(final_parent)?;
        std::fs::rename(&built_path, &final_path)?;
        std::fs::rename(
            &output.symbols_path,
            final_path.with_extension("symbols.json"),
        )?;
        let final_dir = final_path.with_extension("symbols");
        if final_dir.exists() {
            std::fs::remove_dir_all(&final_dir)?;
        }
        std::fs::rename(&output.symbols_dir, final_dir)?;
        state.store.save_names(
            &repo_ref.slug,
            &crate::naming::load_cache(&PathBuf::from(&output.names_cache)),
        )?;
        let row = MapRow {
            slug: repo_ref.slug.clone(),
            owner: repo_ref.owner.clone(),
            repo: repo_ref.repo.clone(),
            commit: output.commit.clone(),
            branch: output.branch,
            lang: output.lang,
            files: output.files as i64,
            districts: output.districts as i64,
            modularity: output.modularity,
            map_path: final_path,
            indexed_at: now_rfc3339(),
        };
        state.store.insert(&row)?;
        Ok(())
    })();
    if let Err(error) = save {
        let _ = std::fs::remove_dir_all(&job_dir);
        return finish_failed(&tx, ApiError::internal(format!("{error:#}")).body);
    }
    let _ = std::fs::remove_dir_all(&job_dir);
    if let Err(error) = state
        .store
        .prune(&repo_ref.slug, state.config.retain_commits_per_repo)
    {
        eprintln!("prune warning for {}: {error:#}", repo_ref.slug);
    }
    tx.send_modify(|snapshot| snapshot.elapsed_s = started.elapsed().as_secs_f64());
    finish_done(&tx);
}

struct WorkerOutput {
    map_path: String,
    symbols_path: String,
    symbols_dir: String,
    names_cache: String,
    commit: String,
    branch: Option<String>,
    lang: String,
    files: usize,
    districts: usize,
    modularity: f64,
}

/// Everything `process_worker_exe` needs to lock the child down, gathered
/// in one place so the spawn call itself stays readable. Not part of
/// `WorkerSpec` -- that struct crosses the stdin wire to the child and is
/// serialised/logged; none of this belongs in it, and `allow_openrouter_key`
/// in particular controls what the *parent* puts in the child's own
/// environment, which has nothing to do with the spec.
struct WorkerHardening {
    /// The job's own per-job directory (see `harden_job_dir`) -- used as
    /// both `HOME` and `TMPDIR` for the child, never the parent's own.
    job_dir: PathBuf,
    uid: u32,
    gid: u32,
    /// True only when `state.config.namer == NamerKind::Model` -- see the
    /// PR body's "OPENROUTER_API_KEY residual exposure" note. Operator
    /// config, set once at service startup, never attacker/request-
    /// controlled; a no-op (key withheld) for today's production default
    /// (`NamerKind::Idf`).
    allow_openrouter_key: bool,
}

#[cfg(test)]
impl WorkerHardening {
    /// Test helper: uses the *current* process's own uid/gid, so
    /// `process_worker_exe`'s uid/gid drop is exercised only when the test
    /// itself runs as root (in which case dropping to itself is a no-op)
    /// and is otherwise correctly skipped via the same `is_root()` check
    /// production goes through -- no separate test-only code path.
    fn for_test(job_dir: &Path) -> Self {
        WorkerHardening {
            job_dir: job_dir.to_path_buf(),
            uid: current_uid(),
            gid: current_gid(),
            allow_openrouter_key: false,
        }
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn current_gid() -> u32 {
    unsafe { libc::getegid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

#[cfg(not(unix))]
fn current_gid() -> u32 {
    0
}

#[cfg(unix)]
fn is_root() -> bool {
    current_uid() == 0
}

#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

/// Logged at most once per process: dropping the worker child to a
/// dedicated uid (see `process_worker_exe`) only works when the service
/// itself is root, which it is not on a GitHub Actions runner or a
/// developer's own machine -- only the Fly.io runtime image (no `USER` in
/// the Dockerfile, deliberately) runs the service as root so this drop can
/// happen. Not a per-job warning: every job in a given process hits the
/// same euid, so repeating it per job would just be noise.
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
/// input file the service wrote before calling this) to the worker's uid
/// and locks it to `0700`, before the worker that will run as that uid
/// ever sees it. A no-op chown when the service is not root -- see
/// `is_root`'s callers -- since a single shared uid already owns
/// everything in that case and `chown` to a *different* uid you don't have
/// privilege for would just fail. The `0700` still applies either way; it
/// is harmless when it is also a no-op (the directory is already
/// exclusively this process's).
#[cfg(unix)]
fn harden_job_dir(job_dir: &Path, uid: u32, gid: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if is_root() {
        chown_recursive(job_dir, uid, gid)?;
    }
    std::fs::set_permissions(job_dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn harden_job_dir(_job_dir: &Path, _uid: u32, _gid: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn chown_recursive(path: &Path, uid: u32, gid: u32) -> std::io::Result<()> {
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            chown_recursive(&entry?.path(), uid, gid)?;
        }
    }
    std::os::unix::fs::chown(path, Some(uid), Some(gid))
}

/// Floor for the *persistent* parts of `/data` that are safe to lock down
/// this way -- `cache_dir/maps`, every stored map for every repo ever
/// indexed: `0700`, so that even a bug that pointed a `tolmap-worker`-owned
/// process at this path would be refused by the mode bits alone, on top of
/// never being handed the path in the first place. Does not chown -- this
/// directory is created and owned by whichever uid the service itself runs
/// as (root in production), which is exactly the owner it should keep;
/// only the *mode* needs tightening from whatever `create_dir_all`'s
/// default (umask-dependent) leaves it at.
///
/// Deliberately **not** used for the sqlite store's own directory (see
/// `store::Store::open`, which locks the *file* down instead): in today's
/// `fly.toml` (`TOLMAP_DB_PATH=/data/tolmap.sqlite3`,
/// `TOLMAP_CACHE_DIR=/data/cache`) the store's directory is `/data`, an
/// *ancestor* of `cache_dir` -- chmod'ing `/data` to `0700` root-owned
/// would deny the dropped-uid worker even `x` (search) permission to reach
/// `/data/cache/work/.../job_dir`, breaking every job, not narrowing what a
/// compromised one can read. `cache_dir/maps` has no such conflict: it is
/// a sibling of `cache_dir/work`, never an ancestor of any path the worker
/// is handed.
#[cfg(unix)]
fn harden_persistent_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn harden_persistent_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

fn process_worker(
    state: &Arc<AppState>,
    tx: &watch::Sender<JobSnapshot>,
    spec: WorkerSpec,
    started: Instant,
    job_dir: &Path,
) -> Result<WorkerOutput, ErrorBody> {
    let exe =
        std::env::current_exe().map_err(|error| ApiError::internal(error.to_string()).body)?;
    let hardening = WorkerHardening {
        job_dir: job_dir.to_path_buf(),
        uid: state.config.worker_uid,
        gid: state.config.worker_gid,
        allow_openrouter_key: state.config.namer == crate::naming::NamerKind::Model,
    };
    process_worker_exe(tx, spec, started, &exe, Some(&state.jobs), &hardening)
}

fn process_worker_exe(
    tx: &watch::Sender<JobSnapshot>,
    spec: WorkerSpec,
    started: Instant,
    exe: &std::path::Path,
    registry: Option<&JobRegistry>,
    hardening: &WorkerHardening,
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
    // service/clone.rs) actually reads, audited by hand; anything else the
    // service's own environment carries -- including any other Fly secret
    // an operator has set -- no longer reaches this child at all.
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
    ] {
        // Pricing/budget knobs and a ledger path -- operator config set at
        // service startup, not secrets (naming.rs's `reserve`/`name_districts`).
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
    let id = tx.borrow().job_id;
    if let Some(registry) = registry {
        registry.register_child(id, child.id());
    }
    struct Registration<'a>(Option<&'a JobRegistry>, Uuid);
    impl Drop for Registration<'_> {
        fn drop(&mut self) {
            if let Some(registry) = self.0 {
                registry.unregister_child(self.1);
            }
        }
    }
    let _registration = Registration(registry, id);
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
    let write_spec = (|| -> anyhow::Result<()> {
        let mut stdin = child.stdin.take().expect("piped worker stdin");
        serde_json::to_writer(&mut stdin, &spec)?;
        stdin.write_all(b"\n")?;
        Ok(())
    })();
    if let Err(error) = write_spec {
        kill_worker_group(child.id());
        let _ = child.kill();
        let _ = child.wait();
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
    // Multi-source extraction revisits parse and resolve. The worker reports
    // each pass separately, while API counters belong to the stable stage ID.
    let mut stage_offsets = [0_u64; StageId::ALL.len()];
    let mut stage_max = [0_u64; StageId::ALL.len()];
    let mut stage_started_at: [Option<Instant>; StageId::ALL.len()] = std::array::from_fn(|_| None);
    let mut last_progress: [Option<(u64, Instant)>; StageId::ALL.len()] =
        std::array::from_fn(|_| None);
    let mut ewma_rate: [Option<f64>; StageId::ALL.len()] = [None; StageId::ALL.len()];
    let mut completed_passes = [0usize; StageId::ALL.len()];
    let mut running_eta: Option<(StageId, f64, Option<f64>, Option<f64>)> = None;
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
        match event {
            WorkerEvent::StageStarted { stage, .. } => {
                last_stage = Some(stage);
                stage_started_at[stage.index() - 1] = Some(Instant::now());
                let eta_stage = if matches!(
                    stage,
                    StageId::CloneObjects | StageId::CloneDeltas | StageId::CloneCheckout
                ) {
                    StageId::Clone
                } else {
                    stage
                };
                running_eta = Some((
                    eta_stage,
                    stage_started_at[eta_stage.index() - 1]
                        .map(|at| at.elapsed().as_secs_f64())
                        .unwrap_or(0.0),
                    None,
                    None,
                ));
                stage_offsets[stage.index() - 1] = stage_max[stage.index() - 1];
                let status = match stage {
                    StageId::Clone
                    | StageId::CloneObjects
                    | StageId::CloneDeltas
                    | StageId::CloneCheckout => JobStatus::Cloning,
                    StageId::Detect => JobStatus::Detecting,
                    _ => JobStatus::Indexing,
                };
                tx.send_modify(|snapshot| {
                    if is_terminal(snapshot) {
                        return;
                    }
                    snapshot.status = status;
                    snapshot.stage = stage.label().to_owned();
                    snapshot.progress = None;
                    snapshot.elapsed_s = started.elapsed().as_secs_f64();
                    let row = &mut snapshot.stages[stage.index() - 1];
                    row.state = StageState::Running;
                    if row.started_at.is_none() {
                        row.started_at = Some(now_rfc3339());
                    }
                });
            }
            WorkerEvent::Progress { value, .. } => {
                last_stage = Some(value.stage);
                let index = value.stage.index() - 1;
                let mut value = value;
                value.done = value.done.saturating_add(stage_offsets[index]);
                value.total = value
                    .total
                    .map(|total| total.saturating_add(stage_offsets[index]));
                value.done = value.done.max(stage_max[index]);
                stage_max[index] = value.done;
                let now = Instant::now();
                if let Some((last_done, last_at)) = last_progress[index] {
                    let dt = now.duration_since(last_at).as_secs_f64();
                    if value.done > last_done && dt > 0.0 {
                        let instantaneous = (value.done - last_done) as f64 / dt;
                        ewma_rate[index] = Some(
                            ewma_rate[index]
                                .map_or(instantaneous, |old| 0.35 * instantaneous + 0.65 * old),
                        );
                    }
                }
                last_progress[index] = Some((value.done, now));
                let rate = ewma_rate[index].or(value.rate_per_s).filter(|r| *r > 0.0);
                let features = registry
                    .map(|registry| registry.features(id))
                    .unwrap_or_default();
                let effective_total = progress_total(&value, &features);
                let remaining = effective_total.and_then(|total| {
                    rate.map(|rate| total.saturating_sub(value.done) as f64 / rate)
                });
                let fraction = effective_total
                    .filter(|total| *total > 0)
                    .map(|total| value.done as f64 / total as f64);
                let eta_stage = if matches!(
                    value.stage,
                    StageId::CloneObjects | StageId::CloneDeltas | StageId::CloneCheckout
                ) {
                    StageId::Clone
                } else {
                    value.stage
                };
                running_eta = Some((
                    eta_stage,
                    stage_started_at[eta_stage.index() - 1]
                        .map(|at| at.elapsed().as_secs_f64())
                        .unwrap_or(0.0),
                    remaining,
                    fraction,
                ));
                tx.send_modify(|snapshot| {
                    if is_terminal(snapshot) {
                        return;
                    }
                    snapshot.progress = Some(value);
                    snapshot.elapsed_s = started.elapsed().as_secs_f64();
                });
            }
            WorkerEvent::StageFinished {
                stage,
                duration_s,
                success,
                ..
            } => {
                if success {
                    completed_passes[stage.index() - 1] += 1;
                }
                tx.send_modify(|snapshot| {
                    if is_terminal(snapshot) {
                        return;
                    }
                    let row = &mut snapshot.stages[stage.index() - 1];
                    row.state = if success {
                        StageState::Done
                    } else {
                        StageState::Failed
                    };
                    row.duration_s = Some(row.duration_s.unwrap_or(0.0) + duration_s);
                    snapshot.elapsed_s = started.elapsed().as_secs_f64();
                });
                if running_eta.is_some_and(|(current, _, _, _)| current == stage) {
                    running_eta = None;
                }
            }
            WorkerEvent::Features { features, .. } => {
                if let Some(registry) = registry {
                    registry.set_features(id, features);
                }
            }
            WorkerEvent::Log { message, .. } => {
                tx.send_modify(|snapshot| {
                    if !is_terminal(snapshot) {
                        snapshot.stage = message;
                    }
                });
            }
            WorkerEvent::Result {
                map_path,
                symbols_path,
                symbols_dir,
                names_cache,
                commit,
                branch,
                lang,
                files,
                districts,
                modularity,
                ..
            } => {
                outcome = Some(WorkerOutput {
                    map_path,
                    symbols_path,
                    symbols_dir,
                    names_cache,
                    commit,
                    branch,
                    lang,
                    files,
                    districts,
                    modularity,
                });
            }
            WorkerEvent::Error { code, message, .. } => {
                error = Some(format!("{code}\n{message}"));
            }
        }
        if let Some(registry) = registry {
            registry.estimate(
                tx,
                running_eta.map(|(stage, _, remaining, fraction)| {
                    (
                        stage,
                        stage_started_at[stage.index() - 1]
                            .map(|at| at.elapsed().as_secs_f64())
                            .unwrap_or(0.0),
                        remaining,
                        fraction,
                    )
                }),
                &completed_passes,
            );
        }
    }
    if error.is_some() || registry.is_some_and(|r| r.is_cancelled(id)) {
        kill_worker_group(child.id());
    }
    let status = child.wait().map_err(|reason| ErrorBody {
        error: "worker_crashed".to_owned(),
        message: format!("wait for worker: {reason}"),
    })?;
    let stderr = stderr_reader.join().unwrap_or_default();
    if registry.is_some_and(|r| r.is_cancelled(id)) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    use axum::body::to_bytes;
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    use crate::pipeline::PruneVariant;
    use crate::service::clone::RepoSource;
    use crate::service::config::{Limits, ServeConfig};
    use crate::service::ratelimit::RateLimiter;
    use crate::service::store::Store;

    fn state(limits: Limits) -> (tempfile::TempDir, Arc<AppState>) {
        let dir = tempfile::tempdir().unwrap();
        let config = ServeConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            db_path: dir.path().join("store.sqlite3"),
            cache_dir: dir.path().join("cache"),
            static_dir: None,
            prune_variant: PruneVariant::NodeRelative,
            namer: crate::naming::NamerKind::Idf,
            namer_model: crate::naming::DEFAULT_MODEL.to_owned(),
            refs: crate::extract::RefsMode::Hand,
            limits,
            retain_commits_per_repo: 20,
            worker_uid: current_uid(),
            worker_gid: current_gid(),
        };
        let state = Arc::new(AppState {
            store: Store::open(&config.db_path).unwrap(),
            config,
            jobs: new_registry(),
            rate_limiter: RateLimiter::new(),
        });
        (dir, state)
    }

    fn repo(name: &str) -> RepoRef {
        RepoRef {
            slug: format!("test/{name}"),
            owner: "test".to_owned(),
            repo: name.to_owned(),
            source: RepoSource::Remote("unused".to_owned()),
        }
    }

    async fn until(mut check: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !check() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("condition did not become true");
    }

    fn snapshot(state: &AppState, id: Uuid) -> JobSnapshot {
        state.jobs.subscribe(id).unwrap().borrow().clone()
    }

    /// A fresh, non-terminal snapshot with every stage `Pending` -- the same
    /// shape `enqueue_job` builds, minus the registry bookkeeping a real job
    /// needs. Lets a test drive `mark_clone_running`/`mark_clone_finished`
    /// (and anything that calls them, like `materialize_job_repo`) without
    /// going through the full job-admission path.
    fn blank_snapshot(slug: &str) -> JobSnapshot {
        JobSnapshot {
            job_id: Uuid::new_v4(),
            slug: slug.to_owned(),
            commit: None,
            status: JobStatus::Queued,
            stage: "queued".to_owned(),
            queue_position: None,
            started_at: now_rfc3339(),
            finished_at: None,
            error: None,
            error_code: None,
            progress: None,
            eta: None,
            eta_start_s: None,
            elapsed_s: 0.0,
            stages: StageId::ALL
                .iter()
                .map(|&id| StageSnapshot {
                    id,
                    label: id.label().to_owned(),
                    state: StageState::Pending,
                    started_at: None,
                    duration_s: None,
                })
                .collect(),
        }
    }

    /// Initialises a one-commit git fixture repo at `dir` -- the same
    /// init/commit sequence `tests/service_hardening.rs` already uses.
    fn init_git_fixture(dir: &Path) {
        assert!(Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
        std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
        assert!(Command::new("git")
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "add",
                "-A"
            ])
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-qm",
                "initial",
            ])
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
    }

    /// The first relative path (under `objects/`, loose or packed alike --
    /// a local clone copies whichever form the source has, no repack) that
    /// exists under both `a_objects` and `b_objects`. `git clone --local`
    /// copies the object store by filename, so a job's local-clone copy and
    /// the shared cache it was copied from are expected to share at least
    /// one identically-named file; the test uses this to find one to
    /// compare inodes on.
    fn first_common_object_relpath(a_objects: &Path, b_objects: &Path) -> PathBuf {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    walk(&path, base, out);
                } else {
                    out.push(path.strip_prefix(base).unwrap().to_path_buf());
                }
            }
        }
        let mut a_files = Vec::new();
        walk(a_objects, a_objects, &mut a_files);
        a_files
            .into_iter()
            .find(|rel| b_objects.join(rel).is_file())
            .expect("shared cache and job local-clone should share at least one object file")
    }

    /// Exercises exactly the sequence `run_blocking` runs before spawning
    /// the worker (materialize into the shared cache, then a local-clone
    /// copy into a job directory) -- the mechanism PR #121's clone-cache-
    /// reuse redesign depends on, and which no `remote-build.yml` dispatch
    /// exercises (that workflow's `tolmap build`/`tolmap worker` steps take
    /// an already-cloned local directory directly, never through
    /// `service::jobs`). Runs entirely offline against a real local git
    /// fixture repo: `RepoSource::Remote` accepts a local filesystem path
    /// as a clone/fetch source exactly as well as an `https://` URL, so
    /// this takes the same `clone_blobless`/`fetch_and_fast_forward` branch
    /// a real remote clone would -- not the `RepoSource::Local` "read in
    /// place" shortcut this file's other fixtures use.
    #[test]
    fn materialize_job_repo_reuses_the_shared_clone_cache_on_a_second_call() {
        let (_dir, state) = state(Limits::default());
        let source = tempfile::tempdir().unwrap();
        init_git_fixture(source.path());
        // `file://` rather than a bare path: a bare local path makes `git
        // clone` prefer its own hardlink-based "local optimization"
        // transport, which git silently drops `--filter` support for.
        // `file://` forces the smart (upload-pack) transport, which is what
        // a real `https://` remote uses too and is what partial clone
        // (`--filter=blob:none`) actually requires -- this is the same
        // transport git's own test suite uses to exercise partial clone
        // locally.
        let repo_ref = RepoRef {
            slug: "acme/widgets".to_owned(),
            owner: "acme".to_owned(),
            repo: "widgets".to_owned(),
            source: RepoSource::Remote(format!("file://{}", source.path().display())),
        };
        let cached = state
            .config
            .cache_dir
            .join("repos")
            .join("acme")
            .join("widgets");
        let job_root = tempfile::tempdir().unwrap();
        let job_repo_one = job_root.path().join("one").join("repo");
        let job_repo_two = job_root.path().join("two").join("repo");
        std::fs::create_dir_all(job_repo_one.parent().unwrap()).unwrap();
        std::fs::create_dir_all(job_repo_two.parent().unwrap()).unwrap();

        let (tx, _rx) = watch::channel(blank_snapshot(&repo_ref.slug));
        materialize_job_repo(
            &state.config.cache_dir,
            &repo_ref,
            &state.config.limits,
            &job_repo_one,
            &tx,
            Instant::now(),
        )
        .unwrap();
        assert!(
            cached.join(".git").is_dir(),
            "first call clones into the shared cache"
        );
        assert!(job_repo_one.join(".git").is_dir());
        assert_eq!(
            snapshot_stage_state(&tx),
            StageState::Done,
            "the Clone stage must be marked done once materialize_job_repo succeeds"
        );

        // A file only a *fetch* (not a fresh `git clone`, which would first
        // remove/recreate the destination) would leave in place across the
        // second call.
        let sentinel = cached.join("only-a-fetch-keeps-this");
        std::fs::write(&sentinel, b"x").unwrap();

        let (tx2, _rx2) = watch::channel(blank_snapshot(&repo_ref.slug));
        materialize_job_repo(
            &state.config.cache_dir,
            &repo_ref,
            &state.config.limits,
            &job_repo_two,
            &tx2,
            Instant::now(),
        )
        .unwrap();
        assert!(
            sentinel.exists(),
            "second materialize for the same owner/repo must fetch/fast-forward \
             the existing shared-cache clone, not delete and re-clone it"
        );
        assert!(job_repo_two.join(".git").is_dir());

        // `--no-hardlinks`: the job's own copy must not share inodes with
        // the shared cache's own objects -- see `clone::local_clone_into`'s
        // doc comment for why sharing them would undo this PR's isolation.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let rel = first_common_object_relpath(
                &cached.join(".git").join("objects"),
                &job_repo_two.join(".git").join("objects"),
            );
            let cached_meta =
                std::fs::metadata(cached.join(".git").join("objects").join(&rel)).unwrap();
            let job_meta =
                std::fs::metadata(job_repo_two.join(".git").join("objects").join(&rel)).unwrap();
            assert_ne!(
                cached_meta.ino(),
                job_meta.ino(),
                "the job's local-clone copy must be real files, not hardlinks \
                 into the shared cache"
            );
        }
    }

    fn snapshot_stage_state(tx: &watch::Sender<JobSnapshot>) -> StageState {
        tx.borrow().stages[StageId::Clone.index() - 1].state
    }

    #[test]
    fn previous_snapshot_shape_deserializes_without_eta_fields() {
        let job_id = Uuid::new_v4();
        let mut value = serde_json::json!({
            "job_id": job_id, "slug": "a/b", "commit": null,
            "status": "queued", "stage": "queued", "queue_position": 1,
            "started_at": "2026-09-24T00:00:00Z", "finished_at": null,
            "error": null, "error_code": null, "progress": null,
            "elapsed_s": 0.0, "stages": []
        });
        let decoded: JobSnapshot = serde_json::from_value(value.take()).unwrap();
        assert!(decoded.eta.is_none() && decoded.eta_start_s.is_none());
    }

    #[tokio::test]
    async fn bounded_fifo_deduplicates_and_updates_positions() {
        let limits = Limits {
            max_concurrent_jobs: 1,
            max_queued_jobs: 2,
            ..Limits::default()
        };
        let (_dir, state) = state(limits);
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let started = Arc::new(Mutex::new(Vec::<String>::new()));
        let runner: JobRunner = Arc::new({
            let release_rx = release_rx.clone();
            let started = started.clone();
            move |_, repo, tx| {
                started.lock().unwrap().push(repo.slug);
                release_rx.lock().unwrap().recv().unwrap();
                finish_done(&tx);
            }
        });

        let first =
            enqueue_job(state.clone(), repo("one"), "a".to_owned(), runner.clone()).unwrap();
        until(|| started.lock().unwrap().len() == 1).await;
        let second =
            enqueue_job(state.clone(), repo("two"), "b".to_owned(), runner.clone()).unwrap();
        let third =
            enqueue_job(state.clone(), repo("three"), "c".to_owned(), runner.clone()).unwrap();
        assert_eq!(snapshot(&state, second).queue_position, Some(1));
        assert_eq!(snapshot(&state, third).queue_position, Some(2));
        let expected = snapshot(&state, first).eta.unwrap().midpoint()
            + snapshot(&state, second).eta.unwrap().midpoint();
        assert!((snapshot(&state, third).eta_start_s.unwrap() - expected).abs() < 0.001);
        assert_eq!(snapshot(&state, second).stage, "queued");
        assert_eq!(
            enqueue_job(state.clone(), repo("two"), "b".to_owned(), runner.clone()).unwrap(),
            second
        );

        let busy =
            enqueue_job(state.clone(), repo("four"), "d".to_owned(), runner.clone()).unwrap_err();
        let response = busy.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::RETRY_AFTER], "30");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"],
            "busy"
        );

        release_tx.send(()).unwrap();
        until(|| started.lock().unwrap().len() == 2).await;
        assert_eq!(snapshot(&state, first).status, JobStatus::Done);
        assert_eq!(snapshot(&state, second).queue_position, None);
        assert_eq!(snapshot(&state, third).queue_position, Some(1));
        release_tx.send(()).unwrap();
        until(|| started.lock().unwrap().len() == 3).await;
        assert_eq!(snapshot(&state, third).queue_position, None);
        release_tx.send(()).unwrap();
        until(|| snapshot(&state, third).status == JobStatus::Done).await;
        assert_ne!(
            enqueue_job(state.clone(), repo("two"), "b".to_owned(), runner.clone()).unwrap(),
            second
        );
        release_tx.send(()).unwrap();
    }

    #[tokio::test]
    async fn cancelling_queued_job_is_idempotent_and_repositions_fifo() {
        use axum::body::Body;
        use axum::extract::ConnectInfo;
        use axum::http::Request;
        use tower::ServiceExt;
        let (_dir, state) = state(Limits::default());
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let runner: JobRunner = Arc::new(move |_, _, tx| {
            release_rx.lock().unwrap().recv().unwrap();
            finish_done(&tx);
        });
        let first =
            enqueue_job(state.clone(), repo("one"), "a".to_owned(), runner.clone()).unwrap();
        let second =
            enqueue_job(state.clone(), repo("two"), "b".to_owned(), runner.clone()).unwrap();
        let third = enqueue_job(state.clone(), repo("three"), "c".to_owned(), runner).unwrap();
        assert_eq!(snapshot(&state, third).queue_position, Some(2));
        let before = snapshot(&state, third).eta_start_s.unwrap();
        let cancel_request = || {
            Request::builder()
                .method("POST")
                .uri(format!("/api/jobs/{second}/cancel"))
                .extension(ConnectInfo(
                    "127.0.0.1:1".parse::<std::net::SocketAddr>().unwrap(),
                ))
                .body(Body::empty())
                .unwrap()
        };
        let response = crate::service::http::router(state.clone())
            .oneshot(cancel_request())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let cancelled: JobSnapshot = serde_json::from_slice(&body).unwrap();
        assert_eq!(cancelled.status, JobStatus::Failed);
        assert_eq!(cancelled.error_code.as_deref(), Some("cancelled"));
        let repeated = crate::service::http::router(state.clone())
            .oneshot(cancel_request())
            .await
            .unwrap();
        assert_eq!(repeated.status(), StatusCode::OK);
        assert_eq!(
            state.jobs.cancel(second).unwrap().error_code.as_deref(),
            Some("cancelled")
        );
        assert_eq!(snapshot(&state, third).queue_position, Some(1));
        assert!(snapshot(&state, third).eta_start_s.unwrap() < before);
        release_tx.send(()).unwrap();
        until(|| snapshot(&state, first).status == JobStatus::Done).await;
        release_tx.send(()).unwrap();
        until(|| snapshot(&state, third).status == JobStatus::Done).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn killed_worker_fails_one_job_and_queue_accepts_the_next() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, state) = state(Limits::default());
        let fake_worker = dir.path().join("killed-worker");
        std::fs::write(&fake_worker,
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"stage_started\",\"v\":1,\"stage\":\"parse\"}'\nsleep 0.3\nkill -9 $$\n"
        ).unwrap();
        std::fs::set_permissions(&fake_worker, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner: JobRunner = Arc::new(move |_, repo, tx| {
            if repo.repo == "first" {
                let spec = WorkerSpec {
                    v: 1,
                    slug: repo.slug,
                    owner: repo.owner,
                    repo: repo.repo,
                    source: "unused".to_owned(),
                    local: false,
                    all_sources: false,
                    cache_dir: String::new(),
                    output_dir: String::new(),
                    clone_cache_bytes: 1,
                    prune_variant: "node-relative".to_owned(),
                    namer: "idf".to_owned(),
                    namer_model: String::new(),
                    previous_maps: vec![],
                    names_cache: None,
                    refs: None,
                };
                let error = process_worker_exe(
                    &tx,
                    spec,
                    Instant::now(),
                    &fake_worker,
                    None,
                    &WorkerHardening::for_test(dir.path()),
                )
                .err()
                .expect("killed child fails");
                finish_failed(&tx, error);
            } else {
                finish_done(&tx);
            }
        });
        let first =
            enqueue_job(state.clone(), repo("first"), "a".to_owned(), runner.clone()).unwrap();
        until(|| snapshot(&state, first).status == JobStatus::Failed).await;
        let failed = snapshot(&state, first);
        assert_eq!(failed.error_code.as_deref(), Some("worker_crashed"));
        assert!(failed.error.unwrap().contains("Parsing files"));
        assert_eq!(
            failed.stages[StageId::Parse.index() - 1].state,
            StageState::Failed
        );
        let second = enqueue_job(state.clone(), repo("second"), "b".to_owned(), runner).unwrap();
        until(|| snapshot(&state, second).status == JobStatus::Done).await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cancelling_running_worker_kills_git_child_and_starts_next_job() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, state) = state(Limits::default());
        let fake_worker = dir.path().join("sleeping-worker");
        let child_pid_file = dir.path().join("descendant.pid");
        let script = format!(
            "#!/bin/sh\ncat >/dev/null\nsleep 30 &\necho $! > '{}'\nprintf '%s\\n' '{{\"type\":\"stage_started\",\"v\":1,\"stage\":\"parse\"}}'\nwait\n",
            child_pid_file.display()
        );
        std::fs::write(&fake_worker, script).unwrap();
        std::fs::set_permissions(&fake_worker, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner: JobRunner = Arc::new(move |state, repo, tx| {
            if repo.repo == "first" {
                let spec = WorkerSpec {
                    v: 1,
                    slug: repo.slug,
                    owner: repo.owner,
                    repo: repo.repo,
                    source: "unused".to_owned(),
                    local: false,
                    all_sources: false,
                    cache_dir: String::new(),
                    output_dir: String::new(),
                    clone_cache_bytes: 1,
                    prune_variant: "node-relative".to_owned(),
                    namer: "idf".to_owned(),
                    namer_model: String::new(),
                    previous_maps: vec![],
                    names_cache: None,
                    refs: None,
                };
                let error = process_worker_exe(
                    &tx,
                    spec,
                    Instant::now(),
                    &fake_worker,
                    Some(&state.jobs),
                    &WorkerHardening::for_test(dir.path()),
                )
                .err()
                .expect("cancelled child must exit");
                finish_failed(&tx, error);
            } else {
                finish_done(&tx);
            }
        });
        let first =
            enqueue_job(state.clone(), repo("first"), "a".to_owned(), runner.clone()).unwrap();
        let second = enqueue_job(state.clone(), repo("second"), "b".to_owned(), runner).unwrap();
        until(|| child_pid_file.exists() && snapshot(&state, first).status == JobStatus::Indexing)
            .await;
        let child_pid: u32 = std::fs::read_to_string(&child_pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let cancelled = state.jobs.cancel(first).unwrap();
        assert_eq!(cancelled.error_code.as_deref(), Some("cancelled"));
        until(|| snapshot(&state, second).status == JobStatus::Done).await;
        until(|| {
            let stat = std::fs::read_to_string(format!("/proc/{child_pid}/stat"));
            stat.is_err()
                || stat
                    .unwrap()
                    .split(") ")
                    .nth(1)
                    .is_some_and(|tail| tail.starts_with('Z'))
        })
        .await;
        assert_eq!(snapshot(&state, first).status, JobStatus::Failed);
        assert!(snapshot(&state, first).eta.is_none());
        assert_eq!(snapshot(&state, second).queue_position, None);
    }

    /// Direct-logic coverage for graceful shutdown's "stop admitting + fail
    /// everything" step (`JobRegistry::shutdown`), independent of the
    /// SIGINT/SIGTERM wiring in `service::serve` -- see this PR's
    /// description for which half is covered where. Uses the same
    /// block-until-released fake runner as `bounded_fifo_deduplicates_...`
    /// above, so both a queued and a running job exist at once.
    #[tokio::test]
    async fn shutdown_fails_queued_and_running_jobs_and_blocks_new_admission() {
        let limits = Limits {
            max_concurrent_jobs: 1,
            max_queued_jobs: 2,
            ..Limits::default()
        };
        let (_dir, state) = state(limits);
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let started = Arc::new(Mutex::new(Vec::<String>::new()));
        let runner: JobRunner = Arc::new({
            let release_rx = release_rx.clone();
            let started = started.clone();
            move |_, repo, tx| {
                started.lock().unwrap().push(repo.slug);
                release_rx.lock().unwrap().recv().unwrap();
                finish_done(&tx);
            }
        });

        let running = enqueue_job(
            state.clone(),
            repo("running"),
            "a".to_owned(),
            runner.clone(),
        )
        .unwrap();
        until(|| started.lock().unwrap().len() == 1).await;
        let queued = enqueue_job(
            state.clone(),
            repo("queued"),
            "b".to_owned(),
            runner.clone(),
        )
        .unwrap();
        assert_eq!(snapshot(&state, queued).queue_position, Some(1));

        state.jobs.shutdown();

        let running_snapshot = snapshot(&state, running);
        assert_eq!(running_snapshot.status, JobStatus::Failed);
        assert_eq!(
            running_snapshot.error_code.as_deref(),
            Some("server_stopping")
        );
        let queued_snapshot = snapshot(&state, queued);
        assert_eq!(queued_snapshot.status, JobStatus::Failed);
        assert_eq!(
            queued_snapshot.error_code.as_deref(),
            Some("server_stopping")
        );

        let rejected = enqueue_job(
            state.clone(),
            repo("after-shutdown"),
            "c".to_owned(),
            runner.clone(),
        )
        .unwrap_err();
        assert_eq!(rejected.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(rejected.body.error, "server_stopping");

        // The "running" job's fake runner closure is still blocked on
        // `release_rx.recv()` in its own spawn_blocking thread --
        // `shutdown` only updates the snapshot and kills a registered *OS*
        // child (there is none here, this runner is a plain closure), it
        // does not and cannot force an arbitrary Rust closure to return.
        // Release it so the thread winds down instead of outliving the test.
        release_tx.send(()).unwrap();
    }

    /// Wiring-adjacent coverage for the other half of shutdown: a real
    /// worker child (with its own descendant process, standing in for git)
    /// actually gets killed, not just marked failed in the registry. Copied
    /// from `cancelling_running_worker_kills_git_child_and_starts_next_job`
    /// above, substituting `JobRegistry::shutdown` for `cancel`. A real
    /// SIGTERM reaching `service::serve`'s signal handler and calling this
    /// is not exercised here -- that end-to-end wiring is covered by code
    /// inspection only, per this PR's description.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn shutdown_kills_running_worker_child_and_fails_it_with_server_stopping() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, state) = state(Limits::default());
        let fake_worker = dir.path().join("sleeping-worker-shutdown");
        let child_pid_file = dir.path().join("descendant-shutdown.pid");
        let script = format!(
            "#!/bin/sh\ncat >/dev/null\nsleep 30 &\necho $! > '{}'\nprintf '%s\\n' '{{\"type\":\"stage_started\",\"v\":1,\"stage\":\"parse\"}}'\nwait\n",
            child_pid_file.display()
        );
        std::fs::write(&fake_worker, script).unwrap();
        std::fs::set_permissions(&fake_worker, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner: JobRunner = Arc::new(move |state, repo, tx| {
            let spec = WorkerSpec {
                v: 1,
                slug: repo.slug,
                owner: repo.owner,
                repo: repo.repo,
                source: "unused".to_owned(),
                local: false,
                all_sources: false,
                cache_dir: String::new(),
                output_dir: String::new(),
                clone_cache_bytes: 1,
                prune_variant: "node-relative".to_owned(),
                namer: "idf".to_owned(),
                namer_model: String::new(),
                previous_maps: vec![],
                names_cache: None,
            };
            let error = process_worker_exe(
                &tx,
                spec,
                Instant::now(),
                &fake_worker,
                Some(&state.jobs),
                &WorkerHardening::for_test(dir.path()),
            )
            .err()
            .expect("shutdown must fail the running child");
            finish_failed(&tx, error);
        });
        let id = enqueue_job(state.clone(), repo("solo"), "a".to_owned(), runner).unwrap();
        until(|| child_pid_file.exists() && snapshot(&state, id).status == JobStatus::Indexing)
            .await;
        let child_pid: u32 = std::fs::read_to_string(&child_pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        state.jobs.shutdown();

        until(|| {
            let stat = std::fs::read_to_string(format!("/proc/{child_pid}/stat"));
            stat.is_err()
                || stat
                    .unwrap()
                    .split(") ")
                    .nth(1)
                    .is_some_and(|tail| tail.starts_with('Z'))
        })
        .await;
        let final_snapshot = snapshot(&state, id);
        assert_eq!(final_snapshot.status, JobStatus::Failed);
        assert_eq!(
            final_snapshot.error_code.as_deref(),
            Some("server_stopping")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sse_progress_done_never_decreases_within_a_stage() {
        use axum::body::Body;
        use axum::http::Request;
        use std::os::unix::fs::PermissionsExt;
        use tower::ServiceExt;
        let (dir, state) = state(Limits::default());
        let fake_worker = dir.path().join("regressing-worker");
        let value = |done| {
            format!(
            "{{\"type\":\"progress\",\"v\":1,\"value\":{{\"stage\":\"parse\",\"stage_index\":6,\"stage_count\":18,\"label\":\"Parsing files\",\"unit\":\"files\",\"done\":{done},\"total\":3,\"rate_per_s\":null}}}}"
        )
        };
        let script = format!(
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{}'\nsleep 0.35\nprintf '%s\\n' '{}'\nsleep 0.35\nprintf '%s\\n' '{}'\nsleep 0.35\nprintf '%s\\n' '{{\"type\":\"stage_finished\",\"v\":1,\"stage\":\"parse\",\"duration_s\":0.1,\"success\":true}}' '{{\"type\":\"stage_started\",\"v\":1,\"stage\":\"parse\"}}' '{}'\nsleep 0.35\nprintf '%s\\n' '{{\"type\":\"error\",\"v\":1,\"code\":\"index_failed\",\"message\":\"test\"}}'\n",
            value(1), value(3), value(2), value(1),
        );
        std::fs::write(&fake_worker, script).unwrap();
        std::fs::set_permissions(&fake_worker, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner: JobRunner = Arc::new(move |_, repo, tx| {
            let spec = WorkerSpec {
                v: 1,
                slug: repo.slug,
                owner: repo.owner,
                repo: repo.repo,
                source: "unused".to_owned(),
                local: false,
                all_sources: false,
                cache_dir: String::new(),
                output_dir: String::new(),
                clone_cache_bytes: 1,
                prune_variant: "node-relative".to_owned(),
                namer: "idf".to_owned(),
                namer_model: String::new(),
                previous_maps: vec![],
                names_cache: None,
                refs: None,
            };
            let error = process_worker_exe(
                &tx,
                spec,
                Instant::now(),
                &fake_worker,
                None,
                &WorkerHardening::for_test(dir.path()),
            )
            .err()
            .expect("worker terminal error");
            finish_failed(&tx, error);
        });
        let id = enqueue_job(state.clone(), repo("sse"), "a".to_owned(), runner).unwrap();
        let response = crate::service::http::router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!("/api/jobs/{id}/events"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = tokio::time::timeout(
            Duration::from_secs(5),
            to_bytes(response.into_body(), 1024 * 1024),
        )
        .await
        .unwrap_or_else(|_| panic!("SSE did not close; snapshot: {:?}", snapshot(&state, id)))
        .unwrap();
        let frames = String::from_utf8(body.to_vec()).unwrap();
        let mut seen = Vec::new();
        for line in frames
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
        {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            if value["progress"]["stage"] == "parse" {
                seen.push(value["progress"]["done"].as_u64().unwrap());
            }
        }
        assert!(
            seen.len() >= 2,
            "expected multiple progress frames: {frames}"
        );
        assert!(seen.windows(2).all(|pair| pair[0] <= pair[1]), "{seen:?}");
    }

    /// Worker hardening item 4a: the child must never see anything set only
    /// in the *parent's* environment. `env_clear()` inside
    /// `process_worker_exe` is exactly what is under test -- everything
    /// else about this test exists only to observe that from outside the
    /// child, since a fake worker script is the only seam this test suite
    /// has for asking a spawned child what it saw (same fake-worker-script
    /// pattern as `killed_worker_fails_one_job_and_queue_accepts_the_next`
    /// and friends above).
    #[cfg(unix)]
    #[tokio::test]
    async fn worker_child_never_observes_a_marker_only_set_in_the_parent_environment() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, state) = state(Limits::default());
        let fake_worker = dir.path().join("env-checking-worker");
        // Reports the marker's value back as a worker `error` event's
        // `message` -- a real event the harness already knows how to
        // parse, rather than a new stdout-protocol special case.
        std::fs::write(
            &fake_worker,
            "#!/bin/sh\ncat >/dev/null\nmarker=${TOLMAP_TEST_ENV_CLEAR_MARKER:-unset}\nprintf '{\"type\":\"error\",\"v\":1,\"code\":\"test\",\"message\":\"marker=%s\"}\\n' \"$marker\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_worker, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner: JobRunner = Arc::new(move |_, repo, tx| {
            let spec = WorkerSpec {
                v: 1,
                slug: repo.slug,
                owner: repo.owner,
                repo: repo.repo,
                source: "unused".to_owned(),
                local: false,
                all_sources: false,
                cache_dir: String::new(),
                output_dir: String::new(),
                clone_cache_bytes: 1,
                prune_variant: "node-relative".to_owned(),
                namer: "idf".to_owned(),
                namer_model: String::new(),
                previous_maps: vec![],
                names_cache: None,
            };
            let error = process_worker_exe(
                &tx,
                spec,
                Instant::now(),
                &fake_worker,
                None,
                &WorkerHardening::for_test(dir.path()),
            )
            .err()
            .expect("fake worker reports the marker via an error event");
            finish_failed(&tx, error);
        });
        // Set only in this test process -- never passed to `enqueue_job` or
        // `process_worker_exe` any other way. Before this change, the old
        // `Command::new(exe)` with no `env_clear()` would have inherited
        // this (and everything else, including a real `OPENROUTER_API_KEY`)
        // straight into the child.
        std::env::set_var("TOLMAP_TEST_ENV_CLEAR_MARKER", "leaked");
        let id = enqueue_job(state.clone(), repo("env-leak"), "a".to_owned(), runner).unwrap();
        until(|| snapshot(&state, id).status == JobStatus::Failed).await;
        std::env::remove_var("TOLMAP_TEST_ENV_CLEAR_MARKER");
        let failed = snapshot(&state, id);
        assert_eq!(
            failed.error.as_deref(),
            Some("marker=unset"),
            "the worker child must never see a variable set only in the parent's environment"
        );
    }

    /// Worker hardening item 4b: with real uid separation (root dropping to
    /// an unprivileged uid, item 2), a worker cannot read a file outside
    /// its own job directory. This needs to actually run as root to prove
    /// anything -- `CommandExt::uid()/gid()` to a *different* uid fails
    /// outright otherwise, which is exactly what `process_worker_exe`'s own
    /// `is_root()` check already skips (see `warn_unprivileged_once`). On a
    /// GitHub Actions `ubuntu-latest` runner (not root) this test compiles
    /// and no-ops via the guard below; state plainly in the PR body whether
    /// a given CI run actually exercised the privileged path or only
    /// compiled it.
    #[cfg(unix)]
    #[tokio::test]
    async fn worker_dropped_to_an_unprivileged_uid_cannot_read_a_root_owned_file_outside_its_job_dir(
    ) {
        if !is_root() {
            eprintln!(
                "skipping worker_dropped_to_an_unprivileged_uid_cannot_read_a_root_owned_file_outside_its_job_dir: \
                 this test process is not root, so CommandExt::uid()/gid() to a different uid \
                 cannot be exercised here -- it compiles and no-ops, same as every non-root CI \
                 run; see the PR body."
            );
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let (dir, state) = state(Limits::default());
        // `tempfile` creates its directory `0700` by default -- which,
        // being root-owned, would stop the dropped-uid worker from even
        // traversing down to `job_dir` (a `execve`/spawn failure, not the
        // read-permission-denied this test means to exercise). Widen only
        // the traversal (`x`) bit, not read/write, on this one ancestor;
        // the actual isolation this test checks is `secret`'s own `0600`
        // below, plus `job_dir`'s ownership.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        // Owned by root (this test process, confirmed above), 0600 --
        // stands in for the persistent `/data` paths `harden_persistent_dir`
        // locks down (the store, `cache_dir/maps`): unreadable by anything
        // but the owning uid.
        let secret = dir.path().join("root-secret.txt");
        std::fs::write(&secret, "do not read me").unwrap();
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).unwrap();
        let job_dir = dir.path().join("job");
        std::fs::create_dir_all(&job_dir).unwrap();
        // 65534 (`nobody`/`nogroup` on Debian, including the
        // `debian:bookworm-slim` base this project's own Dockerfile runtime
        // stage uses) stands in for the dedicated `tolmap-worker` uid the
        // Dockerfile bakes in -- this test only needs *some* unprivileged
        // uid guaranteed to exist wherever it runs as root, not that exact
        // one.
        let uid = 65534;
        let gid = 65534;
        harden_job_dir(&job_dir, uid, gid).unwrap();
        let fake_worker = dir.path().join("secret-reading-worker");
        let script = format!(
            "#!/bin/sh\ncat >/dev/null\nif cat '{}' >/dev/null 2>&1; then printf '%s\\n' '{{\"type\":\"error\",\"v\":1,\"code\":\"test\",\"message\":\"read_secret:yes\"}}'; else printf '%s\\n' '{{\"type\":\"error\",\"v\":1,\"code\":\"test\",\"message\":\"read_secret:no\"}}'; fi\n",
            secret.display(),
        );
        std::fs::write(&fake_worker, script).unwrap();
        std::fs::set_permissions(&fake_worker, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner: JobRunner = Arc::new(move |_, repo, tx| {
            let spec = WorkerSpec {
                v: 1,
                slug: repo.slug,
                owner: repo.owner,
                repo: repo.repo,
                source: "unused".to_owned(),
                local: false,
                all_sources: false,
                cache_dir: String::new(),
                output_dir: String::new(),
                clone_cache_bytes: 1,
                prune_variant: "node-relative".to_owned(),
                namer: "idf".to_owned(),
                namer_model: String::new(),
                previous_maps: vec![],
                names_cache: None,
            };
            let hardening = WorkerHardening {
                job_dir: job_dir.clone(),
                uid,
                gid,
                allow_openrouter_key: false,
            };
            let error =
                process_worker_exe(&tx, spec, Instant::now(), &fake_worker, None, &hardening)
                    .err()
                    .expect("fake worker reports via an error event");
            finish_failed(&tx, error);
        });
        let id = enqueue_job(state.clone(), repo("secret"), "a".to_owned(), runner).unwrap();
        until(|| snapshot(&state, id).status == JobStatus::Failed).await;
        let failed = snapshot(&state, id);
        assert_eq!(
            failed.error.as_deref(),
            Some("read_secret:no"),
            "a worker dropped to an unprivileged uid must not be able to read a root-owned file \
             outside its job directory"
        );
    }
}
