//! Public OSS adapter. Protected hosts use comparison::protected with their
//! trusted #222 context; this router never resolves private credentials.
use super::{error::ApiError, AppState};
use crate::{
    comparison::{self, CompareQuery, Inputs},
    source,
};
use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

pub async fn compare(
    State(state): State<Arc<AppState>>,
    Path((owner, repo)): Path<(String, String)>,
    Query(query): Query<CompareQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (repository, source) = comparison::public_paths(&state.config.cache_dir, &owner, &repo)?;
    let row = state
        .store
        .get(&repository, &query.base)?
        .ok_or_else(comparison::unavailable)?;
    let cache = state.config.cache_dir.join("comparisons");
    tokio::task::spawn_blocking(move || {
        comparison::run(
            Inputs {
                repository: &repository,
                repo: &source,
                base_map: &row.map_path,
                cache: &cache,
            },
            query,
        )
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
    .map(|c| ([("cache-control", "no-store")], Json(c)))
}
#[derive(Deserialize)]
pub struct SourceQuery {
    pub commit: String,
    pub path: Option<String>,
}
pub async fn provenance(
    State(state): State<Arc<AppState>>,
    Path((owner, repo)): Path<(String, String)>,
    Query(query): Query<SourceQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (repository, source) = comparison::public_paths(&state.config.cache_dir, &owner, &repo)?;
    let commit = tokio::task::spawn_blocking(move || comparison::resolve(&source, &query.commit))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))??;
    Ok((
        [("cache-control", "no-store")],
        Json(
            serde_json::json!({"version":source::VERSION, "repository":repository, "commit":commit}),
        ),
    ))
}
pub async fn read_source(
    State(state): State<Arc<AppState>>,
    Path((owner, repo)): Path<(String, String)>,
    Query(query): Query<SourceQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (repository, repo) = comparison::public_paths(&state.config.cache_dir, &owner, &repo)?;
    let path = query
        .path
        .ok_or_else(|| ApiError::invalid_request("path required"))?;
    let text =
        tokio::task::spawn_blocking(move || source::read(&repo, &repository, &query.commit, &path))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))??;
    Ok(([("cache-control", "no-store")], Json(text)))
}
