//! Milestone 3 (issue #5): the job service. `tolmap serve` accepts a
//! repository, indexes it as a background job, streams progress over SSE,
//! and serves the resulting map -- docs/API.md is the contract this module
//! implements.

pub mod agent;
pub mod clone;
pub mod config;
pub mod error;
pub mod eta;
pub mod executor;
pub mod http;
pub mod jobs;
pub mod ratelimit;
pub mod schedule;
pub mod store;
mod time;
mod worker_result;
pub mod workers;

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

/// Starts the service and runs until a shutdown signal is received (or the
/// process is killed outright, which none of this can do anything about).
/// Binds `config.bind` -- always loopback, see `ServeConfig`'s doc comment
/// and docs/API.md; there is no parameter anywhere in this module that
/// widens that.
pub async fn serve(config: ServeConfig) -> Result<()> {
    // Read before anything starts, so a bad `TOLMAP_WORKERS` is a startup
    // error rather than a half-started service. Unset is local mode,
    // exactly as before it existed.
    let workers_mode = workers::WorkersMode::from_env()?;
    let store = Store::open(&config.db_path)
        .with_context(|| format!("open store at {}", config.db_path.display()))?;
    let bind = config.bind;
    let jobs = jobs::new_registry();
    jobs.load_timings(&store)?;
    let state = Arc::new(AppState {
        store,
        config,
        jobs,
        rate_limiter: RateLimiter::new(),
    });

    // Fly keep-alive: investigated for PR #120 (`auto_stop_machines =
    // "stop"` / `min_machines_running = 0`), and deliberately NOT shipped as
    // in-process code -- see this PR's description for the full research
    // trail; summary below for whoever reads this next.
    //
    // Attempt 1 (built, then removed): a periodic GET to this service's own
    // public `/api/healthz` through Fly Proxy while a job was queued or
    // running. Removed once Fly's own docs
    // (docs.fly.io/blueprints/long-running-tasks/) confirmed it does not
    // work: "Empirically, sending a successful HTTP request every 60
    // seconds from a machine to its own `<app>.fly.dev` hostname does not
    // prevent autostop."
    //
    // Attempt 2 (investigated, not built): holding one long-lived
    // connection open through the public proxy (SSE, or a dedicated
    // streaming endpoint) for as long as a job is active, on the theory
    // that Fly Proxy's `http_service.concurrency` setting -- which the docs
    // say directly "configures how to measure load for an application to
    // inform Fly Proxy load balancing and autostop/autostart"
    // (docs.fly.io/apps/concurrency/) -- would keep the Machine's measured
    // load above zero for as long as the connection stayed open, unlike a
    // request that completes in milliseconds. NOT built: this is
    // architecturally plausible but Fly's own docs never confirm it for the
    // autostop case specifically, and there is an unresolved, unrebutted
    // community report of exactly this failing -- a Machine auto-stopped
    // despite an open WebSocket connection that had already been running
    // for 4+ hours
    // (community.fly.io/t/server-auto-scaling-despite-websocket-connection/19900),
    // with no Fly staff reply explaining why. Shipping a new streaming
    // endpoint plus a dedicated reconnect-with-backoff thread on that
    // footing would be an unverified guess dressed up as a fix, so it
    // wasn't built.
    //
    // What's actually needed is an owner decision between two real options,
    // neither of which this code can pick for itself:
    //   (a) don't auto-stop this app at all (`auto_stop_machines` off --
    //       Fly's own "shape A" from the long-running-tasks blueprint):
    //       costs more (the machine runs continuously) but removes this
    //       whole risk category outright; or
    //   (b) have this service call the Fly Machines API directly to
    //       suspend/lift auto-stop around a job's lifetime: needs a Fly API
    //       token as a new credential, which is a security decision, not
    //       something to wire up unilaterally.
    // Whichever way that goes, the graceful-shutdown handling below is
    // unconditionally correct and does not depend on the answer -- it is
    // what turns "Fly stops the Machine mid-job anyway" from a silent loss
    // into a clean, observable failure.

    // `TOLMAP_WORKERS=loopback:N` (#97 phase 1, docs/WORKER_TIER.md §8):
    // the worker listener on loopback and N agents. Local mode starts
    // neither.
    let loopback = match workers_mode {
        workers::WorkersMode::Local => None,
        workers::WorkersMode::Loopback(agents) => {
            Some(workers::start_loopback(&state, agents).await?)
        }
    };

    let app = http::router(state.clone());
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    eprintln!("tolmap serve: listening on http://{bind}");

    // Ordering this depends on: `with_graceful_shutdown`'s future controls
    // when axum stops accepting new connections and begins winding down
    // existing ones (it does NOT forcibly cut an in-flight response short --
    // it lets it finish). `shutdown_signal` below runs `jobs.shutdown()`
    // to completion -- draining the queue, failing every running job, and
    // pushing a terminal `server_stopping` snapshot through each job's
    // `watch::Sender` -- and only *then* resolves. So every open
    // `GET /api/jobs/{id}/events` SSE stream still has its connection alive
    // and still gets to see that final snapshot on its own next poll (see
    // `http::get_job_events`'s 250ms tick) and send it before axum's
    // graceful drain closes the connection out from under it. Sequencing it
    // any other way -- e.g. resolving the shutdown future first and racing
    // `jobs.shutdown()` in a spawned task -- would let axum start tearing
    // down connections concurrently with jobs still being failed, risking
    // exactly the silently-dropped final frame this exists to prevent.
    let shutdown_state = state.clone();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        eprintln!("tolmap serve: shutdown signal received, draining jobs");
        shutdown_state.jobs.shutdown();
        // Loopback mode: the jobs above are failed with `server_stopping`
        // as in local mode (phase 1 keeps its state in memory); then every
        // agent is told `shutdown now` and reaped.
        if let Some(loopback) = loopback {
            loopback.shutdown().await;
        }
    })
    .await
    .context("axum::serve")?;
    eprintln!("tolmap serve: shut down cleanly");
    Ok(())
}

/// Resolves on SIGINT (`Ctrl+C`, `tokio::signal::ctrl_c`) or, on Unix,
/// SIGTERM (what Fly and most process supervisors send first, including
/// Fly's own auto-stop and `fly deploy`). No signal handling existed in
/// this codebase before this change -- see this PR's description.
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
