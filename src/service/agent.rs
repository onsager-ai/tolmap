//! The worker agent, `tolmap worker --connect <url> --token-file <path>
//! --cache-dir <dir>` (docs/WORKER_TIER.md §2, §2.3, §2.4, §3; #97 phase
//! 1): a long-lived process that dials the master, takes one job at a time
//! and runs it with the same `executor::execute` local mode runs, in its
//! own cache and job directories. It plays the part `tolmap serve` plays
//! for its job child in local mode; the job child itself is unchanged.
//!
//! **Why the synchronous WebSocket client.** The brief names
//! `tokio-tungstenite`, and this is its client, used through the crate's
//! own re-export of the synchronous `tungstenite` core: sending on
//! `tokio-tungstenite`'s async stream needs the `futures` `Sink` trait,
//! which no dependency of this crate exports, and adding one is not this
//! change's call. The agent is blocking by nature anyway -- the executor,
//! the job child's pipes and `ureq` all are -- so one thread owns the
//! socket, reading with a short timeout and writing what the job thread
//! queued in between.
//!
//! **Phase 1 limits, on purpose.** Plain `ws://` to a loopback master only;
//! no resume, so a lost channel kills the job and the agent exits non-zero
//! for its supervisor to restart; one slot.

use std::io::Read;
use std::net::{IpAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::tungstenite::{self, ClientRequestBuilder, Message, WebSocket};
use uuid::Uuid;

use crate::naming::{NameCache, NamerKind};
use crate::progress::StageId;
use crate::service::config::ServeConfig;
use crate::service::error::{ApiError, ErrorBody};
use crate::service::executor::{
    self, CancelProbe, EventSink, ExecEnv, Executed, JobInputs, PreviousMapInput,
};
use crate::service::jobs::worker_uid_in_effect;
use crate::service::worker_result;
use crate::service::workers::{own_build, sha256_reader, SHA256_HEADER};
use crate::worker::{
    Artifact, AssignInputs, CancelReason, HeartbeatJob, JobSpec, MasterMessage, ReleasedReason,
    ShutdownMode, WorkerClass, WorkerEvent, WorkerMessage, FEATURE_INSTALL_SANDBOX,
    FEATURE_LOCAL_PATHS, PROTO,
};

/// How long one read waits before the loop sends what the job thread
/// queued and checks the heartbeat.
const READ_POLL: Duration = Duration::from_millis(100);
const WELCOME_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `shutdown now` waits for a killed job's thread to report.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);
/// The same bound `worker_result` puts on a names cache it reads back.
const NAMES_CACHE_MAX_BYTES: u64 = 16 << 20;

/// What `tolmap worker --connect` was given.
pub struct AgentConfig {
    pub connect: String,
    pub token_file: PathBuf,
    pub cache_dir: PathBuf,
    /// The binary each job child runs as `tolmap worker`: this one.
    pub worker_exe: PathBuf,
}

/// The master this agent dials.
#[derive(Clone)]
struct Endpoint {
    uri: Uri,
    host: String,
    port: u16,
    /// `http://<host:port>`: the only origin artifact requests may go to.
    origin: String,
}

impl Endpoint {
    fn parse(url: &str) -> Result<Self> {
        let uri: Uri = url
            .parse()
            .with_context(|| format!("--connect {url:?} is not a URL"))?;
        // Trust decision (§5.1): the bearer token crosses this connection,
        // and TLS is required for anything but loopback. Phase 1 has only
        // loopback agents, so it speaks plain `ws://` to a loopback master
        // and nothing else; `wss://` and remote masters arrive with the
        // first remote worker (phase 3).
        if uri.scheme_str() != Some("ws") {
            bail!("--connect must be a ws:// URL: phase 1 agents dial a loopback master only");
        }
        let authority = uri
            .authority()
            .context("--connect has no host")?
            .as_str()
            .to_owned();
        if authority.contains('@') {
            bail!("--connect must not carry credentials in the URL");
        }
        let host = uri
            .host()
            .context("--connect has no host")?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let loopback =
            host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
        if !loopback {
            bail!("--connect {url:?} is not a loopback address: phase 1 agents dial a loopback master only");
        }
        let port = uri.port_u16().unwrap_or(80);
        Ok(Endpoint {
            origin: format!("http://{authority}"),
            uri,
            host,
            port,
        })
    }

    /// Trust decision: artifact requests carry the token, so they go only
    /// to the artifact routes of the master this agent dialled, whatever
    /// URL an `assign` names.
    fn owns(&self, url: &str) -> bool {
        url.starts_with(&format!("{}/workers/artifacts/", self.origin))
    }
}

/// This host's side of a job, from the agent's flags and environment.
#[derive(Clone)]
struct HostEnv {
    cache_dir: PathBuf,
    clone_cache_bytes: u64,
    worker_exe: PathBuf,
    worker_uid: u32,
    worker_gid: u32,
}

impl HostEnv {
    /// Everything under the agent's own `--cache-dir`, never the master's
    /// store (§8 phase 1).
    fn exec_env(&self, job: &JobSpec) -> ExecEnv {
        ExecEnv {
            clone_cache: self.cache_dir.clone(),
            clone_cache_bytes: self.clone_cache_bytes,
            job_root: self.cache_dir.join("work"),
            install_root: self.cache_dir.join("install"),
            worker_exe: self.worker_exe.clone(),
            worker_uid: self.worker_uid,
            worker_gid: self.worker_gid,
            // As local mode decides it: only a model-named job's child may
            // see the key, and only if this process has one. A loopback
            // agent inherits the master's environment; a remote worker host
            // never holds the key (§1, §5.2).
            allow_openrouter_key: job
                .namer
                .parse::<NamerKind>()
                .is_ok_and(|namer| namer == NamerKind::Model),
        }
    }
}

type Socket = WebSocket<TcpStream>;

fn send(socket: &mut Socket, message: &WorkerMessage) -> Result<()> {
    let text = serde_json::to_string(message).context("serialize a channel message")?;
    socket
        .send(Message::text(text))
        .context("send on the channel to the master")
}

fn would_block(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn read_token(path: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read the token file {}", path.display()))?;
    let token = raw.trim().to_owned();
    if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
        bail!("the token file {} does not hold a token", path.display());
    }
    Ok(token)
}

/// `hello.class` (§2.1): physical memory less a 1 GiB reserve for the
/// agent and the OS, and the CPUs this process may use. Phase 1 schedules
/// on nothing but the one local class; this is what phase 2 will read.
fn host_class() -> WorkerClass {
    #[cfg(unix)]
    let memory_bytes = {
        // SAFETY: `sysconf` reads a system constant and has no
        // preconditions.
        let (pages, size) = unsafe {
            (
                libc::sysconf(libc::_SC_PHYS_PAGES),
                libc::sysconf(libc::_SC_PAGESIZE),
            )
        };
        if pages > 0 && size > 0 {
            (pages as u64).saturating_mul(size as u64)
        } else {
            0
        }
    };
    #[cfg(not(unix))]
    let memory_bytes = 0u64;
    WorkerClass {
        memory_bytes: memory_bytes.saturating_sub(1 << 30),
        cpus: std::thread::available_parallelism()
            .map(|count| count.get() as u32)
            .unwrap_or(1),
    }
}

fn released_reason(reason: CancelReason) -> ReleasedReason {
    match reason {
        CancelReason::Cancelled => ReleasedReason::Cancelled,
        CancelReason::LeaseLost => ReleasedReason::LeaseLost,
        CancelReason::ServerStopping => ReleasedReason::ServerStopping,
        CancelReason::Reroute => ReleasedReason::Reroute,
    }
}

fn internal(message: impl std::fmt::Display) -> ErrorBody {
    ApiError::internal(message.to_string()).body
}

/// Runs the agent until the master sends `shutdown` (`Ok`) or the channel
/// is lost (`Err`, so the process exits non-zero and its supervisor
/// restarts it).
pub fn run(config: AgentConfig) -> Result<()> {
    let token = read_token(&config.token_file)?;
    let endpoint = Endpoint::parse(&config.connect)?;
    std::fs::create_dir_all(&config.cache_dir)
        .with_context(|| format!("create {}", config.cache_dir.display()))?;
    // The same variables local mode reads for its job children: the uid
    // they drop to and the clone cache budget.
    let settings = ServeConfig::from_env();
    let host = HostEnv {
        cache_dir: config.cache_dir.clone(),
        clone_cache_bytes: settings.limits.clone_cache_bytes,
        worker_exe: config.worker_exe.clone(),
        worker_uid: settings.worker_uid,
        worker_gid: settings.worker_gid,
    };
    let stream = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
        .with_context(|| format!("dial {}", endpoint.origin))?;
    let _ = stream.set_nodelay(true);
    // The token goes in the upgrade's `Authorization` header, never in the
    // URL (§5.1).
    let request = ClientRequestBuilder::new(endpoint.uri.clone())
        .with_header("Authorization", format!("Bearer {token}"));
    let (mut socket, _) = tungstenite::client(request, stream)
        .map_err(|error| anyhow!("the master refused the channel: {error}"))?;
    socket
        .get_ref()
        .set_read_timeout(Some(READ_POLL))
        .context("set the channel's read timeout")?;
    let worker_id = format!("agent-{}", std::process::id());
    send(
        &mut socket,
        &WorkerMessage::Hello {
            proto_min: PROTO,
            proto_max: PROTO,
            worker_id: worker_id.clone(),
            build: own_build(),
            class: host_class(),
            slots: 1,
            // `local_paths`: this agent shares the master's host (it only
            // ever dials loopback). No `resume`: phase 1 has none.
            features: vec![
                FEATURE_LOCAL_PATHS.to_owned(),
                FEATURE_INSTALL_SANDBOX.to_owned(),
            ],
            resume: Vec::new(),
        },
    )?;
    let heartbeat = wait_for_welcome(&mut socket)?;
    send(&mut socket, &WorkerMessage::Ready { slots_free: 1 })?;
    eprintln!("worker agent {worker_id}: connected to {}", endpoint.origin);
    let (to_main, from_jobs) = mpsc::channel();
    Agent {
        socket,
        endpoint,
        token,
        host,
        heartbeat,
        to_main,
        from_jobs,
        current: None,
        draining: false,
    }
    .run()
}

fn wait_for_welcome(socket: &mut Socket) -> Result<Duration> {
    let deadline = Instant::now() + WELCOME_TIMEOUT;
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                match serde_json::from_str::<MasterMessage>(text.as_str())
                    .context("an unreadable frame from the master")?
                {
                    MasterMessage::Welcome { heartbeat_s, .. } => {
                        return Ok(Duration::from_secs(heartbeat_s.max(1)))
                    }
                    MasterMessage::Error { code, message } => {
                        bail!("the master refused this agent: {code}: {message}")
                    }
                    other => bail!("expected welcome from the master, got {other:?}"),
                }
            }
            Ok(Message::Close(_)) => bail!("the master closed the channel before welcome"),
            Ok(_) => {}
            Err(tungstenite::Error::Io(error)) if would_block(&error) => {}
            Err(error) => return Err(error).context("read welcome"),
        }
        if Instant::now() > deadline {
            bail!("no welcome from the master");
        }
    }
}

