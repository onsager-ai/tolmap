//! Lens resources (issue #241). A lens lives in the analysed repository at
//! `.tolmap/lenses/<name>.toml` and is read from the same pinned commit as the
//! snapshot it is resolved against, through `source::read`'s git object path:
//! never a working tree, branch or default-branch fallback.
use super::{error::ApiError, AppState};
use crate::{
    comparison,
    lens::{self, Lens, LensList, LensResponse},
    schema::MapDocument,
    service::store::MapRow,
    source,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};

const DIR: &str = ".tolmap/lenses";

#[derive(Deserialize)]
pub struct LensQuery {
    pub commit: Option<String>,
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
}

// The indexed snapshot a lens resolves against. Without `commit` the latest
// indexed one, reported in the response so the caller can pin it.
fn snapshot(
    state: &AppState,
    owner: &str,
    repo: &str,
    commit: Option<String>,
) -> Result<(String, PathBuf, MapRow), ApiError> {
    let (repository, repo_path) = comparison::public_paths(&state.config.cache_dir, owner, repo)?;
    let row = match commit {
        Some(commit) => {
            if !source::valid_commit(&commit) {
                return Err(ApiError::invalid_request("commit must be a full commit ID"));
            }
            state.store.get(&repository, &commit)?
        }
        None => state.store.latest(&repository)?,
    };
    let row = row.ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "snapshot_not_indexed",
            format!("{repository} has no indexed map at that commit"),
        )
    })?;
    Ok((repository, repo_path, row))
}

pub async fn get_lens(
    State(state): State<Arc<AppState>>,
    Path((owner, repo, name)): Path<(String, String, String)>,
    Query(query): Query<LensQuery>,
) -> Result<impl IntoResponse, ApiError> {
    if !lens::valid_lens_name(&name) {
        return Err(ApiError::invalid_request(
            "lens name must match [a-z0-9][a-z0-9_-]{0,63}",
        ));
    }
    let (repository, repo_path, row) = snapshot(&state, &owner, &repo, query.commit)?;
    let response = blocking(move || {
        let file = format!("{DIR}/{name}.toml");
        let read = source::read(&repo_path, &repository, &row.commit, &file)
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let text = match (read.status, read.text) {
            ("ready", Some(text)) => text,
            ("missing", _) => {
                return Err(ApiError::new(
                    StatusCode::NOT_FOUND,
                    "lens_not_found",
                    format!("{file} does not exist at commit {}", row.commit),
                ))
            }
            ("expired", _) => {
                return Err(ApiError::new(
                    StatusCode::NOT_FOUND,
                    "lens_source_unavailable",
                    "the committed source is no longer retained; no other revision was read",
                ))
            }
            (status, _) => {
                return Err(ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "lens_invalid",
                    format!("{file} is not readable lens text ({status})"),
                ))
            }
        };
        let parsed = Lens::parse(&text).map_err(|e| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                match e {
                    lens::LensError::Parse(_) => "lens_parse_error",
                    lens::LensError::Invalid(_) => "lens_invalid",
                },
                e.to_string(),
            )
        })?;
        let bytes = std::fs::read(&row.map_path)
            .map_err(|e| ApiError::internal(format!("read {}: {e}", row.map_path.display())))?;
        let map: MapDocument = serde_json::from_slice(&bytes)
            .map_err(|e| ApiError::internal(format!("parse {}: {e}", row.map_path.display())))?;
        Ok(LensResponse {
            repository,
            commit: row.commit,
            name,
            report: lens::resolve(&parsed, &map),
        })
    })
    .await?;
    Ok(([("cache-control", "no-store")], Json(response)))
}

pub async fn list_lenses(
    State(state): State<Arc<AppState>>,
    Path((owner, repo)): Path<(String, String)>,
    Query(query): Query<LensQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (repository, repo_path, row) = snapshot(&state, &owner, &repo, query.commit)?;
    let response = blocking(move || {
        let commit = comparison::resolve(&repo_path, &row.commit)?;
        let out = source::git(&repo_path)
            .args(["ls-tree", "-z", &commit, "--", &format!("{DIR}/")])
            .output()
            .map_err(|e| ApiError::internal(e.to_string()))?;
        if !out.status.success() {
            return Err(ApiError::internal("git ls-tree failed"));
        }
        let listing = String::from_utf8_lossy(&out.stdout);
        let mut names: Vec<String> = listing
            .split('\0')
            .filter_map(|entry| {
                let (header, path) = entry.split_once('\t')?;
                // Regular blobs only: a symlink or submodule is not a lens.
                if !matches!(header.split(' ').next()?, "100644" | "100755") {
                    return None;
                }
                let name = path
                    .strip_prefix(DIR)?
                    .strip_prefix('/')?
                    .strip_suffix(".toml")?;
                lens::valid_lens_name(name).then(|| name.to_owned())
            })
            .collect();
        names.sort();
        Ok(LensList {
            repository,
            commit,
            names,
        })
    })
    .await?;
    Ok(([("cache-control", "no-store")], Json(response)))
}
