//! FIFO admission and snapshots live in the service. Each blocking build
//! runs in a child process; the child never opens the service database.
//!
//! A job runs in three parts (docs/WORKER_TIER.md §2): `prepare` reads the
//! store, `service::executor::execute` runs the job child, and `register`
//! writes the result back. Only `prepare` and `register` touch the store.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use ts_rs::TS;
use uuid::Uuid;

use crate::progress::{ProgressValue, StageId};
use crate::service::clone::{self, RepoRef, RepoSource};
use crate::service::config::ServeConfig;
use crate::service::error::{ApiError, ErrorBody};
use crate::service::eta::{expected_passes, progress_total, Eta, EtaModel, MemoryModel, TimingRow};
use crate::service::executor::{
    self, cancelled_error, current_uid, harden_persistent_dir, is_root, kill_worker_group,
    CancelProbe, EventSink, JobInputs, PreviousMapInput, WorkerOutput,
};
use crate::service::schedule::{self, Class};
use crate::service::store::MapRow;
use crate::service::time::now_rfc3339;
use crate::service::worker_result;
use crate::service::AppState;
use crate::worker::{JobSpec, RepoFeatures, WorkerEvent};

// The tests below were written against these names while they lived in
// this module, before the executor moved out (#97 phase 1). They reach them
// through `super::*` and run unmodified, so they are imported here for the
// tests only.
#[cfg(test)]
use crate::service::executor::{
    chown_recursive, current_gid, harden_job_dir, ServiceInstall, WorkerHardening,
};
#[cfg(test)]
use crate::worker::WorkerSpec;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::process::Command;

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
    /// Admission sequence (#97 phase 2). Every queue is kept in this
    /// order, so a re-queued job, which keeps its number, goes back to the
    /// head: every job still queued was admitted after it started.
    order: i64,
    /// The class queue it waits in.
    class: usize,
    /// Worker modes only: false until the job's row is in the store. A
    /// slot never starts a job that is not, so no runner reads or writes a
    /// row that does not exist yet (`spawn_job`, `JobRegistry::admitted`).
    /// Always true in local mode.
    persisted: bool,
}

/// Inserts `job` into `queue` by admission order (see `PendingJob::order`).
fn insert_by_order(queue: &mut VecDeque<PendingJob>, job: PendingJob) {
    let position = queue
        .iter()
        .position(|queued| queued.order > job.order)
        .unwrap_or(queue.len());
    queue.insert(position, job);
}

/// One worker slot: a `worker_loop` task while it holds a job, idle
/// otherwise. Its index in `RegistryInner::slots` is its stable worker id.
struct Slot {
    class: usize,
    running: Option<(Uuid, watch::Sender<JobSnapshot>)>,
}

/// One class's demand, from `JobRegistry::class_load` (§7.4). Connected
/// agents are the worker hub's to report, not the registry's; the route
/// handler (`service::workers::get_capacity`) combines the two.
pub(crate) struct ClassLoad {
    pub(crate) running: usize,
    pub(crate) queued: usize,
    /// `None` while busy (`running + queued > 0`) or never busy; `Some`
    /// for how long a class that has been busy at least once has now sat
    /// idle.
    pub(crate) idle_for: Option<Duration>,
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
    /// Set once, from the executor's `wait_with_peak` (through
    /// `SnapshotSink::peak_rss`), right after
    /// the job child is reaped -- including a failed or cancelled job,
    /// whose peak is the out-of-memory evidence (#97 phase 0). Consumed and
    /// removed exactly once, in `worker_loop`'s `TimingRow` construction,
    /// same lifecycle as `features` above. Unlike `features`, nothing ever
    /// pre-inserts an entry here (a queued job that never spawns a child has
    /// none to remove), so there is no analogous queued-cancel/shutdown
    /// cleanup to do.
    peak_rss: HashMap<Uuid, u64>,
    eta_model: EtaModel,
    memory_model: MemoryModel,
    /// Set once by [`JobRegistry::shutdown`] and never cleared -- the
    /// process is exiting, not pausing. Checked by `enqueue_job` so no job
    /// is admitted after a shutdown signal starts draining the registry.
    stopping: bool,
    /// `TOLMAP_WORKERS=loopback:N` (#97 phase 1): the worker hub every job
    /// runs through instead of a local child, and the worker classes, one
    /// slot per agent (#97 phase 2, step 4: `TOLMAP_LOOPBACK_CLASSES`, or
    /// one class of N slots), rather than `TOLMAP_MAX_CONCURRENT_JOBS`.
    /// Both `None` in local mode, which is every registry `new_registry`
    /// builds until `set_remote` runs, so local mode and every existing test
    /// see exactly the registry they saw before.
    remote: Option<Arc<crate::service::workers::WorkerHub>>,
    remote_classes: Option<Vec<Class>>,
    /// The last admission number given out (`PendingJob::order`).
    next_order: i64,
    /// Worker modes (#97 phase 2): jobs whose runner put them back in the
    /// queue (a lost worker, a released job). `worker_loop` re-inserts
    /// them when the runner returns instead of ending them.
    requeue: HashSet<Uuid>,
    /// Worker modes (#97 phase 2, step 4): of those, the ones going back to
    /// another class's queue -- a reroute, an out-of-memory escalation, a
    /// lost worker that died of memory -- by that class. The store has the
    /// new class already (`Store::rebind_job`).
    rebind: HashMap<Uuid, usize>,
    /// Worker modes: jobs a restarted master found leased or running, by
    /// the epoch of that lease. Their runner adopts the lease instead of
    /// asking for a new one, and it expires as any lease does (§6 "master
    /// restarts mid-job").
    orphans: HashMap<Uuid, u64>,
    /// §7.4, `GET /workers/capacity`: the last time each class had a
    /// running or queued job, by class index -- possibly longer than
    /// `classes` (`class_load` grows it to the hub's own class count,
    /// which the registry may not have built yet, see `class_load`'s doc
    /// comment). `None` is "never busy", read the same as "idle past the
    /// window": nothing has ever asked for a worker of that class.
    class_last_busy: Vec<Option<Instant>>,
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
        // Loopback mode: each agent is one slot of its class (one class of
        // unknown size unless `TOLMAP_LOOPBACK_CLASSES` says otherwise), and
        // `TOLMAP_MAX_CONCURRENT_JOBS` does not apply (docs/API.md).
        let mut classes = self.remote_classes.clone().unwrap_or_else(|| {
            vec![Class {
                usable_memory: None,
                slots: limits.max_concurrent_jobs.max(1),
            }]
        });
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

    /// Worker modes: the agents connected now in each class
    /// (`WorkerHub::live_by_class`). `None` in local mode, where a slot is
    /// a thread of this process and always there.
    fn live_by_class(&self) -> Option<Vec<usize>> {
        self.remote.as_ref().map(|hub| hub.live_by_class())
    }

    /// Whether `class` has a connected agent that no busy slot of the class
    /// accounts for (#97 phase 2, step 4). Always in local mode (`live` is
    /// `None`), where a slot is a thread of this process and always there.
    fn has_spare_agent(&self, class: usize, live: Option<&[usize]>) -> bool {
        live.is_none_or(|live| {
            let busy = self
                .slots
                .iter()
                .filter(|slot| slot.class == class && slot.running.is_some())
                .count();
            busy < live.get(class).copied().unwrap_or(0)
        })
    }

