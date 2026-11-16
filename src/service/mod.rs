//! Shared personal library storage, executor and browser adapter.
pub mod application_config;
pub mod clone;
pub mod config;
pub mod debt;
pub mod error;
pub mod eta;
pub mod executor;
pub mod http;
pub mod jobs;
pub mod lens;
pub mod personal_web;
pub mod store;
mod time;
pub mod worker_result;

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install ctrl+c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
