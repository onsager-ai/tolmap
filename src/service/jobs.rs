//! Job orchestration: `POST /api/index` spawns one of these, `GET
//! /api/jobs/{id}` and its SSE sibling read its current [`JobSnapshot`] off
//! a `tokio::sync::watch` channel (one value, overwritten in place, which
//! is exactly "one frame per status change" -- no separate history buffer
//! needed).
//!
//! The whole job body runs on a blocking thread
//! (`tokio::task::spawn_blocking`): cloning shells out to `git` and waits
//! on it, and `extract`/`pipeline` are CPU-bound synchronous code (tree-
//! sitter parsing, Leiden via FFI) with no async equivalent. Running that
//! on an async worker thread would stall every other request this process
//! is serving.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::watch;
use uuid::Uuid;

use crate::detect::{self, Confidence};
use crate::extract;
use crate::geometry;
use crate::service::clone::{self, RepoRef};
use crate::service::error::{ApiError, ErrorBody};
use crate::service::store::{self, MapRow};
use crate::service::time::now_rfc3339;
use crate::service::AppState;

const RESOLUTION: f64 = 1.1;
const WITH_PARCELS: bool = true;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Cloning,
    Detecting,
    Indexing,
    Done,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct JobSnapshot {
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
    /// rendered directly by the client, and a nested object here crashed the
    /// progress view on every job failure -- including `repo_too_large`, the
    /// one failure docs/ARCHITECTURE.md specifically requires to read clearly
    /// rather than as a timeout. The machine code lives beside it.
    pub error: Option<String>,
    /// Machine-readable failure code (`repo_too_large`, `detection_failed`,
    /// ...), or null. Clients branch on this rather than pattern-matching the
    /// message text.
    pub error_code: Option<String>,
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
}

pub struct JobRegistry(Mutex<RegistryInner>);

impl JobRegistry {
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
    };
    let (tx, _rx) = watch::channel(snapshot);
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
        tokio::spawn(worker_loop(state.clone(), job));
    } else {
        job.tx
            .send_modify(|snapshot| snapshot.queue_position = Some(registry.queue.len() + 1));
        registry.queue.push_back(job);
    }
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
            snapshot.started_at = now_rfc3339();
        });
        let max_seconds = state.config.limits.max_job_seconds;
        let blocking_state = state.clone();
        let blocking_tx = tx.clone();
        let mut handle =
            tokio::task::spawn_blocking(move || runner(blocking_state, repo_ref, blocking_tx));
        tokio::select! {
            result = &mut handle => {
                if let Err(join_error) = result {
                    finish_failed(&tx, ErrorBody {
                        error: "internal_error".to_owned(),
                        message: format!("job task panicked: {join_error}"),
                    });
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(max_seconds)) => {
                finish_failed(&tx, ErrorBody {
                    error: "index_failed".to_owned(),
                    message: format!("job exceeded the {max_seconds}s wall-time limit"),
                });
                // spawn_blocking cannot be cancelled. Keep this worker slot
                // occupied until its thread actually exits, or a timed-out
                // build could run beside the next queued build on a 1 GB VM.
                {
                    let mut registry = state.jobs.0.lock().expect("job registry mutex poisoned");
                    if registry.active.get(&key) == Some(&id) { registry.active.remove(&key); }
                }
                let _ = handle.await;
            }
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
        if registry.active.get(&key) == Some(&id) {
            registry.active.remove(&key);
        }
        if let Some(next) = registry.queue.pop_front() {
            for (index, waiting) in registry.queue.iter().enumerate() {
                waiting
                    .tx
                    .send_modify(|snapshot| snapshot.queue_position = Some(index + 1));
            }
            job = next;
        } else {
            registry.running -= 1;
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
    });
}

