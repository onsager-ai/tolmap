//! Acceptance test for the Fly.io deployment change (2026-09-20): one
//! origin serves both the API and the built web bundle when
//! `TOLMAP_STATIC_DIR` is set (`src/service/http.rs::router`).
//!
//! Drives the real router with `tower::ServiceExt::oneshot`, the same
//! pattern `tests/service_hardening.rs` uses to exercise handlers without
//! binding a real socket.
//!
//! Three things are load-bearing here, all named directly in the brief
//! that asked for this change:
//!   - `TOLMAP_STATIC_DIR` unset: the router is byte-for-byte what it was
//!     before static serving existed. No fallback registered at all --
//!     `GET /` gets axum's default "nothing matched" response, not a
//!     static-file 404 and not `index.html`.
//!   - `TOLMAP_STATIC_DIR` set: `GET /` and `GET /django/django` (a path
//!     with no file on disk -- TanStack Router owns it client-side) both
//!     return `index.html`'s content.
//!   - `TOLMAP_STATIC_DIR` set: `GET /api/nonexistent` still gets the
//!     API's own JSON 404 (`ApiError::not_found`), never `index.html`. A
//!     missing API route returning an HTML page would turn a contract
//!     error into a JSON parse error in the client.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use tolmap::service::config::{Limits, ServeConfig};
use tolmap::service::ratelimit::RateLimiter;
use tolmap::service::store::Store;
use tolmap::service::{http, jobs, AppState};

/// A real `AppState` with `static_dir` set to `static_dir` (or left `None`
/// when `static_dir` is `None`) -- everything else at its normal default,
/// same as `tests/service_hardening.rs`'s `state_with_limits`.
fn state_with_static_dir(
    static_dir: Option<std::path::PathBuf>,
) -> (tempfile::TempDir, Arc<AppState>) {
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    let db_path = dir.path().join("tolmap.sqlite3");
    let store = Store::open(&db_path).unwrap();
    let config = ServeConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        db_path,
        cache_dir,
        static_dir,
        terrain: false,
        limits: Limits::default(),
        retain_commits_per_repo: 20,
    };
    let state = Arc::new(AppState {
        store,
        config,
        jobs: jobs::new_registry(),
        rate_limiter: RateLimiter::new(),
    });
    (dir, state)
}

/// A directory with an `index.html` a test can recognise by its body, the
/// way a built `web/dist` would have one.
fn bundle_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("index.html"),
        b"<!doctype html><title>tolmap</title><div id=\"root\"></div>",
    )
    .unwrap();
    dir
}

async fn get(state: Arc<AppState>, path: &str) -> (StatusCode, String) {
    let router = http::router(state);
    let request = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn static_dir_unset_router_is_unchanged() {
    let (_dir, state) = state_with_static_dir(None);

    // No file-serving fallback registered at all when unset -- `GET /`
    // hits axum's default "no route" response, not a 404 served from disk
    // and not index.html. This is the "byte-for-byte equivalent to today"
    // requirement: nothing about the router's shape changes when the
    // feature is off.
    let (status, _) = get(state.clone(), "/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The API itself is completely unaffected.
    let (status, body) = get(state, "/api/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"ok\""));
}

#[tokio::test]
async fn static_dir_set_serves_index_html_for_root_and_spa_routes() {
    let bundle = bundle_dir();
    let (_dir, state) = state_with_static_dir(Some(bundle.path().to_path_buf()));

    let (status, body) = get(state.clone(), "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("tolmap"),
        "expected index.html's content: {body}"
    );

    // /django/django has no file on disk -- TanStack Router owns it
    // client-side (docs/ARCHITECTURE.md: the map lives at /<owner>/<repo>
    // with no forge prefix). A cold load must still get index.html so the
    // client can route it, not a static-file 404.
    let (status, body) = get(state, "/django/django").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("tolmap"),
        "expected the SPA fallback to serve index.html for an unmapped path: {body}"
    );
}

#[tokio::test]
async fn static_dir_set_api_404_is_never_html() {
    let bundle = bundle_dir();
    let (_dir, state) = state_with_static_dir(Some(bundle.path().to_path_buf()));

    let (status, body) = get(state.clone(), "/api/nonexistent").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an unmatched /api/* path must 404, not fall through to the SPA"
    );
    assert!(
        !body.contains("<!doctype html>") && !body.contains("<title>"),
        "expected the API's own JSON 404, got what looks like index.html: {body}"
    );
    assert!(
        body.contains("\"error\":\"not_found\""),
        "expected ApiError::not_found's JSON shape: {body}"
    );

    // A real /api/* route still works normally alongside the catch-all.
    let (status, body) = get(state, "/api/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"ok\""));
}
