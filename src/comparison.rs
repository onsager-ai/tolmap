//! Persisted comparisons reuse `check::run` verbatim. No head layout is built.
//! Trusted adapters must authorize before cache access, and again at delivery.
use crate::service::error::ApiError;
use crate::{access, check, schema::*, source};
use axum::http::StatusCode;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize)]
pub struct CompareQuery {
    pub base: String,
    pub head: String,
    pub max_districts: Option<usize>,
    pub max_dq: Option<f64>,
    #[serde(default)]
    pub report_only: bool,
}

pub struct Inputs<'a> {
    pub repository: &'a str,
    pub repo: &'a Path,
    pub base_map: &'a Path,
    pub cache: &'a Path,
}

pub fn unavailable() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "comparison_unavailable",
        "Committed source or the base map is unavailable; no newer snapshot was substituted",
    )
}
fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(e.to_string())
}
fn check_error(e: check::CheckError) -> ApiError {
    ApiError::new(
        if e.exit_code() == 2 {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        },
        if e.exit_code() == 2 {
            "comparison_input"
        } else {
            "comparison_internal"
        },
        e.to_string(),
    )
}
pub fn resolve(repo: &Path, revision: &str) -> Result<String, ApiError> {
    // No option/revision-expression injection or working-tree service head.
    if !source::valid_commit(revision) {
        return Err(ApiError::invalid_request(
            "base and head must be full commit IDs",
        ));
    }
    let output = source::git(repo)
        .args(["rev-parse", "--verify", &format!("{revision}^{{commit}}")])
        .output()
        .map_err(internal)?;
    if !output.status.success() {
        return Err(unavailable());
    }
    let sha = String::from_utf8(output.stdout)
        .map_err(internal)?
        .trim()
        .to_owned();
    if sha != revision {
        return Err(ApiError::invalid_request("commit identity mismatch"));
    }
    Ok(sha)
}

/// Generic #222 boundary, shared by hosted map/source adapters. There is no
/// HTTP header principal or permissive private fallback in this module.
pub fn protected(
    inputs: Inputs<'_>,
    query: CompareQuery,
    policy: &dyn access::Policy,
    request: access::Request<'_>,
) -> Result<Comparison, ApiError> {
    let denied = || {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "access_denied",
            "Comparison access denied",
        )
    };
    let result =
        access::enforce(policy, request, || run(inputs, query)).map_err(|_| denied())??;
    access::enforce(policy, request, || result).map_err(|_| denied())
}

pub fn run(inputs: Inputs<'_>, query: CompareQuery) -> Result<Comparison, ApiError> {
    let base = resolve(inputs.repo, &query.base)?;
    let head = resolve(inputs.repo, &query.head)?;
    let mut options = check::CheckOptions {
        repo: inputs.repo.to_owned(),
        base: base.clone(),
        head: Some(head.clone()),
        base_map: Some(inputs.base_map.to_owned()),
        max_districts: query.max_districts,
        max_dq: query.max_dq,
        report_only: query.report_only,
    };
    let thresholds = check::resolve_thresholds(&options).map_err(check_error)?;
    let map_bytes = fs::read(inputs.base_map).map_err(|_| unavailable())?;
    let map_hash = format!("{:x}", Sha256::digest(&map_bytes));
    // Include the exact map partition and implementation revision: rebuilding a
    // map or changing code must never silently reuse a different comparison.
    let key = serde_json::to_vec(&(
        inputs.repository,
        &base,
        &head,
        &thresholds,
        env!("CARGO_PKG_VERSION"),
        BUILD_REVISION,
        &map_hash,
    ))
    .map_err(internal)?;
    let id = format!("{:x}", Sha256::digest(key));
    fs::create_dir_all(inputs.cache).map_err(internal)?;
    let path = inputs.cache.join(format!("{id}.json"));
    if path.exists() {
        return load(&path);
    }
    static COMPUTATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _permit = COMPUTATION
        .try_lock()
        .map_err(|_| ApiError::busy("Another comparison is running; retry when it finishes"))?;
    // Local snapshots call their map map.json. check's provenance validation
    // deliberately requires <full-sha>.json, so copy verified bytes to an
    // isolated staging directory without changing the immutable snapshot.
    let staging = inputs.cache.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&staging).map_err(internal)?;
    let staged_map = staging.join(format!("{base}.json"));
    let computed = (|| {
        fs::write(&staged_map, &map_bytes).map_err(internal)?;
        options.base_map = Some(staged_map);
        let report =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check::run(&options)))
                .map_err(|_| {
                    ApiError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "comparison_internal",
                        "check panicked",
                    )
                })?
                .map_err(check_error)?;
        let indexed: BTreeSet<_> = report.files.iter().map(|f| f.path.as_str()).collect();
        let outside_graph = check::diff(inputs.repo, &base, Some(&head))
            .map_err(check_error)?
            .into_iter()
            .filter_map(|c| {
                let (path, base_path, status) = match c {
                    check::Change::Added(p) => (p, None, CheckFileStatus::Added),
                    check::Change::Modified(p) => (p, None, CheckFileStatus::Modified),
                    check::Change::Deleted(p) => (p, None, CheckFileStatus::Deleted),
                    check::Change::Renamed { from, to } => {
                        (to, Some(from), CheckFileStatus::Renamed)
                    }
                };
                (!indexed.contains(path.as_str())).then_some(CheckFile {
                    path,
                    base_path,
                    status,
                    district: None,
                    placed: false,
                })
            })
            .collect::<Vec<_>>();
        let provenance = ComparisonProvenance {
            repository: inputs.repository.into(),
            base,
            head,
            report_version: report.version,
            thresholds: report.thresholds.clone(),
            base_map_source: "stored".into(),
            base_map_sha256: map_hash,
            base_files: report.base_files,
            base_districts: report.base_districts,
            indexed_changes: report.files.len(),
            outside_graph,
            tolmap_version: env!("CARGO_PKG_VERSION").into(),
            build_revision: BUILD_REVISION.into(),
            resolution: check::RESOLUTION,
            prune: "node-relative".into(),
            seed: crate::SEED as u32,
            refs: "hand".into(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(internal)?
                .as_secs()
                .to_string(),
        };
        let comparison = Comparison {
            id,
            report,
            provenance,
        };
        let temp = staging.join("comparison.json");
        fs::write(&temp, serde_json::to_vec(&comparison).map_err(internal)?).map_err(internal)?;
        // Atomic no-clobber publication. Concurrent identical requests return
        // the first published envelope, including its original creation time.
        match fs::hard_link(&temp, &path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(internal(e)),
        }
        load(&path)
    })();
    let _ = fs::remove_dir_all(staging);
    computed
}

pub const BUILD_REVISION: &str = match option_env!("TOLMAP_BUILD_COMMIT") {
    Some(s) => s,
    None => "development",
};
fn load(path: &Path) -> Result<Comparison, ApiError> {
    let result: Comparison =
        serde_json::from_slice(&fs::read(path).map_err(internal)?).map_err(internal)?;
    if result.report.version != check::REPORT_VERSION
        || result.provenance.report_version != check::REPORT_VERSION
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "comparison_version",
            "Unsupported check report version",
        ));
    }
    Ok(result)
}

pub fn public_paths(cache: &Path, owner: &str, repo: &str) -> Result<(String, PathBuf), ApiError> {
    let r = crate::service::clone::resolve(Some(&format!("{owner}/{repo}")), None)?;
    Ok((r.slug, cache.join("repos").join(r.owner).join(r.repo)))
}
