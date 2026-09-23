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

use std::collections::HashMap;
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

pub type JobRegistry = Mutex<HashMap<Uuid, watch::Sender<JobSnapshot>>>;

pub fn new_registry() -> JobRegistry {
    Mutex::new(HashMap::new())
}

/// Queues a job and returns its id immediately; the work happens on a
/// spawned task. `state.jobs` keeps the sending half so `GET
/// /api/jobs/{id}` and the SSE endpoint can each get their own receiver via
/// `.subscribe()`.
pub fn spawn_job(state: Arc<AppState>, repo_ref: RepoRef) -> Uuid {
    let job_id = Uuid::new_v4();
    let snapshot = JobSnapshot {
        job_id,
        slug: repo_ref.slug.clone(),
        commit: None,
        status: JobStatus::Queued,
        stage: "queued".to_owned(),
        started_at: now_rfc3339(),
        finished_at: None,
        error: None,
        error_code: None,
    };
    let (tx, _rx) = watch::channel(snapshot);
    state
        .jobs
        .lock()
        .expect("job registry mutex poisoned")
        .insert(job_id, tx.clone());

    tokio::spawn(async move {
        let max_seconds = state.config.limits.max_job_seconds;
        let blocking_state = state.clone();
        let blocking_tx = tx.clone();
        let handle = tokio::task::spawn_blocking(move || {
            run_blocking(blocking_state, repo_ref, blocking_tx)
        });
        match tokio::time::timeout(Duration::from_secs(max_seconds), handle).await {
            Ok(Ok(())) => {} // run_blocking always leaves the snapshot in a terminal state itself.
            Ok(Err(join_error)) => finish_failed(
                &tx,
                ErrorBody {
                    error: "internal_error".to_owned(),
                    message: format!("job task panicked: {join_error}"),
                },
            ),
            Err(_) => {
                // NOTE (reported as a known limitation): this marks the job
                // failed for anyone watching it, but does not and cannot
                // kill the still-running blocking thread underneath it --
                // spawn_blocking tasks are not cancellable. The thread
                // finishes (or hangs) on its own; its eventual result is
                // discarded since nothing still holds its JoinHandle. In
                // practice the pre-checks in `clone.rs` (file count, clone
                // size, history depth) are what keep this from being the
                // common case rather than this timeout.
                finish_failed(
                    &tx,
                    ErrorBody {
                        error: "index_failed".to_owned(),
                        message: format!("job exceeded the {max_seconds}s wall-time limit"),
                    },
                );
            }
        }
    });

    job_id
}

fn advance(tx: &watch::Sender<JobSnapshot>, status: JobStatus, stage: &str) {
    tx.send_modify(|snapshot| {
        snapshot.status = status;
        snapshot.stage = stage.to_owned();
    });
}

fn set_commit(tx: &watch::Sender<JobSnapshot>, commit: &str) {
    tx.send_modify(|snapshot| snapshot.commit = Some(commit.to_owned()));
}

fn finish_done(tx: &watch::Sender<JobSnapshot>) {
    tx.send_modify(|snapshot| {
        snapshot.status = JobStatus::Done;
        snapshot.stage = "done".to_owned();
        snapshot.finished_at = Some(now_rfc3339());
        snapshot.error = None;
        snapshot.error_code = None;
    });
}

fn finish_failed(tx: &watch::Sender<JobSnapshot>, error: ErrorBody) {
    tx.send_modify(|snapshot| {
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
