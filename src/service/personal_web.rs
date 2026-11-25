//! Personal browser adapter over the CLI library and the service router.
use super::{
    error::{ApiError, ApiQuery},
    jobs::{JobSnapshot, JobStatus},
    store::{JobRow, PersonalSnapshot, Repository},
};
use crate::{
    catalogue::*,
    personal::{Library, SourceArgs},
    personal_index::PersonalControl,
};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use serde::Deserialize;
use sha2::Digest;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

pub struct WebState {
    pub library: Library,
    controls: Mutex<HashMap<String, Arc<PersonalControl>>>,
    slots: Arc<tokio::sync::Semaphore>,
}
impl WebState {
    pub fn new(library: Library) -> Arc<Self> {
        Arc::new(Self {
            slots: Arc::new(tokio::sync::Semaphore::new(
                library.config.service.limits.max_concurrent_jobs,
            )),
            library,
            controls: Mutex::new(HashMap::new()),
        })
    }
    fn repository(&self, id: &str) -> Result<Repository, ApiError> {
        self.library
            .store
            .repositories()?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| ApiError::not_found("repository unavailable"))
    }
}
fn invalid(error: anyhow::Error) -> ApiError {
    ApiError::invalid_request(format!("{error:#}"))
}
fn action(path: String, method: ActionMethod) -> RepositoryAction {
    RepositoryAction { path, method }
}
fn map_url(snapshot: &PersonalSnapshot) -> String {
    format!("/snapshot/{}", snapshot.id)
}
fn document(snapshot: &PersonalSnapshot) -> Result<crate::schema::MapDocument, ApiError> {
    super::store::read_map_document(&snapshot.map_path)
        .map_err(|e| ApiError::internal(e.to_string()))
}
fn repository_status(
    state: &WebState,
    repo: &Repository,
) -> Result<Option<RepositoryStatus>, ApiError> {
    let status = state.library.status(repo).map_err(invalid)?;
    for id in state.library.store.personal_repository_jobs(&repo.id)? {
        if state.controls.lock().unwrap().contains_key(&id) {
            let job = read_job(state, &id)?;
            return Ok(Some(RepositoryStatus::Indexing {
                phase: job.stage,
                job_id: Some(id),
            }));
        }
    }
    Ok(match status["attempt"][0].as_str() {
        Some("running") => Some(RepositoryStatus::Indexing {
            phase: "Indexing committed source".into(),
            job_id: None,
        }),
        Some("failed") => Some(RepositoryStatus::Failed {
            message: status["attempt"][1]
                .as_str()
                .unwrap_or("Indexing failed")
                .into(),
        }),
        _ => None,
    })
}