    /// The queue idle `slot` takes its next job from (§7.1,
    /// `schedule::next_for`), if its head may start: persisted, in worker
    /// modes, and, when the slot would spill down to a smaller class, only
    /// while the slot's own class has a connected agent to run it on.
    ///
    /// A slot of the job's own class takes it whatever its agents are
    /// doing: only that class's agents (and larger ones, which spill only
    /// once their own queue is empty) can run the job anyway, so it waits
    /// for them in the slot as it would in the queue, and a class whose
    /// agent is restarting keeps its jobs and its ETAs as they were. A
    /// spill is different: a large slot with no large agent behind it must
    /// not take a small job the small agent could run. That is what keeps
    /// an idle slot with no agent from ever starting anything, and
    /// `simulate_queue_etas` leaves the same slots out.
    fn next_queue_for(&self, slot: usize, live: Option<&[usize]>) -> Option<usize> {
        let class = self.slots[slot].class;
        let next = schedule::next_for(class, &self.queues)?;
        let head_ready = self.queues[next].front().is_some_and(|job| job.persisted);
        (head_ready && (next == class || self.has_spare_agent(class, live))).then_some(next)
    }

    /// The idle slot a job bound to `class` may start on at once
    /// (`idle_slot_for`), counting a larger class's slot only while that
    /// class has a spare connected agent (see `next_queue_for`). The same
    /// slot as `idle_slot_for` in local mode.
    fn startable_slot_for(&self, class: usize, live: Option<&[usize]>) -> Option<usize> {
        (0..self.slots.len()).find(|&slot| {
            let slot_class = self.slots[slot].class;
            self.slots[slot].running.is_none()
                && slot_class >= class
                && (slot_class == class || self.has_spare_agent(slot_class, live))
        })
    }
}

pub struct JobRegistry(Mutex<RegistryInner>);

