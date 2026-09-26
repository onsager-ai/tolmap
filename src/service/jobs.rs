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
use crate::service::schedule::{self, Class};
use crate::service::store::MapRow;
use crate::service::time::now_rfc3339;
use crate::service::worker_result;
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

/// One worker slot: a `worker_loop` task while it holds a job, idle
/// otherwise. Its index in `RegistryInner::slots` is its stable worker id.
struct Slot {
    class: usize,
    running: Option<(Uuid, watch::Sender<JobSnapshot>)>,
}

#[derive(Default)]
struct RegistryInner {
    jobs: HashMap<Uuid, watch::Sender<JobSnapshot>>,
    active: HashMap<JobKey, Uuid>,
    /// Worker classes, smallest first (`schedule::order_classes`); a
    /// class is its index here. Empty until the first admission -- see
    /// `ensure_classes`.
    classes: Vec<Class>,
    /// One FIFO per class, same index as `classes` (docs/WORKER_TIER.md
    /// §7.1).
    queues: Vec<VecDeque<PendingJob>>,
    /// Worker slots, numbered smallest class first
    /// (`schedule::worker_classes`).
    slots: Vec<Slot>,
    children: HashMap<Uuid, u32>,
    cancelled: HashSet<Uuid>,
    features: HashMap<Uuid, RepoFeatures>,
    eta_model: EtaModel,
    /// Set once by [`JobRegistry::shutdown`] and never cleared -- the
    /// process is exiting, not pausing. Checked by `enqueue_job` so no job
    /// is admitted after a shutdown signal starts draining the registry.
    stopping: bool,
}

impl RegistryInner {
    /// Local mode's classes: exactly one, of unknown size (`None` usable
    /// memory), with `TOLMAP_MAX_CONCURRENT_JOBS` slots, which is today's
    /// single FIFO (§7.1: "with one class this is exactly today's single
    /// FIFO"). Phase 0 adds no configuration.
    ///
    /// Built on the first admission rather than in `new_registry`, which
    /// takes no configuration and is called that way from the integration
    /// tests; the limits cannot change while the service runs, so building
    /// late is the same as building early. Nothing reads the classes before
    /// a job exists: every other path finds no slots and empty queues.
    fn ensure_classes(&mut self, limits: &crate::service::config::Limits) {
        if !self.classes.is_empty() {
            return;
        }
        let mut classes = vec![Class {
            usable_memory: None,
            slots: limits.max_concurrent_jobs.max(1),
        }];
        schedule::order_classes(&mut classes);
        self.slots = schedule::worker_classes(&classes)
            .into_iter()
            .map(|class| Slot {
                class,
                running: None,
            })
            .collect();
        self.queues = classes.iter().map(|_| VecDeque::new()).collect();
        self.classes = classes;
    }

