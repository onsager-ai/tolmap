//! The built-in debt lens (issue #250): `GET /api/maps/{owner}/{repo}/debt`
//! returns the debt lens of a stored map as a `LensResponse`, and
//! `GET /api/compare/{owner}/{repo}/debt` compares it between two indexed
//! commits as a `LensComparison`. Both are computed from stored maps alone
//! (`crate::debt`); nothing is read from `.tolmap/lenses/`, so no repository
//! lens name is reserved.
use super::{
    error::ApiError,
    lens::{blocking, renames, snapshot, LensQuery},
    AppState,
};
use crate::{
    comparison, debt, lens::LensResponse, lens_comparison::LensComparison, schema::MapDocument,
};
use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};

pub async fn get_debt(
    State(state): State<Arc<AppState>>,
    Path((owner, repo)): Path<(String, String)>,
    Query(query): Query<LensQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (repository, _, row) = snapshot(&state, &owner, &repo, query.commit)?;
    let response = blocking(move || debt_at(repository, row.commit, row.map_path)).await?;
    Ok(([("cache-control", "no-store")], Json(response)))
}

#[derive(Clone, Deserialize)]
pub struct DebtCompareQuery {
    pub base: String,
    pub head: String,
}

pub async fn compare_debt(
    State(state): State<Arc<AppState>>,
    Path((owner, repo)): Path<(String, String)>,
    Query(query): Query<DebtCompareQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (repository, source, row) = snapshot(&state, &owner, &repo, Some(query.base.clone()))?;
    let (_, _, head) = snapshot(&state, &owner, &repo, Some(query.head.clone()))?;
    let value = blocking(move || {
        compare_debt_at(
            repository,
            source,
            row.map_path,
            head.map_path,
            query.base,
            query.head,
        )
    })
    .await?;
    Ok(([("cache-control", "no-store")], Json(value)))
}

fn read_map(path: &std::path::Path) -> Result<(Vec<u8>, MapDocument), ApiError> {
    let bytes = std::fs::read(path).map_err(|_| comparison::unavailable())?;
    let map = serde_json::from_slice(&bytes)
        .map_err(|e| ApiError::internal(format!("parse {}: {e}", path.display())))?;
    Ok((bytes, map))
}

// Shared by hosted and local snapshots.
pub(crate) fn debt_at(
    repository: String,
    commit: String,
    map_path: PathBuf,
) -> Result<LensResponse, ApiError> {
    let (_, map) = read_map(&map_path)?;
    Ok(LensResponse {
        repository,
        commit,
        name: debt::LENS_NAME.into(),
        report: debt::lens_report(&map),
    })
}

pub(crate) fn compare_debt_at(
    repository: String,
    repo: PathBuf,
    map_path: PathBuf,
    head_map_path: PathBuf,
    base: String,
    head: String,
) -> Result<LensComparison, ApiError> {
    use sha2::{Digest, Sha256};
    comparison::resolve(&repo, &base)?;
    comparison::resolve(&repo, &head)?;
    let (bytes, frame) = read_map(&map_path)?;
    let renames = renames(&repo, &base, &head)?;
    let (head_map, head_frame) = if base == head {
        (frame.clone(), frame.clone())
    } else {
        let (_, snapshot) = read_map(&head_map_path)?;
        let projected = crate::check::lens_head_map(&repo, &base, &head, &frame, &snapshot)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        (snapshot, projected)
    };
    let (before, after, changes) = debt::compare(&frame, &head_map, &head_frame, &renames);
    Ok(LensComparison {
        version: 1,
        repository,
        base,
        head,
        lens: debt::LENS_NAME.into(),
        other: debt::LENS_NAME.into(),
        base_map_sha256: format!("{:x}", Sha256::digest(bytes)),
        lower_bound: true,
        before,
        after,
        changes,
    })
}