impl JobRegistry {
    pub fn load_timings(&self, store: &crate::service::store::Store) -> anyhow::Result<()> {
        let rows = store.recent_timings()?;
        let mut registry = self.0.lock().expect("job registry mutex poisoned");
        registry.memory_model = MemoryModel::from_rows(rows.clone());
        registry.eta_model = EtaModel::from_rows(rows);
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

    /// Loopback mode (#97 phase 1): every job from now on runs through
    /// `hub`'s agents, one worker slot per agent in `classes` (ordered by
    /// `schedule::order_classes`, the order `hub` numbers its classes in).
    /// Called once by `service::serve` before the first admission;
    /// `ensure_classes` builds the slots on that first admission.
    pub fn set_remote(&self, hub: Arc<crate::service::workers::WorkerHub>, classes: Vec<Class>) {
        let mut registry = self.0.lock().expect("job registry mutex poisoned");
        registry.remote = Some(hub);
        registry.remote_classes = Some(classes);
    }

    /// The class of the slot running `id`: the class of the agent it runs
    /// on, which is larger than the job's own class when the slot spilled
    /// down to it (§7.1). A runner asks for an agent of this class.
    pub(crate) fn slot_class(&self, id: Uuid) -> Option<usize> {
        let registry = self.0.lock().expect("job registry mutex poisoned");
        registry
            .slots
            .iter()
            .find(|slot| {
                slot.running
                    .as_ref()
                    .is_some_and(|(running, _)| *running == id)
            })
            .map(|slot| slot.class)
    }

    /// A class's usable memory; `None` for one of unknown size.
    pub(crate) fn class_memory(&self, class: usize) -> Option<u64> {
        self.0
            .lock()
            .expect("job registry mutex poisoned")
            .classes
            .get(class)
            .and_then(|class| class.usable_memory)
    }

    /// The next larger class than `class`, if there is one.
    pub(crate) fn next_class(&self, class: usize) -> Option<usize> {
        let registry = self.0.lock().expect("job registry mutex poisoned");
        (class + 1 < registry.classes.len()).then_some(class + 1)
    }

    /// One class's running and queued job counts, plus how long it has sat
    /// idle, for `GET /workers/capacity` (§7.4, `service::workers::
    /// get_capacity`). `count` is the *hub's* number of configured classes,
    /// not `registry.classes.len()`: the registry only builds its classes
    /// on the first admission (`ensure_classes`), but the capacity route
    /// has to answer for every configured class from the moment the worker
    /// listener opens, before any job has ever been queued. A class past
    /// what the registry has built yet is reported `running: 0, queued: 0`
    /// with no idle history (`idle_for: None`, read as "never busy" by
    /// `service::workers::desired_capacity`) -- exactly correct, since
    /// nothing can be running or queued in a class nothing has admitted a
    /// job to.
    pub(crate) fn class_load(&self, count: usize) -> Vec<ClassLoad> {
        let mut registry = self.0.lock().expect("job registry mutex poisoned");
        if registry.class_last_busy.len() < count {
            registry.class_last_busy.resize(count, None);
        }
        let now = Instant::now();
        (0..count)
            .map(|class| {
                let running = registry
                    .slots
                    .iter()
                    .filter(|slot| slot.class == class && slot.running.is_some())
                    .count();
                let queued = registry.queues.get(class).map_or(0, |queue| queue.len());
                let idle_for = if running + queued > 0 {
                    registry.class_last_busy[class] = Some(now);
                    None
                } else {
                    registry.class_last_busy[class].map(|last| now.saturating_duration_since(last))
                };
                ClassLoad {
                    running,
                    queued,
                    idle_for,
                }
            })
            .collect()
    }

    /// §2.1 step 3 (#97 phase 2, step 4): once a running job has reported
    /// the files it holds, whether its predicted peak exceeds the class it
    /// runs on, `running_class`, and a larger class exists: the class to
    /// move it to -- the smallest that holds the prediction, the largest if
    /// none does (`schedule::bind`) -- with the prediction. Never a class
    /// at or below `running_class`: a job is never rerouted down, and since
    /// a rerouted job is bound to the class it moved to and runs on that
    /// class or a larger one, a second reroute can only go further up, so
    /// no job is ever rerouted twice to the same class. `None` before the
    /// job has reported any files (the prediction is then the one it was
    /// bound with) and on a class of unknown size.
    pub(crate) fn reroute_target(&self, id: Uuid, running_class: usize) -> Option<(usize, u64)> {
        let registry = self.0.lock().expect("job registry mutex poisoned");
        let features = registry.features.get(&id)?;
        if features
            .languages
            .values()
            .all(|language| language.files == 0)
        {
            return None;
        }
        let slug = registry.jobs.get(&id)?.borrow().slug.clone();
        let usable = registry.classes.get(running_class)?.usable_memory?;
        let peak = registry
            .memory_model
            .predict_for(Some(slug.as_str()), features);
        if peak <= usable {
            return None;
        }
        let target = schedule::bind(Some(peak), &registry.classes);
        (target > running_class).then_some((target, peak))
    }

    pub(crate) fn remote(&self) -> Option<Arc<crate::service::workers::WorkerHub>> {
        self.0
            .lock()
            .expect("job registry mutex poisoned")
            .remote
            .clone()
    }

    /// Whether [`JobRegistry::shutdown`] has run: a remote job cancelled by
    /// it tells its agent `server_stopping` rather than `cancelled`.
    pub(crate) fn is_stopping(&self) -> bool {
        self.0.lock().expect("job registry mutex poisoned").stopping
    }

    pub(crate) fn is_cancelled(&self, id: Uuid) -> bool {
        self.0
            .lock()
            .expect("job registry mutex poisoned")
            .cancelled
            .contains(&id)
    }

    /// Worker modes (#97 phase 2): jobs are durable, persisted in the
    /// store's `jobs` table, and this registry is a cache of it. False in
    /// local mode, which keeps its in-memory registry exactly as before.
    pub(crate) fn is_durable(&self) -> bool {
        self.0
            .lock()
            .expect("job registry mutex poisoned")
            .remote
            .is_some()
    }

    /// The runner put `id` back in a queue -- its own class's, or `class`
    /// when it moved -- and `worker_loop` re-inserts it there when the
    /// runner returns.
    pub(crate) fn mark_requeue(&self, id: Uuid, class: Option<usize>) {
        let mut registry = self.0.lock().expect("job registry mutex poisoned");
        registry.requeue.insert(id);
        if let Some(class) = class {
            registry.rebind.insert(id, class);
        }
    }

    /// The epoch of the lease a restarted master found `id` holding, once.
    pub(crate) fn take_orphan(&self, id: Uuid) -> Option<u64> {
        self.0
            .lock()
            .expect("job registry mutex poisoned")
            .orphans
            .remove(&id)
    }

    /// Worker modes: `id`'s row is in the store now, so a slot may start
    /// it. Starts every idle slot that has a job it may take.
    fn admitted(&self, state: &Arc<AppState>, id: Uuid) {
        let mut registry = self.0.lock().expect("job registry mutex poisoned");
        for job in registry.queues.iter_mut().flatten() {
            if job.id == id {
                job.persisted = true;
            }
        }
        dispatch_idle(state, &mut registry);
    }

    /// Worker modes: the admission row could not be written, so the job is
    /// withdrawn -- a job that exists only in memory would not survive the
    /// restart it was promised to survive.
    fn abandon(&self, id: Uuid, error: ErrorBody) {
        let mut registry = self.0.lock().expect("job registry mutex poisoned");
        for queue in &mut registry.queues {
            queue.retain(|job| job.id != id);
        }
        registry.active.retain(|_, active| *active != id);
        registry.features.remove(&id);
        if let Some(tx) = registry.jobs.remove(&id) {
            finish_failed(&tx, error);
        }
        simulate_queue_etas(&registry);
    }

    /// See `peak_rss`'s doc comment on `RegistryInner`. Called from
    /// `SnapshotSink::peak_rss`, right after the executor's `wait_with_peak`
    /// reaps the child,
    /// on every path that reaches a reap -- success, failure and cancel
    /// alike.
    pub(crate) fn set_peak_rss(&self, id: Uuid, bytes: u64) {
        self.0
            .lock()
            .expect("job registry mutex poisoned")
            .peak_rss
            .insert(id, bytes);
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
        // Worker modes (#97 phase 2, docs/WORKER_TIER.md §6 "master
        // graceful stop"): jobs are durable, so a stop fails none of them.
        // Admission stops (the flag above); queued jobs stay queued in the
        // table; running ones are released by the agents on `shutdown now`
        // (`workers::Loopback::shutdown`) and their runners put them back
        // to `queued` without counting an attempt. A restarted master runs
        // them all. Local mode continues below, unchanged.
        if registry.remote.is_some() {
            return;
        }
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
    // Worker modes (#97 phase 2, step 4): only slots with an agent behind
    // them. A busy slot always counts -- its job holds it, and it frees when
    // that job ends -- and an idle one only while its class has a connected
    // agent the busy ones do not account for (`has_spare_agent`). A queued
    // job sees an idle slot only when that slot may not take it
    // (`next_queue_for`: no agent to spill with), so leaving those out is
    // what dispatch does too: a queued job no counted slot can take gets no
    // start at all (`schedule::simulate` returns `None`), never a start of
    // 0 on a slot with nothing to run it. In local mode every slot counts.
    let live = registry.live_by_class();
    let mut counted = vec![0usize; registry.classes.len()];
    for slot in registry.slots.iter().filter(|slot| slot.running.is_some()) {
        counted[slot.class] += 1;
    }
    let workers: Vec<schedule::Worker> = registry
        .slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| {
            let Some(live) = live.as_deref().filter(|_| slot.running.is_none()) else {
                return true;
            };
            let class = slot.class;
            let available = counted[class] < live.get(class).copied().unwrap_or(0);
            if available {
                counted[class] += 1;
            }
            available
        })
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
    let runner = runner_for(&state);
    let admission = admit(state.clone(), repo_ref.clone(), commit.clone(), runner)?;
    // Worker modes (#97 phase 2): the job is in the store before `POST
    // /api/index` answers, so a job a client was told about survives a
    // restart. Written here, off the registry's lock, which is why a new
    // job waits unstartable (`PendingJob::persisted`) until the row exists.
    if let Some(new) = admission.new {
        persist_admission(&state, admission.id, &repo_ref, &commit, new)?;
    }
    Ok(admission.id)
}

/// The runner a job of this service runs with. Loopback mode runs the job
/// through an agent (`workers::run_remote`); with no hub, which is local
/// mode, the runner is `run_blocking` as it always was.
fn runner_for(state: &AppState) -> JobRunner {
    match state.jobs.remote() {
        Some(hub) => Arc::new(move |state, repo_ref, tx| {
            crate::service::workers::run_remote(state, &hub, repo_ref, tx)
        }),
        None => Arc::new(run_blocking),
    }
}

/// A job `admit` created in worker mode, still to be written to the store.
struct NewJob {
    order: i64,
    class: usize,
    tx: watch::Sender<JobSnapshot>,
}

struct Admission {
    id: Uuid,
    /// Worker modes only: set when this call created the job (not a
    /// duplicate of an active one).
    new: Option<NewJob>,
}

/// Worker modes: writes a newly admitted job's row, then lets a slot start
/// it. A job cancelled meanwhile has its terminal state written too, since
/// the cancel may have found no row to write it to.
fn persist_admission(
    state: &Arc<AppState>,
    id: Uuid,
    repo_ref: &RepoRef,
    commit: &str,
    new: NewJob,
) -> Result<(), ApiError> {
    let spec = job_spec(&state.config, repo_ref, commit);
    let snapshot = new.tx.borrow().clone();
    let row = crate::service::store::JobRow {
        job_id: id.to_string(),
        slug: repo_ref.slug.clone(),
        commit: commit.to_owned(),
        spec_json: serde_json::to_string(&spec).expect("a JobSpec serializes"),
        class: new.class as i64,
        status: table_status(&snapshot, "queued").to_owned(),
        attempt: 1,
        epoch: 0,
        lease_holder: None,
        lease_deadline_ms: None,
        queue_order: new.order,
        snapshot_json: serde_json::to_string(&snapshot).expect("a JobSnapshot serializes"),
    };
    if let Err(error) = state.store.insert_job(&row) {
        let error = ApiError::internal(format!("could not record the job: {error:#}"));
        state.jobs.abandon(id, error.body.clone());
        return Err(error);
    }
    state.jobs.admitted(state, id);
    persist_terminal(&state.store, &new.tx.borrow());
    Ok(())
}

/// A snapshot's `jobs.status`: `done` or `failed` once terminal, otherwise
/// `live`, the state only the caller knows (queued, leased, running).
pub(crate) fn table_status<'a>(snapshot: &JobSnapshot, live: &'a str) -> &'a str {
    match snapshot.status {
        JobStatus::Done => "done",
        JobStatus::Failed => "failed",
        _ => live,
    }
}

/// Worker modes: writes a terminal snapshot to the job's row (a no-op for
/// one that is not terminal, and for a row already terminal). Every
/// terminal outcome passes here: `worker_loop` for whatever the runner
/// left, `cancel_job` for a cancel. A failed write is logged; the job's
/// in-memory state stands.
pub(crate) fn persist_terminal(store: &crate::service::store::Store, snapshot: &JobSnapshot) {
    if !is_terminal(snapshot) {
        return;
    }
    let json = serde_json::to_string(snapshot).expect("a JobSnapshot serializes");
    if let Err(error) = store.finish_job(
        &snapshot.job_id.to_string(),
        table_status(snapshot, "failed"),
        &json,
    ) {
        eprintln!(
            "job {}: could not record its end in the store: {error:#}",
            snapshot.job_id
        );
    }
}