/// What the job thread tells the socket thread.
enum FromJob {
    Send(WorkerMessage),
    Finished {
        outcome: Outcome,
        peak_rss_bytes: Option<u64>,
    },
}

enum Outcome {
    /// A `result` naming uploaded artifacts.
    Result(WorkerEvent),
    Failed(ErrorBody),
    Cancelled,
}

/// The job this agent holds.
struct Current {
    job_id: String,
    epoch: u64,
    /// The last `seq` sent for it; the job thread and the socket thread
    /// both take the next one from here.
    seq: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    child: Arc<Mutex<Option<u32>>>,
    /// Set by the master's `cancel`: the job ends in `released` with this
    /// reason, whatever it produced.
    released: Option<ReleasedReason>,
    /// The result is sent and the master has not answered yet.
    awaiting_verdict: bool,
}

impl Current {
    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Stops the job: the flag the executor checks, and the job child's
    /// process group if it has one. The flag is set before the lock is
    /// taken, and `AgentProbe::child_started` checks it after taking the
    /// lock, so a child that starts meanwhile is killed by one side or the
    /// other.
    fn kill(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(pid) = *self.child.lock().expect("agent child mutex poisoned") {
            executor::kill_worker_group(pid);
        }
    }
}

struct Agent {
    socket: Socket,
    endpoint: Endpoint,
    token: String,
    host: HostEnv,
    heartbeat: Duration,
    to_main: mpsc::Sender<FromJob>,
    from_jobs: mpsc::Receiver<FromJob>,
    current: Option<Current>,
    draining: bool,
}