/// The whole job, synchronously: clone/fetch, detect, limit checks, index,
/// warm-start, store. Every early return leaves the snapshot in a terminal
/// `Failed` state via `finish_failed` before returning, so the caller never
/// needs to know *why* it stopped, only that it eventually reaches `Done`
/// or `Failed`.
fn run_blocking(state: Arc<AppState>, repo_ref: RepoRef, tx: watch::Sender<JobSnapshot>) {
    advance(
        &tx,
        JobStatus::Cloning,
        &format!("cloning {}", repo_ref.slug),
    );
    let materialized =
        match clone::materialize(&state.config.cache_dir, &repo_ref, &state.config.limits) {
            Ok(m) => m,
            Err(api_error) => return finish_failed(&tx, api_error.body),
        };
    set_commit(&tx, &materialized.commit);

    advance(
        &tx,
        JobStatus::Detecting,
        "detecting language and source root",
    );
    // issue #4's real detector (src/detect.rs), landed on main after this
    // milestone's placeholder was written -- see that module's doc comment
    // for what `chosen`/`candidates` carry and why confidence is never
    // inflated.
    let detection = match detect::detect(&materialized.path) {
        Ok(detection) => detection,
        Err(err) => {
            return finish_failed(
                &tx,
                ErrorBody {
                    error: "detection_failed".to_owned(),
                    message: err.to_string(),
                },
            )
        }
    };
    let chosen = detection.chosen;

    if chosen.confidence == Confidence::Low {
        // Finding 7: a wrong source root produces a plausible-looking wrong
        // map, not a loud failure. Surfaced as its own terminal state
        // (rather than indexed anyway) so the frontend can render "we are
        // not sure this is right" instead of a map that looks trustworthy
        // and is not.
        return finish_failed(&tx, ApiError::detection_uncertain(chosen.describe()).body);
    }

    if chosen.file_count > state.config.limits.max_files {
        return finish_failed(
            &tx,
            ApiError::repo_too_large(format!(
                "file count {} exceeds the configured limit of {}",
                chosen.file_count, state.config.limits.max_files
            ))
            .body,
        );
    }

    advance(&tx, JobStatus::Indexing, "indexing (extract)");
    let graph = match extract::build(&materialized.path, &chosen.pkg, chosen.language) {
        Ok(graph) => graph,
        Err(err) => {
            return finish_failed(
                &tx,
                ErrorBody {
                    error: "index_failed".to_owned(),
                    message: format!("{err:#}"),
                },
            )
        }
    };

    // Warm start (finding 4): read the previous commit's membership out of
    // the store, if there is one for this slug, and seed the partitioner
    // with it. `state.store.warm_start_source` already picks same-branch
    // history first.
    let warm_start_row = state
        .store
        .warm_start_source(&repo_ref.slug, materialized.branch.as_deref())
        .ok()
        .flatten();
    match &warm_start_row {
        Some(row) => eprintln!(
            "warm start: seeding {} from {}'s membership ({})",
            repo_ref.slug,
            row.commit,
            row.map_path.display()
        ),
        None => eprintln!(
            "warm start: no prior indexed commit for {} -- cold start",
            repo_ref.slug
        ),
    }
    let previous_document =
        warm_start_row.and_then(|row| store::read_map_document(&row.map_path).ok());

    advance(
        &tx,
        JobStatus::Indexing,
        "indexing (partition, geometry, naming)",
    );
    let work_dir = state
        .config
        .cache_dir
        .join("work")
        .join(&repo_ref.owner)
        .join(&repo_ref.repo);
    if let Err(err) = std::fs::create_dir_all(&work_dir) {
        return finish_failed(
            &tx,
            ErrorBody {
                error: "internal_error".to_owned(),
                message: err.to_string(),
            },
        );
    }
    // map_name stays the bare repo name (stable across commits) so the
    // names cache (`<work_dir>/<repo>.names.json`) is reused build over
    // build -- naming.rs's cache is keyed by a fingerprint of district
    // membership, not by commit, so this is what makes it actually pay off
    // across re-indexes rather than starting cold every time.
    let built_path = match geometry::build_from_graph_warm(
        graph,
        repo_ref.repo.clone(),
        &work_dir,
        RESOLUTION,
        geometry::BuildFeatures {
            parcels: WITH_PARCELS,
            prune_variant: state.config.prune_variant,
        },
        previous_document.as_ref(),
    ) {
        Ok(path) => path,
        Err(err) => {
            return finish_failed(
                &tx,
                ErrorBody {
                    error: "index_failed".to_owned(),
                    message: format!("{err:#}"),
                },
            )
        }
    };

    let document = match store::read_map_document(&built_path) {
        Ok(document) => document,
        Err(err) => {
            return finish_failed(
                &tx,
                ErrorBody {
                    error: "internal_error".to_owned(),
                    message: err.to_string(),
                },
            )
        }
    };

    // Content-addressed final home for this commit's map (store.rs's
    // module doc explains the choice); `built_path` is scratch, overwritten
    // by the next build for this slug, so it is moved out from under it
    // rather than left to be clobbered.
    let final_path = state
        .config
        .cache_dir
        .join("maps")
        .join(&repo_ref.owner)
        .join(&repo_ref.repo)
        .join(format!("{}.json", materialized.commit));
    if let Some(parent) = final_path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            return finish_failed(
                &tx,
                ErrorBody {
                    error: "internal_error".to_owned(),
                    message: err.to_string(),
                },
            );
        }
    }
    if let Err(err) = std::fs::rename(&built_path, &final_path) {
        return finish_failed(
            &tx,
            ErrorBody {
                error: "internal_error".to_owned(),
                message: err.to_string(),
            },
        );
    }

    let row = MapRow {
        slug: repo_ref.slug.clone(),
        owner: repo_ref.owner.clone(),
        repo: repo_ref.repo.clone(),
        commit: materialized.commit.clone(),
        branch: materialized.branch.clone(),
        lang: document.lang.clone(),
        files: document.files.len() as i64,
        districts: document.districts.len() as i64,
        modularity: document.q,
        map_path: final_path,
        indexed_at: now_rfc3339(),
    };
    if let Err(err) = state.store.insert(&row) {
        return finish_failed(
            &tx,
            ErrorBody {
                error: "internal_error".to_owned(),
                message: err.to_string(),
            },
        );
    }

    // Bound store growth (issue #23 gap 2): keep only the newest
    // `retain_commits_per_repo` indexed commits for this slug, evicting
    // older rows and their map files. Run right after `insert` succeeds so
    // the row just written is always counted as the newest -- prune never
    // evicts it (see `store::Store::prune`'s doc comment on why that
    // matters for finding 4's warm start). Best-effort like cache
    // eviction in `clone.rs`: failing to reclaim space is not a reason to
    // fail a job that already finished successfully.
    if let Err(err) = state
        .store
        .prune(&repo_ref.slug, state.config.retain_commits_per_repo)
    {
        eprintln!("prune warning for {}: {err:#}", repo_ref.slug);
    }

    finish_done(&tx);
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
    async fn timeout_keeps_worker_slot_until_blocking_body_exits() {
        let limits = Limits {
            max_concurrent_jobs: 1,
            max_queued_jobs: 1,
            max_job_seconds: 1,
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
        let second = enqueue_job(state.clone(), repo("two"), "b".to_owned(), runner).unwrap();
        until(|| snapshot(&state, first).status == JobStatus::Failed).await;
        assert_eq!(started.lock().unwrap().len(), 1);
        assert_eq!(snapshot(&state, second).queue_position, Some(1));
        release_tx.send(()).unwrap();
        until(|| started.lock().unwrap().len() == 2).await;
        assert_eq!(snapshot(&state, first).status, JobStatus::Failed);
        release_tx.send(()).unwrap();
    }
}