/// `POST /api/jobs/{id}/cancel`: [`JobRegistry::cancel`], and in worker
/// modes the terminal state written to the store before the answer (§2.4:
/// the cancel is terminal the moment it is recorded). A job a restarted
/// master no longer holds in memory answers with its persisted snapshot,
/// terminal by construction (`restore` reloads every live one).
pub fn cancel_job(state: &AppState, id: Uuid) -> Result<JobSnapshot, ApiError> {
    match state.jobs.cancel(id) {
        Ok(snapshot) => {
            if state.jobs.is_durable() {
                persist_terminal(&state.store, &snapshot);
            }
            Ok(snapshot)
        }
        Err(error) => persisted_snapshot(state, id).ok_or(error),
    }
}

/// Worker modes: the last snapshot the store holds for `id`, for a job no
/// longer in memory -- one that ended before a restart. `None` in local
/// mode, which persists no jobs.
pub fn persisted_snapshot(state: &AppState, id: Uuid) -> Option<JobSnapshot> {
    if !state.jobs.is_durable() {
        return None;
    }
    let row = match state.store.job(&id.to_string()) {
        Ok(row) => row?,
        Err(error) => {
            eprintln!("job {id}: could not read it from the store: {error:#}");
            return None;
        }
    };
    serde_json::from_str(&row.snapshot_json).ok()
}

/// A job's snapshot as it goes back to its queue (#97 phase 2, §2.2): a
/// re-queued job shows `queued` with its stages reset and `stage` saying
/// why. Pure, so the store can be written with it before memory is.
pub(crate) fn requeued_snapshot(snapshot: &JobSnapshot, why: &str) -> JobSnapshot {
    let mut next = snapshot.clone();
    next.status = JobStatus::Queued;
    next.stage = why.to_owned();
    next.queue_position = None;
    next.finished_at = None;
    next.error = None;
    next.error_code = None;
    next.progress = None;
    next.eta_start_s = None;
    next.elapsed_s = 0.0;
    for stage in &mut next.stages {
        stage.state = StageState::Pending;
        stage.started_at = None;
        stage.duration_s = None;
    }
    next
}

/// What the ETA and memory models know of a job before its worker reports
/// anything: the service's reference mode. See `admit`.
fn prior_features(config: &ServeConfig) -> RepoFeatures {
    RepoFeatures {
        refs: (config.refs == crate::extract::RefsMode::Scip).then(|| config.refs.to_string()),
        ..RepoFeatures::default()
    }
}

/// [`admit`] as it was before worker modes needed its second half: the
/// signature the tests below were written against. Production admits
/// through `spawn_job`.
#[cfg(test)]
fn enqueue_job(
    state: Arc<AppState>,
    repo_ref: RepoRef,
    commit: String,
    runner: JobRunner,
) -> Result<Uuid, ApiError> {
    admit(state, repo_ref, commit, runner).map(|admission| admission.id)
}

fn admit(
    state: Arc<AppState>,
    repo_ref: RepoRef,
    commit: String,
    runner: JobRunner,
) -> Result<Admission, ApiError> {
    let key = (repo_ref.slug.clone(), commit.clone());
    let mut registry = state.jobs.0.lock().expect("job registry mutex poisoned");
    if registry.stopping {
        return Err(ApiError::server_stopping(
            "the service is shutting down and is not accepting new jobs",
        ));
    }
    if let Some(id) = registry.active.get(&key) {
        return Ok(Admission { id: *id, new: None });
    }
    registry.ensure_classes(&state.config.limits);
    // Until the worker reports the repository's features, all the ETA
    // and memory models know is the service's reference mode. Recording it
    // now lets a queued job under `TOLMAP_REFS=scip` be costed with
    // indexing, while one on the hand default keeps the hand prior (`refs`
    // absent, exactly as before #110 P2a). The worker's own `Features`
    // event replaces this row, and `worker_loop` (or a queued cancel)
    // removes it.
    let prior = prior_features(&state.config);
    // The class comes from this slug's own last measurement (§2.1 step 1,
    // #97 phase 2 step 4), else the memory model's reference-mode prior
    // (step 2). Local mode has one class, so every job still binds to it;
    // with `TOLMAP_LOOPBACK_CLASSES` the prediction picks the class.
    let predicted_peak = registry
        .memory_model
        .predict_for(Some(repo_ref.slug.as_str()), &prior);
    let class = schedule::bind(Some(predicted_peak), &registry.classes);
    let live = registry.live_by_class();
    let idle_slot = registry.startable_slot_for(class, live.as_deref());
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
    let initial_eta = registry
        .eta_model
        .predict(&prior, &[false; StageId::ALL.len()], None);
    registry.features.insert(job_id, prior);
    tx.send_modify(|snapshot| snapshot.eta = Some(initial_eta));
    registry.jobs.insert(job_id, tx.clone());
    registry.active.insert(key.clone(), job_id);
    registry.next_order += 1;
    let order = registry.next_order;
    // Worker modes: the job may not start before its row is written, off
    // this lock, by `spawn_job`; it waits in its queue, unstartable, until
    // `JobRegistry::admitted`.
    let durable = registry.remote.is_some();
    let job = PendingJob {
        id: job_id,
        key,
        repo_ref,
        tx,
        runner,
        order,
        class,
        persisted: !durable,
    };
    let new = durable.then(|| NewJob {
        order,
        class,
        tx: job.tx.clone(),
    });
    match idle_slot.filter(|_| !durable) {
        Some(slot) => {
            job.tx
                .send_modify(|snapshot| snapshot.eta_start_s = Some(0.0));
            registry.slots[slot].running = Some((job_id, job.tx.clone()));
            tokio::spawn(worker_loop(state.clone(), slot, job));
        }
        None => {
            let position = registry.queues[class].len() + 1;
            job.tx
                .send_modify(|snapshot| snapshot.queue_position = Some(position));
            registry.queues[class].push_back(job);
        }
    }
    simulate_queue_etas(&registry);
    Ok(Admission { id: job_id, new })
}

/// Worker modes: starts every idle slot on the job it would take next
/// (`schedule::next_for`), if that job is persisted -- after an admission
/// is written, and after a restart.
fn dispatch_idle(state: &Arc<AppState>, registry: &mut RegistryInner) {
    // A stopping master starts nothing more: an agent going away during a
    // graceful stop calls in here too, and what is queued stays in the table
    // for the next process.
    if registry.stopping {
        simulate_queue_etas(registry);
        return;
    }
    let live = registry.live_by_class();
    for slot in 0..registry.slots.len() {
        if registry.slots[slot].running.is_some() {
            continue;
        }
        let Some(next_class) = registry.next_queue_for(slot, live.as_deref()) else {
            continue;
        };
        let job = registry.queues[next_class]
            .pop_front()
            .expect("next_for names a non-empty queue");
        registry.slots[slot].running = Some((job.id, job.tx.clone()));
        tokio::spawn(worker_loop(state.clone(), slot, job));
    }
    simulate_queue_etas(registry);
}

/// Worker modes (#97 phase 2, step 4): an agent connected or went away
/// (`WorkerHub::on_agents_changed`). Starts whatever the connected agents
/// can take now -- a spill a large agent that just came back may make --
/// and quotes every queued job again against the agents there are.
pub(crate) fn agents_changed(state: &Arc<AppState>) {
    let mut registry = state.jobs.0.lock().expect("job registry mutex poisoned");
    dispatch_idle(state, &mut registry);
}

