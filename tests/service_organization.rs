//! `GET /api/organizations/{name}` (issue #255): an organization resource is
//! a prebuilt document over member maps that have already been indexed at its
//! pinned commits. The handler must neither accept source locations nor start
//! an indexing job.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::json;
use tower::ServiceExt;

use tolmap::service::config::{Limits, ServeConfig};
use tolmap::service::ratelimit::RateLimiter;
use tolmap::service::store::{MapRow, Store};
use tolmap::service::{http, jobs, AppState};

const APP_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const CORE_COMMIT: &str = "fedcba9876543210fedcba9876543210fedcba98";

fn state(dir: &tempfile::TempDir, organization_dir: std::path::PathBuf) -> Arc<AppState> {
    let config = ServeConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        allow_local_paths: false,
        db_path: dir.path().join("store.sqlite3"),
        cache_dir: dir.path().join("cache"),
        static_dir: None,
        organization_dir: Some(organization_dir),
        prune_variant: tolmap::pipeline::PruneVariant::NodeRelative,
        namer: tolmap::naming::NamerKind::Idf,
        namer_model: tolmap::naming::DEFAULT_MODEL.to_owned(),
        refs: tolmap::extract::RefsMode::Hand,
        scip_install: false,
        limits: Limits::default(),
        retain_commits_per_repo: 20,
        worker_uid: 0,
        worker_gid: 0,
    };
    Arc::new(AppState {
        store: Store::open(&config.db_path).unwrap(),
        config,
        jobs: jobs::new_registry(),
        rate_limiter: RateLimiter::new(),
    })
}

fn insert_map(state: &AppState, dir: &tempfile::TempDir, slug: &str, commit: &str) {
    let (owner, repo) = slug.split_once('/').unwrap();
    let map_path = dir.path().join(format!("{owner}-{repo}-{commit}.json"));
    std::fs::write(&map_path, b"{}").unwrap();
    state
        .store
        .insert(&MapRow {
            slug: slug.to_owned(),
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            commit: commit.to_owned(),
            branch: None,
            lang: "rust".to_owned(),
            files: 1,
            districts: 1,
            modularity: 0.0,
            map_path,
            indexed_at: "2026-11-10T00:00:00Z".to_owned(),
        })
        .unwrap();
}

fn document() -> serde_json::Value {
    json!({
        "version": 2,
        "name": "acme",
        "members": [
            {"repo": "acme/app", "commit": APP_COMMIT, "path": null, "map": "stored", "map_digest": "sha256:app", "files": 1, "districts": 1, "packages": []},
            {"repo": "acme/core", "commit": CORE_COMMIT, "path": null, "map": "stored", "map_digest": "sha256:core", "files": 1, "districts": 1, "packages": []}
        ],
        "placement": {
            "regions": [
                {"repo": "acme/app", "translation": [-1.0, 0.0], "scale": 1.0},
                {"repo": "acme/core", "translation": [1.0, 0.0], "scale": 1.0}
            ],
            "roads": [{"consumer": "acme/app", "provider": "acme/core", "from": [-1.0, 0.0], "to": [1.0, 0.0]}]
        },
        "edges": [],
        "pin_skew": [],
        "external": [],
        "unresolved": []
    })
}

async fn get(state: Arc<AppState>, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = http::router(state)
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn organization_needs_every_pinned_member_map_already_stored() {
    let dir = tempfile::tempdir().unwrap();
    let organization_dir = dir.path().join("organizations");
    std::fs::create_dir(&organization_dir).unwrap();
    std::fs::write(
        organization_dir.join("acme.json"),
        serde_json::to_vec(&document()).unwrap(),
    )
    .unwrap();
    let state = state(&dir, organization_dir);

    insert_map(&state, &dir, "acme/app", APP_COMMIT);
    let (status, body) = get(state.clone(), "/api/organizations/acme").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["message"].as_str().unwrap().contains(CORE_COMMIT));

    insert_map(&state, &dir, "acme/core", CORE_COMMIT);
    let (status, body) = get(state, "/api/organizations/acme").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["placement"]["roads"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn prior_document_version_is_not_a_resource() {
    let dir = tempfile::tempdir().unwrap();
    let organization_dir = dir.path().join("organizations");
    std::fs::create_dir(&organization_dir).unwrap();
    let mut prior = document();
    prior["version"] = json!(1);
    prior.as_object_mut().unwrap().remove("placement");
    std::fs::write(
        organization_dir.join("acme.json"),
        serde_json::to_vec(&prior).unwrap(),
    )
    .unwrap();
    let state = state(&dir, organization_dir);
    let (status, body) = get(state, "/api/organizations/acme").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["message"].as_str().unwrap().contains("version 1"));
}

#[tokio::test]
async fn organization_name_cannot_be_a_path_or_url() {
    let dir = tempfile::tempdir().unwrap();
    let organization_dir = dir.path().join("organizations");
    std::fs::create_dir(&organization_dir).unwrap();
    let state = state(&dir, organization_dir);
    for name in ["file:secret", "Acme", "secret%2Fpart"] {
        let (status, body) = get(state.clone(), &format!("/api/organizations/{name}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name:?}: {body}");
        assert_eq!(body["error"], "invalid_request");
    }
}