impl Agent {
    fn run(mut self) -> Result<()> {
        let mut next_heartbeat = Instant::now() + self.heartbeat;
        loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    let message = serde_json::from_str::<MasterMessage>(text.as_str());
                    let handled = message
                        .context("an unreadable frame from the master")
                        .and_then(|message| self.on_message(message));
                    match handled {
                        Ok(true) => return Ok(()),
                        Ok(false) => {}
                        Err(error) => return self.lost(format!("{error:#}")),
                    }
                }
                Ok(Message::Close(_)) => return self.lost("the master closed the channel".into()),
                Ok(_) => {}
                Err(tungstenite::Error::Io(error)) if would_block(&error) => {}
                Err(error) => return self.lost(error.to_string()),
            }
            while let Ok(item) = self.from_jobs.try_recv() {
                let sent = match item {
                    FromJob::Send(message) => send(&mut self.socket, &message),
                    FromJob::Finished {
                        outcome,
                        peak_rss_bytes,
                    } => self.finished(outcome, peak_rss_bytes),
                };
                if let Err(error) = sent {
                    return self.lost(format!("{error:#}"));
                }
                if self.draining && self.current.is_none() {
                    self.close();
                    return Ok(());
                }
            }
            if Instant::now() >= next_heartbeat {
                next_heartbeat = Instant::now() + self.heartbeat;
                let jobs = self
                    .current
                    .iter()
                    .map(|current| HeartbeatJob {
                        job_id: current.job_id.clone(),
                        epoch: current.epoch,
                        last_seq: current.seq.load(Ordering::SeqCst),
                    })
                    .collect();
                let beat = WorkerMessage::Heartbeat {
                    jobs,
                    rss_bytes: None,
                };
                if let Err(error) = send(&mut self.socket, &beat) {
                    return self.lost(format!("{error:#}"));
                }
            }
        }
    }

    /// No resume in phase 1: whatever this agent runs dies with the
    /// channel, and the process exits non-zero so its supervisor starts a
    /// fresh one.
    fn lost(&self, reason: String) -> Result<()> {
        if let Some(current) = &self.current {
            current.kill();
        }
        Err(anyhow!(
            "the channel to the master was lost ({reason}); with no resume in phase 1, \
             the job this agent held, if any, was killed"
        ))
    }

    fn close(&mut self) {
        let _ = self.socket.close(None);
        let _ = self.socket.flush();
    }

    /// Ready for the next job, or, while draining, done.
    fn idle(&mut self) -> Result<()> {
        if self.draining {
            return Ok(());
        }
        send(&mut self.socket, &WorkerMessage::Ready { slots_free: 1 })
    }

    fn holds(&self, job_id: &str, epoch: u64) -> bool {
        self.current
            .as_ref()
            .is_some_and(|current| current.job_id == job_id && current.epoch == epoch)
    }

    /// `Ok(true)`: exit cleanly.
    fn on_message(&mut self, message: MasterMessage) -> Result<bool> {
        match message {
            MasterMessage::Assign {
                job_id,
                epoch,
                job,
                inputs,
                outputs,
                ..
            } => {
                if self.current.is_some() || self.draining {
                    // Not free for it: say so at once rather than let the
                    // master wait out the lease.
                    send(
                        &mut self.socket,
                        &WorkerMessage::JobEvent {
                            job_id,
                            epoch,
                            seq: 1,
                            event: WorkerEvent::Error {
                                v: 1,
                                code: "worker_crashed".to_owned(),
                                message: "the agent was not free for this job".to_owned(),
                            },
                            peak_rss_bytes: None,
                        },
                    )?;
                } else {
                    self.start(job_id, epoch, job, inputs, outputs);
                }
            }
            MasterMessage::Cancel {
                job_id,
                epoch,
                reason,
            } => {
                let reason = released_reason(reason);
                if self.holds(&job_id, epoch) {
                    let current = self.current.as_mut().expect("held");
                    current.released = Some(reason);
                    current.kill();
                    // A job still running reports `released` from
                    // `finished`, once its thread is done with it. One whose
                    // result is already sent holds nothing any more.
                    if current.awaiting_verdict {
                        self.current = None;
                        self.released(job_id, epoch, reason, None)?;
                        self.idle()?;
                    }
                } else {
                    // Nothing held for it (already finished here): release
                    // at once so the master frees the slot.
                    self.released(job_id, epoch, reason, None)?;
                }
            }
            MasterMessage::ResultAccepted { job_id, epoch, .. } => {
                if self.holds(&job_id, epoch)
                    && self.current.as_ref().is_some_and(|c| c.awaiting_verdict)
                {
                    self.current = None;
                    self.idle()?;
                }
            }
            MasterMessage::ResultRejected {
                job_id,
                epoch,
                reason,
            } => {
                if self.holds(&job_id, epoch)
                    && self.current.as_ref().is_some_and(|c| c.awaiting_verdict)
                {
                    eprintln!("job {job_id}: the master rejected the result: {reason}");
                    self.current = None;
                    self.idle()?;
                }
            }
            MasterMessage::Shutdown { mode, reason } => {
                eprintln!("worker agent: the master asked it to stop ({mode:?}): {reason}");
                match mode {
                    ShutdownMode::Now => {
                        self.stop_now();
                        return Ok(true);
                    }
                    ShutdownMode::Drain => {
                        self.draining = true;
                        send(&mut self.socket, &WorkerMessage::Draining)?;
                        if self.current.is_none() {
                            self.close();
                            return Ok(true);
                        }
                    }
                }
            }
            MasterMessage::Error { code, message } => {
                bail!("the master reported a protocol error: {code}: {message}")
            }
            MasterMessage::LeaseRenewed { .. } | MasterMessage::Welcome { .. } => {}
        }
        Ok(false)
    }

    fn released(
        &mut self,
        job_id: String,
        epoch: u64,
        reason: ReleasedReason,
        peak_rss_bytes: Option<u64>,
    ) -> Result<()> {
        send(
            &mut self.socket,
            &WorkerMessage::Released {
                job_id,
                epoch,
                reason,
                peak_rss_bytes,
            },
        )
    }

    /// `shutdown now`: kill the job, give its thread a moment to report,
    /// release it, close.
    fn stop_now(&mut self) {
        if let Some(current) = &self.current {
            current.kill();
            let (job_id, epoch) = (current.job_id.clone(), current.epoch);
            let deadline = Instant::now() + SHUTDOWN_GRACE;
            let mut peak = None;
            while Instant::now() < deadline {
                match self.from_jobs.recv_timeout(Duration::from_millis(100)) {
                    Ok(FromJob::Finished { peak_rss_bytes, .. }) => {
                        peak = peak_rss_bytes;
                        break;
                    }
                    Ok(FromJob::Send(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            self.current = None;
            let _ = self.released(job_id, epoch, ReleasedReason::ServerStopping, peak);
        }
        self.close();
    }

    /// The job thread is done with its job.
    fn finished(&mut self, outcome: Outcome, peak_rss_bytes: Option<u64>) -> Result<()> {
        let Some(current) = self.current.as_mut() else {
            return Ok(());
        };
        let (job_id, epoch) = (current.job_id.clone(), current.epoch);
        if let Some(reason) = current.released {
            self.current = None;
            self.released(job_id, epoch, reason, peak_rss_bytes)?;
            return self.idle();
        }
        match outcome {
            Outcome::Result(event) => {
                let seq = current.next_seq();
                current.awaiting_verdict = true;
                send(
                    &mut self.socket,
                    &WorkerMessage::JobEvent {
                        job_id,
                        epoch,
                        seq,
                        event,
                        peak_rss_bytes,
                    },
                )
            }
            Outcome::Failed(error) => {
                let seq = current.next_seq();
                self.current = None;
                send(
                    &mut self.socket,
                    &WorkerMessage::JobEvent {
                        job_id,
                        epoch,
                        seq,
                        event: WorkerEvent::Error {
                            v: 1,
                            code: error.error,
                            message: error.message,
                        },
                        peak_rss_bytes,
                    },
                )?;
                self.idle()
            }
            // Only a lost channel cancels without the master's `cancel`,
            // and that ends this process first; release it all the same.
            Outcome::Cancelled => {
                self.current = None;
                self.released(job_id, epoch, ReleasedReason::Cancelled, peak_rss_bytes)?;
                self.idle()
            }
        }
    }

    fn start(
        &mut self,
        job_id: String,
        epoch: u64,
        job: JobSpec,
        inputs: AssignInputs,
        outputs: String,
    ) {
        let seq = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None));
        self.current = Some(Current {
            job_id: job_id.clone(),
            epoch,
            seq: seq.clone(),
            cancel: cancel.clone(),
            child: child.clone(),
            released: None,
            awaiting_verdict: false,
        });
        let context = JobContext {
            job_id,
            epoch,
            job,
            inputs,
            outputs,
            endpoint: self.endpoint.clone(),
            token: self.token.clone(),
            host: self.host.clone(),
            seq,
            cancel,
            child,
            out: self.to_main.clone(),
        };
        std::thread::spawn(move || context.run());
    }
}

/// The agent's [`EventSink`]: each event becomes a `job_event` with the
/// next `seq` (§3.1). The executor's own clone, which local mode reports
/// through `clone_started`/`clone_finished`, crosses as the v1
/// `stage_started`/`stage_finished` pair for `clone`, the first of its kind
/// the master sees for the job; the master turns them back into the same
/// two calls on its own sink (`workers::run_remote`). `result` and `error`
/// are not forwarded here: the executor also returns them, and the agent
/// sends the terminal event itself once it has checked and uploaded the
/// result.
struct AgentSink {
    out: mpsc::Sender<FromJob>,
    job_id: String,
    epoch: u64,
    seq: Arc<AtomicU64>,
    peak_rss_bytes: Option<u64>,
}

impl AgentSink {
    fn forward(&self, event: WorkerEvent) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.out.send(FromJob::Send(WorkerMessage::JobEvent {
            job_id: self.job_id.clone(),
            epoch: self.epoch,
            seq,
            event,
            peak_rss_bytes: None,
        }));
    }
}

