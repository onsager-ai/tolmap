//! Lens resources read from retained personal snapshot source.
use super::error::ApiError;
use crate::{
    comparison,
    lens::{self, Lens, LensList, LensResponse},
    schema::MapDocument,
    source,
};
use axum::http::StatusCode;
use std::path::PathBuf;
const DIR: &str = ".tolmap/lenses";
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
