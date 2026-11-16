//! Personal routes share CLI state without opening the public catalogue.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use std::{path::Path, sync::Arc};
use tolmap::{
    personal::Library,
    service::{
        application_config::ApplicationConfig,
        config::ServeConfig,
        jobs,
        personal_web::{self, WebState},
        ratelimit::RateLimiter,
        store::{PersonalSnapshot, Repository, Store},
        AppState,
    },
};
use tower::ServiceExt;

fn setup() -> (tempfile::TempDir, Router, Arc<WebState>) {
    let dir = tempfile::tempdir().unwrap();
    let config = ServeConfig {
        db_path: dir.path().join("db"),
        cache_dir: dir.path().join("cache"),
        static_dir: Some(dir.path().join("assets")),
        ..ServeConfig::from_env()
    };
    std::fs::create_dir(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets/index.html"), "<!doctype html><html><head><title>tolmap</title></head><body><div id=\"root\"></div></body></html>").unwrap();
    std::fs::create_dir(dir.path().join("repositories")).unwrap();
    std::fs::create_dir(config.db_path.with_extension("locks")).unwrap();
    let library = Library {
        store: Store::open(&config.db_path).unwrap(),
        config: ApplicationConfig {
            data: dir.path().into(),
            service: config.clone(),
            retain_commits_per_repo: None,
        },
    };
    let web = WebState::new(library);
    let state = Arc::new(AppState {
        store: Store::open(&config.db_path).unwrap(),
        config,
        jobs: jobs::new_registry(),
        rate_limiter: RateLimiter::new(),
    });
    (dir, personal_web::router(state, web.clone()), web)
}
async fn request(app: &Router, method: &str, path: &str, body: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}
fn repo(store: &Store, location: &Path) -> Repository {
    let repo = Repository {
        id: "repo-id".into(),
        name: "example".into(),
        kind: "local".into(),
        location: location.to_string_lossy().into_owned(),
        observed_head: None,
        observed_at: None,
    };
    store.register(&repo).unwrap();
    repo
}
fn publish(store: &Store, root: &Path, id: &str) {
    let dir = root.join("repositories/repo-id/snapshots").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    // Full real map schema, small pinned fixture; indexing is deliberately not run.
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/data/httpx.json"),
        dir.join("map.json"),
    )
    .unwrap();
    std::fs::create_dir(dir.join("symbols")).unwrap();
    std::fs::write(dir.join("symbols/0.json"), "{\"district\":0}").unwrap();
    store.personal_begin("repo-id", id).unwrap();
    store
        .personal_finish(
            id,
            Ok(&PersonalSnapshot {
                id: id.into(),
                repository: "repo-id".into(),
                commit: "a".repeat(40),
                map_path: dir.join("map.json"),
                source_path: root.join("source.git"),
                indexed_at: String::new(),
            }),
        )
        .unwrap();
}
#[tokio::test]
async fn catalogue_and_history_reread_cli_publications_and_repeated_commits() {
    let (dir, app, state) = setup();
    let cli = Store::open(&state.library.config.service.db_path).unwrap();
    repo(&cli, &dir.path().join("missing-checkout"));
    let (status, body) = request(&app, "GET", "/api/maps", "").await;
    assert_eq!(status, StatusCode::OK);
    let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert!(rows[0]["latest_snapshot_id"].is_null());
    publish(&cli, dir.path(), "first");
    publish(&cli, dir.path(), "second");
    let (_, body) = request(&app, "GET", "/api/maps", "").await;
    let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["snapshot_count"], 2);
    assert_eq!(rows[0]["latest_snapshot_id"], "second");
    let (_, body) = request(&app, "GET", "/api/repositories/repo-id/history", "").await;
    let history: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(history["snapshots"][0]["id"], "second");
    assert_eq!(history["snapshots"][1]["reopen"]["path"], "/snapshot/first");
    let (status, bytes) = request(&app, "GET", "/api/maps/snapshot/first", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        bytes.as_bytes(),
        std::fs::read(
            dir.path()
                .join("repositories/repo-id/snapshots/first/map.json")
        )
        .unwrap()
    );
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/maps/snapshot/first/symbols?district=0",
            ""
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/api/maps/snapshot/first?commit={}", "b".repeat(40)),
            ""
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    cli.personal_begin("repo-id", "failed").unwrap();
    cli.personal_finish("failed", Err("indexing failed"))
        .unwrap();
    let (_, body) = request(&app, "GET", "/api/repositories/repo-id/history", "").await;
    let history: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(history["status"]["kind"], "failed");
    assert_eq!(history["snapshots"].as_array().unwrap().len(), 2);
}
#[tokio::test]
async fn personal_shell_has_add_profile_and_api_misses_stay_json() {
    let (_dir, app, _state) = setup();
    for path in ["/", "/snapshot/id", "/repositories/repo-id/history"] {
        let (status, body) = request(&app, "GET", path, "").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("tolmap-personal"));
        assert!(body.contains("/api/personal/index"));
        assert!(body.contains("Personal library"));
    }
    for path in [
        "/api/nonexistent",
        "/api/jobs/unknown/events",
        "/api/maps/public/repo",
    ] {
        let (status, body) = request(&app, "GET", path, "").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["error"].is_string());
    }
    assert_eq!(
        request(&app, "GET", "/api/healthz", "").await.0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "POST", "/api/index", "{\"repo\":\"owner/repo\"}")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&app, "POST", "/api/personal/index", "{\"provider_ids\":[]}")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}