/// Reuse the service's static handler, with personal management/read routes.
/// Public catalogue/admission handlers are not reachable here.
pub fn router(static_dir: Option<&PathBuf>, state: Arc<WebState>) -> Router {
    let loopback = state.library.config.service.bind.ip().is_loopback();
    let router = Router::new()
        .route(
            "/api/healthz",
            get(|| async { Json(serde_json::json!({"ok":true})) }),
        )
        .route("/api/maps", get(catalogue))
        .route("/api/repositories/{id}/history", get(history))
        .route("/api/repositories/{id}/refresh", post(refresh))
        .route("/api/repositories/{id}", axum::routing::delete(remove))
        .route("/api/personal/scan", get(scan))
        .route("/api/personal/index", post(add))
        .route("/api/personal/batches/{id}", get(batch))
        .route("/api/jobs/{id}", get(job))
        .route("/api/jobs/{id}/cancel", post(cancel))
        .route("/api/jobs/{id}/retry", post(retry))
        .route("/api/compare/{owner}/{id}", get(compare))
        .route("/api/compare/{owner}/{id}/lenses", get(compare_lenses))
        .route("/api/compare/{owner}/{id}/debt", get(compare_debt))
        .route("/api/maps/{owner}/{id}", get(map))
        .route("/api/maps/{owner}/{id}/{*rest}", get(map_detail))
        .route("/api/index", any(unavailable));
    super::http::static_files(router, static_dir)
        .with_state(state)
        // Response layers run from the inner router outwards: inject into
        // plaintext HTML before compression encodes the response body.
        .layer(middleware::map_response(mark_personal))
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(middleware::from_fn(move |request, next| {
            browser_requests(loopback, request, next)
        }))
}
async fn browser_requests(
    loopback: bool,
    request: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    if loopback {
        let allowed = request
            .headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            // `http::uri::Authority::host()` stops at `]`, so `[::1].evil:8787`
            // would pass as `[::1]`; compare the exact host text instead.
            .and_then(|host| host.rsplit_once(':'))
            .is_some_and(|(host, port)| {
                port.parse::<u16>().is_ok() && matches!(host, "localhost" | "127.0.0.1" | "[::1]")
            });
        if !allowed {
            return ApiError::new(
                StatusCode::FORBIDDEN,
                "invalid_host",
                "loopback requests require a localhost, 127.0.0.1 or [::1] Host with a port",
            )
            .into_response();
        }
    }
    if matches!(
        *request.method(),
        axum::http::Method::POST
            | axum::http::Method::PUT
            | axum::http::Method::PATCH
            | axum::http::Method::DELETE
    ) && !request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
        })
    {
        return ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "json_required",
            "mutating requests require Content-Type: application/json",
        )
        .into_response();
    }
    next.run(request).await
}
async fn unavailable() -> ApiError {
    ApiError::not_found("no personal API route")
}
async fn mark_personal(response: Response) -> Response {
    if !response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|v| v.as_bytes().starts_with(b"text/html"))
    {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, 4 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(e) => return ApiError::internal(e.to_string()).into_response(),
    };
    let profile = HostProfile {
        version: 1,
        private: true,
        workspace: "Personal library".into(),
        repositories: vec![],
        actions: vec![],
        onboarding: Some(HostOnboarding {
            offered_repositories_path: "/api/personal/scan".into(),
            batch_index_path: "/api/personal/index".into(),
            batch_progress_path: "/api/personal/batches/{batch_id}".into(),
            open_sheet_parameter: "add".into(),
            install: None,
            manage_access: None,
            sign_in: None,
            authorize: None,
        }),
    };
    let injection = format!("<meta name=\"tolmap-personal\" content=\"1\"><script id=\"tolmap-host\" type=\"application/json\">{}</script></head>", serde_json::to_string(&profile).unwrap());
    let html = String::from_utf8_lossy(&bytes).replace("</head>", &injection);
    parts.headers.remove(header::CONTENT_LENGTH);
    parts
        .headers
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    Response::from_parts(parts, Body::from(html))
}
async fn catalogue(
    State(state): State<Arc<WebState>>,
) -> Result<Json<Vec<CatalogueEntry>>, ApiError> {
    let mut entries = Vec::new();
    for repo in state.library.store.repositories()? {
        let snapshots = state.library.store.personal_history(&repo.id)?;
        let latest = snapshots.first();
        let read = latest.map(document).transpose();
        let unreadable = read.as_ref().err().map(|e| RepositoryStatus::Failed {
            message: format!("Saved map is unreadable: {}", e.body.message),
        });
        let status = repository_status(&state, &repo)?;
        let status = match status {
            Some(active @ RepositoryStatus::Indexing { .. }) => Some(active),
            status => unreadable.or(status),
        };
        let doc = read.ok().flatten();
        entries.push(CatalogueEntry {
            slug: format!("personal/{}", repo.id),
            label: Some(repo.name.clone()),
            owner: "personal".into(),
            repo: repo.id.clone(),
            file: String::new(),
            files: doc.as_ref().map_or(0, |d| d.files.len() as i64),
            districts: doc.as_ref().map_or(0, |d| d.districts.len() as i64),
            modularity: doc.as_ref().map_or(0., |d| d.q),
            lang: doc.map_or_else(String::new, |d| d.lang),
            source: CatalogueSource::Service,
            commit: latest.map(|s| s.commit.clone()),
            repository_id: Some(repo.id.clone()),
            latest_snapshot_id: latest.map(|s| s.id.clone()),
            indexed_at: latest.map(|s| s.indexed_at.clone()),
            snapshot_count: Some(snapshots.len() as u64),
            status,
        });
    }
    entries.sort_by(|a, b| {
        b.indexed_at
            .cmp(&a.indexed_at)
            .then_with(|| a.repository_id.cmp(&b.repository_id))
    });
    Ok(Json(entries))
}
async fn history(
    State(state): State<Arc<WebState>>,
    Path(id): Path<String>,
) -> Result<Json<RepositoryHistory>, ApiError> {
    let repo = state.repository(&id)?;
    let history = state.library.store.personal_history(&id)?;
    let mut unreadable = false;
    let reopen = history
        .first()
        .filter(|s| document(s).is_ok())
        .map(|s| action(map_url(s), ActionMethod::Get));
    let snapshots = history
        .iter()
        .map(|s| {
            let doc = document(s).ok();
            unreadable |= doc.is_none();
            Ok(RepositorySnapshot {
                id: s.id.clone(),
                commit: s.commit.clone(),
                indexed_at: s.indexed_at.clone(),
                files: doc.as_ref().map_or(0, |d| d.files.len() as i64),
                districts: doc.as_ref().map_or(0, |d| d.districts.len() as i64),
                core_revision: None,
                reopen: doc.map(|_| action(map_url(s), ActionMethod::Get)),
                delete_snapshot: None,
            })
        })
        .collect::<Result<_, ApiError>>()?;
    let status = repository_status(&state, &repo)?;
    let status = match status {
        Some(active @ RepositoryStatus::Indexing { .. }) => Some(active),
        _status if unreadable => Some(RepositoryStatus::Failed {
            message: "One or more saved maps are unreadable; other snapshots remain available."
                .into(),
        }),
        status => status,
    };
    Ok(Json(RepositoryHistory {
        repository_id: id.clone(),
        name: repo.name.clone(),
        status,
        current_core_revision: None,
        actions: RepositoryActions {
            reopen,
            reindex: Some(action(
                format!("/api/repositories/{id}/refresh"),
                ActionMethod::Post,
            )),
            delete_repository: Some(action(
                format!("/api/repositories/{id}"),
                ActionMethod::Delete,
            )),
            disconnect: None,
        },
        snapshots,
    }))
}
async fn remove(
    State(state): State<Arc<WebState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let repo = state.repository(&id)?;
    if state
        .library
        .store
        .personal_repository_jobs(&id)?
        .iter()
        .any(|job| state.controls.lock().unwrap().contains_key(job))
    {
        return Err(ApiError::busy(
            "cancel mapping before removing this repository",
        ));
    }
    tokio::task::spawn_blocking(move || state.library.remove(&repo))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(invalid)?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
struct ScanQuery {
    path: String,
}
async fn scan(
    State(state): State<Arc<WebState>>,
    ApiQuery(query): ApiQuery<ScanQuery>,
) -> Result<Json<Vec<OfferedRepository>>, ApiError> {
    tokio::task::spawn_blocking(move || {
        let repos = state.library.store.repositories()?;
        let roots = crate::personal::scan(std::path::Path::new(&query.path)).map_err(invalid)?;
        Ok(Json(
            roots
                .into_iter()
                .map(|p| {
                    let location = p.to_string_lossy().into_owned();
                    OfferedRepository {
                        provider_id: location.clone(),
                        full_name: location.clone(),
                        visibility: RepositoryVisibility::Public,
                        connected_repository_id: repos
                            .iter()
                            .find(|r| r.kind == "local" && r.location == location)
                            .map(|r| r.id.clone()),
                    }
                })
                .collect(),
        ))
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
}

fn initial_job(id: Uuid, slug: String) -> JobSnapshot {
    JobSnapshot {
        job_id: id,
        slug,
        commit: None,
        status: JobStatus::Queued,
        stage: "Waiting to index committed source".into(),
        queue_position: None,
        started_at: super::time::now_rfc3339(),
        finished_at: None,
        error: None,
        error_code: None,
        progress: None,
        eta: None,
        eta_start_s: None,
        elapsed_s: 0.,
        stages: crate::progress::StageId::ALL
            .into_iter()
            .map(|stage| super::jobs::StageSnapshot {
                id: stage,
                label: stage.label().into(),
                state: super::jobs::StageState::Pending,
                started_at: None,
                duration_s: None,
            })
            .collect(),
        map_ready: false,
    }
}
fn start(state: Arc<WebState>, repo: Repository, batch: &str) -> Result<String, ApiError> {
    let mut controls = state.controls.lock().unwrap();
    let live = controls.len();
    let matching = state.library.store.personal_repository_jobs(&repo.id)?;
    if matching.iter().any(|id| controls.contains_key(id)) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "already_indexing",
            "repository is already indexing",
        ));
    }
    let limits = &state.library.config.service.limits;
    if live >= limits.max_concurrent_jobs + limits.max_queued_jobs {
        return Err(ApiError::busy("mapping queue is full"));
    }
    let id = Uuid::new_v4();
    let snapshot = Arc::new(Mutex::new(initial_job(id, repo.name.clone())));
    state.library.store.insert_job(&JobRow {
        job_id: id.to_string(),
        slug: format!("personal-web/{batch}"),
        commit: String::new(),
        spec_json: serde_json::json!({"repository_id":repo.id}).to_string(),
        class: 0,
        status: "running".into(),
        attempt: 0,
        epoch: 0,
        lease_holder: None,
        lease_deadline_ms: None,
        queue_order: 0,
        snapshot_json: serde_json::to_string(&*snapshot.lock().unwrap()).unwrap(),
    })?;
    let progress_state = Arc::downgrade(&state);
    let progress_snapshot = snapshot.clone();
    let control = Arc::new(PersonalControl::new(move |event| {
        let Some(progress_state) = progress_state.upgrade() else {
            return;
        };
        let mut snapshot = progress_snapshot.lock().unwrap();
        snapshot.status = JobStatus::Indexing;
        match event {
            crate::worker::WorkerEvent::StageStarted { stage, .. } => {
                snapshot.stage = stage.label().into();
                if let Some(row) = snapshot.stages.iter_mut().find(|s| s.id == *stage) {
                    row.state = super::jobs::StageState::Running;
                }
            }
            crate::worker::WorkerEvent::StageFinished {
                stage,
                duration_s,
                success,
                ..
            } => {
                if let Some(row) = snapshot.stages.iter_mut().find(|s| s.id == *stage) {
                    row.state = if *success {
                        super::jobs::StageState::Done
                    } else {
                        super::jobs::StageState::Failed
                    };
                    row.duration_s = Some(*duration_s);
                }
            }
            crate::worker::WorkerEvent::Progress { value, .. } => {
                snapshot.stage = value.label.clone();
                snapshot.progress = Some(value.clone());
            }
            _ => return,
        }
        if let Err(e) = progress_state.library.store.save_job_state(
            &id.to_string(),
            0,
            "running",
            None,
            None,
            &serde_json::to_string(&*snapshot).unwrap(),
        ) {
            eprintln!("save personal progress: {e}");
        }
    }));
    controls.insert(id.to_string(), control.clone());
    drop(controls);
    tokio::spawn(async move {
        let permit = state
            .slots
            .clone()
            .acquire_owned()
            .await
            .expect("web slots remain open");
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let result = (|| {
                anyhow::ensure!(!control.cancelled(), "cancelled");
                state.library.refresh_controlled(
                    &repo,
                    None,
                    &SourceArgs::default(),
                    Some(&control),
                )
            })();
            let mut snapshot = snapshot.lock().unwrap();
            match result {
                Ok(saved) => {
                    snapshot.status = JobStatus::Done;
                    snapshot.slug = format!("snapshot/{}", saved.id);
                    snapshot.commit = Some(saved.commit);
                }
                Err(e) => {
                    snapshot.status = JobStatus::Failed;
                    snapshot.error = Some(format!("{e:#}"));
                    snapshot.error_code = Some(
                        if control.cancelled() {
                            "cancelled"
                        } else {
                            "index_failed"
                        }
                        .into(),
                    );
                }
            }
            snapshot.finished_at = Some(super::time::now_rfc3339());
            if let Err(e) = state.library.store.finish_job(
                &id.to_string(),
                if snapshot.status == JobStatus::Done {
                    "done"
                } else {
                    "failed"
                },
                &serde_json::to_string(&*snapshot).unwrap(),
            ) {
                eprintln!("finish personal job: {e}");
            }
            state.controls.lock().unwrap().remove(&id.to_string());
        })
        .await
        .unwrap_or_else(|e| eprintln!("personal job stopped: {e}"));
    });
    Ok(id.to_string())
}
async fn add(
    State(state): State<Arc<WebState>>,
    Json(request): Json<BatchIndexRequest>,
) -> Result<(StatusCode, Json<BatchIndexResponse>), ApiError> {
    if request.provider_ids.is_empty() {
        return Err(ApiError::invalid_request("select at least one source"));
    }
    let limits = &state.library.config.service.limits;
    let validation_state = state.clone();
    let inputs = request.provider_ids.clone();
    let sources = tokio::task::spawn_blocking(move || {
        let mut sources = std::collections::HashSet::new();
        for input in inputs {
            sources.insert(validation_state.library.validate_source(&input)?);
        }
        Ok::<_, anyhow::Error>(sources)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
    .map_err(invalid)?;
    if state.controls.lock().unwrap().len() + sources.len()
        > limits.max_concurrent_jobs + limits.max_queued_jobs
    {
        return Err(ApiError::busy("mapping queue is full"));
    }
    let batch_id = Uuid::new_v4().to_string();
    let mut jobs = Vec::new();
    let mut sources = std::collections::HashSet::new();
    let mut registrations = Vec::new();
    for source in request.provider_ids {
        if !sources.insert(source.clone()) {
            continue;
        }
        let registration_state = state.clone();
        let repo =
            tokio::task::spawn_blocking(move || registration_state.library.register(&source, None))
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?
                .map_err(invalid)?;
        if !registrations
            .iter()
            .any(|existing: &Repository| existing.id == repo.id)
        {
            registrations.push(repo);
        }
    }
    for repo in &registrations {
        if state
            .library
            .store
            .personal_repository_jobs(&repo.id)?
            .iter()
            .any(|id| state.controls.lock().unwrap().contains_key(id))
        {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "already_indexing",
                "a selected repository is already indexing",
            ));
        }
    }
    if state.controls.lock().unwrap().len() + registrations.len()
        > limits.max_concurrent_jobs + limits.max_queued_jobs
    {
        return Err(ApiError::busy("mapping queue is full"));
    }
    for repo in registrations {
        let repository_id = repo.id.clone();
        let id = start(state.clone(), repo, &batch_id)?;
        jobs.push(BatchIndexJob {
            repository_id,
            job_id: id,
        });
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(BatchIndexResponse { batch_id, jobs }),
    ))
}
async fn refresh(
    State(state): State<Arc<WebState>>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let repo = state.repository(&id)?;
    let job_id = start(state, repo, &Uuid::new_v4().to_string())?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"job_id":job_id,"status":"queued"})),
    ))
}
fn read_job(state: &WebState, id: &str) -> Result<JobSnapshot, ApiError> {
    let row = state
        .library
        .store
        .job(id)?
        .filter(|r| r.slug.starts_with("personal-web/"))
        .ok_or_else(|| ApiError::not_found("job unavailable"))?;
    let mut job: JobSnapshot =
        serde_json::from_str(&row.snapshot_json).map_err(|e| ApiError::internal(e.to_string()))?;
    if !matches!(job.status, JobStatus::Done | JobStatus::Failed)
        && !state.controls.lock().unwrap().contains_key(id)
    {
        job.status = JobStatus::Failed;
        job.error = Some("Web process stopped; retry from the repository page.".into());
        job.error_code = Some("server_stopping".into());
    }
    Ok(job)
}
async fn job(
    State(state): State<Arc<WebState>>,
    Path(id): Path<String>,
) -> Result<Json<JobSnapshot>, ApiError> {
    Ok(Json(read_job(&state, &id)?))
}
async fn cancel(
    State(state): State<Arc<WebState>>,
    Path(id): Path<String>,
) -> Result<Json<JobSnapshot>, ApiError> {
    if let Some(control) = state.controls.lock().unwrap().get(&id) {
        control.stop();
    }
    Ok(Json(read_job(&state, &id)?))
}
async fn retry(
    State(state): State<Arc<WebState>>,
    Path(id): Path<String>,
) -> Result<Json<BatchIndexResponse>, ApiError> {
    read_job(&state, &id)?;
    let row = state
        .library
        .store
        .job(&id)?
        .ok_or_else(|| ApiError::not_found("job unavailable"))?;
    let spec: serde_json::Value =
        serde_json::from_str(&row.spec_json).map_err(|e| ApiError::internal(e.to_string()))?;
    let repo = state.repository(
        spec["repository_id"]
            .as_str()
            .ok_or_else(|| ApiError::not_found("repository unavailable"))?,
    )?;
    let repository_id = repo.id.clone();
    let batch_id = Uuid::new_v4().to_string();
    let job_id = start(state, repo, &batch_id)?;
    Ok(Json(BatchIndexResponse {
        batch_id,
        jobs: vec![BatchIndexJob {
            repository_id,
            job_id,
        }],
    }))
}
async fn batch(
    State(state): State<Arc<WebState>>,
    Path(id): Path<String>,
) -> Result<Json<BatchProgress>, ApiError> {
    let jobs: Vec<JobSnapshot> = state
        .library
        .store
        .personal_web_jobs(&format!("personal-web/{id}"))?
        .iter()
        .map(|id| read_job(&state, id))
        .collect::<Result<_, _>>()?;
    if jobs.is_empty() {
        return Err(ApiError::not_found("batch unavailable"));
    }
    Ok(Json(BatchProgress { batch_id: id, jobs }))
}
#[derive(Default, Deserialize)]
struct MapQuery {
    commit: Option<String>,
    path: Option<String>,
    district: Option<usize>,
}
fn resolve_snapshot(
    state: &WebState,
    owner: &str,
    id: &str,
    commit: Option<&str>,
) -> Result<PersonalSnapshot, ApiError> {
    let snapshot = if owner == "snapshot" {
        state
            .library
            .store
            .personal_snapshot(id)?
            .ok_or_else(|| ApiError::not_found("snapshot unavailable"))?
    } else if owner == "personal" {
        let repo = state.repository(id)?;
        state
            .library
            .snapshot(&repo, commit, None)
            .map_err(invalid)?
    } else {
        return Err(ApiError::not_found("personal map unavailable"));
    };
    if commit.is_some_and(|c| c != snapshot.commit) {
        return Err(ApiError::not_found("snapshot commit mismatch"));
    }
    Ok(snapshot)
}
/// The retained snapshot of the repository behind `{owner}/{id}` at exactly
/// `commit`. No newer or nearest snapshot is substituted.
fn snapshot_at(
    state: &WebState,
    owner: &str,
    id: &str,
    commit: &str,
) -> Result<PersonalSnapshot, ApiError> {
    if !crate::source::valid_commit(commit) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "comparison_input",
            "base and head must be full commit IDs",
        ));
    }
    let repository = match owner {
        "snapshot" => {
            state
                .library
                .store
                .personal_snapshot(id)?
                .ok_or_else(|| ApiError::not_found("snapshot unavailable"))?
                .repository
        }
        "personal" => id.to_owned(),
        _ => return Err(ApiError::not_found("personal map unavailable")),
    };
    state
        .library
        .store
        .personal_history(&repository)?
        .into_iter()
        .find(|s| s.commit == commit)
        .ok_or_else(crate::comparison::unavailable)
}
/// Each snapshot retains a full clone only up to its own commit, so a base
/// snapshot's source lacks later heads. The newest snapshot's source holds
/// the history every comparison resolves against.
fn latest_source(state: &WebState, repository: &str) -> Result<PathBuf, ApiError> {
    Ok(state
        .library
        .store
        .personal_history(repository)?
        .into_iter()
        .next()
        .ok_or_else(crate::comparison::unavailable)?
        .source_path)
}
async fn compare(
    State(state): State<Arc<WebState>>,
    Path((owner, id)): Path<(String, String)>,
    ApiQuery(query): ApiQuery<crate::comparison::CompareQuery>,
) -> Result<Response, ApiError> {
    let base = snapshot_at(&state, &owner, &id, &query.base)?;
    let source = latest_source(&state, &base.repository)?;
    let cache = state.library.config.service.cache_dir.join("comparisons");
    let repository = format!("{owner}/{id}");
    let value = tokio::task::spawn_blocking(move || {
        crate::comparison::run(
            crate::comparison::Inputs {
                repository: &repository,
                repo: &source,
                base_map: &base.map_path,
                cache: &cache,
            },
            query,
        )
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))??;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response())
}
async fn compare_lenses(
    State(state): State<Arc<WebState>>,
    Path((owner, id)): Path<(String, String)>,
    ApiQuery(query): ApiQuery<super::lens::LensCompareQuery>,
) -> Result<Response, ApiError> {
    let base = snapshot_at(&state, &owner, &id, &query.base)?;
    let head = snapshot_at(&state, &owner, &id, &query.head)?;
    let source = latest_source(&state, &base.repository)?;
    let repository = format!("{owner}/{id}");
    let value = tokio::task::spawn_blocking(move || {
        super::lens::compare_at(repository, source, base.map_path, head.map_path, query)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))??;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response())
}
#[derive(Deserialize)]
struct DebtCompareQuery {
    base: String,
    head: String,
}
async fn compare_debt(
    State(state): State<Arc<WebState>>,
    Path((owner, id)): Path<(String, String)>,
    ApiQuery(query): ApiQuery<DebtCompareQuery>,
) -> Result<Response, ApiError> {
    let base = snapshot_at(&state, &owner, &id, &query.base)?;
    let head = snapshot_at(&state, &owner, &id, &query.head)?;
    let source = latest_source(&state, &base.repository)?;
    let repository = format!("{owner}/{id}");
    let value = tokio::task::spawn_blocking(move || {
        super::debt::compare_debt_at(
            repository,
            source,
            base.map_path,
            head.map_path,
            query.base,
            query.head,
        )
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))??;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response())
}
async fn map(
    State(state): State<Arc<WebState>>,
    Path((owner, id)): Path<(String, String)>,
    ApiQuery(query): ApiQuery<MapQuery>,
) -> Result<Response, ApiError> {
    let snapshot = resolve_snapshot(&state, &owner, &id, query.commit.as_deref())?;
    artifact(snapshot.map_path, Some(&snapshot.commit)).await
}
async fn artifact(path: PathBuf, commit: Option<&str>) -> Result<Response, ApiError> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| ApiError::not_found(e.to_string()))?;
    // A map's identity for stale-comparison checks. Browsers expose
    // crypto.subtle only in secure contexts, so plain-HTTP remote viewers
    // cannot hash the body themselves.
    let digest = commit.map(|_| format!("{:x}", sha2::Sha256::digest(&bytes)));
    let mut response = (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response();
    if let (Some(commit), Some(digest)) = (commit, digest) {
        let headers = response.headers_mut();
        headers.insert(
            "x-tolmap-commit",
            commit
                .parse()
                .map_err(|_| ApiError::internal("invalid stored commit"))?,
        );
        headers.insert(
            "x-tolmap-map-sha256",
            digest.parse().expect("hex digest is a valid header value"),
        );
    }
    Ok(response)
}
async fn map_detail(
    State(state): State<Arc<WebState>>,
    Path((owner, id, rest)): Path<(String, String, String)>,
    ApiQuery(query): ApiQuery<MapQuery>,
) -> Result<Response, ApiError> {
    let snapshot = resolve_snapshot(&state, &owner, &id, query.commit.as_deref())?;
    let repository = format!("{owner}/{id}");
    match rest.as_str() {
        "symbols" => artifact(snapshot.map_path.with_file_name("symbols").join(format!("{}.json", query.district.ok_or_else(|| ApiError::invalid_request("district required"))?)), None).await,
        "provenance" => Ok(Json(serde_json::json!({"version":crate::source::VERSION,"repository":repository,"commit":snapshot.commit})).into_response()),
        "source" => {
            let path = query.path.ok_or_else(|| ApiError::invalid_request("path required"))?;
            if query.commit.is_none() { return Err(ApiError::invalid_request("immutable commit required")); }
            let value = tokio::task::spawn_blocking(move || crate::source::read(&snapshot.source_path, &repository, &snapshot.commit, &path)).await.map_err(|e| ApiError::internal(e.to_string()))?.map_err(invalid)?;
            Ok(Json(value).into_response())
        },
        "debt" => Ok(Json(super::debt::debt_at(repository, snapshot.commit, snapshot.map_path)?).into_response()),
        "lenses" => Ok(Json(super::lens::list_at(repository, snapshot.source_path, snapshot.commit)?).into_response()),
        "freshness" => {
            let repo = state.repository(&snapshot.repository)?;
            let status = state.library.status(&repo).map_err(invalid)?;
            Ok(Json(serde_json::json!({"indexed_commit":snapshot.commit,"requested_commit":status["checkout_commit"],"state": if status["attempt"][0] == "running" {"indexing"} else if status["attempt"][0] == "failed" {"failed"} else if status["checkout_commit"] == snapshot.commit {"complete"} else {"stale"},"failure_reason":status["attempt"][1],"failure_code":null,"retryable":status["attempt"][0] == "failed","job_id":null})).into_response())
        },
        _ if rest.starts_with("lenses/") => Ok(Json(super::lens::resolve_at(repository, snapshot.source_path, snapshot.commit, snapshot.map_path, rest[7..].into())?).into_response()),
        _ => Err(ApiError::not_found("map detail unavailable")),
    }
}

