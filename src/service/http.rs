//! The axum router and handlers -- see docs/API.md for the contract this
//! file implements. Kept as one file: the handlers are short, and the
//! interesting logic (clone, detect, index, store) lives in the modules
//! they call into, not here.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Path as AxPath, Query, State};
use axum::http::{header, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::ReceiverStream;
use tower_http::services::{ServeDir, ServeFile};
use uuid::Uuid;

use super::clone;
use super::error::ApiError;
use super::jobs::{self, JobSnapshot, JobStatus};
use super::ratelimit::Verdict;
use super::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    let mut router = Router::new()
        .route("/api/index", post(post_index))
        .route("/api/jobs/{job_id}", get(get_job))
        .route("/api/jobs/{job_id}/events", get(get_job_events))
        .route("/api/maps", get(get_maps))
        .route("/api/maps/{owner}/{repo}", get(get_map))
        .route("/api/maps/{owner}/{repo}/symbols", get(get_symbols))
        .route("/api/healthz", get(get_healthz));

    // Opt-in (TOLMAP_STATIC_DIR unset keeps this identical to before it
    // existed): serve the built web bundle for one-origin deployment
    // (Fly.io, 2026-09-20), with an SPA fallback to `index.html` so
    // TanStack Router can own client-side paths like `/django/django`
    // that have no file on disk -- docs/ARCHITECTURE.md's "MVP: a site
    // that maps any public repository".
    //
    // Two services, not one `fallback_service(serve_dir)`: a plain
    // ServeDir-with-not-found-service fallback would also catch an
    // unmatched `/api/*` path (nothing above matches `/api/nonexistent`)
    // and serve it `index.html` -- turning a contract error into a JSON
    // parse error in the client, exactly what docs/API.md's "every non-2xx
    // response is JSON" promises callers never happens. The catch-all
    // route registered here wins over the outer fallback for every path
    // under `/api/` (axum/matchit prefer a literal route -- `/api/index`,
    // `/api/healthz`, etc. -- over a same-router catch-all, so this only
    // ever catches what nothing above already matched), so `/api/*` always
    // gets `ApiError::not_found`, never the SPA fallback. See
    // tests/static_serving.rs.
    if let Some(static_dir) = &state.config.static_dir {
        // `.fallback(ServeFile::new(index_html))`, not tower-http's
        // `not_found_service` -- that helper (`SetStatus`) forces the
        // response status to 404 even when it successfully serves
        // `index.html`'s body, which is right for "serve a custom 404
        // page" and wrong here: a cold load of a client-routed path like
        // `/django/django` is a real page, not a broken link, and must
        // answer 200 the way `GET /` does. `.fallback()` alone keeps
        // whatever status `ServeFile` naturally returns for a successful
        // read, which is 200. See tests/static_serving.rs.
        let index_html = static_dir.join("index.html");
        let serve_dir = ServeDir::new(static_dir).fallback(ServeFile::new(index_html));
        router = router
            .route("/api/{*rest}", any(api_not_found))
            .fallback_service(serve_dir);
    }

    router.with_state(state)
}

/// Catch-all for any `/api/*` path none of the explicit routes above
/// matched -- registered only when static serving is on, so `/api/*`
/// never falls through to the SPA's `index.html` fallback (see `router`'s
/// doc comment). With static serving off there is no outer fallback for
/// this to protect against, so it is not registered at all: the router
/// must stay byte-for-byte what it was when `TOLMAP_STATIC_DIR` is unset.
async fn api_not_found(uri: axum::http::Uri) -> ApiError {
    ApiError::not_found(format!("no route for {}", uri.path()))
}

// ---- POST /api/index ----------------------------------------------------

#[derive(Debug, Deserialize)]
struct IndexRequest {
    repo: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Serialize)]
struct QueuedResponse {
    job_id: Uuid,
    slug: String,
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct CachedResponse {
    job_id: Option<Uuid>,
    slug: String,
    status: &'static str,
    commit: String,
}

async fn post_index(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    body: Result<Json<IndexRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    let limits = &state.config.limits;
    if let Verdict::Denied { message } = state.rate_limiter.check_ip(
        addr.ip(),
        limits.rate_limit_per_ip,
        Duration::from_secs(limits.rate_limit_window_seconds),
    ) {
        return Err(ApiError::rate_limited(message));
    }

    let Json(body) = body.map_err(|err| ApiError::invalid_request(err.to_string()))?;
    let repo_ref = clone::resolve(body.repo.as_deref(), body.path.as_deref())?;

    if let Verdict::Denied { message } = state.rate_limiter.check_repo(
        &repo_ref.slug,
        limits.rate_limit_per_repo,
        Duration::from_secs(limits.rate_limit_per_repo_window_seconds),
    ) {
        return Err(ApiError::rate_limited(message));
    }

    // Cheap HEAD resolution (git ls-remote / rev-parse -- see clone.rs)
    // before committing to a job, so a repeat request for an
    // already-indexed commit is answered in this same request rather than
    // always queuing. Runs on a blocking thread since it shells out to git.
    let source = repo_ref.source.clone();
    let head = tokio::task::spawn_blocking(move || clone::resolve_head(&source))
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?
        .map_err(|err| ApiError::clone_failed(err.to_string()))?;

    if let Some(row) = state.store.get(&repo_ref.slug, &head)? {
        return Ok((
            StatusCode::OK,
            Json(CachedResponse {
                job_id: None,
                slug: row.slug,
                status: "done",
                commit: row.commit,
            }),
        )
            .into_response());
    }

    let job_id = jobs::spawn_job(state.clone(), repo_ref.clone(), head)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(QueuedResponse {
            job_id,
            slug: repo_ref.slug,
            status: "queued",
        }),
    )
        .into_response())
}

