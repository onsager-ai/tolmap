//! FIFO admission and snapshots live in the service. Each blocking build
//! runs in a child process; the child never opens the service database.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
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
    let prior = registry
        .eta_model
        .predict(&RepoFeatures::default(), &[false; 18], None);
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
        let eta = registry.eta_model.predict(&features, &[false; 18], None);
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
    let initial_eta = registry
        .eta_model
        .predict(&RepoFeatures::default(), &[false; 18], None);
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
    let output_dir = state
        .config
        .cache_dir
        .join("work")
        .join(&repo_ref.owner)
        .join(&repo_ref.repo)
        .join(tx.borrow().job_id.to_string());
    if let Err(error) = std::fs::create_dir_all(&output_dir) {
        return finish_failed(&tx, ApiError::internal(error.to_string()).body);
    }
    let names_input = output_dir.join("worker-names-input.json");
    let names = match state.store.load_names(&repo_ref.slug) {
        Ok(names) => names,
        Err(error) => return finish_failed(&tx, ApiError::internal(error.to_string()).body),
    };
    if let Err(error) = crate::naming::save_cache(&names_input, &names) {
        return finish_failed(&tx, ApiError::internal(error.to_string()).body);
    }
    let (source, local) = match &repo_ref.source {
        clone::RepoSource::Local(path) => (path.to_string_lossy().into_owned(), true),
        clone::RepoSource::Remote(url) => (url.clone(), false),
    };
    let spec = WorkerSpec {
        v: 1,
        slug: repo_ref.slug.clone(),
        owner: repo_ref.owner.clone(),
        repo: repo_ref.repo.clone(),
        source,
        local,
        all_sources: false,
        cache_dir: state.config.cache_dir.to_string_lossy().into_owned(),
        output_dir: output_dir.to_string_lossy().into_owned(),
        clone_cache_bytes: state.config.limits.clone_cache_bytes,
        prune_variant: state.config.prune_variant.to_string(),
        namer: state.config.namer.to_string(),
        namer_model: state.config.namer_model.clone(),
        previous_maps,
        names_cache: Some(names_input.to_string_lossy().into_owned()),
    };
    let output = match process_worker(&state, &tx, spec, started) {
        Ok(output) => output,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&output_dir);
            return finish_failed(&tx, error);
        }
    };
    if state.jobs.is_cancelled(tx.borrow().job_id) {
        let _ = std::fs::remove_dir_all(&output_dir);
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
        std::fs::create_dir_all(final_path.parent().expect("map parent"))?;
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
        let _ = std::fs::remove_dir_all(&output_dir);
        return finish_failed(&tx, ApiError::internal(format!("{error:#}")).body);
    }
    let _ = std::fs::remove_dir_all(&output_dir);
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

fn process_worker(
    state: &Arc<AppState>,
    tx: &watch::Sender<JobSnapshot>,
    spec: WorkerSpec,
    started: Instant,
) -> Result<WorkerOutput, ErrorBody> {
    let exe =
        std::env::current_exe().map_err(|error| ApiError::internal(error.to_string()).body)?;
    process_worker_exe(tx, spec, started, &exe, Some(&state.jobs))
}

fn process_worker_exe(
    tx: &watch::Sender<JobSnapshot>,
    spec: WorkerSpec,
    started: Instant,
    exe: &std::path::Path,
    registry: Option<&JobRegistry>,
) -> Result<WorkerOutput, ErrorBody> {
    let mut command = Command::new(exe);
    command
        .arg("worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
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
            limits,
            retain_commits_per_repo: 20,
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
                };
                let error = process_worker_exe(&tx, spec, Instant::now(), &fake_worker, None)
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
                };
                let error =
                    process_worker_exe(&tx, spec, Instant::now(), &fake_worker, Some(&state.jobs))
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
            };
            let error = process_worker_exe(&tx, spec, Instant::now(), &fake_worker, None)
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
}