/// Worker modes, §6 "job child OOM-killed" (#97 phase 2, step 4): records
/// the peak an attempt reached when it was killed for memory as a timing
/// row of its own, so the memory model learns from it although the job
/// goes on -- a floor under this slug's next prediction
/// (`MemoryModel::predict_for`). Its stages are all unfinished, so it
/// never narrows the population curve or the ETA model. Keyed
/// `<job>/e<epoch>`, so the job's own final row does not replace it, and
/// the peak is taken off the job, whose final row is another attempt's.
pub(crate) fn record_oom(state: &AppState, id: Uuid, epoch: u64, peak_rss_bytes: Option<u64>) {
    let row = {
        let mut registry = state.jobs.0.lock().expect("job registry mutex poisoned");
        registry.peak_rss.remove(&id);
        let Some(tx) = registry.jobs.get(&id).cloned() else {
            return;
        };
        let (elapsed_s, slug) = {
            let snapshot = tx.borrow();
            (snapshot.elapsed_s, snapshot.slug.clone())
        };
        let features = registry.features.get(&id).cloned().unwrap_or_default();
        TimingRow {
            features,
            elapsed_s,
            stage_s: vec![None; StageId::ALL.len()],
            peak_rss_bytes,
            slug: Some(slug),
        }
    };
    // Off the registry's lock: the store is never written under it.
    if let Err(error) = state.store.save_timing(&format!("{id}/e{epoch}"), &row) {
        eprintln!("timing store warning for {id}: {error:#}");
        return;
    }
    let mut registry = state.jobs.0.lock().expect("job registry mutex poisoned");
    registry.memory_model.record(row.clone());
    registry.eta_model.record(row);
}

/// Worker modes (#97 phase 2, docs/WORKER_TIER.md §6 "master restarts
/// mid-job"): reloads every live job from the store before the service
/// admits anything. Queued jobs return to their class queues in admission
/// order, so they keep their positions and the dispatch order their ETAs
/// were computed for. A job that was leased or running holds a slot again
/// and its runner adopts the lease, whose deadline starts over at one TTL
/// from now; the agent that held it is gone (loopback agents die with the
/// channel), so the lease expires and the job goes back to the head of its
/// queue without counting an attempt -- a master restart is not a lost
/// worker. The dedup keys and the queue bounds are rebuilt with it.
pub fn restore(state: &Arc<AppState>) -> anyhow::Result<()> {
    let store = &state.store;
    if let Err(error) = store.prune_finished_jobs(FINISHED_JOBS_KEPT) {
        eprintln!("tolmap serve: could not prune finished jobs: {error:#}");
    }
    let rows = store.live_jobs()?;
    let max_order = store.max_job_order()?;
    let runner = runner_for(state);
    let prior = prior_features(&state.config);
    let (mut running, mut queued) = (0usize, 0usize);
    // Written after the lock is released: rows that could not be read
    // back, and leased jobs with no slot left to hold them.
    let mut unreadable = Vec::new();
    let mut without_slot = Vec::new();
    {
        let mut registry = state.jobs.0.lock().expect("job registry mutex poisoned");
        registry.ensure_classes(&state.config.limits);
        registry.next_order = registry.next_order.max(max_order);
        let mut to_start = Vec::new();
        for row in rows {
            let parsed = Uuid::parse_str(&row.job_id).ok().zip(
                serde_json::from_str::<JobSpec>(&row.spec_json)
                    .ok()
                    .zip(serde_json::from_str::<JobSnapshot>(&row.snapshot_json).ok()),
            );
            let key = (row.slug.clone(), row.commit.clone());
            let Some((id, (spec, snapshot))) =
                parsed.filter(|_| !registry.active.contains_key(&key))
            else {
                unreadable.push(row);
                continue;
            };
            // A class the configuration no longer has (fewer
            // `TOLMAP_LOOPBACK_CLASSES` across the restart) is read as the
            // largest there is, where a job that fits nowhere goes (§2.1).
            let largest = registry.classes.len().saturating_sub(1);
            let class = usize::try_from(row.class)
                .ok()
                .map_or(largest, |class| class.min(largest));
            let (tx, _rx) = watch::channel(snapshot);
            registry.jobs.insert(id, tx.clone());
            registry.active.insert(key.clone(), id);
            registry.features.insert(id, prior.clone());
            let job = PendingJob {
                id,
                key,
                repo_ref: repo_ref_of(&spec),
                tx: tx.clone(),
                runner: runner.clone(),
                order: row.queue_order,
                class,
                persisted: true,
            };
            let epoch = u64::try_from(row.epoch).unwrap_or(0);
            if matches!(row.status.as_str(), "leased" | "running") {
                running += 1;
                if let Some(slot) = registry.idle_slot_for(class) {
                    registry.orphans.insert(id, epoch);
                    registry.slots[slot].running = Some((id, tx));
                    to_start.push((slot, job));
                    continue;
                }
                // More leased jobs than slots (the agent count went down
                // across the restart): back to the queue at once.
                let reset = requeued_snapshot(&tx.borrow(), RESTARTED);
                tx.send_replace(reset.clone());
                without_slot.push((row.job_id.clone(), epoch, reset));
            } else {
                queued += 1;
            }
            let class_queue = &mut registry.queues[class];
            insert_by_order(class_queue, job);
        }
        for (slot, job) in to_start {
            tokio::spawn(worker_loop(state.clone(), slot, job));
        }
        dispatch_idle(state, &mut registry);
    }
    for (job_id, epoch, reset) in without_slot {
        let json = serde_json::to_string(&reset).expect("a JobSnapshot serializes");
        if let Err(error) = store.requeue_job(&job_id, epoch, false, 0, &json) {
            eprintln!("job {job_id}: could not re-queue it in the store: {error:#}");
        }
    }
    for row in unreadable {
        eprintln!(
            "job {}: its stored row cannot be reloaded (unreadable, or a second live job for \
             {} at {}); marking it failed",
            row.job_id, row.slug, row.commit
        );
        let _ = store.finish_job(&row.job_id, "failed", &row.snapshot_json);
    }
    if running + queued > 0 {
        eprintln!(
            "tolmap serve: restored {running} leased or running and {queued} queued job(s) from \
             the store"
        );
    }
    Ok(())
}

/// How many finished jobs' rows `restore` keeps for `GET /api/jobs/{id}`.
const FINISHED_JOBS_KEPT: usize = 1000;

/// `stage` of a job a master restart put back in its queue.
pub(crate) const RESTARTED: &str = "the service restarted; retrying the job on a worker";

