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
    let response =
        blocking(move || resolve_at(repository, repo_path, row.commit, row.map_path, name)).await?;
    Ok(([("cache-control", "no-store")], Json(response)))
}

pub async fn list_lenses(
    State(state): State<Arc<AppState>>,
    Path((owner, repo)): Path<(String, String)>,
    Query(query): Query<LensQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (repository, repo_path, row) = snapshot(&state, &owner, &repo, query.commit)?;
    let response = blocking(move || list_at(repository, repo_path, row.commit)).await?;
    Ok(([("cache-control", "no-store")], Json(response)))
}

// Shared by hosted and opt-in local snapshots. All reads use retained git
// objects at the snapshot commit; neither caller reads working-tree lenses.
pub(crate) fn resolve_at(
    repository: String,
    repo_path: PathBuf,
    commit: String,
    map_path: PathBuf,
    name: String,
) -> Result<LensResponse, ApiError> {
    if !lens::valid_lens_name(&name) {
        return Err(ApiError::invalid_request("invalid lens name"));
    }
    let (parsed, file) = read_lens(&repository, &repo_path, &commit, &name)?;
    let bytes = std::fs::read(&map_path)
        .map_err(|e| ApiError::internal(format!("read {}: {e}", map_path.display())))?;
    let map: MapDocument = serde_json::from_slice(&bytes)
        .map_err(|e| ApiError::internal(format!("parse {}: {e}", map_path.display())))?;
    let mut report = lens::resolve(&parsed, &map);
    let changed = source::git(&repo_path)
        .args(["log", "-1", "--format=%H", &commit, "--", &file])
        .output()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if !changed.status.success() {
        return Err(ApiError::internal("lens history unavailable"));
    }
    let since = String::from_utf8_lossy(&changed.stdout).trim().to_owned();
    if !since.is_empty() && since != commit {
        lens::report_renames(&mut report, &renames(&repo_path, &since, &commit)?);
    }
    Ok(LensResponse {
        repository,
        commit,
        name,
        report,
    })
}

fn read_lens(
    repository: &str,
    repo_path: &std::path::Path,
    commit: &str,
    name: &str,
) -> Result<(Lens, String), ApiError> {
    if !lens::valid_lens_name(name) {
        return Err(ApiError::invalid_request("invalid lens name"));
    }
    let file = format!("{DIR}/{name}.toml");
    let read = source::read(&repo_path, &repository, &commit, &file)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let text = match (read.status, read.text) {
        ("ready", Some(text)) => text,
        ("missing", _) => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "lens_not_found",
                format!("{file} does not exist at commit {}", commit),
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
    Ok((parsed, file))
}

#[derive(Clone, Deserialize)]
pub struct LensCompareQuery {
    pub base: String,
    pub head: String,
    pub lens: String,
    pub other: Option<String>,
}

pub async fn compare_lenses(
    State(state): State<Arc<AppState>>,
    Path((owner, repo)): Path<(String, String)>,
    Query(query): Query<LensCompareQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (repository, source, row) = snapshot(&state, &owner, &repo, Some(query.base.clone()))?;
    let (_, _, head) = snapshot(&state, &owner, &repo, Some(query.head.clone()))?;
    let value =
        blocking(move || compare_at(repository, source, row.map_path, head.map_path, query))
            .await?;
    Ok(([("cache-control", "no-store")], Json(value)))
}

pub(crate) fn renames(
    repo: &std::path::Path,
    base: &str,
    head: &str,
) -> Result<Vec<lens::FileRename>, ApiError> {
    comparison::resolve(repo, base)?;
    comparison::resolve(repo, head)?;
    let out = source::git(repo)
        .args([
            "-c",
            "diff.renameLimit=0",
            "diff",
            "--name-status",
            "-z",
            "--find-renames=50%",
            "--diff-algorithm=myers",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            base,
            head,
            "--",
        ])
        .output()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if !out.status.success() {
        return Err(ApiError::internal("rename detection failed"));
    }
    Ok(crate::check::parse_name_status(&out.stdout)
        .map_err(|e| ApiError::internal(e.to_string()))?
        .into_iter()
        .filter_map(|change| match change {
            crate::check::Change::Renamed { from, to } => Some(lens::FileRename {
                from,
                to,
                commit: head.into(),
            }),
            _ => None,
        })
        .collect())
}

pub(crate) fn compare_at(
    repository: String,
    repo: PathBuf,
    map_path: PathBuf,
    head_map_path: PathBuf,
    query: LensCompareQuery,
) -> Result<crate::lens_comparison::LensComparison, ApiError> {
    use sha2::{Digest, Sha256};
    comparison::resolve(&repo, &query.base)?;
    comparison::resolve(&repo, &query.head)?;
    let other = query.other.as_deref().unwrap_or(&query.lens);
    if other != query.lens && query.base != query.head {
        return Err(ApiError::invalid_request(
            "two lenses must use the same commit",
        ));
    }
    let bytes = std::fs::read(&map_path).map_err(|_| comparison::unavailable())?;
    let frame: MapDocument =
        serde_json::from_slice(&bytes).map_err(|e| ApiError::internal(e.to_string()))?;
    let before = resolve_at(
        repository.clone(),
        repo.clone(),
        query.base.clone(),
        map_path.clone(),
        query.lens.clone(),
    )?
    .report;
    let (head_lens, _) = read_lens(&repository, &repo, &query.head, other)?;
    let renames = renames(&repo, &query.base, &query.head)?;
    let head_map = if query.base == query.head {
        frame.clone()
    } else {
        let head_bytes = std::fs::read(head_map_path).map_err(|_| comparison::unavailable())?;
        let snapshot: MapDocument =
            serde_json::from_slice(&head_bytes).map_err(|e| ApiError::internal(e.to_string()))?;
        crate::check::lens_head_map(&repo, &query.base, &query.head, &frame, &snapshot)
            .map_err(|e| ApiError::internal(e.to_string()))?
    };
    let mut after = lens::resolve(&head_lens, &head_map);
    lens::report_renames(&mut after, &renames);
    let mode = if other == query.lens {
        crate::lens_comparison::ComparisonMode::Commits
    } else {
        crate::lens_comparison::ComparisonMode::Membership
    };
    let changes = crate::lens_comparison::changes(&before, &after, &frame, &renames, mode);
    Ok(crate::lens_comparison::LensComparison {
        version: 1,
        repository,
        base: query.base,
        head: query.head,
        lens: query.lens.clone(),
        other: other.into(),
        base_map_sha256: format!("{:x}", Sha256::digest(bytes)),
        lower_bound: true,
        before,
        after,
        changes,
    })
}

pub(crate) fn list_at(
    repository: String,
    repo_path: PathBuf,
    commit: String,
) -> Result<LensList, ApiError> {
    let commit = comparison::resolve(&repo_path, &commit)?;
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
}