impl EventSink for AgentSink {
    fn clone_started(&mut self) {
        self.forward(WorkerEvent::StageStarted {
            v: 1,
            stage: StageId::Clone,
        });
    }

    fn clone_finished(&mut self, duration_s: f64, success: bool) {
        self.forward(WorkerEvent::StageFinished {
            v: 1,
            stage: StageId::Clone,
            duration_s,
            success,
        });
    }

    fn event(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::Result { .. } | WorkerEvent::Error { .. } => {}
            event => self.forward(event),
        }
    }

    // The heartbeat keeps the lease through an install; the master's own
    // elapsed time moves on each heartbeat.
    fn install_tick(&self) {}

    fn peak_rss(&mut self, bytes: u64) {
        self.peak_rss_bytes = Some(bytes);
    }
}

/// The agent's [`CancelProbe`]: the master's `cancel` sets the flag and
/// kills the child's process group (see `Current::kill`).
struct AgentProbe {
    cancel: Arc<AtomicBool>,
    child: Arc<Mutex<Option<u32>>>,
}

impl CancelProbe for AgentProbe {
    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn child_started(&self, pid: u32) {
        let mut child = self.child.lock().expect("agent child mutex poisoned");
        *child = Some(pid);
        if self.cancel.load(Ordering::SeqCst) {
            executor::kill_worker_group(pid);
        }
    }

