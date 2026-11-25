//! API-safe static bundle serving through the personal web router.
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;

use tolmap::personal::Library;
use tolmap::service::application_config::ApplicationConfig;
use tolmap::service::config::ServeConfig;
use tolmap::service::personal_web::{self, WebState};
use tolmap::service::store::Store;

fn state_with_static_dir(
    static_dir: Option<std::path::PathBuf>,
) -> (tempfile::TempDir, Arc<WebState>) {
    let dir = tempfile::tempdir().unwrap();
    let config = ServeConfig {
        db_path: dir.path().join("tolmap.sqlite3"),
        cache_dir: dir.path().join("cache"),
        static_dir,
        ..ServeConfig::from_env()
    };
    let state = WebState::new(Library {
        store: Store::open(&config.db_path).unwrap(),
        config: ApplicationConfig {
            data: dir.path().into(),
            service: config,
            retain_commits_per_repo: None,
        },
    });
    (dir, state)
}
fn router(state: Arc<WebState>) -> axum::Router {
    personal_web::router(
        state.library.config.service.static_dir.as_ref(),
        state.clone(),
    )
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

async fn get(state: Arc<WebState>, path: &str) -> (StatusCode, String) {
    let router = router(state);
    let request = Request::builder()
        .method("GET")
        .header("host", "localhost:8787")
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
async fn static_map_file_uses_gzip_when_accepted() {
    let bundle = bundle_dir();
    std::fs::create_dir(bundle.path().join("maps")).unwrap();
    let map = format!("{{\"files\":{:?}}}", vec!["district"; 100]);
    std::fs::write(bundle.path().join("maps/example.json"), &map).unwrap();
    let (_dir, state) = state_with_static_dir(Some(bundle.path().to_path_buf()));
    let response = router(state)
        .oneshot(
            Request::builder()
                .uri("/maps/example.json")
                .header("host", "localhost:8787")
                .header(header::ACCEPT_ENCODING, "gzip")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
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

    for path in ["/api", "/api/"] {
        let (status, body) = get(state.clone(), path).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{path} must return the API 404 instead of falling through to the SPA"
        );
        assert!(
            !body.contains("<!doctype html>") && !body.contains("<title>"),
            "expected the API's JSON 404 for {path}, got what looks like index.html: {body}"
        );
        assert!(
            body.contains("\"error\":\"not_found\""),
            "expected ApiError::not_found's JSON shape for {path}: {body}"
        );
    }

    // A real /api/* route still works normally alongside the catch-all.
    let (status, body) = get(state, "/api/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"ok\""));
}

#[tokio::test]
async fn personal_html_is_injected_before_gzip_for_root_and_snapshot() {
    use std::io::Read;
    let bundle = bundle_dir();
    std::fs::write(
        bundle.path().join("index.html"),
        format!(
            "<!doctype html><html><head><title>tolmap</title></head><body>{}</body></html>",
            "viewer content ".repeat(100)
        ),
    )
    .unwrap();
    let (_dir, state) = state_with_static_dir(Some(bundle.path().to_path_buf()));
    for path in ["/", "/snapshot/retained-id"] {
        let response = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("host", "localhost:8787")
                    .header(header::ACCEPT_ENCODING, "gzip")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let mut html = String::new();
        flate2::read::GzDecoder::new(bytes.as_ref())
            .read_to_string(&mut html)
            .unwrap();
        assert!(
            html.contains("<meta name=\"tolmap-personal\" content=\"1\">"),
            "{html}"
        );
        assert!(html.contains("<script id=\"tolmap-host\""));
        assert!(html.ends_with("</body></html>"));
    }
}