/// The repository a stored `JobSpec` names, as `executor::execute` reads it.
fn repo_ref_of(spec: &JobSpec) -> RepoRef {
    RepoRef {
        slug: spec.slug.clone(),
        owner: spec.owner.clone(),
        repo: spec.repo.clone(),
        source: if spec.local {
            RepoSource::Local(std::path::PathBuf::from(&spec.source))
        } else {
            RepoSource::Remote(spec.source.clone())
        },
    }
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
            order,
            class: job_class,
            persisted,
        } = job;
        tx.send_modify(|snapshot| {
            snapshot.queue_position = None;
            snapshot.eta_start_s = None;
            snapshot.started_at = now_rfc3339();
        });
        // Kept for a worker-mode re-queue, which puts the same job back.
        let again = (repo_ref.clone(), runner.clone());
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
        // Worker modes (#97 phase 2): a job the runner put back in its
        // queue, or one a graceful stop leaves as it is for the next
        // process, is not over, so it is neither failed here nor ended
        // below. `held` is always false in local mode.
        let (durable, held) = {
            let registry = state.jobs.0.lock().expect("job registry mutex poisoned");
            let durable = registry.remote.is_some();
            (
                durable,
                durable && (registry.stopping || registry.requeue.contains(&id)),
            )
        };
        // The runner normally sets a terminal snapshot itself. A panic or
        // an unexpected return must never leave an accepted job in flight.
        if !held && !is_terminal(&tx.borrow()) {
            finish_failed(
                &tx,
                ErrorBody {
                    error: "internal_error".to_owned(),
                    message: "job exited without a terminal state".to_owned(),
                },
            );
        }
        // Off the registry's lock: the store is never written under it.
        if durable {
            persist_terminal(&state.store, &tx.borrow());
        }
        let mut registry = state.jobs.0.lock().expect("job registry mutex poisoned");
        let snapshot = tx.borrow().clone();
        let requeued = registry.requeue.remove(&id);
        let rebound = registry.rebind.remove(&id);
        // Re-read under the lock: a cancel that landed since has made the
        // job terminal, and then it ends here like any other.
        if durable && !is_terminal(&snapshot) {
            // No timing row for a run that did not finish, and the job
            // keeps its dedup key and features. A re-queued job goes back
            // by its admission order, which is the head of its queue; its
            // row says `queued` already (`workers::run_remote`). During a
            // graceful stop it stays where the table has it, for the next
            // process.
            if requeued && !registry.stopping {
                let (repo_ref, runner) = again;
                // A job that moved class (#97 phase 2, step 4) goes to that
                // class's queue, by its admission order like any re-queue.
                let class = rebound
                    .filter(|class| *class < registry.queues.len())
                    .unwrap_or(job_class);
                insert_by_order(
                    &mut registry.queues[class],
                    PendingJob {
                        id,
                        key,
                        repo_ref,
                        tx: tx.clone(),
                        runner,
                        order,
                        class,
                        persisted,
                    },
                );
            }
        } else {
            let peak_rss_bytes = registry.peak_rss.remove(&id);
            let features = registry.features.remove(&id).unwrap_or_default();
            log_peak_memory(id, &features, peak_rss_bytes, &registry.memory_model);
            let row = TimingRow {
                features,
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
                peak_rss_bytes,
                slug: Some(key.0.clone()),
            };
            if let Err(error) = state.store.save_timing(&id.to_string(), &row) {
                eprintln!("timing store warning for {id}: {error:#}");
            } else {
                registry.memory_model.record(row.clone());
                registry.eta_model.record(row);
            }
            registry.cancelled.remove(&id);
            if registry.active.get(&key) == Some(&id) {
                registry.active.remove(&key);
            }
        }
        // A stopping master in worker mode starts nothing more: whatever
        // is queued stays in the table for the next process.
        if durable && registry.stopping {
            registry.slots[slot].running = None;
            simulate_queue_etas(&registry);
            break;
        }
        // The slot is handed straight to the next job under the same lock,
        // as the single FIFO did, so no admission can see it idle in
        // between and start a second job on it. In worker modes a job
        // whose row is not written yet is not started; `admitted` starts
        // it once it is. Nor does a slot spill down to a smaller class
        // without an agent of its own class to run the job on
        // (`RegistryInner::next_queue_for`; in local mode this is exactly
        // the old hand-over).
        registry.slots[slot].running = None;
        let live = registry.live_by_class();
        if let Some(next_class) = registry.next_queue_for(slot, live.as_deref()) {
            let next = registry.queues[next_class]
                .pop_front()
                .expect("next_for names a non-empty queue");
            registry.slots[slot].running = Some((next.id, next.tx.clone()));
            simulate_queue_etas(&registry);
            job = next;
        } else {
            // Worker modes: what this slot cannot take another may -- a job
            // it just put back for a class it does not serve, or a small one
            // it may not spill to with no agent of its own. `dispatch_idle`
            // also quotes the queue again.
            if durable {
                dispatch_idle(&state, &mut registry);
            } else {
                simulate_queue_etas(&registry);
            }
            break;
        }
    }
}

/// One stderr line per finished job (#97 phase 0 point 4): the human-visible
/// evidence that `wait_with_peak` measured something real, in particular
/// through the uid drop (see the image e2e run cited in the PR body). Never
/// prints 0 for "nothing was measured" -- that would read as a real, tiny
/// peak rather than a missing one.
fn log_peak_memory(
    id: Uuid,
    features: &RepoFeatures,
    peak_rss_bytes: Option<u64>,
    memory_model: &MemoryModel,
) {
    let Some(peak) = peak_rss_bytes else {
        eprintln!("job {id}: no peak memory measured for this job");
        return;
    };
    let predicted = memory_model.predict_peak(features);
    let files: u64 = features.languages.values().map(|lang| lang.files).sum();
    let refs = features.refs.as_deref().unwrap_or("hand");
    eprintln!(
        "job {id} peak memory {} (predicted \u{2264} {}, files {files}, refs {refs})",
        format_mib(peak),
        format_mib(predicted),
    );
}

fn format_mib(bytes: u64) -> String {
    format!("{:.0} MiB", bytes as f64 / (1024.0 * 1024.0))
}