    fn child_finished(&self) {
        *self.child.lock().expect("agent child mutex poisoned") = None;
    }
}

/// One job, run on its own thread.
struct JobContext {
    job_id: String,
    epoch: u64,
    job: JobSpec,
    inputs: AssignInputs,
    outputs: String,
    endpoint: Endpoint,
    token: String,
    host: HostEnv,
    seq: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    child: Arc<Mutex<Option<u32>>>,
    out: mpsc::Sender<FromJob>,
}

/// Artifact requests carry the bearer token. Trust decision: never through
/// a proxy and never following a redirect -- either would hand the token
/// to someone other than the master this agent dialled.
fn http_agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .proxy(None)
        .max_redirects(0)
        .http_status_as_error(false)
        .build();
    ureq::Agent::new_with_config(config)
}

impl JobContext {
    fn run(self) {
        let mut sink = AgentSink {
            out: self.out.clone(),
            job_id: self.job_id.clone(),
            epoch: self.epoch,
            seq: self.seq.clone(),
            peak_rss_bytes: None,
        };
        let probe = AgentProbe {
            cancel: self.cancel.clone(),
            child: self.child.clone(),
        };
        let outcome = self.execute(&mut sink, &probe);
        let outcome = if self.cancel.load(Ordering::SeqCst) {
            Outcome::Cancelled
        } else {
            outcome
        };
        let _ = self.out.send(FromJob::Finished {
            outcome,
            peak_rss_bytes: sink.peak_rss_bytes,
        });
    }

