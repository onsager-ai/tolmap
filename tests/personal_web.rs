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
        store::{PersonalSnapshot, Repository, Store},
    },
};
use tower::ServiceExt;

fn setup() -> (tempfile::TempDir, Router, Arc<WebState>) {
    let dir = tempfile::tempdir().unwrap();
    let config = ServeConfig {
        db_path: dir.path().join("db"),
        cache_dir: dir.path().join("cache"),
        static_dir: Some(dir.path().join("assets")),
        bind: "127.0.0.1:8787".parse().unwrap(),
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
    let app = personal_web::router(config.static_dir.as_ref(), web.clone());
    (dir, app, web)
}
async fn request(app: &Router, method: &str, path: &str, body: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .header("host", "localhost:8787")
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
    // Plain-HTTP remote browsers have no crypto.subtle; the server supplies
    // the map identity used by stale-comparison checks.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/maps/snapshot/first")
                .header("host", "localhost:8787")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    {
        use sha2::Digest;
        assert_eq!(
            response.headers()["x-tolmap-map-sha256"],
            format!("{:x}", sha2::Sha256::digest(bytes.as_bytes())).as_str()
        );
    }
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

#[tokio::test]
async fn unreadable_maps_do_not_hide_catalogue_or_other_history() {
    let (dir, app, state) = setup();
    repo(&state.library.store, &dir.path().join("missing"));
    publish(&state.library.store, dir.path(), "good");
    publish(&state.library.store, dir.path(), "broken");
    state
        .library
        .store
        .register(&Repository {
            id: "other".into(),
            name: "other".into(),
            kind: "local".into(),
            location: dir.path().join("other").to_string_lossy().into_owned(),
            observed_head: None,
            observed_at: None,
        })
        .unwrap();
    let broken = dir
        .path()
        .join("repositories/repo-id/snapshots/broken/map.json");
    for corrupt in [true, false] {
        if corrupt {
            std::fs::write(&broken, "invalid JSON").unwrap();
        } else {
            std::fs::remove_file(&broken).unwrap();
        }
        let (status, body) = request(&app, "GET", "/api/maps", "").await;
        assert_eq!(status, StatusCode::OK);
        let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(rows[0]["status"]["kind"], "failed");
        assert_eq!(rows.as_array().unwrap().len(), 2);
        assert_eq!(rows[1]["repository_id"], "other");
        let (status, body) = request(&app, "GET", "/api/repositories/repo-id/history", "").await;
        assert_eq!(status, StatusCode::OK);
        let history: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(history["snapshots"].as_array().unwrap().len(), 2);
        assert!(history["snapshots"][0]["reopen"].is_null());
        assert_eq!(history["snapshots"][1]["reopen"]["path"], "/snapshot/good");
    }
}
#[tokio::test]
async fn browser_requests_reject_rebinding_hosts_and_simple_mutations() {
    let (_dir, app, _state) = setup();
    for host in [
        "evil.example:8787",
        "127.0.0.1.evil.example:8787",
        "localhost",
        "[::1].evil:8787",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/maps")
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{host}");
    }
    for host in ["localhost:8787", "127.0.0.1:8787", "[::1]:8787"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/maps")
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{host}");
    }
    for content_type in [
        None,
        Some("text/plain"),
        Some("application/x-www-form-urlencoded"),
        Some("multipart/form-data"),
    ] {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/jobs/missing/cancel")
            .header("host", "localhost:8787");
        if let Some(value) = content_type {
            builder = builder.header("content-type", value);
        }
        let response = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
}
#[tokio::test]
async fn invalid_batch_source_registers_nothing() {
    let (dir, app, state) = setup();
    let checkout = dir.path().join("valid");
    std::fs::create_dir(&checkout).unwrap();
    assert!(std::process::Command::new("git")
        .arg("init")
        .arg(&checkout)
        .output()
        .unwrap()
        .status
        .success());
    let body = serde_json::json!({"provider_ids":[checkout.to_str().unwrap(), "https://invalid.example/repo"]}).to_string();
    assert_eq!(
        request(&app, "POST", "/api/personal/index", &body).await.0,
        StatusCode::BAD_REQUEST
    );
    assert!(state.library.store.repositories().unwrap().is_empty());
}

/// Run the actual web binary so its executor launches `tolmap worker`, rather
/// than launching this integration-test executable as a worker.
#[test]
fn real_local_repository_add_poll_done() {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        process::{Child, Command},
        time::{Duration, Instant},
    };
    struct WebProcess(Child);
    impl Drop for WebProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn http(
        address: std::net::SocketAddr,
        method: &str,
        path: &str,
        body: &str,
    ) -> std::io::Result<(u16, serde_json::Value)> {
        let mut stream = TcpStream::connect(address)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        write!(stream, "{method} {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
        let mut text = String::new();
        stream.read_to_string(&mut text)?;
        let (headers, body) = text.split_once("\r\n\r\n").unwrap();
        let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        Ok((status, serde_json::from_str(body).unwrap()))
    }
    let root = tempfile::tempdir().unwrap();
    let checkout = root.path().join("repo");
    std::fs::create_dir_all(checkout.join("pkg")).unwrap();
    std::fs::write(
        checkout.join("pyproject.toml"),
        "[project]\nname='pkg'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::write(checkout.join("pkg/__init__.py"), "").unwrap();
    for i in 0..8 {
        std::fs::write(
            checkout.join(format!("pkg/f{i}.py")),
            format!(
                "from .f{} import value\ndef value():\n    return {i}\n",
                (i + 1) % 8
            ),
        )
        .unwrap();
    }
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(&checkout)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    let assets = root.path().join("assets");
    std::fs::create_dir(&assets).unwrap();
    std::fs::write(
        assets.join("index.html"),
        "<html><head></head><body></body></html>",
    )
    .unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    drop(socket);
    let _web = WebProcess(
        Command::new(env!("CARGO_BIN_EXE_tolmap"))
            .args(["--data"])
            .arg(root.path().join("data"))
            .args(["web", "--addr", &address.to_string()])
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("TOLMAP_STATIC_DIR", assets)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if http(address, "GET", "/api/healthz", "").is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "web startup timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    let body = serde_json::json!({"provider_ids":[checkout.to_str().unwrap()]}).to_string();
    let (status, accepted) = http(address, "POST", "/api/personal/index", &body).unwrap();
    assert_eq!(status, 202, "{accepted}");
    let id = accepted["jobs"][0]["job_id"].as_str().unwrap();
    let done = loop {
        let (status, job) = http(address, "GET", &format!("/api/jobs/{id}"), "").unwrap();
        assert_eq!(status, 200);
        assert_ne!(job["status"], "failed", "{job}");
        if job["status"] == "done" {
            break job;
        }
        assert!(Instant::now() < deadline, "job timed out: {job}");
        std::thread::sleep(Duration::from_millis(20));
    };
    let (status, rows) = http(address, "GET", "/api/maps", "").unwrap();
    assert_eq!(status, 200);
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(
        done["slug"],
        format!(
            "snapshot/{}",
            rows[0]["latest_snapshot_id"].as_str().unwrap()
        )
    );
    assert_eq!(rows[0]["snapshot_count"], 1);
    // A complete snapshot offers no retry; only a failed attempt does.
    let (status, freshness) = http(
        address,
        "GET",
        &format!(
            "/api/maps/{}/{}/freshness?commit={}",
            rows[0]["owner"].as_str().unwrap(),
            rows[0]["repo"].as_str().unwrap(),
            rows[0]["commit"].as_str().unwrap()
        ),
        "",
    )
    .unwrap();
    assert_eq!(status, 200, "{freshness}");
    assert_eq!(freshness["state"], "complete", "{freshness}");
    assert_eq!(freshness["retryable"], false, "{freshness}");
}
/// Index two commits of a tiny repository through the CLI, then compare them
/// through the web routes against the retained base snapshot.
#[test]
fn compare_routes_use_retained_base_snapshot() {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        process::{Child, Command},
        time::{Duration, Instant},
    };
    struct WebProcess(Child);
    impl Drop for WebProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn http(
        address: std::net::SocketAddr,
        path: &str,
    ) -> std::io::Result<(u16, serde_json::Value)> {
        let mut stream = TcpStream::connect(address)?;
        stream.set_read_timeout(Some(Duration::from_secs(60)))?;
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
        )?;
        let mut text = String::new();
        stream.read_to_string(&mut text)?;
        let (headers, body) = text.split_once("\r\n\r\n").unwrap();
        let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        Ok((status, serde_json::from_str(body).unwrap()))
    }
    let root = tempfile::tempdir().unwrap();
    let checkout = root.path().join("repo");
    std::fs::create_dir_all(checkout.join("pkg")).unwrap();
    std::fs::write(
        checkout.join("pyproject.toml"),
        "[project]\nname='pkg'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::write(checkout.join("pkg/__init__.py"), "").unwrap();
    for i in 0..8 {
        std::fs::write(
            checkout.join(format!("pkg/f{i}.py")),
            format!(
                "from .f{} import value\ndef value():\n    return {i}\n",
                (i + 1) % 8
            ),
        )
        .unwrap();
    }
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(&checkout)
            .args(["-c", "user.name=Fixture"])
            .args(["-c", "user.email=fixture@example.invalid"])
            .args(["-c", "commit.gpgsign=false"])
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
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-qm", "base"]);
    let base = git(&["rev-parse", "HEAD"]);
    let data = root.path().join("data");
    let tolmap = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_tolmap"))
            .arg("--data")
            .arg(&data)
            .args(args)
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "tolmap {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    tolmap(&["add", checkout.to_str().unwrap()]);
    std::fs::write(
        checkout.join("pkg/f0.py"),
        "from .f1 import value\ndef value():\n    return 100\n",
    )
    .unwrap();
    git(&["commit", "-qam", "head"]);
    let head = git(&["rev-parse", "HEAD"]);
    tolmap(&["refresh", "repo"]);
    let assets = root.path().join("assets");
    std::fs::create_dir(&assets).unwrap();
    std::fs::write(
        assets.join("index.html"),
        "<html><head></head><body></body></html>",
    )
    .unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    drop(socket);
    let _web = WebProcess(
        Command::new(env!("CARGO_BIN_EXE_tolmap"))
            .arg("--data")
            .arg(&data)
            .args(["web", "--addr", &address.to_string()])
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("TOLMAP_STATIC_DIR", assets)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    while http(address, "/api/healthz").is_err() {
        assert!(Instant::now() < deadline, "web startup timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (_, rows) = http(address, "/api/maps").unwrap();
    let snapshot = rows[0]["latest_snapshot_id"].as_str().unwrap();
    let owned = format!("/api/compare/snapshot/{snapshot}");
    let (status, comparison) = http(address, &format!("{owned}?base={base}&head={head}")).unwrap();
    assert_eq!(status, 200, "{comparison}");
    assert_eq!(comparison["provenance"]["base"], base);
    assert_eq!(comparison["provenance"]["head"], head);
    let (status, debt) = http(address, &format!("{owned}/debt?base={base}&head={head}")).unwrap();
    assert_eq!(status, 200, "{debt}");
    assert_eq!(debt["base"], base);
    // An un-indexed base is unavailable; a malformed commit is a bad request.
    let unindexed = "c".repeat(40);
    let (status, error) = http(address, &format!("{owned}?base={unindexed}&head={head}")).unwrap();
    assert_eq!(status, 404, "{error}");
    assert_eq!(error["error"], "comparison_unavailable");
    let (status, _) = http(address, &format!("{owned}?base=main&head={head}")).unwrap();
    assert_eq!(status, 400);
}