#[tokio::test]
async fn removal_preserves_original_checkout_and_hides_maps() {
    let (dir, app, state) = setup();
    let original = dir.path().join("original");
    std::fs::create_dir(&original).unwrap();
    std::fs::write(original.join("keep"), "user source").unwrap();
    repo(&state.library.store, &original);
    publish(&state.library.store, dir.path(), "snapshot");
    assert_eq!(
        request(&app, "DELETE", "/api/repositories/repo-id", "")
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert!(original.join("keep").is_file());
    assert!(!dir.path().join("repositories/repo-id").exists());
    assert_eq!(
        request(&app, "GET", "/api/maps/snapshot/snapshot", "")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&app, "POST", "/api/repositories/repo-id/refresh", "")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}
#[tokio::test]
async fn retained_source_uses_exact_snapshot_commit_after_checkout_disappears() {
    let (dir, app, state) = setup();
    let checkout = dir.path().join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&checkout)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    };
    git(&["init"]);
    std::fs::write(checkout.join("source.txt"), "committed source\n").unwrap();
    git(&["add", "source.txt"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "-m",
        "fixture",
    ]);
    let commit = git(&["rev-parse", "HEAD"]);
    let retained = dir.path().join("source.git");
    assert!(std::process::Command::new("git")
        .args(["clone", "--bare", "--no-hardlinks"])
        .arg(&checkout)
        .arg(&retained)
        .output()
        .unwrap()
        .status
        .success());
    repo(&state.library.store, &checkout);
    let map_path = dir.path().join("map.json");
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/data/httpx.json"),
        &map_path,
    )
    .unwrap();
    state
        .library
        .store
        .personal_begin("repo-id", "source")
        .unwrap();
    state
        .library
        .store
        .personal_finish(
            "source",
            Ok(&PersonalSnapshot {
                id: "source".into(),
                repository: "repo-id".into(),
                commit: commit.clone(),
                map_path,
                source_path: retained,
                indexed_at: String::new(),
            }),
        )
        .unwrap();
    std::fs::remove_dir_all(checkout).unwrap();
    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/maps/snapshot/source/source?commit={commit}&path=source.txt"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["commit"], commit);
    assert_eq!(value["text"], "committed source\n");
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/api/maps/snapshot/source/source?commit={commit}&path=../secret"),
            ""
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/maps/snapshot/source/source?path=source.txt",
            ""
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
#[tokio::test]
async fn interrupted_web_jobs_remain_readable_in_batch_and_cancel_is_safe() {
    let (_dir, app, state) = setup();
    let job_id = uuid::Uuid::new_v4();
    let job = jobs::JobSnapshot {
        job_id,
        slug: "example".into(),
        commit: None,
        status: jobs::JobStatus::Indexing,
        stage: "Parsing files".into(),
        queue_position: None,
        started_at: "2026-11-16T00:00:00Z".into(),
        finished_at: None,
        error: None,
        error_code: None,
        progress: None,
        eta: None,
        eta_start_s: None,
        elapsed_s: 0.,
        stages: vec![],
        map_ready: false,
    };
    state
        .library
        .store
        .insert_job(&tolmap::service::store::JobRow {
            job_id: job_id.to_string(),
            slug: "personal-web/batch".into(),
            commit: String::new(),
            spec_json: "{}".into(),
            class: 0,
            status: "running".into(),
            attempt: 0,
            epoch: 0,
            lease_holder: None,
            lease_deadline_ms: None,
            queue_order: 0,
            snapshot_json: serde_json::to_string(&job).unwrap(),
        })
        .unwrap();
    let (status, body) = request(&app, "GET", &format!("/api/jobs/{job_id}"), "").await;
    assert_eq!(status, StatusCode::OK);
    let job: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(job["status"], "failed");
    assert_eq!(job["error_code"], "server_stopping");
    let (_, body) = request(&app, "GET", "/api/personal/batches/batch", "").await;
    let batch: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(batch["jobs"][0]["job_id"], job_id.to_string());
    assert_eq!(
        request(&app, "POST", &format!("/api/jobs/{job_id}/cancel"), "")
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "GET", "/api/personal/batches/missing", "")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}
#[tokio::test]
async fn scans_are_sorted_skip_symlinks_and_mark_registered_checkouts() {
    let (dir, app, state) = setup();
    let roots = dir.path().join("projects");
    for name in ["b", "a"] {
        let checkout = roots.join(name);
        std::fs::create_dir_all(&checkout).unwrap();
        assert!(std::process::Command::new("git")
            .arg("init")
            .arg(&checkout)
            .output()
            .unwrap()
            .status
            .success());
    }
    std::os::unix::fs::symlink(roots.join("a"), roots.join("link")).unwrap();
    repo(&state.library.store, &roots.join("a"));
    let (status, body) = request(
        &app,
        "GET",
        &format!("/api/personal/scan?path={}", roots.display()),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2);
    assert!(rows[0]["provider_id"].as_str().unwrap().ends_with("/a"));
    assert_eq!(rows[0]["connected_repository_id"], "repo-id");
}