    fn execute(&self, sink: &mut AgentSink, probe: &AgentProbe) -> Outcome {
        let Ok(id) = Uuid::parse_str(&self.job_id) else {
            return Outcome::Failed(internal("the job id is not a UUID"));
        };
        let http = http_agent();
        let inputs_dir = self.host.cache_dir.join("inputs").join(id.to_string());
        let inputs = self.fetch_inputs(&http, &inputs_dir);
        let executed = inputs.and_then(|inputs| {
            executor::execute(
                &self.host.exec_env(&self.job),
                id,
                &self.job,
                &inputs,
                sink,
                probe,
            )
        });
        let _ = std::fs::remove_dir_all(&inputs_dir);
        let executed = match executed {
            Ok(executed) => executed,
            Err(error) => return Outcome::Failed(error),
        };
        let outcome = self.upload(&http, id, &executed, probe);
        // Phase 1 keeps nothing for a resume that does not exist.
        let _ = std::fs::remove_dir_all(&executed.job_dir);
        outcome
    }

    /// Downloads the job's inputs into a directory of the agent's own
    /// (§3.3): the names cache, required, and one previous map per branch,
    /// each a cold start if it cannot be fetched.
    fn fetch_inputs(&self, http: &ureq::Agent, dir: &Path) -> Result<JobInputs, ErrorBody> {
        let _ = std::fs::remove_dir_all(dir);
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent).map_err(internal)?;
        }
        worker_result::create_private_dir(dir).map_err(internal)?;
        let names = match &self.inputs.names_cache {
            Some(url) => {
                let path = dir.join("names.json");
                self.download(http, url, &path)?;
                let mut bytes = Vec::new();
                std::fs::File::open(&path)
                    .and_then(|file| file.take(NAMES_CACHE_MAX_BYTES).read_to_end(&mut bytes))
                    .map_err(internal)?;
                // Strict, unlike `naming::load_cache`: an unreadable cache
                // read as empty would rename every district, and CLAUDE.md
                // forbids renaming without the previous name in hand.
                serde_json::from_slice::<NameCache>(&bytes)
                    .map_err(|error| internal(format!("the names cache is unreadable: {error}")))?
            }
            None => NameCache::default(),
        };
        let mut previous_maps = Vec::new();
        for (index, previous) in self.inputs.previous_maps.iter().enumerate() {
            let path = dir.join(format!("previous-{index}.json"));
            match self.download(http, &previous.url, &path) {
                Ok(()) => previous_maps.push(PreviousMapInput {
                    commit: previous.commit.clone(),
                    branch: previous.branch.clone(),
                    path,
                }),
                Err(error) => eprintln!(
                    "job {}: previous map {} not fetched, so it cannot be a warm start: {}",
                    self.job_id, previous.commit, error.message
                ),
            }
        }
        Ok(JobInputs {
            names,
            previous_maps,
        })
    }

    fn download(&self, http: &ureq::Agent, url: &str, dest: &Path) -> Result<(), ErrorBody> {
        if !self.endpoint.owns(url) {
            return Err(internal(format!(
                "input URL {url} is not on the master this agent dialled"
            )));
        }
        let response = http
            .get(url)
            .header("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|error| internal(format!("GET {url}: {error}")))?;
        if response.status().as_u16() != 200 {
            return Err(internal(format!("GET {url}: {}", response.status())));
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(dest).map_err(internal)?;
        std::io::copy(&mut response.into_body().into_reader(), &mut file)
            .map_err(|error| internal(format!("GET {url}: {error}")))?;
        Ok(())
    }

    /// Checks the child's result the way local mode's
    /// `jobs::store_worker_result` does, here on the host where the child's
    /// files are, then uploads each file (§3.4, per-file) and returns the
    /// `result` to forward.
    ///
    /// Trust decision: the job child is untrusted and owns its output
    /// directory, so the agent -- root, in the runtime image -- must not
    /// read whatever path the child reports. `worker_result::adopt` moves
    /// the output into a directory only the agent can write and checks each
    /// expected file there (regular, one link, owned by the worker uid)
    /// before anything is read, exactly as local mode does before it stores
    /// a map. The commit must be the one this agent's own checkout resolved.
    fn upload(
        &self,
        http: &ureq::Agent,
        id: Uuid,
        executed: &Executed,
        probe: &AgentProbe,
    ) -> Outcome {
        let output = &executed.output;
        if !worker_result::is_object_id(&output.commit) || output.commit != executed.checkout.commit
        {
            return Outcome::Failed(ErrorBody {
                error: "invalid_worker_result".to_owned(),
                message: "the reported commit is not the commit the agent checked out".to_owned(),
            });
        }
        let Some(parent) = executed.job_dir.parent() else {
            return Outcome::Failed(internal("the job directory has no parent"));
        };
        // Beside the job directory, so `adopt`'s renames stay on one
        // filesystem.
        let staging = parent.join(format!(".agent-{id}"));
        let _ = std::fs::remove_dir_all(&staging);
        if let Err(error) = worker_result::create_private_dir(&staging) {
            return Outcome::Failed(internal(error));
        }
        let result = self.adopt_and_upload(http, executed, &staging, probe);
        let _ = std::fs::remove_dir_all(&staging);
        match result {
            Ok(event) => Outcome::Result(event),
            Err(error) => Outcome::Failed(error),
        }
    }

    fn adopt_and_upload(
        &self,
        http: &ureq::Agent,
        executed: &Executed,
        staging: &Path,
        probe: &AgentProbe,
    ) -> Result<WorkerEvent, ErrorBody> {
        let output = &executed.output;
        let adopted = worker_result::adopt(
            &executed.output_dir,
            staging,
            &self.job.repo,
            &worker_result::Reported {
                map_path: &output.map_path,
                symbols_path: &output.symbols_path,
                symbols_dir: &output.symbols_dir,
                names_cache: &output.names_cache,
            },
            Some(worker_uid_in_effect(self.host.worker_uid)),
        )
        .map_err(|refused| match refused {
            worker_result::Refused::Invalid(message) => ErrorBody {
                error: "invalid_worker_result".to_owned(),
                message,
            },
            worker_result::Refused::Io(error) => internal(error),
        })?;
        let mut files = vec![
            ("map".to_owned(), adopted.map.clone()),
            ("symbols".to_owned(), adopted.symbols.clone()),
        ];
        let mut entries = std::fs::read_dir(&adopted.symbols_dir)
            .map_err(internal)?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(internal)?;
        entries.sort();
        for entry in entries {
            let path = adopted.symbols_dir.join(&entry);
            files.push((format!("symbols_dir/{entry}"), path));
        }
        // `adopt` leaves the names cache it took at `staging/names.json`;
        // none means the child wrote none, which local mode stores as empty.
        let names = staging.join("names.json");
        if names.is_file() {
            files.push(("names".to_owned(), names));
        }
        let mut artifacts = Vec::with_capacity(files.len());
        for (name, path) in files {
            if probe.is_cancelled() {
                return Err(executor::cancelled_error());
            }
            artifacts.push(self.put(http, &name, &path)?);
        }
        Ok(WorkerEvent::Result {
            v: 1,
            map_path: "artifact:map".to_owned(),
            symbols_path: "artifact:symbols".to_owned(),
            symbols_dir: "artifact:symbols_dir".to_owned(),
            names_cache: "artifact:names".to_owned(),
            commit: executed.checkout.commit.clone(),
            branch: executed.checkout.branch.clone(),
            lang: output.lang.clone(),
            files: output.files,
            districts: output.districts,
            modularity: output.modularity,
            artifacts,
        })
    }

    /// One `PUT`, with the file's SHA-256 and, from the file's size, its
    /// `Content-Length` (§4.2).
    fn put(&self, http: &ureq::Agent, name: &str, path: &Path) -> Result<Artifact, ErrorBody> {
        let url = format!("{}/{name}", self.outputs);
        if !self.endpoint.owns(&url) {
            return Err(internal(format!(
                "output URL {url} is not on the master this agent dialled"
            )));
        }
        let (sha256, bytes) = std::fs::File::open(path)
            .and_then(|file| sha256_reader(file))
            .map_err(internal)?;
        let file = std::fs::File::open(path).map_err(internal)?;
        let response = http
            .put(&url)
            .header("Authorization", &format!("Bearer {}", self.token))
            .header(SHA256_HEADER, &sha256)
            .send(file)
            .map_err(|error| ErrorBody {
                error: "worker_crashed".to_owned(),
                message: format!("upload of {name} failed: {error}"),
            })?;
        let status = response.status();
        if !status.is_success() {
            let mut reason = String::new();
            let _ = response
                .into_body()
                .into_reader()
                .take(4096)
                .read_to_string(&mut reason);
            return Err(ErrorBody {
                error: "worker_crashed".to_owned(),
                message: format!("the master refused {name}: {status}: {reason}"),
            });
        }
        Ok(Artifact {
            name: name.to_owned(),
            sha256,
            bytes,
        })
    }
}