pub(crate) fn is_terminal(snapshot: &JobSnapshot) -> bool {
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

/// Marks the `Clone` stage running for the executor's own
/// `clone::materialize_with_progress` call (`executor::materialize_job_repo`,
/// through `SnapshotSink::clone_started`), made *before*
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

/// `executor::materialize_job_repo` with the snapshot sink `run_blocking`
/// gives it, under the signature the tests below were written against
/// before the executor moved out. Production calls go through
/// `executor::execute`.
#[cfg(test)]
fn materialize_job_repo(
    cache_dir: &Path,
    repo_ref: &RepoRef,
    limits: &crate::service::config::Limits,
    job_repo_dir: &Path,
    tx: &watch::Sender<JobSnapshot>,
    started: Instant,
) -> Result<clone::Materialized, ErrorBody> {
    executor::materialize_job_repo(
        cache_dir,
        repo_ref,
        limits.clone_cache_bytes,
        job_repo_dir,
        &mut SnapshotSink::new(tx, started, None),
    )
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

pub(crate) fn finish_failed(tx: &watch::Sender<JobSnapshot>, error: ErrorBody) {
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

/// Runs one job in local mode: [`prepare`], [`executor::execute`] and
/// [`register`] (docs/WORKER_TIER.md §2), all in this process, with the
/// same behaviour as before the split. The job child runs outside the API
/// process; a child that exits without a terminal protocol event is a job
/// failure, not a service failure. The queue loop retains the slot until
/// this function returns.
fn run_blocking(state: Arc<AppState>, repo_ref: RepoRef, tx: watch::Sender<JobSnapshot>) {
    let started = Instant::now();
    let job_id = tx.borrow().job_id;
    if state.jobs.is_cancelled(job_id) {
        return;
    }
    let (job, inputs) = match prepare(&state, &repo_ref, &tx) {
        Ok(prepared) => prepared,
        Err(error) => return finish_failed(&tx, error),
    };
    let env = match exec_env(&state.config) {
        Ok(env) => env,
        Err(error) => return finish_failed(&tx, error),
    };
    let mut sink = SnapshotSink::new(&tx, started, Some(&state.jobs));
    let probe = RegistryProbe {
        registry: Some(&state.jobs),
        id: job_id,
    };
    let executed = match executor::execute(&env, job_id, &job, &inputs, &mut sink, &probe) {
        Ok(executed) => executed,
        Err(error) => return finish_failed(&tx, error),
    };
    if state.jobs.is_cancelled(job_id) {
        let _ = std::fs::remove_dir_all(&executed.job_dir);
        return;
    }
    register(&state, &repo_ref, &tx, started, executed);
}

/// The master's half before the executor: everything the job needs from
/// the store, read here so the executor never has to. The result is a
/// portable [`JobSpec`] plus [`JobInputs`] -- data, not a way to read more.
///
/// `JobSpec::commit` is the commit resolved at admission. Local mode does
/// not use it yet: the executor checks out whatever the clone resolves HEAD
/// to, as before this split (docs/WORKER_TIER.md §3.3). Remote mode will
/// check out this commit.
pub(crate) fn prepare(
    state: &AppState,
    repo_ref: &RepoRef,
    tx: &watch::Sender<JobSnapshot>,
) -> Result<(JobSpec, JobInputs), ErrorBody> {
    let internal = |error: anyhow::Error| ApiError::internal(error.to_string()).body;
    let warm_start_candidates = state
        .store
        .warm_start_candidates(&repo_ref.slug)
        .map_err(internal)?;
    let names = state.store.load_names(&repo_ref.slug).map_err(internal)?;
    let commit = tx.borrow().commit.clone().unwrap_or_default();
    let job = job_spec(&state.config, repo_ref, &commit);
    let inputs = JobInputs {
        names,
        previous_maps: previous_map_inputs(&warm_start_candidates),
    };
    Ok((job, inputs))
}

/// The portable [`JobSpec`] for `repo_ref` at `commit` under this service's
/// configuration: what `prepare` sends the executor, and what worker modes
/// store at admission so a restarted master can rebuild the job.
fn job_spec(config: &ServeConfig, repo_ref: &RepoRef, commit: &str) -> JobSpec {
    let (source, local) = match &repo_ref.source {
        RepoSource::Remote(url) => (url.clone(), false),
        RepoSource::Local(path) => (path.to_string_lossy().into_owned(), true),
    };
    JobSpec {
        slug: repo_ref.slug.clone(),
        owner: repo_ref.owner.clone(),
        repo: repo_ref.repo.clone(),
        source,
        local,
        commit: commit.to_owned(),
        all_sources: false,
        prune_variant: config.prune_variant.to_string(),
        namer: config.namer.to_string(),
        namer_model: config.namer_model.clone(),
        refs: Some(config.refs.to_string()),
        install: (config.refs == crate::extract::RefsMode::Scip && config.scip_install)
            .then(|| "sandbox".to_owned()),
    }
}

/// The master's own host environment for the executor in local mode: its
/// cache directory, its uid drop and its own binary as the job child.
fn exec_env(config: &ServeConfig) -> Result<executor::ExecEnv, ErrorBody> {
    let worker_exe =
        std::env::current_exe().map_err(|error| ApiError::internal(error.to_string()).body)?;
    Ok(executor::ExecEnv {
        clone_cache: config.cache_dir.clone(),
        clone_cache_bytes: config.limits.clone_cache_bytes,
        job_root: config.cache_dir.join("work"),
        install_root: config.cache_dir.join("install"),
        worker_exe,
        worker_uid: config.worker_uid,
        worker_gid: config.worker_gid,
        allow_openrouter_key: config.namer == crate::naming::NamerKind::Model,
    })
}

/// The master's half after the executor: check and move the artifacts into
/// the store, insert the map row, save names and prune. Timing and peak
/// bookkeeping stays in `worker_loop`, which sees every job end, not only
/// the ones that get this far.
fn register(
    state: &AppState,
    repo_ref: &RepoRef,
    tx: &watch::Sender<JobSnapshot>,
    started: Instant,
    executed: executor::Executed,
) {
    // A refused result has already failed the job's snapshot.
    let _ = register_owned(
        state,
        repo_ref,
        tx,
        started,
        executed,
        Some(worker_uid_in_effect(state.config.worker_uid)),
    );
}

/// [`register`], with the uid the result's files must be owned by named by
/// the caller: the job child's in local mode, where the child wrote them
/// into its own output directory, and the master's own in loopback mode
/// (`workers::run_remote`), where the master wrote them itself from
/// uploads it verified. Returns whether the map row was stored, which is
/// when loopback mode may send `result_accepted` (docs/WORKER_TIER.md §2.3).
pub(crate) fn register_owned(
    state: &AppState,
    repo_ref: &RepoRef,
    tx: &watch::Sender<JobSnapshot>,
    started: Instant,
    executed: executor::Executed,
    owner: Option<u32>,
) -> Result<(), ErrorBody> {
    let job_id = tx.borrow().job_id;
    let stored = store_worker_result_owned(
        state,
        repo_ref,
        job_id,
        &executed.output_dir,
        &executed.checkout,
        &executed.output,
        owner,
    );
    let _ = std::fs::remove_dir_all(&executed.job_dir);
    if let Err(error) = stored {
        finish_failed(tx, error.clone());
        return Err(error);
    }
    set_commit(tx, &executed.checkout.commit);
    if let Err(error) = state
        .store
        .prune(&repo_ref.slug, state.config.retain_commits_per_repo)
    {
        eprintln!("prune warning for {}: {error:#}", repo_ref.slug);
    }
    tx.send_modify(|snapshot| snapshot.elapsed_s = started.elapsed().as_secs_f64());
    finish_done(tx);
    Ok(())
}

/// The store's warm-start rows as the executor's inputs: which stored map,
/// on which branch, at which path (see `executor::stage_previous_map`).
fn previous_map_inputs(rows: &[MapRow]) -> Vec<PreviousMapInput> {
    rows.iter()
        .map(|row| PreviousMapInput {
            commit: row.commit.clone(),
            branch: row.branch.clone(),
            path: row.map_path.clone(),
        })
        .collect()
}

/// `executor::stage_previous_map` on store rows, under the signature the
/// tests below were written against before the executor moved out.
#[cfg(test)]
fn stage_previous_map(
    job_dir: &Path,
    candidates: &[MapRow],
    branch: Option<&str>,
    log: &dyn Fn(String),
) -> Vec<crate::worker::PreviousMap> {
    executor::stage_previous_map(job_dir, &previous_map_inputs(candidates), branch, log)
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
    store_worker_result_owned(
        state,
        repo_ref,
        job_id,
        output_dir,
        checkout,
        output,
        Some(worker_uid_in_effect(state.config.worker_uid)),
    )
}

/// [`store_worker_result`] with the files' owner named by the caller -- see
/// [`register_owned`].
fn store_worker_result_owned(
    state: &AppState,
    repo_ref: &RepoRef,
    job_id: Uuid,
    output_dir: &Path,
    checkout: &clone::Materialized,
    output: &WorkerOutput,
    owner: Option<u32>,
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
                files: output.files,
                districts: output.districts,
            },
            owner,
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
/// the service is root and drops to it (`executor::run_child`), otherwise the
/// service's own, which the worker then runs as.
pub(crate) fn worker_uid_in_effect(configured: u32) -> u32 {
    if is_root() {
        configured
    } else {
        current_uid()
    }
}

/// The master's [`EventSink`] in local mode: what `process_worker_exe` did
/// to the job's snapshot and the registry before the executor moved out,
/// unchanged. It keeps the per-stage counters that turn the child's raw
/// events into monotone progress and a running ETA; the executor only
/// relays events.
pub(crate) struct SnapshotSink<'a> {
    tx: &'a watch::Sender<JobSnapshot>,
    started: Instant,
    registry: Option<&'a JobRegistry>,
    id: Uuid,
    // Multi-source extraction revisits parse and resolve. The worker reports
    // each pass separately, while API counters belong to the stable stage ID.
    stage_offsets: [u64; StageId::ALL.len()],
    stage_max: [u64; StageId::ALL.len()],
    stage_started_at: [Option<Instant>; StageId::ALL.len()],
    last_progress: [Option<(u64, Instant)>; StageId::ALL.len()],
    ewma_rate: [Option<f64>; StageId::ALL.len()],
    completed_passes: [usize; StageId::ALL.len()],
    running_eta: Option<(StageId, f64, Option<f64>, Option<f64>)>,
}

impl<'a> SnapshotSink<'a> {
    pub(crate) fn new(
        tx: &'a watch::Sender<JobSnapshot>,
        started: Instant,
        registry: Option<&'a JobRegistry>,
    ) -> Self {
        SnapshotSink {
            tx,
            started,
            registry,
            id: tx.borrow().job_id,
            stage_offsets: [0; StageId::ALL.len()],
            stage_max: [0; StageId::ALL.len()],
            stage_started_at: std::array::from_fn(|_| None),
            last_progress: std::array::from_fn(|_| None),
            ewma_rate: [None; StageId::ALL.len()],
            completed_passes: [0; StageId::ALL.len()],
            running_eta: None,
        }
    }
}

impl EventSink for SnapshotSink<'_> {
    fn clone_started(&mut self) {
        mark_clone_running(self.tx, self.started);
    }

    fn clone_finished(&mut self, duration_s: f64, success: bool) {
        mark_clone_finished(self.tx, duration_s, success);
    }

    fn install_tick(&self) {
        let started = self.started;
        self.tx.send_modify(|snapshot| {
            if !is_terminal(snapshot) {
                snapshot.elapsed_s = started.elapsed().as_secs_f64();
            }
        });
    }

    fn peak_rss(&mut self, bytes: u64) {
        if let Some(registry) = self.registry {
            registry.set_peak_rss(self.id, bytes);
        }
    }

    fn event(&mut self, event: WorkerEvent) {
        let tx = self.tx;
        let started = self.started;
        let id = self.id;
        match event {
            WorkerEvent::StageStarted { stage, .. } => {
                self.stage_started_at[stage.index() - 1] = Some(Instant::now());
                let eta_stage = if matches!(
                    stage,
                    StageId::CloneObjects | StageId::CloneDeltas | StageId::CloneCheckout
                ) {
                    StageId::Clone
                } else {
                    stage
                };
                self.running_eta = Some((
                    eta_stage,
                    self.stage_started_at[eta_stage.index() - 1]
                        .map(|at| at.elapsed().as_secs_f64())
                        .unwrap_or(0.0),
                    None,
                    None,
                ));
                self.stage_offsets[stage.index() - 1] = self.stage_max[stage.index() - 1];
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
                let index = value.stage.index() - 1;
                let mut value = value;
                value.done = value.done.saturating_add(self.stage_offsets[index]);
                value.total = value
                    .total
                    .map(|total| total.saturating_add(self.stage_offsets[index]));
                value.done = value.done.max(self.stage_max[index]);
                self.stage_max[index] = value.done;
                let now = Instant::now();
                if let Some((last_done, last_at)) = self.last_progress[index] {
                    let dt = now.duration_since(last_at).as_secs_f64();
                    if value.done > last_done && dt > 0.0 {
                        let instantaneous = (value.done - last_done) as f64 / dt;
                        self.ewma_rate[index] = Some(
                            self.ewma_rate[index]
                                .map_or(instantaneous, |old| 0.35 * instantaneous + 0.65 * old),
                        );
                    }
                }
                self.last_progress[index] = Some((value.done, now));
                let rate = self.ewma_rate[index]
                    .or(value.rate_per_s)
                    .filter(|r| *r > 0.0);
                let features = self
                    .registry
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
                self.running_eta = Some((
                    eta_stage,
                    self.stage_started_at[eta_stage.index() - 1]
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
                    self.completed_passes[stage.index() - 1] += 1;
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
                if self
                    .running_eta
                    .is_some_and(|(current, _, _, _)| current == stage)
                {
                    self.running_eta = None;
                }
            }
            WorkerEvent::Features { features, .. } => {
                if let Some(registry) = self.registry {
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
            // The executor keeps the result and the error for its return
            // value, and has already answered an install request; none of
            // them changes the snapshot until the job ends.
            WorkerEvent::Result { .. }
            | WorkerEvent::Error { .. }
            | WorkerEvent::InstallRequest { .. } => {}
        }
        if let Some(registry) = self.registry {
            let stage_started_at = &self.stage_started_at;
            registry.estimate(
                tx,
                self.running_eta.map(|(stage, _, remaining, fraction)| {
                    (
                        stage,
                        stage_started_at[stage.index() - 1]
                            .map(|at| at.elapsed().as_secs_f64())
                            .unwrap_or(0.0),
                        remaining,
                        fraction,
                    )
                }),
                &self.completed_passes,
            );
        }
    }
}

/// The master's [`CancelProbe`] in local mode: the registry's cancel set
/// and child table, as before the split. `None` (tests only) never cancels.
struct RegistryProbe<'a> {
    registry: Option<&'a JobRegistry>,
    id: Uuid,
}

impl CancelProbe for RegistryProbe<'_> {
    fn is_cancelled(&self) -> bool {
        self.registry
            .is_some_and(|registry| registry.is_cancelled(self.id))
    }

    fn child_started(&self, pid: u32) {
        if let Some(registry) = self.registry {
            registry.register_child(self.id, pid);
        }
    }

    fn child_finished(&self) {
        if let Some(registry) = self.registry {
            registry.unregister_child(self.id);
        }
    }
}

/// `executor::run_child` with the local-mode sink and probe, under the
/// signature the tests below were written against before the executor
/// moved out. Production calls go through `executor::execute`.
#[cfg(test)]
fn process_worker_exe(
    tx: &watch::Sender<JobSnapshot>,
    spec: WorkerSpec,
    started: Instant,
    exe: &Path,
    registry: Option<&JobRegistry>,
    hardening: &WorkerHardening,
    install: Option<&ServiceInstall>,
) -> Result<WorkerOutput, ErrorBody> {
    let id = tx.borrow().job_id;
    executor::run_child(
        exe,
        spec,
        id,
        hardening,
        install,
        &mut SnapshotSink::new(tx, started, registry),
        &RegistryProbe { registry, id },
    )
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
            // Matches `result()`'s reported `files: 1, districts: 1` --
            // `worker_result::check_counts` refuses a result whose counts
            // disagree with the map document it shipped (#97 phase 2).
            std::fs::write(
                output_dir.join("demo.json"),
                br#"{"F": ["a.py"], "districts": {"0": {}}}"#,
            )
            .unwrap();
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

    /// §5.4, #97 phase 2: the map document itself has one file and one
    /// district; a `files` that disagrees is refused before anything is
    /// stored, the local-mode half of `worker_result::check_counts`.
    #[cfg(unix)]
    #[test]
    fn a_files_count_other_than_the_map_is_refused() {
        let fixture = ResultFixture::new();
        let mut result = fixture.result();
        result["files"] = serde_json::json!(2);
        fixture.assert_refused(fixture.store(result));
    }

    #[cfg(unix)]
    #[test]
    fn a_districts_count_other_than_the_map_is_refused() {
        let fixture = ResultFixture::new();
        let mut result = fixture.result();
        result["districts"] = serde_json::json!(2);
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
