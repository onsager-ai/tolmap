//! Milestone 3 (issue #5): the job service. `tolmap serve` accepts a
//! repository, indexes it as a background job, streams progress over SSE,
//! and serves the resulting map -- docs/API.md is the contract this module
//! implements.

pub mod clone;
pub mod config;
pub mod error;
pub mod http;
pub mod jobs;
pub mod ratelimit;
pub mod store;
mod time;

use std::sync::Arc;

use anyhow::{Context, Result};

use config::ServeConfig;
use ratelimit::RateLimiter;
use store::Store;

/// Shared state behind every handler, held as `Arc<AppState>`.
pub struct AppState {
    pub store: Store,
    pub config: ServeConfig,
    pub jobs: jobs::JobRegistry,
    pub rate_limiter: RateLimiter,
}

/// Starts the service and runs until the process is killed. Binds
/// `config.bind` -- always loopback, see `ServeConfig`'s doc comment and
/// docs/API.md; there is no parameter anywhere in this module that widens
/// that.
pub async fn serve(config: ServeConfig) -> Result<()> {
    let store = Store::open(&config.db_path)
        .with_context(|| format!("open store at {}", config.db_path.display()))?;
    let bind = config.bind;
    let state = Arc::new(AppState {
        store,
        config,
        jobs: jobs::new_registry(),
        rate_limiter: RateLimiter::new(),
    });
    let app = http::router(state);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    eprintln!("tolmap serve: listening on http://{bind}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .context("axum::serve")?;
    Ok(())
}
