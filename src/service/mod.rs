//! Milestone 3 (issue #5): the job service. `tolmap serve` accepts a
//! repository, indexes it as a background job, streams progress over SSE,
//! and serves the resulting map -- docs/API.md is the contract this module
//! implements.

pub mod clone;
pub mod config;
pub mod error;
pub mod eta;
pub mod http;
pub mod jobs;
pub mod ratelimit;
pub mod store;
mod time;

use std::sync::Arc;
use std::time::Duration;

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

    // Fly keep-alive (issue: PR #120 turns on `auto_stop_machines = "stop"`
    // / `min_machines_running = 0`). `FLY_APP_NAME` is set automatically
    // inside a running Fly Machine (docs.fly.io/reference/runtime-
    // environment/: "Each app running on Fly.io has a unique app name...")
    // and absent everywhere else (local dev, Railway staging), so this is a
    // complete no-op off Fly with no separate feature flag needed.
    //
    // IMPORTANT CAVEAT, found while implementing this and not assumed: Fly's
    // own docs (docs.fly.io/blueprints/long-running-tasks/) say plainly that
    // this exact tactic does not reliably work --
    // "Empirically, sending a successful HTTP request every 60 seconds from
    // a machine to its own `<app>.fly.dev` hostname does not prevent
    // autostop" -- because Fly Proxy's autostop loop looks at whether the
    // Machine currently has any traffic (a live, open connection), not at
    // when it last saw a request; a periodic GET that completes in
    // milliseconds is at "zero load" the rest of the time, same as no ping
    // at all. The two mechanisms Fly documents as actually working --
    // `auto_stop_machines = "off"` entirely, or moving background work into
    // a process group with no `[http_service]` -- are both incompatible
    // with what #120 wants (an idle *and* auto-stoppable single web+worker
    // machine). So this loop is kept as a cheap, harmless best-effort nudge
    // (it costs nothing and is what was asked for), but it is NOT the fix
    // for the underlying risk; `jobs::JobRegistry::shutdown` below (wired
    // through `shutdown_signal`) is what actually closes it: whether or not
    // this ping keeps the Machine up, a Machine that Fly stops mid-job now
    // fails that job cleanly with `server_stopping` instead of the job
    // silently vanishing. See this PR's description for the full citation
    // trail.
    if let Ok(app_name) = std::env::var("FLY_APP_NAME") {
        tokio::spawn(fly_keepalive(state.clone(), app_name));
    }

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

/// While any job is queued or running, pings this service's own public URL
/// through Fly Proxy every ~30s so Fly sees live traffic during a job --
/// see `serve`'s doc comment for the citation and the caveat that this is
/// best-effort, not a guarantee. Only spawned when `FLY_APP_NAME` is set.
/// `ureq` (already a dependency -- see `naming::ModelNamer` for the same
/// blocking-HTTPS-call-from-async pattern) does the request on a blocking
/// thread via `spawn_blocking`, never on the async runtime; a failed ping
/// (DNS hiccup, transient network error, whatever) only logs and retries
/// next tick -- it must never be worse than a no-op for real request
/// handling.
async fn fly_keepalive(state: Arc<AppState>, app_name: String) {
    let url = format!("https://{app_name}.fly.dev/api/healthz");
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        if !state.jobs.has_active_jobs() {
            continue;
        }
        let ping_url = url.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            let config = ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(5)))
                .build();
            ureq::Agent::new_with_config(config).get(&ping_url).call()
        })
        .await;
        match outcome {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => eprintln!("fly keep-alive ping to {url} failed: {error}"),
            Err(join_error) => eprintln!("fly keep-alive ping task panicked: {join_error}"),
        }
    }
}
