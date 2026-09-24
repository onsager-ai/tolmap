use std::sync::Arc;
use std::{
    io::Write,
    process::{Command, Stdio},
};

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use serde_json::json;
use tower::ServiceExt;

use tolmap::service::config::{Limits, ServeConfig};
use tolmap::service::ratelimit::RateLimiter;
use tolmap::service::store::{MapRow, Store};
use tolmap::service::{http, jobs, AppState};

#[tokio::test]
async fn district_route_includes_remote_symbol_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let map_path = dir.path().join("abc.json");
    let map = json!({
        "repo": "example", "q": 0.0, "names": {},
        "districts": {
            "0": {"size": 1, "c": [0.0, 0.0], "blob": []},
            "1": {"size": 1, "c": [1.0, 1.0], "blob": []}
        },
        "F": ["a.py", "b.py"],
        "N": [[0,0.0,0.0,1,0,0,0.0,0.0,0.0,0.0,0.0], [1,1.0,1.0,1,0,0,0.0,0.0,0.0,0.0,0.0]],
        "E": [], "L": [], "S": {}, "U": {}, "roads": [], "lang": "py"
    });
    std::fs::write(&map_path, serde_json::to_vec(&map).unwrap()).unwrap();
    let symbols = json!({
        "files": [0, 1],
        "symbols": [[0,"from",1,1,2,-1,2], [1,"to",1,1,2,-1,2]],
        "edges": [[0,1,3]],
        "module_code_lines": {"0": 1, "1": 2},
        "coverage": {"calls_total": 3, "calls_resolved": 3, "unresolved": {}}
    });
    std::fs::write(
        map_path.with_extension("symbols.json"),
        serde_json::to_vec(&symbols).unwrap(),
    )
    .unwrap();
    let config = ServeConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        db_path: dir.path().join("store.sqlite3"),
        cache_dir: dir.path().join("cache"),
        static_dir: None,
        prune_variant: tolmap::pipeline::PruneVariant::NodeRelative,
        namer: tolmap::naming::NamerKind::Idf,
        namer_model: tolmap::naming::DEFAULT_MODEL.to_owned(),
        limits: Limits::default(),
        retain_commits_per_repo: 20,
        // Unused by this test -- it never spawns a worker -- so the
        // literal value doesn't matter, unlike `service::jobs`'s own test
        // helper, which needs the *current* uid/gid so its uid-drop tests
        // behave correctly whether or not the test runner is root.
        worker_uid: 0,
        worker_gid: 0,
    };
    let store = Store::open(&config.db_path).unwrap();
    store
        .insert(&MapRow {
            slug: "local/example".to_owned(),
            owner: "local".to_owned(),
            repo: "example".to_owned(),
            commit: "abc".to_owned(),
            branch: None,
            lang: "py".to_owned(),
            files: 2,
            districts: 2,
            modularity: 0.0,
            map_path,
            indexed_at: "2026-09-23T00:00:00Z".to_owned(),
        })
        .unwrap();
    let state = Arc::new(AppState {
        store,
        config,
        jobs: jobs::new_registry(),
        rate_limiter: RateLimiter::new(),
    });
    let response = http::router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/maps/LOCAL/Example/symbols?district=0&commit=abc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["files"], json!([0]));
    assert_eq!(body["symbol_indices"], json!([0, 1]));
    assert_eq!(body["edges"], json!([[0, 1, 3, 0]]));
    assert_eq!(body["kinds"][0], "unknown");
    assert_eq!(body["symbols"][0][7], false);
    assert_eq!(body["module_code_lines"], json!({"0": 1}));

    // New commits carry this exact serialized response. Corrupting the
    // historical inputs proves the ordinary path never parses either one.
    let district_dir = dir.path().join("abc.symbols");
    std::fs::create_dir(&district_dir).unwrap();
    let district_bytes = serde_json::to_vec(&body).unwrap();
    std::fs::write(district_dir.join("0.json"), &district_bytes).unwrap();
    std::fs::write(dir.path().join("abc.json"), b"invalid map").unwrap();
    std::fs::write(dir.path().join("abc.symbols.json"), b"invalid symbols").unwrap();

    let request = |gzip: bool, district: usize| {
        let mut builder = Request::builder().uri(format!(
            "/api/maps/local/example/symbols?district={district}"
        ));
        if gzip {
            builder = builder.header(header::ACCEPT_ENCODING, "gzip");
        }
        builder.body(Body::empty()).unwrap()
    };
    let plain = http::router(state.clone())
        .oneshot(request(false, 0))
        .await
        .unwrap();
    assert_eq!(plain.status(), StatusCode::OK);
    assert!(plain.headers().get(header::CONTENT_ENCODING).is_none());
    let plain_bytes = to_bytes(plain.into_body(), usize::MAX).await.unwrap();
    assert_eq!(plain_bytes.as_ref(), district_bytes.as_slice());

    let compressed = http::router(state.clone())
        .oneshot(request(true, 0))
        .await
        .unwrap();
    assert_eq!(compressed.status(), StatusCode::OK);
    assert_eq!(compressed.headers()[header::CONTENT_ENCODING], "gzip");
    let compressed_bytes = to_bytes(compressed.into_body(), usize::MAX).await.unwrap();
    let mut gzip = Command::new("gzip")
        .arg("-dc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    gzip.stdin
        .take()
        .unwrap()
        .write_all(&compressed_bytes)
        .unwrap();
    let decoded = gzip.wait_with_output().unwrap();
    assert!(decoded.status.success());
    let decoded_json: serde_json::Value = serde_json::from_slice(&decoded.stdout).unwrap();
    let plain_json: serde_json::Value = serde_json::from_slice(&plain_bytes).unwrap();
    assert_eq!(decoded_json, plain_json);

    let missing = http::router(state)
        .oneshot(request(false, 1))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}
