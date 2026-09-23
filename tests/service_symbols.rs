use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
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
    let response = http::router(state)
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
    assert_eq!(body["edges"], json!([[0, 1, 3]]));
    assert_eq!(body["module_code_lines"], json!({"0": 1}));
}
