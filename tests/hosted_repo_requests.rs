//! Issue #226: a hosted request cannot name a file on the server's disk
//! through `repo`. Drives the real `service::http::router` (as
//! `tests/static_serving.rs` does) with bare filesystem paths and `file://`
//! URLs in `repo`, and asserts every one is rejected as `invalid_request`
//! before any job, clone-cache directory or stored map exists, and that the
//! rejection never carries the content of the file it named.
//!
//! Not covered, deliberately: `{"path": ...}` is the documented
//! fixtures/dev form of `POST /api/index` (docs/API.md "Repository
//! identifiers") and standalone `tolmap serve` is one trust domain
//! (docs/ACCESS.md); it is not asserted either way here.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;

use tolmap::service::config::{Limits, ServeConfig};
use tolmap::service::ratelimit::RateLimiter;
use tolmap::service::store::Store;
use tolmap::service::{http, jobs, AppState};

fn state(dir: &std::path::Path) -> Arc<AppState> {
    let db_path = dir.join("tolmap.sqlite3");
    let config = ServeConfig {
        bind: "0.0.0.0:8787".parse().unwrap(),
        db_path: db_path.clone(),
        cache_dir: dir.join("cache"),
        static_dir: None,
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
        store: Store::open(&db_path).unwrap(),
        config,
        jobs: jobs::new_registry(),
        rate_limiter: RateLimiter::new(),
    })
}

async fn post_index(state: Arc<AppState>, body: &str) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/index")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap();
    // The peer address `into_make_service_with_connect_info` supplies.
    request
        .extensions_mut()
        .insert(ConnectInfo("203.0.113.9:4000".parse::<SocketAddr>().unwrap()));
    let response = http::router(state).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn bare_paths_and_file_urls_in_repo_are_rejected_without_reading_the_server_disk() {
    let dir = tempfile::tempdir().unwrap();
    let secret_dir = dir.path().join("private-checkout");
    std::fs::create_dir_all(&secret_dir).unwrap();
    std::fs::write(secret_dir.join("secret.py"), "SERVER_SIDE_SECRET_226 = 1\n").unwrap();
    let secret = secret_dir.display().to_string();
    let state = state(dir.path());

    let attempts = [
        secret.clone(),
        format!("{secret}/"),
        format!("file://{secret}"),
        format!("file://{secret}/.git"),
        "file:///etc/passwd".to_owned(),
        "file:/etc/passwd".to_owned(),
        "/etc/passwd".to_owned(),
        "../../etc/passwd".to_owned(),
        "./private-checkout".to_owned(),
        "..\\..\\windows".to_owned(),
        "owner/../../etc".to_owned(),
    ];
    for repo in attempts {
        let body = serde_json::json!({ "repo": repo }).to_string();
        let (status, text) = post_index(state.clone(), &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "repo {repo:?}: {text}");
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["error"], "invalid_request", "repo {repo:?}: {text}");
        assert!(
            !text.contains("SERVER_SIDE_SECRET_226"),
            "repo {repo:?} leaked source: {text}"
        );
    }

    // Nothing was admitted, cloned or stored for any attempt.
    assert!(state.store.list_latest().unwrap().is_empty());
    assert!(!dir.path().join("cache/repos").exists());
}

#[tokio::test]
async fn naming_both_repo_and_path_is_rejected_before_either_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let body = serde_json::json!({ "repo": "owner/name", "path": dir.path() }).to_string();
    let (status, text) = post_index(state.clone(), &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(state.store.list_latest().unwrap().is_empty());
}