pub async fn serve(
    data: Option<&std::path::Path>,
    addr: Option<std::net::SocketAddr>,
) -> anyhow::Result<()> {
    let mut library = Library::open(data)?;
    if let Some(addr) = addr {
        library.config.service.bind = addr;
    }
    if library.config.service.static_dir.is_none() {
        library.config.service.static_dir = Some(PathBuf::from("web/dist"));
    }
    anyhow::ensure!(
        library
            .config
            .service
            .static_dir
            .as_ref()
            .unwrap()
            .join("index.html")
            .is_file(),
        "web bundle missing; build web/ and set TOLMAP_STATIC_DIR to its dist directory"
    );
    let bind = library.config.service.bind;
    let static_dir = library.config.service.static_dir.clone();
    let state = WebState::new(library);
    let app = router(static_dir.as_ref(), state.clone());
    let listener = tokio::net::TcpListener::bind(bind).await?;
    if !bind.ip().is_loopback() {
        eprintln!("No authentication: anyone who can reach this address can read source and manage repositories.");
    }
    eprintln!("tolmap web: listening on http://{}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            super::shutdown_signal().await;
            for control in state.controls.lock().unwrap().values() {
                control.stop();
            }
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{application_config::ApplicationConfig, config::ServeConfig, store::Store};
    use super::*;
    #[tokio::test]
    async fn queued_cancel_publishes_no_snapshot_and_duplicate_start_is_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let config = ServeConfig {
            db_path: dir.path().join("db"),
            ..ServeConfig::from_env()
        };
        let library = Library {
            store: Store::open(&config.db_path).unwrap(),
            config: ApplicationConfig {
                data: dir.path().into(),
                service: config,
                retain_commits_per_repo: None,
            },
        };
        let repo = Repository {
            id: "queued".into(),
            name: "queued".into(),
            kind: "local".into(),
            location: "/missing".into(),
            observed_head: None,
            observed_at: None,
        };
        library.store.register(&repo).unwrap();
        let state = WebState::new(library);
        let permit = state
            .slots
            .clone()
            .acquire_many_owned(state.library.config.service.limits.max_concurrent_jobs as u32)
            .await
            .unwrap();
        let id = start(state.clone(), repo.clone(), "batch").unwrap();
        use tower::ServiceExt;
        let app = router(None, state.clone());
        let duplicate = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/repositories/queued/refresh")
                    .header("host", "localhost:8787")
                    .header("content-type", "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);
        cancel(State(state.clone()), Path(id.clone()))
            .await
            .unwrap();
        drop(permit);
        let job = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let job = read_job(&state, &id).unwrap();
                if job.status == JobStatus::Failed {
                    break job;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(job.error_code.as_deref(), Some("cancelled"));
        assert!(state
            .library
            .store
            .personal_history("queued")
            .unwrap()
            .is_empty());
        assert!(state
            .library
            .store
            .personal_attempt("queued")
            .unwrap()
            .is_none());
    }
}
