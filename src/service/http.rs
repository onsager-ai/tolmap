//! API-safe static bundle and SPA fallback for the personal web application.
use super::error::ApiError;
use axum::{routing::any, Router};
use tower_http::services::{ServeDir, ServeFile};

pub(super) fn static_files<S: Clone + Send + Sync + 'static>(
    mut router: Router<S>,
    static_dir: Option<&std::path::PathBuf>,
) -> Router<S> {
    // Opt-in (TOLMAP_STATIC_DIR unset keeps this identical to before it
    // existed): serve the built web bundle for one-origin deployment
    // (Fly.io, 2026-09-20), with an SPA fallback to `index.html` so
    // TanStack Router can own client-side paths like `/django/django`
    // that have no file on disk -- docs/ARCHITECTURE.md's "MVP: a site
    // that maps any public repository".
    //
    // Two services, not one `fallback_service(serve_dir)`: a plain
    // ServeDir-with-not-found-service fallback would also catch an
    // unmatched `/api/*` path (nothing above matches `/api/nonexistent`)
    // and serve it `index.html` -- turning a contract error into a JSON
    // parse error in the client, exactly what docs/API.md's "every non-2xx
    // response is JSON" promises callers never happens. The catch-all
    // route registered here wins over the outer fallback for every path
    // under `/api/` (axum/matchit prefer a literal route -- `/api/index`,
    // `/api/healthz`, etc. -- over a same-router catch-all, so this only
    // ever catches what nothing above already matched), so `/api/*` always
    // gets `ApiError::not_found`, never the SPA fallback. See
    // tests/static_serving.rs.
    if let Some(static_dir) = static_dir {
        // `.fallback(ServeFile::new(index_html))`, not tower-http's
        // `not_found_service` -- that helper (`SetStatus`) forces the
        // response status to 404 even when it successfully serves
        // `index.html`'s body, which is right for "serve a custom 404
        // page" and wrong here: a cold load of a client-routed path like
        // `/django/django` is a real page, not a broken link, and must
        // answer 200 the way `GET /` does. `.fallback()` alone keeps
        // whatever status `ServeFile` naturally returns for a successful
        // read, which is 200. See tests/static_serving.rs.
        let index_html = static_dir.join("index.html");
        let serve_dir = ServeDir::new(static_dir).fallback(ServeFile::new(index_html));
        router = router
            .route("/api", any(api_not_found))
            .route("/api/", any(api_not_found))
            .route("/api/{*rest}", any(api_not_found))
            .fallback_service(serve_dir);
    }

    router
}

/// Catch-all for any `/api/*` path none of the explicit routes above
/// matched -- registered only when static serving is on, so `/api/*`
/// never falls through to the SPA's `index.html` fallback (see `router`'s
/// doc comment). With static serving off there is no outer fallback for
/// this to protect against, so it is not registered at all: the router
/// must stay byte-for-byte what it was when `TOLMAP_STATIC_DIR` is unset.
async fn api_not_found(uri: axum::http::Uri) -> ApiError {
    ApiError::not_found(format!("no route for {}", uri.path()))
}