    /// The idle slot a job bound to `class` starts on at once, if any: one
    /// of its own class or larger (a larger idle worker spills down, as it
    /// would have on its last `next_for`), the lowest id first. Ids run
    /// smallest class first, so this prefers the job's own class, and it is
    /// the same tie-break `schedule::simulate` uses for workers free now.
    fn idle_slot_for(&self, class: usize) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.class >= class && slot.running.is_none())
    }
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
        let queued = registry
            .queues
            .iter()
            .enumerate()
            .find_map(|(class, queue)| {
                queue
                    .iter()
                    .position(|job| job.id == id)
                    .map(|position| (class, position))
            });
        if let Some((class, position)) = queued {
            let job = registry.queues[class]
                .remove(position)
                .expect("position exists");
            registry.active.remove(&job.key);
            registry.features.remove(&job.id);
            finish_failed(&tx, cancelled_error());
            simulate_queue_etas(&registry);
        } else {
            registry.cancelled.insert(id);
            registry.active.retain(|_, active_id| *active_id != id);
            finish_failed(&tx, cancelled_error());
            if let Some(&pid) = registry.children.get(&id) {
                kill_worker_group(pid);
            }
            simulate_queue_etas(&registry);
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
        simulate_queue_etas(&registry);
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
        // Every class's queue, smallest class first, each in FIFO order.
        let queued: Vec<PendingJob> = registry
            .queues
            .iter_mut()
            .flat_map(|queue| queue.drain(..))
            .collect();
        for job in queued {
            registry.active.remove(&job.key);
            registry.features.remove(&job.id);
            finish_failed(&job.tx, server_stopping_error());
        }
        let running: Vec<(Uuid, watch::Sender<JobSnapshot>)> = registry
            .slots
            .iter()
            .filter_map(|slot| slot.running.clone())
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
        simulate_queue_etas(&registry);
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

/// Writes `queue_position`, `eta_start_s` and `eta` into every queued
/// snapshot from `schedule::simulate` (docs/WORKER_TIER.md §7.2).
///
/// This replaced a running sum: the remaining time of every running job
/// added together, then each queued job's in turn. That is exact for one
/// slot and overstates the wait for several, because a queued job starts
/// when the first slot frees, not after all of them. The simulation keeps
/// the one-slot numbers bit for bit (`schedule`'s
/// `one_slot_reproduces_todays_queue_eta_exactly`), so the inputs below are
/// the ones the sum used: a running job's remaining ETA midpoint, the
/// prior's when it has none yet, 0 once its row is terminal (a cancelled
/// job whose worker has not been reaped), and 0 for an idle slot.
fn simulate_queue_etas(registry: &RegistryInner) {
    let never_started = [false; StageId::ALL.len()];
    let prior = registry
        .eta_model
        .predict(&RepoFeatures::default(), &never_started, None);
    let workers: Vec<schedule::Worker> = registry
        .slots
        .iter()
        .enumerate()
        .map(|(id, slot)| schedule::Worker {
            id,
            class: slot.class,
            free_in_s: slot.running.as_ref().map_or(0.0, |(_, tx)| {
                let row = tx.borrow();
                if is_terminal(&row) {
                    0.0
                } else {
                    row.eta.unwrap_or(prior).midpoint()
                }
            }),
        })
        .collect();
    let mut etas: BTreeMap<Uuid, Eta> = BTreeMap::new();
    let mut queued: Vec<Vec<schedule::Queued<Uuid>>> = Vec::with_capacity(registry.queues.len());
    for queue in &registry.queues {
        let mut class_queue = Vec::with_capacity(queue.len());
        for job in queue {
            let features = registry.features.get(&job.id).cloned().unwrap_or_default();
            let eta = registry.eta_model.predict(&features, &never_started, None);
            etas.insert(job.id, eta);
            class_queue.push(schedule::Queued {
                job: job.id,
                midpoint_s: eta.midpoint(),
            });
        }
        queued.push(class_queue);
    }
    let starts = schedule::simulate(&workers, &queued);
    for job in registry.queues.iter().flatten() {
        let start = starts[&job.id];
        let eta = etas[&job.id];
        job.tx.send_modify(|snapshot| {
            snapshot.queue_position = Some(start.queue_position);
            snapshot.eta_start_s = start.eta_start_s;
            snapshot.eta = Some(eta);
        });
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
    registry.ensure_classes(&state.config.limits);
    // Phase 0 has no peak-memory prediction wired in yet, and `None` binds
    // to the largest class -- local mode's only one. When the memory model
    // lands this becomes its prediction for `prior` below.
    let class = schedule::bind(None, &registry.classes);
    let idle_slot = registry.idle_slot_for(class);
    // §7.3: the queue bound applies per class, so a backlog of large jobs
    // cannot fill the queue small ones need. With one class this is the
    // single bound it always was.
    if idle_slot.is_none() && registry.queues[class].len() >= state.config.limits.max_queued_jobs {
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
    // Until the worker reports the repository's features, all the ETA
    // model knows is the service's reference mode. Recording it now lets a
    // queued job under `TOLMAP_REFS=scip` be costed with indexing, while one
    // on the hand default keeps the hand prior (`refs` absent, exactly as
    // before #110 P2a). The worker's own `Features` event replaces this
    // row, and `worker_loop` (or a queued cancel) removes it.
    let prior = RepoFeatures {
        refs: (state.config.refs == crate::extract::RefsMode::Scip)
            .then(|| state.config.refs.to_string()),
        ..RepoFeatures::default()
    };
    let initial_eta = registry
        .eta_model
        .predict(&prior, &[false; StageId::ALL.len()], None);
    registry.features.insert(job_id, prior);
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
    if let Some(slot) = idle_slot {
        job.tx
            .send_modify(|snapshot| snapshot.eta_start_s = Some(0.0));
        registry.slots[slot].running = Some((job_id, job.tx.clone()));
        tokio::spawn(worker_loop(state.clone(), slot, job));
    } else {
        let position = registry.queues[class].len() + 1;
        job.tx
            .send_modify(|snapshot| snapshot.queue_position = Some(position));
        registry.queues[class].push_back(job);
    }
    simulate_queue_etas(&registry);
    Ok(job_id)
}

/// Runs jobs on one worker slot until [`schedule::next_for`] finds nothing
/// this slot's class may take, then leaves the slot idle for `enqueue_job`
/// to start again.
async fn worker_loop(state: Arc<AppState>, slot: usize, first: PendingJob) {
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
        registry.cancelled.remove(&id);
        if registry.active.get(&key) == Some(&id) {
            registry.active.remove(&key);
        }
        // The slot is handed straight to the next job under the same lock,
        // as the single FIFO did, so no admission can see it idle in
        // between and start a second job on it.
        let class = registry.slots[slot].class;
        if let Some(next_class) = schedule::next_for(class, &registry.queues) {
            let next = registry.queues[next_class]
                .pop_front()
                .expect("next_for names a non-empty queue");
            registry.slots[slot].running = Some((next.id, next.tx.clone()));
            simulate_queue_etas(&registry);
            job = next;
        } else {
            registry.slots[slot].running = None;
            simulate_queue_etas(&registry);
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
) -> Result<clone::Materialized, ErrorBody> {
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
    Ok(materialized)
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
    let warm_start_candidates = match state.store.warm_start_candidates(&repo_ref.slug) {
        Ok(rows) => rows,
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
    // `checkout` is the service's own record of what it handed the worker:
    // the commit and branch the result is stored under come from here, not
    // from the worker, whose copy of the checkout is its to change.
    let checkout = match materialize_job_repo(
        &state.config.cache_dir,
        &repo_ref,
        &state.config.limits,
        &job_repo_dir,
        &tx,
        started,
    ) {
        Ok(checkout) => checkout,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&job_dir);
            return finish_failed(&tx, error);
        }
    };
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
    // Like the names cache above, and before the same chown: the worker's
    // uid cannot read the map store (see `stage_previous_map`).
    let previous_maps = stage_previous_map(
        &job_dir,
        &warm_start_candidates,
        checkout.branch.as_deref(),
        &|line: String| eprintln!("job {}: {line}", tx.borrow().job_id),
    );
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
    // Issue #110 P1c: a SCIP job may install TypeScript dependencies. The
    // worker cannot start the sandbox (it is unprivileged), so it asks this
    // process, which can (root in the runtime image), over the protocol;
    // see `ServiceInstall`. Anywhere the sandbox cannot start, the answer
    // is a recorded fallback, never a failed job.
    let install = (state.config.refs == crate::extract::RefsMode::Scip
        && state.config.scip_install)
        .then(|| ServiceInstall {
            repo: job_repo_dir.clone(),
            scratch: state
                .config
                .cache_dir
                .join("install")
                .join(tx.borrow().job_id.to_string()),
            settings: crate::indexers::InstallSettings::from_env(
                state.config.worker_uid,
                state.config.worker_gid,
            ),
        });
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
        install: install.as_ref().map(|_| "sandbox".to_owned()),
    };
    let output = process_worker(&state, &tx, spec, started, &job_dir, install.as_ref());
    if let Some(install) = &install {
        let _ = std::fs::remove_dir_all(&install.scratch);
    }
    let output = match output {
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
    let job_id = tx.borrow().job_id;
    let stored = store_worker_result(&state, &repo_ref, job_id, &output_dir, &checkout, &output);
    let _ = std::fs::remove_dir_all(&job_dir);
    if let Err(error) = stored {
        return finish_failed(&tx, error);
    }
    set_commit(&tx, &checkout.commit);
    if let Err(error) = state
        .store
        .prune(&repo_ref.slug, state.config.retain_commits_per_repo)
    {
        eprintln!("prune warning for {}: {error:#}", repo_ref.slug);
    }
    tx.send_modify(|snapshot| snapshot.elapsed_s = started.elapsed().as_secs_f64());
    finish_done(&tx);
}

/// Issue #141: the map store (`cache_dir/maps/<owner>/<repo>`) is the
/// service's own and `0700` (`harden_persistent_dir`), and in the runtime
/// image the worker runs at another uid (`process_worker_exe`), so a store
/// path handed to the worker cannot be read. The worker treated that as "no
/// previous map", and every re-index cold-started: finding 4's warm start,
/// 88% district retention against 46% cold, silently off.
///
/// So the service picks the previous map the worker would have picked --
/// the newest on the checked-out branch, else the newest overall
/// (`warm_start_candidates` is newest first) -- and copies it into
/// `job_dir/previous/<commit>.json` while `job_dir` is still its own, before
/// `harden_job_dir` hands the whole directory to the worker's uid. The store
/// keeps its owner and mode. Only that one map is copied: the worker never
/// uses more than one, and a repository's retained maps can be large.
///
/// A map that cannot be copied is a cold start, as an unreadable one always
/// was, but logged, not silent.
fn stage_previous_map(
    job_dir: &Path,
    candidates: &[MapRow],
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
        .and_then(|()| worker_result::copy_for_worker(&row.map_path, &copy));
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

/// Moves a finished worker's map, symbols and names cache into the store and
/// records the map. Nothing the worker reported is used as a path: the commit
/// must be the service's own checkout commit, and the files are taken from
/// the job's output directory under the names the service expects there,
/// checked only once they are out of the worker's reach -- see
/// `worker_result`. A result that fails a check fails the job with
/// `invalid_worker_result`; the caller removes the job directory either way.
fn store_worker_result(
    state: &AppState,
    repo_ref: &RepoRef,
    job_id: Uuid,
    output_dir: &Path,
    checkout: &clone::Materialized,
    output: &WorkerOutput,
) -> Result<(), ErrorBody> {
    // The commit names the stored file (`<commit>.json`).
    if !worker_result::is_object_id(&output.commit) || output.commit != checkout.commit {
        return Err(invalid_worker_result(
            "the reported commit is not the commit the service checked out",
        ));
    }
    let internal = |error: std::io::Error| ApiError::internal(error.to_string()).body;
    let final_path = state
        .config
        .cache_dir
        .join("maps")
        .join(&repo_ref.owner)
        .join(&repo_ref.repo)
        .join(format!("{}.json", checkout.commit));
    let final_parent = final_path.parent().expect("map parent").to_path_buf();
    std::fs::create_dir_all(&final_parent).map_err(internal)?;
    // Persistent store, not a per-job scratch dir: owned by the
    // service's own uid (root in production), not `tolmap-worker` --
    // even a bug that pointed the worker at this path could not read
    // every stored map for every repo ever indexed. See the PR body's
    // "at minimum" floor.
    harden_persistent_dir(&final_parent).map_err(internal)?;
    // Beside the final location, so every rename below stays on one
    // filesystem, and private to the service from the moment it exists.
    let staging = final_parent.join(format!(".job-{job_id}"));
    worker_result::create_private_dir(&staging).map_err(internal)?;
    let stored = (|| -> Result<(), ErrorBody> {
        let adopted = worker_result::adopt(
            output_dir,
            &staging,
            &repo_ref.repo,
            &worker_result::Reported {
                map_path: &output.map_path,
                symbols_path: &output.symbols_path,
                symbols_dir: &output.symbols_dir,
                names_cache: &output.names_cache,
            },
            Some(worker_uid_in_effect(state.config.worker_uid)),
        )
        .map_err(|refused| match refused {
            worker_result::Refused::Invalid(message) => invalid_worker_result(message),
            worker_result::Refused::Io(error) => internal(error),
        })?;
        let save = (|| -> anyhow::Result<()> {
            std::fs::rename(&adopted.map, &final_path)?;
            std::fs::rename(&adopted.symbols, final_path.with_extension("symbols.json"))?;
            let final_dir = final_path.with_extension("symbols");
            if final_dir.exists() {
                std::fs::remove_dir_all(&final_dir)?;
            }
            std::fs::rename(&adopted.symbols_dir, final_dir)?;
            state.store.save_names(&repo_ref.slug, &adopted.names)?;
            let row = MapRow {
                slug: repo_ref.slug.clone(),
                owner: repo_ref.owner.clone(),
                repo: repo_ref.repo.clone(),
                commit: checkout.commit.clone(),
                branch: checkout.branch.clone(),
                lang: output.lang.clone(),
                files: output.files as i64,
                districts: output.districts as i64,
                modularity: output.modularity,
                map_path: final_path.clone(),
                indexed_at: now_rfc3339(),
            };
            state.store.insert(&row)?;
            Ok(())
        })();
        save.map_err(|error| ApiError::internal(format!("{error:#}")).body)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    stored
}

/// A worker result the service will not store (see `store_worker_result`).
fn invalid_worker_result(message: impl Into<String>) -> ErrorBody {
    ErrorBody {
        error: "invalid_worker_result".to_owned(),
        message: message.into(),
    }
}

/// The uid the worker's files are owned by: the configured worker uid when
/// the service is root and drops to it (`process_worker_exe`), otherwise the
/// service's own, which the worker then runs as.
fn worker_uid_in_effect(configured: u32) -> u32 {
    if is_root() {
        configured
    } else {
        current_uid()
    }
}

/// A TypeScript dependency install the service runs for its worker
/// (issue #110 P1c) when the worker sends `WorkerEvent::InstallRequest`.
/// The service holds this, not the worker: it names the job's checkout and
/// a scratch directory under `cache_dir/install`, which is root's and
/// outside the job directory the worker's uid owns, so nothing the worker
/// (or the repository) can write decides where root creates the jail's
/// home, the proxy socket or the log. `indexers::install` re-reads the
/// install policy from the checkout itself; the request carries nothing.
struct ServiceInstall {
    repo: PathBuf,
    scratch: PathBuf,
    settings: crate::indexers::InstallSettings,
}

impl ServiceInstall {
    fn run(
        &self,
        id: Uuid,
        tick: &dyn Fn(),
        cancelled: &dyn Fn() -> bool,
    ) -> crate::schema::InstallCoverage {
        let log = |line: String| eprintln!("job {id}: {line}");
        // The checkout must still be the plain directory the service made:
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
/// the one the service's own checkout resolved (`store_worker_result`).
struct WorkerOutput {
    map_path: String,
    symbols_path: String,
    symbols_dir: String,
    names_cache: String,
    commit: String,
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

/// Never follows a symlink: the tree holds a checkout of the repository, and
/// a committed symlink names whatever its author chose. `symlink_metadata`
/// decides whether to descend and `lchown` changes the link itself, so the
/// walk stays inside `path`. Children are done before their directory, so
/// every directory whose entries are being walked is still the service's
/// own and nothing else can change them meanwhile.
#[cfg(unix)]
fn chown_recursive(path: &Path, uid: u32, gid: u32) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path)?.file_type().is_dir() {
        for entry in std::fs::read_dir(path)? {
            chown_recursive(&entry?.path(), uid, gid)?;
        }
    }
    std::os::unix::fs::lchown(path, Some(uid), Some(gid))
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
/// production deployment (`TOLMAP_DB_PATH=/data/tolmap.sqlite3`,
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
    install: Option<&ServiceInstall>,
) -> Result<WorkerOutput, ErrorBody> {
    let exe =
        std::env::current_exe().map_err(|error| ApiError::internal(error.to_string()).body)?;
    let hardening = WorkerHardening {
        job_dir: job_dir.to_path_buf(),
        uid: state.config.worker_uid,
        gid: state.config.worker_gid,
        allow_openrouter_key: state.config.namer == crate::naming::NamerKind::Model,
    };
    process_worker_exe(
        tx,
        spec,
        started,
        &exe,
        Some(&state.jobs),
        &hardening,
        install,
    )
}

fn process_worker_exe(
    tx: &watch::Sender<JobSnapshot>,
    spec: WorkerSpec,
    started: Instant,
    exe: &std::path::Path,
    registry: Option<&JobRegistry>,
    hardening: &WorkerHardening,
    install: Option<&ServiceInstall>,
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
                // Also into the service's own log: the snapshot's `stage`
                // is overwritten by the next event, and whether a job
                // warm-started (issue #141) must be answerable afterwards.
                eprintln!("job {id}: {message}");
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
                    lang,
                    files,
                    districts,
                    modularity,
                });
            }
            WorkerEvent::Error { code, message, .. } => {
                error = Some(format!("{code}\n{message}"));
            }
            WorkerEvent::InstallRequest { .. } => {
                // The worker blocks until this answers, so its events wait
                // in the pipe meanwhile; `tick` keeps the job's elapsed
                // time moving instead.
                let coverage = match install {
                    Some(install) if worker_stdin.is_some() => install.run(
                        id,
                        &|| {
                            tx.send_modify(|snapshot| {
                                if !is_terminal(snapshot) {
                                    snapshot.elapsed_s = started.elapsed().as_secs_f64();
                                }
                            })
                        },
                        &|| registry.is_some_and(|registry| registry.is_cancelled(id)),
                    ),
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
        state_with_refs(limits, crate::extract::RefsMode::Hand)
    }

    fn state_with_refs(
        limits: Limits,
        refs: crate::extract::RefsMode,
    ) -> (tempfile::TempDir, Arc<AppState>) {
        let dir = tempfile::tempdir().unwrap();
        let config = ServeConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            db_path: dir.path().join("store.sqlite3"),
            cache_dir: dir.path().join("cache"),
            static_dir: None,
            prune_variant: PruneVariant::NodeRelative,
            namer: crate::naming::NamerKind::Idf,
            namer_model: crate::naming::DEFAULT_MODEL.to_owned(),
            refs,
            scip_install: false,
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

    // #110 P2a: before the worker reports anything about the repository, a
    // job under `TOLMAP_REFS=scip` is quoted a range that covers indexing
    // (finding 44's n8n build, 371.4 s), and one on the hand default is not.
    #[tokio::test]
    async fn unstarted_scip_job_eta_covers_indexing() {
        let mut high = Vec::new();
        // The default first: it must be costed as the hand path it is.
        for refs in [
            crate::extract::RefsMode::default(),
            crate::extract::RefsMode::Scip,
        ] {
            let (_dir, state) = state_with_refs(Limits::default(), refs);
            let (release_tx, release_rx) = mpsc::channel::<()>();
            let release_rx = Arc::new(Mutex::new(release_rx));
            let runner: JobRunner = Arc::new(move |_, _, tx| {
                release_rx.lock().unwrap().recv().unwrap();
                finish_done(&tx);
            });
            let id = enqueue_job(state.clone(), repo("one"), "a".to_owned(), runner).unwrap();
            high.push(snapshot(&state, id).eta.unwrap().high_s);
            release_tx.send(()).unwrap();
            until(|| snapshot(&state, id).status == JobStatus::Done).await;
        }
        assert!(high[0] < 371.4, "hand: {}", high[0]);
        assert!(high[1] >= 371.4, "scip: {}", high[1]);
    }

    // #97 phase 0: with two slots a queued job starts when the first slot
    // frees, not after both. The running sum this replaced quoted the two
    // queued jobs below 2r and 2r + m.
    #[tokio::test]
    async fn two_slots_quote_the_first_free_slot_not_the_sum_of_both() {
        let limits = Limits {
            max_concurrent_jobs: 2,
            max_queued_jobs: 4,
            ..Limits::default()
        };
        let (_dir, state) = state(limits);
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let started = Arc::new(Mutex::new(Vec::<String>::new()));
        let runner: JobRunner = Arc::new({
            let started = started.clone();
            move |_, repo, tx| {
                started.lock().unwrap().push(repo.slug);
                release_rx.lock().unwrap().recv().unwrap();
                finish_done(&tx);
            }
        });
        let first =
            enqueue_job(state.clone(), repo("one"), "a".to_owned(), runner.clone()).unwrap();
        let second =
            enqueue_job(state.clone(), repo("two"), "b".to_owned(), runner.clone()).unwrap();
        until(|| started.lock().unwrap().len() == 2).await;
        let third =
            enqueue_job(state.clone(), repo("three"), "c".to_owned(), runner.clone()).unwrap();
        let fourth = enqueue_job(state.clone(), repo("four"), "d".to_owned(), runner).unwrap();

        // Neither running job has reported anything, so both slots free at
        // the same remaining midpoint r, the one each was admitted with.
        let r = snapshot(&state, first).eta.unwrap().midpoint();
        assert_eq!(snapshot(&state, second).eta.unwrap().midpoint(), r);
        // third: slot 0 at r (tie with slot 1, lower id wins);
        // fourth: slot 1 at r.
        assert_eq!(snapshot(&state, third).queue_position, Some(1));
        assert_eq!(snapshot(&state, third).eta_start_s, Some(r));
        assert_eq!(snapshot(&state, fourth).queue_position, Some(2));
        assert_eq!(snapshot(&state, fourth).eta_start_s, Some(r));

        // One slot frees and takes the head of the queue.
        release_tx.send(()).unwrap();
        until(|| started.lock().unwrap().len() == 3).await;
        assert_eq!(snapshot(&state, third).queue_position, None);
        assert_eq!(snapshot(&state, fourth).queue_position, Some(1));
        for _ in 0..3 {
            release_tx.send(()).unwrap();
        }
        for id in [first, second, third, fourth] {
            until(|| snapshot(&state, id).status == JobStatus::Done).await;
        }
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
                    install: None,
                };
                let error = process_worker_exe(
                    &tx,
                    spec,
                    Instant::now(),
                    &fake_worker,
                    None,
                    &WorkerHardening::for_test(dir.path()),
                    None,
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
                    install: None,
                };
                let error = process_worker_exe(
                    &tx,
                    spec,
                    Instant::now(),
                    &fake_worker,
                    Some(&state.jobs),
                    &WorkerHardening::for_test(dir.path()),
                    None,
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
                refs: None,
                install: None,
            };
            let error = process_worker_exe(
                &tx,
                spec,
                Instant::now(),
                &fake_worker,
                Some(&state.jobs),
                &WorkerHardening::for_test(dir.path()),
                None,
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
                install: None,
            };
            let error = process_worker_exe(
                &tx,
                spec,
                Instant::now(),
                &fake_worker,
                None,
                &WorkerHardening::for_test(dir.path()),
                None,
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
                refs: None,
                install: None,
            };
            let error = process_worker_exe(
                &tx,
                spec,
                Instant::now(),
                &fake_worker,
                None,
                &WorkerHardening::for_test(dir.path()),
                None,
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
                refs: None,
                install: None,
            };
            let hardening = WorkerHardening {
                job_dir: job_dir.clone(),
                uid,
                gid,
                allow_openrouter_key: false,
            };
            let error = process_worker_exe(
                &tx,
                spec,
                Instant::now(),
                &fake_worker,
                None,
                &hardening,
                None,
            )
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

    /// Issue #110 P1c: a worker that asks for an install gets exactly one
    /// answer line on its stdin. Where the sandbox cannot start (this test
    /// process is not root, and nsjail is pointed at nothing in case it is)
    /// that answer is a recorded fallback: never a hang, never a failed job,
    /// and nothing runs in the checkout.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_install_request_is_answered_with_a_fallback_where_the_sandbox_cannot_start() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, state) = state(Limits::default());
        let checkout = dir.path().join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::write(
            checkout.join("package.json"),
            r#"{"name":"root","private":true}"#,
        )
        .unwrap();
        std::fs::write(checkout.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
        std::fs::write(
            checkout.join("pnpm-workspace.yaml"),
            "packages:\n  - packages/*\n",
        )
        .unwrap();
        let fake_worker = dir.path().join("installing-worker");
        let script = concat!(
            "#!/bin/sh\n",
            "read -r spec\n",
            "printf '%s\\n' '{\"type\":\"install_request\",\"v\":1}'\n",
            "read -r reply\n",
            "printf '{\"type\":\"error\",\"v\":1,\"code\":\"test\",\"message\":\"%s\"}\\n' ",
            "\"$(printf '%s' \"$reply\" | tr -d '\"{}')\"\n",
        );
        std::fs::write(&fake_worker, script).unwrap();
        std::fs::set_permissions(&fake_worker, std::fs::Permissions::from_mode(0o755)).unwrap();
        let scratch = dir.path().join("cache/install/job");
        let job_dir = dir.path().to_path_buf();
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
                refs: Some("scip".to_owned()),
                install: Some("sandbox".to_owned()),
            };
            let install = ServiceInstall {
                repo: checkout.clone(),
                scratch: scratch.clone(),
                settings: crate::indexers::InstallSettings {
                    nsjail: "/nonexistent/nsjail".to_owned(),
                    node_prefix: None,
                    uid: 10001,
                    gid: 10001,
                    time_limit: crate::indexers::INSTALL_TIME_LIMIT,
                    disk_budget: crate::indexers::INSTALL_DISK_BUDGET,
                    memory_max: None,
                },
            };
            let error = process_worker_exe(
                &tx,
                spec,
                Instant::now(),
                &fake_worker,
                None,
                &WorkerHardening::for_test(&job_dir),
                Some(&install),
            )
            .err()
            .expect("the fake worker reports through an error event");
            finish_failed(&tx, error);
        });
        let id = enqueue_job(state.clone(), repo("install"), "a".to_owned(), runner).unwrap();
        until(|| snapshot(&state, id).status == JobStatus::Failed).await;
        assert_eq!(
            snapshot(&state, id).error.as_deref(),
            Some("status:fell_back,reason:sandbox_unavailable,manager:pnpm")
        );
        assert!(!dir.path().join("checkout/node_modules").exists());
        assert!(!dir.path().join("cache/install/job").exists());
    }

    /// The commit the service's own checkout resolved in the tests below.
    const RESULT_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    /// A job directory as a worker leaves it for the repository `test/demo`,
    /// and a file outside it the worker must never be able to reach.
    #[cfg(unix)]
    struct ResultFixture {
        _dir: tempfile::TempDir,
        state: Arc<AppState>,
        repo_ref: RepoRef,
        job_dir: PathBuf,
        output_dir: PathBuf,
        outside: PathBuf,
    }

    #[cfg(unix)]
    impl ResultFixture {
        fn new() -> Self {
            let (dir, state) = state(Limits::default());
            let repo_ref = repo("demo");
            let job_dir = state.config.cache_dir.join("work/test/demo/job");
            let output_dir = job_dir.join("output");
            std::fs::create_dir_all(output_dir.join("demo.symbols")).unwrap();
            std::fs::write(output_dir.join("demo.json"), b"{}").unwrap();
            std::fs::write(output_dir.join("demo.symbols.json"), b"{}").unwrap();
            std::fs::write(output_dir.join("demo.symbols/0.json"), b"{}").unwrap();
            std::fs::write(
                output_dir.join("demo.names.json"),
                br#"{"0123456789ab": {"name": "core", "district": 0, "size": 1}}"#,
            )
            .unwrap();
            let outside = dir.path().join("outside.json");
            std::fs::write(&outside, b"keep me").unwrap();
            ResultFixture {
                _dir: dir,
                state,
                repo_ref,
                job_dir,
                output_dir,
                outside,
            }
        }

        /// The `result` event an honest worker sends for this job.
        fn result(&self) -> serde_json::Value {
            let path = |name: &str| self.output_dir.join(name).to_string_lossy().into_owned();
            serde_json::json!({
                "type": "result",
                "v": 1,
                "map_path": path("demo.json"),
                "symbols_path": path("demo.symbols.json"),
                "symbols_dir": path("demo.symbols"),
                "names_cache": path("demo.names.json"),
                "commit": RESULT_COMMIT,
                "branch": "reported-by-the-worker",
                "lang": "py",
                "files": 1,
                "districts": 1,
                "modularity": 0.5
            })
        }

        /// Runs a fake worker that sends `result` through the real protocol
        /// reader, then stores what it reported the way `run_blocking` does.
        fn store(&self, result: serde_json::Value) -> Result<(), ErrorBody> {
            use std::os::unix::fs::PermissionsExt;
            let worker = self.job_dir.parent().unwrap().join("result-worker");
            let line = serde_json::to_string(&result).unwrap();
            assert!(!line.contains('\''), "the script quotes the line in ''");
            std::fs::write(
                &worker,
                format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{line}'\n"),
            )
            .unwrap();
            std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755)).unwrap();
            let (tx, _rx) = watch::channel(blank_snapshot(&self.repo_ref.slug));
            let spec = WorkerSpec {
                v: 1,
                slug: self.repo_ref.slug.clone(),
                owner: self.repo_ref.owner.clone(),
                repo: self.repo_ref.repo.clone(),
                source: "unused".to_owned(),
                local: true,
                all_sources: false,
                cache_dir: String::new(),
                output_dir: self.output_dir.to_string_lossy().into_owned(),
                clone_cache_bytes: 1,
                prune_variant: "node-relative".to_owned(),
                namer: "idf".to_owned(),
                namer_model: String::new(),
                previous_maps: vec![],
                names_cache: None,
                refs: None,
                install: None,
            };
            let output = process_worker_exe(
                &tx,
                spec,
                Instant::now(),
                &worker,
                None,
                &WorkerHardening::for_test(&self.job_dir),
                None,
            )?;
            let checkout = clone::Materialized {
                path: self.job_dir.join("repo"),
                commit: RESULT_COMMIT.to_owned(),
                branch: Some("main".to_owned()),
            };
            let job_id = tx.borrow().job_id;
            store_worker_result(
                &self.state,
                &self.repo_ref,
                job_id,
                &self.output_dir,
                &checkout,
                &output,
            )
        }

        fn maps_dir(&self) -> PathBuf {
            self.state.config.cache_dir.join("maps/test/demo")
        }

        /// The job failed as an invalid result, and left nothing behind: no
        /// row, no stored file, no staging directory, the outside file as it
        /// was.
        fn assert_refused(&self, stored: Result<(), ErrorBody>) {
            let error = stored.expect_err("the result must be refused");
            assert_eq!(error.error, "invalid_worker_result", "{}", error.message);
            assert!(self
                .state
                .store
                .get(&self.repo_ref.slug, RESULT_COMMIT)
                .unwrap()
                .is_none());
            if let Ok(entries) = std::fs::read_dir(self.maps_dir()) {
                let left: Vec<_> = entries.map(|entry| entry.unwrap().file_name()).collect();
                assert!(left.is_empty(), "left in the store: {left:?}");
            }
            assert_eq!(std::fs::read(&self.outside).unwrap(), b"keep me");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_well_formed_worker_result_is_stored_under_the_service_commit() {
        let fixture = ResultFixture::new();
        fixture.store(fixture.result()).unwrap();
        let row = fixture
            .state
            .store
            .get(&fixture.repo_ref.slug, RESULT_COMMIT)
            .unwrap()
            .expect("stored row");
        let map = fixture.maps_dir().join(format!("{RESULT_COMMIT}.json"));
        assert_eq!(row.map_path, map);
        // The service's checkout names the branch, not the worker.
        assert_eq!(row.branch.as_deref(), Some("main"));
        assert!(map.is_file());
        assert!(map.with_extension("symbols.json").is_file());
        assert!(map.with_extension("symbols").join("0.json").is_file());
        let names = fixture
            .state
            .store
            .load_names(&fixture.repo_ref.slug)
            .unwrap();
        assert_eq!(names["0123456789ab"].name, "core");
        let left: Vec<_> = std::fs::read_dir(fixture.maps_dir())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".job-"))
            .collect();
        assert!(left.is_empty(), "staging left behind: {left:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_commit_with_a_path_in_it_is_refused() {
        let fixture = ResultFixture::new();
        let mut result = fixture.result();
        result["commit"] = serde_json::json!("../../../outside");
        fixture.assert_refused(fixture.store(result));
        assert!(!fixture.state.config.cache_dir.join("maps").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_commit_that_is_not_hex_is_refused() {
        let fixture = ResultFixture::new();
        let mut result = fixture.result();
        result["commit"] = serde_json::json!("g".repeat(40));
        fixture.assert_refused(fixture.store(result));
    }

    #[cfg(unix)]
    #[test]
    fn a_commit_other_than_the_checked_out_one_is_refused() {
        let fixture = ResultFixture::new();
        let mut result = fixture.result();
        result["commit"] = serde_json::json!("f".repeat(40));
        fixture.assert_refused(fixture.store(result));
    }

    #[cfg(unix)]
    #[test]
    fn a_map_path_outside_the_output_directory_is_refused() {
        let fixture = ResultFixture::new();
        let mut result = fixture.result();
        result["map_path"] = serde_json::json!(fixture.outside.to_string_lossy());
        fixture.assert_refused(fixture.store(result));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_map_file_is_refused() {
        let fixture = ResultFixture::new();
        let map = fixture.output_dir.join("demo.json");
        std::fs::remove_file(&map).unwrap();
        std::os::unix::fs::symlink(&fixture.outside, &map).unwrap();
        fixture.assert_refused(fixture.store(fixture.result()));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_the_symbols_directory_is_refused() {
        let fixture = ResultFixture::new();
        std::os::unix::fs::symlink(
            &fixture.outside,
            fixture.output_dir.join("demo.symbols/1.json"),
        )
        .unwrap();
        fixture.assert_refused(fixture.store(fixture.result()));
    }

    /// The job directory holds a checkout of the repository, so any symlink
    /// in it was chosen by the repository's author. A dangling one fails a
    /// walk that follows links; a link to a directory holding one it cannot
    /// read fails it too (except as root, which reads it anyway).
    #[cfg(unix)]
    #[test]
    fn chown_recursive_changes_symlinks_themselves_and_never_walks_through_them() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let job = dir.path().join("job");
        std::fs::create_dir_all(job.join("repo")).unwrap();
        symlink(dir.path().join("missing"), job.join("repo/dangling")).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(outside.join("locked")).unwrap();
        std::fs::set_permissions(
            outside.join("locked"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        symlink(&outside, job.join("repo/escape")).unwrap();
        let result = chown_recursive(&job, current_uid(), current_gid());
        std::fs::set_permissions(
            outside.join("locked"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        result.unwrap();
    }

    fn stored_row(dir: &Path, commit: char, branch: Option<&str>) -> MapRow {
        let commit = commit.to_string().repeat(40);
        let map_path = dir.join(format!("{commit}.json"));
        std::fs::write(&map_path, format!("{{\"commit\":\"{commit}\"}}")).unwrap();
        MapRow {
            slug: "test/demo".to_owned(),
            owner: "test".to_owned(),
            repo: "demo".to_owned(),
            commit,
            branch: branch.map(str::to_owned),
            lang: "py".to_owned(),
            files: 1,
            districts: 1,
            modularity: 0.0,
            map_path,
            indexed_at: String::new(),
        }
    }

    /// Issue #141: the worker gets a copy of the one previous map it would
    /// have chosen, inside its own job directory, never a store path.
    #[test]
    fn the_previous_map_is_copied_into_the_job_directory() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("maps");
        std::fs::create_dir_all(&store).unwrap();
        // Newest first, as `warm_start_candidates` returns them.
        let rows = vec![
            stored_row(&store, 'a', Some("dev")),
            stored_row(&store, 'b', Some("main")),
        ];
        let no_log = |line: String| panic!("unexpected log line: {line}");

        let job = dir.path().join("job-main");
        std::fs::create_dir_all(&job).unwrap();
        let staged = stage_previous_map(&job, &rows, Some("main"), &no_log);
        assert_eq!(staged.len(), 1);
        let copy = job
            .join("previous")
            .join(format!("{}.json", "b".repeat(40)));
        assert_eq!(Path::new(&staged[0].path), copy.as_path());
        assert_eq!(staged[0].branch.as_deref(), Some("main"));
        assert_eq!(
            std::fs::read(&copy).unwrap(),
            std::fs::read(&rows[1].map_path).unwrap()
        );

        // No map on this branch yet: the newest overall.
        let job = dir.path().join("job-other");
        std::fs::create_dir_all(&job).unwrap();
        let staged = stage_previous_map(&job, &rows, Some("feature"), &no_log);
        assert_eq!(
            Path::new(&staged[0].path),
            job.join("previous")
                .join(format!("{}.json", "a".repeat(40)))
                .as_path()
        );

        // A first index has nothing to copy.
        assert!(stage_previous_map(&job, &[], Some("main"), &no_log).is_empty());
    }

    #[test]
    fn a_previous_map_that_cannot_be_copied_is_a_logged_cold_start() {
        let dir = tempfile::tempdir().unwrap();
        let rows = vec![stored_row(dir.path(), 'c', Some("main"))];
        std::fs::remove_file(&rows[0].map_path).unwrap();
        let job = dir.path().join("job");
        std::fs::create_dir_all(&job).unwrap();
        let lines = std::cell::RefCell::new(Vec::new());
        let staged = stage_previous_map(&job, &rows, Some("main"), &|line: String| {
            lines.borrow_mut().push(line)
        });
        assert!(staged.is_empty());
        let lines = lines.into_inner();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("cold start: previous map"),
            "{lines:?}"
        );
    }
}
