//! Debt reports computed from retained personal maps.
use super::error::ApiError;
use crate::{comparison, debt, lens::LensResponse, schema::MapDocument};
use std::path::PathBuf;
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