// ---- GET /api/jobs/{job_id} ----------------------------------------------

async fn get_job(
    State(state): State<Arc<AppState>>,
    AxPath(job_id): AxPath<Uuid>,
) -> Result<Json<JobSnapshot>, ApiError> {
    let snapshot = {
        let rx = state
            .jobs
            .subscribe(job_id)
            .ok_or_else(|| ApiError::not_found(format!("no job {job_id}")))?;
        let current = rx.borrow().clone();
        current
    };
    Ok(Json(snapshot))
}

// ---- GET /api/jobs/{job_id}/events ---------------------------------------

async fn get_job_events(
    State(state): State<Arc<AppState>>,
    AxPath(job_id): AxPath<Uuid>,
) -> Result<Sse<ReceiverStream<Result<Event, Infallible>>>, ApiError> {
    let mut rx = state
        .jobs
        .subscribe(job_id)
        .ok_or_else(|| ApiError::not_found(format!("no job {job_id}")))?;

    // A relay task, not the watch::Receiver wrapped directly: this is what
    // lets the stream emit the *current* value immediately on connect and
    // then close itself (dropping `out_tx`, which ends the SSE response)
    // right after the frame carrying a terminal status, per docs/API.md
    // ("the stream ends ... after the frame carrying status: done or
    // failed"). `WatchStream` alone has no such stopping rule.
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(8);
    tokio::spawn(async move {
        loop {
            let snapshot = rx.borrow_and_update().clone();
            let terminal = matches!(snapshot.status, JobStatus::Done | JobStatus::Failed);
            let event = match Event::default().json_data(&snapshot) {
                Ok(event) => event,
                Err(_) => Event::default().data("{\"error\":\"internal_error\"}"),
            };
            if out_tx.send(Ok(event)).await.is_err() {
                return; // client went away
            }
            if terminal {
                return;
            }
            if rx.changed().await.is_err() {
                return; // sender (the job) dropped without a terminal frame -- should not happen
            }
        }
    });

    Ok(Sse::new(ReceiverStream::new(out_rx)))
}

// ---- GET /api/maps --------------------------------------------------------

#[derive(Debug, Serialize)]
struct MapsListEntry {
    slug: String,
    owner: String,
    repo: String,
    lang: String,
    files: i64,
    districts: i64,
    modularity: f64,
    commit: String,
    indexed_at: String,
}

async fn get_maps(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<MapsListEntry>>, ApiError> {
    let rows = state.store.list_latest()?;
    Ok(Json(
        rows.into_iter()
            .map(|row| MapsListEntry {
                slug: row.slug,
                owner: row.owner,
                repo: row.repo,
                lang: row.lang,
                files: row.files,
                districts: row.districts,
                modularity: row.modularity,
                commit: row.commit,
                indexed_at: row.indexed_at,
            })
            .collect(),
    ))
}

// ---- GET /api/maps/{owner}/{repo} -----------------------------------------

#[derive(Debug, Deserialize)]
struct MapQuery {
    commit: Option<String>,
}

async fn get_map(
    State(state): State<Arc<AppState>>,
    AxPath((owner, repo)): AxPath<(String, String)>,
    Query(query): Query<MapQuery>,
) -> Result<Response, ApiError> {
    // Canonicalise the same way `clone::resolve` does (issue #23 gap 3), so
    // `GET /api/maps/Owner/Repo` finds the row `POST /api/index {"repo":
    // "owner/repo"}` created -- the store's key is always lowercase.
    let slug = format!(
        "{}/{}",
        clone::canonicalize(&owner),
        clone::canonicalize(&repo)
    );
    let row = match &query.commit {
        Some(commit) => state.store.get(&slug, commit)?,
        None => state.store.latest(&slug)?,
    };
    let row = row.ok_or_else(|| {
        ApiError::not_found(match &query.commit {
            Some(commit) => format!("{slug} has no indexed map at commit {commit}"),
            None => format!("{slug} has not been indexed"),
        })
    })?;
    let bytes = tokio::fs::read(&row.map_path)
        .await
        .map_err(|err| ApiError::internal(format!("read {}: {err}", row.map_path.display())))?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        bytes,
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
struct SymbolsQuery {
    commit: Option<String>,
    district: usize,
}

async fn get_symbols(
    State(state): State<Arc<AppState>>,
    AxPath((owner, repo)): AxPath<(String, String)>,
    Query(query): Query<SymbolsQuery>,
) -> Result<Json<crate::schema::DistrictSymbols>, ApiError> {
    let slug = format!(
        "{}/{}",
        clone::canonicalize(&owner),
        clone::canonicalize(&repo)
    );
    let row = match &query.commit {
        Some(commit) => state.store.get(&slug, commit)?,
        None => state.store.latest(&slug)?,
    }
    .ok_or_else(|| ApiError::not_found(format!("{slug} has not been indexed")))?;
    let map: crate::schema::MapDocument = super::store::read_map_document(&row.map_path)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let path = row.map_path.with_extension("symbols.json");
    let bytes = tokio::fs::read(&path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ApiError::not_found(format!("{slug} has no symbols document at this commit"))
        } else {
            ApiError::internal(format!("read {}: {e}", path.display()))
        }
    })?;
    let symbols: crate::schema::SymbolsDocument = serde_json::from_slice(&bytes)
        .map_err(|e| ApiError::internal(format!("parse {}: {e}", path.display())))?;
    let district = symbols.district(&map, query.district).ok_or_else(|| {
        ApiError::not_found(format!("district {} does not exist", query.district))
    })?;
    Ok(Json(district))
}

// ---- GET /api/healthz ------------------------------------------------------

async fn get_healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok"}))
}
