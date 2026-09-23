//! Acceptance test for issue #23 gap 5: the `repo_too_large` path had never
//! actually fired (nothing in the fixture corpus exceeds the default
//! limits, by design). Drives the real router (`service::http::router`)
//! through the real `POST /api/index` -> spawned job -> `GET
//! /api/jobs/{id}` pipeline against a real fixture, with a limit lowered
//! enough to trip on purpose, and checks the job's terminal state carries
//! exactly the shape docs/API.md promises.
//!
//! Requests are driven with `tower::ServiceExt::oneshot` directly against
//! the `axum::Router` rather than a bound TCP socket -- there is no need
//! for a real listener to exercise handler and job logic, and it keeps
//! this test from needing to pick a free port.
//!
//! Skips (with a clear message) when `TOLMAP_FIXTURE_REPOS` is not set,
//! mirroring `tests/fixtures_detect.rs`'s convention: CI has no clones.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use tolmap::service::config::{Limits, ServeConfig};
use tolmap::service::ratelimit::RateLimiter;
use tolmap::service::store::Store;
use tolmap::service::{http, jobs, AppState};

fn fixture(name: &str) -> Option<PathBuf> {
    let dir = std::env::var("TOLMAP_FIXTURE_REPOS").ok()?;
    let path = Path::new(&dir).join(name);
    path.is_dir().then_some(path)
}

fn skip(reason: &str) {
    eprintln!(
        "skipping: {reason} -- e.g. TOLMAP_FIXTURE_REPOS=/tmp/tolmap-fixtures.XXXXXX \
         cargo test --release --test service_hardening"
    );
}

/// A real `AppState` (real SQLite store, real cache dir under a tempdir)
/// with the given `Limits` -- everything else at its normal default.
fn state_with_limits(limits: Limits) -> (tempfile::TempDir, Arc<AppState>) {
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    let db_path = dir.path().join("tolmap.sqlite3");
    let store = Store::open(&db_path).unwrap();
    let config = ServeConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        db_path,
        cache_dir,
        static_dir: None,
        prune_variant: tolmap::pipeline::PruneVariant::NodeRelative,
        namer: tolmap::naming::NamerKind::Idf,
        namer_model: tolmap::naming::DEFAULT_MODEL.to_owned(),
        limits,
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

fn connect_info() -> ConnectInfo<SocketAddr> {
    ConnectInfo("127.0.0.1:1".parse().unwrap())
}

/// `POST /api/index {"path": <path>}` against the real router, returning
/// the parsed 202 body. Asserts 202 unconditionally: the size/history
/// checks under test all run *inside* the spawned job, not synchronously
/// in this handler (docs/API.md: a job that fails still carries its
/// `error` in the job's own terminal state, since the request that created
/// it already answered 202) -- so queuing must always succeed here, and
/// the interesting assertions are on the job's later state.
async fn post_index_for_path(state: Arc<AppState>, path: &Path) -> Value {
    let router = http::router(state);
    let body = json!({ "path": path.to_string_lossy() });
    let request = Request::builder()
        .method("POST")
        .uri("/api/index")
        .header("content-type", "application/json")
        .extension(connect_info())
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::ACCEPTED,
        "queuing must succeed even for a repo that will fail its size checks -- \
         those checks run inside the job"
    );
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Polls `GET /api/jobs/{job_id}` until `status` is `"done"` or `"failed"`,
/// returning that final snapshot as JSON.
async fn wait_for_terminal_status(state: Arc<AppState>, job_id: &str) -> Value {
    for _ in 0..300 {
        let router = http::router(state.clone());
        let request = Request::builder()
            .method("GET")
            .uri(format!("/api/jobs/{job_id}"))
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let snapshot: Value = serde_json::from_slice(&bytes).unwrap();
        let status = snapshot["status"].as_str().unwrap();
        if status == "done" || status == "failed" {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job {job_id} did not reach a terminal state within 15s");
}

#[tokio::test]
async fn repo_too_large_fires_on_file_count() {
    let Some(django) = fixture("django") else {
        skip("TOLMAP_FIXTURE_REPOS is not set (or django is not there)");
        return;
    };

    let limits = Limits {
        max_files: 100, // django is 851 files -- well under this
        // Raised well above django's real history so this test isolates
        // file_count rather than tripping history depth first (materialize's
        // history check runs before detect's file-count check).
        max_history_commits: 100_000,
        ..Limits::default()
    };

    let (_dir, state) = state_with_limits(limits.clone());
    let queued = post_index_for_path(state.clone(), &django).await;
    let job_id = queued["job_id"]
        .as_str()
        .expect("202 response carries a job_id")
        .to_owned();

    let snapshot = wait_for_terminal_status(state, &job_id).await;
    assert_eq!(snapshot["status"], "failed");
    let error = &snapshot["error"];
    assert_eq!(
        error["error"], "repo_too_large",
        "unexpected job outcome: {snapshot}"
    );
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains("file count"),
        "message should name the limit: {message:?}"
    );
    assert!(
        message.contains(&limits.max_files.to_string()),
        "message should name the configured value: {message:?}"
    );
}

#[tokio::test]
async fn repo_too_large_fires_on_history_depth() {
    let Some(django) = fixture("django") else {
        skip("TOLMAP_FIXTURE_REPOS is not set (or django is not there)");
        return;
    };

    let limits = Limits {
        max_history_commits: 10, // django's fixture clone carries its full upstream history
        ..Limits::default()
    };

    let (_dir, state) = state_with_limits(limits.clone());
    let queued = post_index_for_path(state.clone(), &django).await;
    let job_id = queued["job_id"]
        .as_str()
        .expect("202 response carries a job_id")
        .to_owned();

    let snapshot = wait_for_terminal_status(state, &job_id).await;
    assert_eq!(snapshot["status"], "failed");
    let error = &snapshot["error"];
    assert_eq!(
        error["error"], "repo_too_large",
        "unexpected job outcome: {snapshot}"
    );
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains("history depth"),
        "message should name the limit: {message:?}"
    );
    assert!(
        message.contains(&limits.max_history_commits.to_string()),
        "message should name the configured value: {message:?}"
    );
}
