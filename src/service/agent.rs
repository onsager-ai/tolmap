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
//! **Resume (#97 phase 2, step 3; §2.5, §3.5, §6).** A lost channel no
//! longer ends anything. Phase 1 killed the job and exited, and #155 kept
//! that; now the agent keeps running its job, buffers the job's events and
//! redials with backoff and jitter. Its `hello.resume` names the job it
//! holds, and the master answers either `continue` from its own
//! `acked_seq`, when the agent replays what came after, or `cancel`, when
//! the agent kills the job and discards its files. A finished result stays
//! on disk until the master's verdict, for at most the hold time
//! (`TOLMAP_WORKER_RESULT_HOLD_S`, 24 h by default). The agent exits only on
//! `shutdown` or on an error redialling cannot mend: the master refused its
//! token, sent a protocol `error`, or -- this agent dials the loopback
//! master that started it -- nothing listens there any more.
//!
//! **Invariants**, each also stated where the code keeps it:
//! - *Seq ordering.* An event gets its `seq` when it is written to a
//!   channel, never before: one more than the last one written for the job
//!   and epoch. The master applies a job's events in `seq` order and
//!   ignores repeats, so its `acked_seq` A says exactly what it applied:
//!   `seq <= A`. On `continue` the agent drops that prefix and writes the
//!   rest again from A + 1 (`Outbox::rewind`), so the master sees one
//!   gapless sequence per job and epoch whatever the drop lost in flight.
//! - *Epoch fencing.* Every frame for a job carries the epoch it was
//!   assigned at, and nothing of an epoch is written again after a drop
//!   without the master's `continue` for that epoch; its `cancel` ends the
//!   job here.
//! - *Buffer bound.* `progress` is coalesced to the latest value per stage
//!   since the last stage boundary, `log` lines waiting for a channel are
//!   capped at `LOG_BOUND` with one marker counting the rest, and every
//!   other event is one per stage boundary, `features` or terminal event:
//!   the buffer grows with the number of stages a job runs, not with how
//!   long it runs (`Outbox`).
//!
//! **Classes and out-of-memory kills (#97 phase 2, step 4; §2.1, §6).** A
//! loopback agent advertises the usable memory its master gives it
//! (`--class-memory`, from `TOLMAP_LOOPBACK_CLASSES`) instead of its host's,
//! so one runner can hold a "small" and a "large" agent. It reports its
//! job's resident memory in every heartbeat (`rss_bytes`: the job child's
//! process group, read from `/proc`), and it tells an out-of-memory kill of
//! its job child from any other death: the child died of SIGKILL that the
//! agent did not send, and, where the memory cgroup's `oom_kill` count can
//! be read, that count rose while the job ran. It then answers `released`
//! `oom` with the child's peak instead of an `error`, and the master moves
//! the job to a larger class.
//!
//! **Limits, on purpose.** Plain `ws://` to a loopback master only; one
//! slot.

use std::collections::VecDeque;
use std::io::Read;
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::tungstenite::{
    self, ClientRequestBuilder, HandshakeError, Message, WebSocket,
};
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
    ResumeAction, ResumeEntry, ShutdownMode, WelcomeResume, WorkerClass, WorkerEvent,
    WorkerMessage, FEATURE_INSTALL_SANDBOX, FEATURE_LOCAL_PATHS, FEATURE_RESUME, PROTO,
};

/// How long one read waits before the loop sends what the job thread
/// queued and checks the heartbeat.
const READ_POLL: Duration = Duration::from_millis(100);
/// One dial: the TCP connect, the upgrade, then `hello` to `welcome`. A
/// dial that takes longer -- a stalled path -- is abandoned and tried again
/// after the backoff.
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
/// A write that cannot finish in this long means nothing reads the other
/// end: the channel is dead even if the socket has not said so.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `shutdown now` waits for a killed job's thread to report.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);
/// The same bound `worker_result` puts on a names cache it reads back.
const NAMES_CACHE_MAX_BYTES: u64 = 16 << 20;
/// §3.5: `log` lines kept while they wait for a channel. Later ones are
/// counted in one marker line instead.
const LOG_BOUND: usize = 64;
/// §3.5: how long a finished job waits for the master's verdict before its
/// result is discarded (`TOLMAP_WORKER_RESULT_HOLD_S`).
pub const DEFAULT_RESULT_HOLD_S: u64 = 24 * 60 * 60;
/// The first redial waits about this long; each failure doubles it, up to
/// `Agent::backoff_cap`.
const REDIAL_FIRST: Duration = Duration::from_millis(200);
/// An upload that got no answer is sent again after a delay doubling up to
/// this.
const UPLOAD_BACKOFF_CAP: Duration = Duration::from_secs(5);

/// What `tolmap worker --connect` was given.
pub struct AgentConfig {
    pub connect: String,
    pub token_file: PathBuf,
    pub cache_dir: PathBuf,
    /// The binary each job child runs as `tolmap worker`: this one.
    pub worker_exe: PathBuf,
    /// `--class-memory`: the usable memory to advertise in `hello.class`
    /// instead of this host's (#97 phase 2, step 4). A loopback master sets
    /// it from `TOLMAP_LOOPBACK_CLASSES`; unset, the host's physical memory
    /// less the reserve, as before.
    pub class_memory: Option<u64>,
    /// The file whose `oom_kill` count tells an out-of-memory kill from
    /// another SIGKILL. `None` finds the memory cgroup's own
    /// (`memory_events_path`); tests name a file they write themselves,
    /// the "marker" a fake out-of-memory job child raises (§9).
    pub memory_events: Option<PathBuf>,
}

/// The master this agent dials.
#[derive(Clone)]
struct Endpoint {
    uri: Uri,
    host: String,
    port: u16,
    /// `http://<host:port>`: the only origin artifact requests may go to.
    origin: String,
    /// The master is on this host (always, until phase 3's remote workers).
    loopback: bool,
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
            loopback,
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

/// `delay`, less up to half of it at random (§2.5 "backoff and jitter"),
/// so agents that lost their channels together do not all redial in the
/// same instant. Not map output: `SEED` has nothing to do with it.
fn jittered(delay: Duration) -> Duration {
    use rand::Rng;
    let millis = (delay.as_millis() as u64).max(2);
    Duration::from_millis(rand::rng().random_range(millis / 2..=millis))
}

/// Sleeps `delay`, waking early if the job is cancelled.
fn sleep_unless_cancelled(delay: Duration, probe: &AgentProbe) {
    let until = Instant::now() + delay;
    while Instant::now() < until && !probe.is_cancelled() {
        std::thread::sleep(
            Duration::from_millis(50).min(until.saturating_duration_since(Instant::now())),
        );
    }
}

/// `TOLMAP_WORKER_RESULT_HOLD_S`, a positive number of seconds, else the
/// default -- as every other `TOLMAP_*` tunable falls back.
fn result_hold() -> Duration {
    Duration::from_secs(
        std::env::var("TOLMAP_WORKER_RESULT_HOLD_S")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .unwrap_or(DEFAULT_RESULT_HOLD_S),
    )
}

/// Whether a job child that ended with `signal` died of an out-of-memory
/// kill (docs/WORKER_TIER.md §6 "job child OOM-killed"). The kernel's OOM
/// killer, global or a cgroup's, sends SIGKILL, so it must be SIGKILL and
/// one this agent did not send: not the executor's own kill (`killed_here`,
/// a stream it could not read) and not a cancel (`cancelled`: the master's
/// `cancel`, `shutdown now`, a lost lease). Where the memory cgroup's
/// `oom_kill` count was read before the job and after the child died, it
/// must have risen: that tells the OOM killer from anything else on the
/// host that sends SIGKILL. Where it could not be read (no cgroup v1 or v2
/// memory controller, or no permission), SIGKILL not sent by the agent is
/// all the evidence there is, and it is taken, as §6 allows.
fn killed_by_oom(
    signal: Option<i32>,
    killed_here: bool,
    cancelled: bool,
    oom_kills_before: Option<u64>,
    oom_kills_after: Option<u64>,
) -> bool {
    const SIGKILL: i32 = 9;
    if signal != Some(SIGKILL) || killed_here || cancelled {
        return false;
    }
    match (oom_kills_before, oom_kills_after) {
        (Some(before), Some(after)) => after > before,
        _ => true,
    }
}

/// The `oom_kill` count in a memory cgroup's event file: `memory.events`
/// on cgroup v2, `memory.oom_control` on v1 (Linux 4.13 and later), both
/// `oom_kill <n>` lines.
fn read_oom_kills(path: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_oom_kills(&text)
}

fn parse_oom_kills(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix("oom_kill "))
        .and_then(|count| count.trim().parse().ok())
}

/// This process's memory cgroup event file, which its job children share
/// (they inherit the cgroup). v2 first (`0::<path>` in `/proc/self/cgroup`),
/// then v1's `memory` controller. `None` where neither is readable.
fn memory_events_path() -> Option<PathBuf> {
    let cgroups = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    memory_events_candidates(&cgroups)
        .into_iter()
        .find(|path| read_oom_kills(path).is_some())
}

fn memory_events_candidates(cgroups: &str) -> Vec<PathBuf> {
    let under = |root: &str, path: &str| Path::new(root).join(path.trim_start_matches('/'));
    let mut candidates = Vec::new();
    for line in cgroups.lines() {
        let mut fields = line.splitn(3, ':');
        let (Some(id), Some(controllers), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if id == "0" && controllers.is_empty() {
            candidates.insert(0, under("/sys/fs/cgroup", path).join("memory.events"));
        } else if controllers
            .split(',')
            .any(|controller| controller == "memory")
        {
            candidates.push(under("/sys/fs/cgroup/memory", path).join("memory.oom_control"));
        }
    }
    candidates
}

/// The resident memory of every process in the process group `pgid` (the
/// job child leads its own group, and its indexers and git children join
/// it), from `/proc/<pid>/stat`. Linux only; `None` elsewhere, or when no
/// process of the group is left.
fn process_group_rss(pgid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: `sysconf` reads a system constant and has no
        // preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let page = u64::try_from(page).ok().filter(|page| *page > 0)?;
        let mut total = None;
        for entry in std::fs::read_dir("/proc").ok()?.flatten() {
            let name = entry.file_name();
            let Some(pid) = name
                .to_str()
                .filter(|name| name.bytes().all(|b| b.is_ascii_digit()))
            else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };
            if let Some((group, pages)) = stat_group_and_rss(&stat) {
                if group == pgid {
                    total = Some(total.unwrap_or(0) + pages.saturating_mul(page));
                }
            }
        }
        total
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pgid;
        None
    }
}

/// `pgrp` (field 5) and `rss` in pages (field 24) of a `/proc/<pid>/stat`
/// line. The command name (field 2) may hold spaces and parentheses, so
/// fields are counted after its last `)`: field 3 is the first there.
fn stat_group_and_rss(stat: &str) -> Option<(u32, u64)> {
    let tail = stat.rsplit_once(") ")?.1;
    let fields: Vec<&str> = tail.split(' ').collect();
    let group = fields.get(5 - 3)?.parse().ok()?;
    let rss = fields.get(24 - 3)?.parse().ok()?;
    Some((group, rss))
}

/// Runs the agent until the master sends `shutdown` (`Ok`) or something
/// redialling cannot mend happens (`Err`, so the process exits non-zero
/// and its supervisor restarts it).
pub fn run(config: AgentConfig) -> Result<()> {
    let token = read_token(&config.token_file)?;
    let endpoint = Endpoint::parse(&config.connect)?;
    std::fs::create_dir_all(&config.cache_dir)
        .with_context(|| format!("create {}", config.cache_dir.display()))?;
    // A starting agent holds no job, so any job or input directory under
    // its cache is a killed predecessor's. They must go: the master
    // re-queues that job (#97 phase 2), it may come back to this agent
    // under the same id, and the executor would find the old checkout in
    // its way. The clone cache beside them is kept.
    for leftover in ["work", "inputs"] {
        let _ = std::fs::remove_dir_all(config.cache_dir.join(leftover));
    }
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
    let (to_main, from_jobs) = mpsc::channel();
    let now = Instant::now();
    let mut class = host_class();
    if let Some(memory_bytes) = config.class_memory {
        class.memory_bytes = memory_bytes;
    }
    let memory_events = config.memory_events.clone().or_else(memory_events_path);
    let mut agent = Agent {
        link: None,
        endpoint,
        token,
        host,
        worker_id: format!("agent-{}", std::process::id()),
        class,
        memory_events,
        // Both replaced by the master's `welcome`.
        heartbeat: Duration::from_secs(1),
        lease_ttl: Duration::from_secs(crate::service::workers::DEFAULT_LEASE_TTL_S),
        hold: result_hold(),
        to_main,
        from_jobs,
        current: None,
        pending: VecDeque::new(),
        want_ready: false,
        draining: false,
        last_heard: now,
        next_heartbeat: now,
        backoff: REDIAL_FIRST,
        next_dial: now,
    };
    // The first dial is not retried: an agent that cannot reach its master
    // at all exits for its supervisor to restart, as in phase 1. Only a
    // channel that was up is redialled, because only then may a job be
    // waiting on the other side.
    if let Err(Dial::Fatal(error) | Dial::Retry(error)) = agent.dial() {
        return Err(error);
    }
    eprintln!(
        "worker agent {}: connected to {}",
        agent.worker_id, agent.endpoint.origin
    );
    agent.run()
}

/// Why a dial failed.
enum Dial {
    /// Redialling cannot help: exit.
    Fatal(anyhow::Error),
    /// Try again after the backoff.
    Retry(anyhow::Error),
}

/// A channel that has been welcomed.
struct Session {
    socket: Socket,
    heartbeat: Duration,
    lease_ttl: Duration,
    resume: Vec<WelcomeResume>,
}

fn dial(endpoint: &Endpoint, token: &str, hello: &WorkerMessage) -> Result<Session, Dial> {
    let address = (endpoint.host.as_str(), endpoint.port)
        .to_socket_addrs()
        .with_context(|| format!("resolve {}", endpoint.origin))
        .map_err(Dial::Retry)?
        .next()
        .ok_or_else(|| Dial::Retry(anyhow!("{} resolves to no address", endpoint.origin)))?;
    let stream = match TcpStream::connect_timeout(&address, DIAL_TIMEOUT) {
        Ok(stream) => stream,
        // Unrecoverable for a loopback agent: the master that started it
        // listens on this host for as long as it lives, so nothing
        // listening means it is gone. A successor mints new tokens this
        // agent could never present, and holding on would only keep a job
        // child running for nobody. A remote agent (phase 3) will keep
        // redialling a master that restarts.
        Err(error)
            if error.kind() == std::io::ErrorKind::ConnectionRefused && endpoint.loopback =>
        {
            return Err(Dial::Fatal(anyhow!(
                "nothing listens at {} any more: the master that started this agent is gone",
                endpoint.origin
            )))
        }
        Err(error) => return Err(Dial::Retry(anyhow!("dial {}: {error}", endpoint.origin))),
    };
    let _ = stream.set_nodelay(true);
    // The upgrade and `welcome` wait at most a dial's length; the loop
    // then polls with `READ_POLL`.
    stream
        .set_read_timeout(Some(DIAL_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(WRITE_TIMEOUT)))
        .context("set the channel's timeouts")
        .map_err(Dial::Retry)?;
    // The token goes in the upgrade's `Authorization` header, never in the
    // URL (§5.1).
    let request = ClientRequestBuilder::new(endpoint.uri.clone())
        .with_header("Authorization", format!("Bearer {token}"));
    let mut socket = match tungstenite::client(request, stream) {
        Ok((socket, _)) => socket,
        Err(HandshakeError::Failure(tungstenite::Error::Http(response)))
            if response.status().as_u16() == 401 =>
        {
            return Err(Dial::Fatal(anyhow!(
                "the master refused this agent's token: it is not the master that minted it"
            )))
        }
        Err(error) => return Err(Dial::Retry(anyhow!("the channel upgrade failed: {error}"))),
    };
    socket
        .get_ref()
        .set_read_timeout(Some(READ_POLL))
        .context("set the channel's read timeout")
        .map_err(Dial::Retry)?;
    send(&mut socket, hello).map_err(Dial::Retry)?;
    let deadline = Instant::now() + DIAL_TIMEOUT;
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                return match serde_json::from_str::<MasterMessage>(text.as_str()) {
                    Ok(MasterMessage::Welcome {
                        heartbeat_s,
                        lease_ttl_s,
                        resume,
                        ..
                    }) => Ok(Session {
                        socket,
                        heartbeat: Duration::from_secs(heartbeat_s.max(1)),
                        lease_ttl: Duration::from_secs(lease_ttl_s.max(1)),
                        resume,
                    }),
                    Ok(MasterMessage::Error { code, message }) => Err(Dial::Fatal(anyhow!(
                        "the master refused this agent: {code}: {message}"
                    ))),
                    Ok(other) => Err(Dial::Fatal(anyhow!(
                        "expected welcome from the master, got {other:?}"
                    ))),
                    Err(error) => Err(Dial::Fatal(anyhow!(
                        "an unreadable frame from the master: {error}"
                    ))),
                };
            }
            Ok(Message::Close(_)) => {
                return Err(Dial::Retry(anyhow!(
                    "the master closed the channel before welcome"
                )))
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(error)) if would_block(&error) => {}
            Err(error) => return Err(Dial::Retry(anyhow!("read welcome: {error}"))),
        }
        if Instant::now() > deadline {
            return Err(Dial::Retry(anyhow!(
                "no welcome from the master within {} s",
                DIAL_TIMEOUT.as_secs()
            )));
        }
    }
}

/// One `job_event` the master has not acknowledged (§3.5).
struct Entry {
    /// Set when the event is written to a channel; cleared by a rewind.
    seq: Option<u64>,
    event: WorkerEvent,
    peak_rss_bytes: Option<u64>,
    /// `Some(lines)` for the dropped-lines marker: a `log` the agent made,
    /// not one the child wrote.
    dropped: Option<u64>,
}

/// The held job's events until the master acknowledges them (§3.5): what a
/// `continue` replays.
///
/// Invariant (buffer bound): `push` coalesces a `progress` with an earlier
/// one for the same stage when only `log` lines and other stages' progress
/// lie between them -- a stage boundary, `features`, `error` or `result`
/// keeps what comes before it, so the master's per-stage counters see the
/// passes they would have seen -- and caps the `log` lines not yet written
/// at `LOG_BOUND`, counting the rest in one marker. What is left is one
/// entry per stage boundary, `features` and terminal event, at most one
/// `progress` per stage between two of them, and a bounded number of lines.
///
/// Invariant (seq ordering): written entries come before unwritten ones,
/// with rising `seq`s, so the entries the master acknowledged are always a
/// prefix. Coalescing may remove an entry that was written: whether or not
/// the master applied it, the later value that replaces it supersedes it.
#[derive(Default)]
struct Outbox {
    entries: VecDeque<Entry>,
    /// The last `seq` written for this job and epoch.
    last_seq: u64,
}

impl Outbox {
    fn push(&mut self, event: WorkerEvent, peak_rss_bytes: Option<u64>) {
        match &event {
            WorkerEvent::Progress { value, .. } => {
                let stage = value.stage;
                let mut index = self.entries.len();
                while index > 0 {
                    index -= 1;
                    match &self.entries[index].event {
                        WorkerEvent::Progress { value: earlier, .. } if earlier.stage == stage => {
                            self.entries.remove(index);
                            break;
                        }
                        WorkerEvent::Progress { .. } | WorkerEvent::Log { .. } => {}
                        _ => break,
                    }
                }
            }
            WorkerEvent::Log { .. } => {
                let waiting = self
                    .entries
                    .iter()
                    .filter(|entry| {
                        entry.seq.is_none()
                            && entry.dropped.is_none()
                            && matches!(entry.event, WorkerEvent::Log { .. })
                    })
                    .count();
                if waiting >= LOG_BOUND {
                    self.note_dropped(1);
                    return;
                }
            }
            _ => {}
        }
        self.entries.push_back(Entry {
            seq: None,
            event,
            peak_rss_bytes,
            dropped: None,
        });
    }

    /// Counts `lines` more dropped lines in the marker not yet written, or
    /// starts one where the dropping began.
    fn note_dropped(&mut self, lines: u64) {
        let unwritten = self
            .entries
            .iter_mut()
            .find(|entry| entry.seq.is_none() && entry.dropped.is_some());
        if let Some(marker) = unwritten {
            let total = marker.dropped.unwrap_or(0) + lines;
            marker.dropped = Some(total);
            marker.event = Self::marker(total);
            return;
        }
        self.entries.push_back(Entry {
            seq: None,
            event: Self::marker(lines),
            peak_rss_bytes: None,
            dropped: Some(lines),
        });
    }

    fn marker(lines: u64) -> WorkerEvent {
        WorkerEvent::Log {
            v: 1,
            message: format!(
                "{lines} log line(s) dropped while the channel to the master was down"
            ),
        }
    }

    /// `lease_renewed.acked_seq`: the master applied everything up to
    /// `acked`, which is a prefix (invariant: seq ordering).
    fn ack(&mut self, acked: u64) {
        while self
            .entries
            .front()
            .is_some_and(|entry| entry.seq.is_some_and(|seq| seq <= acked))
        {
            self.entries.pop_front();
        }
    }

    /// `continue` with the master's `acked`: drop what it applied, and make
    /// everything else unwritten again, re-coalesced, to be written with
    /// fresh seqs from `acked + 1` (invariant: seq ordering).
    fn rewind(&mut self, acked: u64) {
        self.ack(acked);
        self.last_seq = acked;
        for entry in std::mem::take(&mut self.entries) {
            match entry.dropped {
                Some(lines) => self.note_dropped(lines),
                None => self.push(entry.event, entry.peak_rss_bytes),
            }
        }
    }

    /// Writes every unwritten entry, in order, each with the next seq.
    fn flush(&mut self, socket: &mut Socket, job_id: &str, epoch: u64) -> Result<()> {
        for entry in self.entries.iter_mut().filter(|entry| entry.seq.is_none()) {
            let seq = self.last_seq + 1;
            // Taken before the write: a write that fails ends the channel,
            // and the rewind that follows a `continue` renumbers it anyway.
            self.last_seq = seq;
            entry.seq = Some(seq);
            send(
                socket,
                &WorkerMessage::JobEvent {
                    job_id: job_id.to_owned(),
                    epoch,
                    seq,
                    event: entry.event.clone(),
                    peak_rss_bytes: entry.peak_rss_bytes,
                },
            )?;
        }
        Ok(())
    }

    fn written(&self) -> bool {
        self.entries.iter().all(|entry| entry.seq.is_some())
    }
}

/// What the job thread tells the socket thread.
enum FromJob {
    Event(WorkerEvent),
    Finished {
        outcome: Outcome,
        peak_rss_bytes: Option<u64>,
    },
}

enum Outcome {
    /// A `result` naming uploaded artifacts, and the directory holding the
    /// files it names, kept until the master's verdict.
    Result(WorkerEvent, PathBuf),
    Failed(ErrorBody),
    Cancelled,
    /// The job child was killed for memory (`killed_by_oom`): released as
    /// `oom`, and the master moves the job to a larger class (§6).
    OutOfMemory,
}

fn discard(outcome: &Outcome) {
    if let Outcome::Result(_, files) = outcome {
        let _ = std::fs::remove_dir_all(files);
    }
}

/// How a held job's thread ended.
enum Done {
    /// The `result` is written or waiting in the outbox; the job is held,
    /// with its files, until the master's verdict or the hold time.
    Result { files: PathBuf, at: Instant },
    /// The `error` is waiting in the outbox; the job is let go once it is
    /// written.
    Failed { at: Instant },
}

impl Done {
    fn at(&self) -> Instant {
        match self {
            Done::Result { at, .. } | Done::Failed { at } => *at,
        }
    }
}

/// The job this agent holds.
struct Current {
    job_id: String,
    epoch: u64,
    cancel: Arc<AtomicBool>,
    child: Arc<Mutex<Option<u32>>>,
    /// Set by the master's `cancel` (or a `cancel` answer to a resume): the
    /// job ends in `released` with this reason, whatever it produced.
    released: Option<ReleasedReason>,
    outbox: Outbox,
    /// Set once the job thread has reported; the hold time runs from here.
    done: Option<Done>,
}

impl Current {
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

    fn awaits_verdict(&self) -> bool {
        matches!(self.done, Some(Done::Result { .. }))
    }

    /// The files a finished result kept for the master.
    fn discard_files(&self) {
        if let Some(Done::Result { files, .. }) = &self.done {
            let _ = std::fs::remove_dir_all(files);
        }
    }
}

/// What one turn of the loop leads to.
enum Step {
    Go,
    Exit,
    /// The channel is gone; the agent redials.
    Lost(String),
}

struct Agent {
    /// The channel, while it is up.
    link: Option<Socket>,
    endpoint: Endpoint,
    token: String,
    host: HostEnv,
    worker_id: String,
    /// `hello.class`, fixed for the process: every redial advertises the
    /// same class, so the master never sees the agent change class.
    class: WorkerClass,
    /// Where the memory cgroup's `oom_kill` count is read, if anywhere.
    memory_events: Option<PathBuf>,
    heartbeat: Duration,
    lease_ttl: Duration,
    hold: Duration,
    to_main: mpsc::Sender<FromJob>,
    from_jobs: mpsc::Receiver<FromJob>,
    current: Option<Current>,
    /// Frames that wait for the channel but belong to no held job:
    /// `released`, `draining`, a refusal of an `assign`.
    pending: VecDeque<WorkerMessage>,
    /// The agent owes the master a `ready` once a channel is up and it
    /// holds nothing.
    want_ready: bool,
    draining: bool,
    last_heard: Instant,
    next_heartbeat: Instant,
    backoff: Duration,
    next_dial: Instant,
}

impl Agent {
    fn run(mut self) -> Result<()> {
        loop {
            let step = if self.link.is_some() {
                self.read()
            } else {
                self.redial()
            };
            match step {
                Ok(Step::Go) => {}
                Ok(Step::Exit) => return Ok(()),
                Ok(Step::Lost(why)) => self.disconnect(&why),
                Err(error) => return self.fail(error),
            }
            while let Ok(item) = self.from_jobs.try_recv() {
                self.on_job(item);
            }
            if let Err(error) = self.flush() {
                self.disconnect(&format!("{error:#}"));
            }
            self.tick();
            if self.draining && self.current.is_none() && self.pending.is_empty() {
                self.close();
                return Ok(());
            }
        }
    }

    /// Something redialling cannot mend: the job dies with the agent, and
    /// the non-zero exit tells its supervisor.
    fn fail(&mut self, error: anyhow::Error) -> Result<()> {
        if let Some(current) = self.current.take() {
            current.kill();
            current.discard_files();
        }
        Err(anyhow!(
            "{error:#}; the job this agent held, if any, was killed and its lease left to run out"
        ))
    }

    fn read(&mut self) -> Result<Step> {
        let socket = self.link.as_mut().expect("read needs a channel");
        match socket.read() {
            Ok(Message::Text(text)) => {
                self.last_heard = Instant::now();
                let message = serde_json::from_str::<MasterMessage>(text.as_str())
                    .context("an unreadable frame from the master")?;
                self.on_message(message)
            }
            Ok(Message::Close(_)) => Ok(Step::Lost("the master closed the channel".to_owned())),
            Ok(_) => {
                self.last_heard = Instant::now();
                Ok(Step::Go)
            }
            Err(tungstenite::Error::Io(error)) if would_block(&error) => Ok(Step::Go),
            Err(error) => Ok(Step::Lost(error.to_string())),
        }
    }

    /// Drops the channel and schedules the first redial. The job keeps
    /// running, and its events keep collecting in its outbox (§2.5).
    fn disconnect(&mut self, why: &str) {
        if self.link.take().is_none() {
            return;
        }
        match &self.current {
            Some(current) => eprintln!(
                "worker agent {}: the channel to the master was lost ({why}); job {} keeps \
                 running and the agent redials to resume it",
                self.worker_id, current.job_id
            ),
            None => eprintln!(
                "worker agent {}: the channel to the master was lost ({why}); redialling",
                self.worker_id
            ),
        }
        self.backoff = REDIAL_FIRST;
        self.next_dial = Instant::now() + jittered(self.backoff);
    }

    /// Short enough that several dials fit in one lease TTL, so a resume
    /// can still find the lease live after a brief drop.
    fn backoff_cap(&self) -> Duration {
        (self.lease_ttl / 4).clamp(Duration::from_secs(1), Duration::from_secs(10))
    }

    fn redial(&mut self) -> Result<Step> {
        let now = Instant::now();
        if now < self.next_dial {
            // Nothing to read: wait for the job thread or the next dial.
            if let Ok(item) = self
                .from_jobs
                .recv_timeout((self.next_dial - now).min(READ_POLL))
            {
                self.on_job(item);
            }
            return Ok(Step::Go);
        }
        match self.dial() {
            Ok(()) => {
                eprintln!(
                    "worker agent {}: reconnected to {}",
                    self.worker_id, self.endpoint.origin
                );
                Ok(Step::Go)
            }
            Err(Dial::Fatal(error)) => Err(error),
            Err(Dial::Retry(error)) => {
                self.backoff = (self.backoff * 2).min(self.backoff_cap());
                let wait = jittered(self.backoff);
                self.next_dial = Instant::now() + wait;
                eprintln!(
                    "worker agent {}: could not reconnect ({error:#}); trying again in {} ms",
                    self.worker_id,
                    wait.as_millis()
                );
                Ok(Step::Go)
            }
        }
    }

    fn dial(&mut self) -> Result<(), Dial> {
        let hello = WorkerMessage::Hello {
            proto_min: PROTO,
            proto_max: PROTO,
            worker_id: self.worker_id.clone(),
            build: own_build(),
            class: self.class.clone(),
            slots: 1,
            // `local_paths`: this agent shares the master's host (it only
            // ever dials loopback). `resume`: it keeps its job across a
            // lost channel and names it below (§3.5, §3.6).
            features: vec![
                FEATURE_LOCAL_PATHS.to_owned(),
                FEATURE_INSTALL_SANDBOX.to_owned(),
                FEATURE_RESUME.to_owned(),
            ],
            // The held job, unless it is being let go already: then only
            // its `released` is still to send, after the `welcome`.
            resume: self
                .current
                .iter()
                .filter(|current| current.released.is_none())
                .map(|current| ResumeEntry {
                    job_id: current.job_id.clone(),
                    epoch: current.epoch,
                    last_seq: current.outbox.last_seq,
                })
                .collect(),
        };
        let session = dial(&self.endpoint, &self.token, &hello)?;
        let now = Instant::now();
        self.link = Some(session.socket);
        self.heartbeat = session.heartbeat;
        self.lease_ttl = session.lease_ttl;
        self.last_heard = now;
        self.next_heartbeat = now + self.heartbeat;
        self.backoff = REDIAL_FIRST;
        self.on_welcome(session.resume);
        Ok(())
    }

    /// `welcome.resume` (§2.5, §3.5): replay from the master's own
    /// acknowledgement, or let the job go.
    fn on_welcome(&mut self, answers: Vec<WelcomeResume>) {
        let Some(current) = self.current.as_mut() else {
            self.want_ready = true;
            return;
        };
        if current.released.is_some() {
            return;
        }
        let answer = answers
            .into_iter()
            .find(|answer| answer.job_id == current.job_id);
        let lost = match answer {
            Some(WelcomeResume {
                action: ResumeAction::Continue,
                acked_seq,
                ..
            }) => {
                // Invariant (seq ordering): the replay starts after what the
                // master says it applied, never after what this agent
                // believes it wrote.
                current.outbox.rewind(acked_seq);
                eprintln!(
                    "job {}: resumed at epoch {}; the master holds its events up to seq {acked_seq}",
                    current.job_id, current.epoch
                );
                None
            }
            // No answer for a job the `hello` named is read as `cancel`.
            other => Some(
                other
                    .and_then(|answer| answer.reason)
                    .map_or(ReleasedReason::LeaseLost, released_reason),
            ),
        };
        if let Some(reason) = lost {
            // Invariant (epoch fencing): without the master's `continue`
            // nothing of this epoch is written again. The job dies here and
            // its files with it; `released` follows once its thread is done.
            eprintln!(
                "job {}: the master no longer holds this agent's lease at epoch {} ({reason:?}); \
                 killing the job and discarding its files",
                current.job_id, current.epoch
            );
            current.outbox = Outbox::default();
            current.released = Some(reason);
            current.kill();
            if current.done.is_some() {
                self.let_go(Some(reason), None);
            }
        }
    }

    /// Writes what waits for the channel, if it is up: the queued frames,
    /// then the held job's unwritten events in seq order, then the `ready`
    /// the agent owes.
    fn flush(&mut self) -> Result<()> {
        let Some(socket) = self.link.as_mut() else {
            return Ok(());
        };
        while let Some(message) = self.pending.front() {
            send(socket, message)?;
            self.pending.pop_front();
        }
        let mut error_written = false;
        if let Some(current) = self.current.as_mut() {
            if current.released.is_none() {
                current
                    .outbox
                    .flush(socket, &current.job_id, current.epoch)?;
            }
            // The terminal `error` is on a live channel: nothing more comes
            // for this job, and the master ends the lease on it (§2.2). A
            // drop between this write and the master's read loses it, and
            // the lease runs out as for a lost worker.
            error_written =
                matches!(current.done, Some(Done::Failed { .. })) && current.outbox.written();
        }
        if error_written {
            self.current = None;
            self.want_ready = true;
        }
        if self.want_ready && self.current.is_none() && !self.draining {
            if let Some(socket) = self.link.as_mut() {
                send(socket, &WorkerMessage::Ready { slots_free: 1 })?;
            }
            self.want_ready = false;
        }
        Ok(())
    }

    /// The heartbeat, the silence check and the hold time.
    fn tick(&mut self) {
        let now = Instant::now();
        if self.link.is_some() && now >= self.next_heartbeat {
            self.next_heartbeat = now + self.heartbeat;
            let jobs = self
                .current
                .iter()
                .map(|current| HeartbeatJob {
                    job_id: current.job_id.clone(),
                    epoch: current.epoch,
                    last_seq: current.outbox.last_seq,
                })
                .collect();
            // The job child's process group's resident memory now, so a
            // master that loses this agent can tell whether it died of
            // memory (§6 "worker host dies"). Nothing while no child runs.
            let rss_bytes = self
                .current
                .as_ref()
                .and_then(|current| *current.child.lock().expect("agent child mutex poisoned"))
                .and_then(process_group_rss);
            let beat = WorkerMessage::Heartbeat { jobs, rss_bytes };
            let sent = send(self.link.as_mut().expect("checked above"), &beat);
            if let Err(error) = sent {
                self.disconnect(&format!("{error:#}"));
            }
        }
        // A held job's lease is renewed on every heartbeat, so a whole TTL
        // without a word from the master means the channel is dead even
        // though the socket reports nothing: a stalled path or a half-open
        // connection (§6). Redialling is how the agent learns whether its
        // lease survived. A job being let go has no lease to renew, only a
        // `released` to send once its thread is done.
        let leased = self
            .current
            .as_ref()
            .is_some_and(|current| current.released.is_none());
        if self.link.is_some() && leased && self.last_heard.elapsed() > self.lease_ttl {
            let why = format!("no word from the master in {} s", self.lease_ttl.as_secs());
            self.disconnect(&why);
        }
        // §3.5: a finished job waits for the master at most the hold time.
        let expired = self
            .current
            .as_ref()
            .and_then(|current| current.done.as_ref())
            .is_some_and(|done| done.at().elapsed() > self.hold);
        if expired {
            if let Some(current) = &self.current {
                eprintln!(
                    "job {}: no word from the master within {} s of finishing; its result is \
                     discarded",
                    current.job_id,
                    self.hold.as_secs()
                );
            }
            self.let_go(None, None);
        }
    }

    fn close(&mut self) {
        if let Some(socket) = self.link.as_mut() {
            let _ = socket.close(None);
            let _ = socket.flush();
        }
    }

    fn holds(&self, job_id: &str, epoch: u64) -> bool {
        self.current
            .as_ref()
            .is_some_and(|current| current.job_id == job_id && current.epoch == epoch)
    }

    /// The held job is over here: its files are discarded, `released` is
    /// queued when there is a reason to send one, and the agent owes the
    /// master a `ready`.
    fn let_go(&mut self, release: Option<ReleasedReason>, peak_rss_bytes: Option<u64>) {
        if let Some(current) = self.current.take() {
            current.discard_files();
            if let Some(reason) = release {
                self.pending.push_back(WorkerMessage::Released {
                    job_id: current.job_id,
                    epoch: current.epoch,
                    reason,
                    peak_rss_bytes,
                });
            }
        }
        self.want_ready = true;
    }

    fn on_job(&mut self, item: FromJob) {
        match item {
            FromJob::Event(event) => {
                // Nothing is sent for a job being let go.
                if let Some(current) = self
                    .current
                    .as_mut()
                    .filter(|current| current.released.is_none())
                {
                    current.outbox.push(event, None);
                }
            }
            FromJob::Finished {
                outcome,
                peak_rss_bytes,
            } => self.finished(outcome, peak_rss_bytes),
        }
    }

    fn on_message(&mut self, message: MasterMessage) -> Result<Step> {
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
                    self.pending.push_back(WorkerMessage::JobEvent {
                        job_id,
                        epoch,
                        seq: 1,
                        event: WorkerEvent::Error {
                            v: 1,
                            code: "worker_crashed".to_owned(),
                            message: "the agent was not free for this job".to_owned(),
                        },
                        peak_rss_bytes: None,
                    });
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
                    // thread has finished holds nothing to wait for.
                    if current.done.is_some() {
                        self.let_go(Some(reason), None);
                    }
                } else {
                    // Nothing held for it (already finished here): release
                    // at once so the master frees the slot.
                    self.pending.push_back(WorkerMessage::Released {
                        job_id,
                        epoch,
                        reason,
                        peak_rss_bytes: None,
                    });
                }
            }
            MasterMessage::ResultAccepted { job_id, epoch, .. } => {
                if self.holds(&job_id, epoch)
                    && self.current.as_ref().is_some_and(Current::awaits_verdict)
                {
                    self.let_go(None, None);
                }
            }
            MasterMessage::ResultRejected {
                job_id,
                epoch,
                reason,
            } => {
                if self.holds(&job_id, epoch)
                    && self.current.as_ref().is_some_and(Current::awaits_verdict)
                {
                    eprintln!("job {job_id}: the master rejected the result: {reason}");
                    self.let_go(None, None);
                }
            }
            MasterMessage::LeaseRenewed {
                job_id,
                epoch,
                acked_seq,
                ..
            } => {
                if let Some(current) = self
                    .current
                    .as_mut()
                    .filter(|current| current.job_id == job_id && current.epoch == epoch)
                {
                    current.outbox.ack(acked_seq);
                }
            }
            MasterMessage::Shutdown { mode, reason } => {
                eprintln!("worker agent: the master asked it to stop ({mode:?}): {reason}");
                match mode {
                    ShutdownMode::Now => {
                        self.stop_now();
                        return Ok(Step::Exit);
                    }
                    ShutdownMode::Drain => {
                        self.draining = true;
                        self.pending.push_back(WorkerMessage::Draining);
                        if self.current.is_none() {
                            let _ = self.flush();
                            self.close();
                            return Ok(Step::Exit);
                        }
                    }
                }
            }
            MasterMessage::Error { code, message } => {
                bail!("the master reported a protocol error: {code}: {message}")
            }
            MasterMessage::Welcome { .. } => {}
        }
        Ok(Step::Go)
    }

    /// `shutdown now`: kill the job, give its thread a moment to report,
    /// release it, close.
    fn stop_now(&mut self) {
        if let Some(current) = &self.current {
            current.kill();
            let mut peak = None;
            // Only a job whose thread still runs has anything to report.
            if current.done.is_none() {
                let deadline = Instant::now() + SHUTDOWN_GRACE;
                while Instant::now() < deadline {
                    match self.from_jobs.recv_timeout(Duration::from_millis(100)) {
                        Ok(FromJob::Finished {
                            outcome,
                            peak_rss_bytes,
                        }) => {
                            discard(&outcome);
                            peak = peak_rss_bytes;
                            break;
                        }
                        Ok(FromJob::Event(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            }
            self.let_go(Some(ReleasedReason::ServerStopping), peak);
        }
        let _ = self.flush();
        self.close();
    }

    /// The job thread is done with its job.
    fn finished(&mut self, outcome: Outcome, peak_rss_bytes: Option<u64>) {
        let Some(current) = self.current.as_mut() else {
            discard(&outcome);
            return;
        };
        if let Some(reason) = current.released {
            discard(&outcome);
            self.let_go(Some(reason), peak_rss_bytes);
            return;
        }
        let at = Instant::now();
        match outcome {
            Outcome::Result(event, files) => {
                current.outbox.push(event, peak_rss_bytes);
                current.done = Some(Done::Result { files, at });
            }
            Outcome::Failed(error) => {
                current.outbox.push(
                    WorkerEvent::Error {
                        v: 1,
                        code: error.error,
                        message: error.message,
                    },
                    peak_rss_bytes,
                );
                current.done = Some(Done::Failed { at });
            }
            // Only a kill the agent made without the master's word cancels
            // with no `released` set -- the hold running out, or `fail` --
            // and both have let the job go already; release it all the same.
            Outcome::Cancelled => self.let_go(Some(ReleasedReason::Cancelled), peak_rss_bytes),
            // §6: `released` `oom` with the peak the child reached, which the
            // master records so the memory model learns from it. It goes
            // through `pending`, like any `released`, so a channel that is
            // down meanwhile delivers it on the next one.
            Outcome::OutOfMemory => {
                eprintln!(
                    "job {}: the job child was killed for memory; releasing it for a larger                      worker class",
                    current.job_id
                );
                self.let_go(Some(ReleasedReason::Oom), peak_rss_bytes)
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
        let cancel = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None));
        self.current = Some(Current {
            job_id: job_id.clone(),
            epoch,
            cancel: cancel.clone(),
            child: child.clone(),
            released: None,
            outbox: Outbox::default(),
            done: None,
        });
        let context = JobContext {
            job_id,
            job,
            inputs,
            outputs,
            endpoint: self.endpoint.clone(),
            token: self.token.clone(),
            host: self.host.clone(),
            hold: self.hold,
            memory_events: self.memory_events.clone(),
            cancel,
            child,
            out: self.to_main.clone(),
        };
        std::thread::spawn(move || context.run());
    }
}

/// The agent's [`EventSink`]: each event goes to the socket thread, which
/// gives it its `seq` when it writes it (§3.1, §3.5). The executor's own
/// clone, which local mode reports through `clone_started`/
/// `clone_finished`, crosses as the v1 `stage_started`/`stage_finished`
/// pair for `clone`, the first of its kind the master sees for the job; the
/// master turns them back into the same two calls on its own sink
/// (`workers::run_remote`). `result` and `error` are not forwarded here:
/// the executor also returns them, and the agent sends the terminal event
/// itself once it has checked and uploaded the result.
struct AgentSink {
    out: mpsc::Sender<FromJob>,
    peak_rss_bytes: Option<u64>,
    /// How the job child ended: the signal that killed it, if one did, and
    /// whether the executor sent that kill itself.
    exit: Option<(Option<i32>, bool)>,
}

impl AgentSink {
    fn forward(&self, event: WorkerEvent) {
        let _ = self.out.send(FromJob::Event(event));
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

    fn child_exited(&mut self, status: std::process::ExitStatus, killed_here: bool) {
        #[cfg(unix)]
        let signal = std::os::unix::process::ExitStatusExt::signal(&status);
        #[cfg(not(unix))]
        let signal = {
            let _ = status;
            None
        };
        self.exit = Some((signal, killed_here));
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
    job: JobSpec,
    inputs: AssignInputs,
    outputs: String,
    endpoint: Endpoint,
    token: String,
    host: HostEnv,
    /// Uploads are retried for at most this long (§3.5).
    hold: Duration,
    /// Where the memory cgroup's `oom_kill` count is read, if anywhere.
    memory_events: Option<PathBuf>,
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
            peak_rss_bytes: None,
            exit: None,
        };
        let probe = AgentProbe {
            cancel: self.cancel.clone(),
            child: self.child.clone(),
        };
        let outcome = self.execute(&mut sink, &probe);
        let outcome = if self.cancel.load(Ordering::SeqCst) {
            discard(&outcome);
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
        // Read before the job starts: an out-of-memory kill of this job
        // raises the count past this (§6). Clone and child alike run after
        // it, and only the child's death is ever read as out of memory.
        let oom_kills_before = self.memory_events.as_deref().and_then(read_oom_kills);
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
            Err(error) => {
                if let Some((signal, killed_here)) = sink.exit {
                    let oom_kills_after = self.memory_events.as_deref().and_then(read_oom_kills);
                    if killed_by_oom(
                        signal,
                        killed_here,
                        probe.is_cancelled(),
                        oom_kills_before,
                        oom_kills_after,
                    ) {
                        return Outcome::OutOfMemory;
                    }
                }
                return Outcome::Failed(error);
            }
        };
        let outcome = self.upload(&http, id, &executed, probe);
        // The checkout and whatever the child left go now; a result's files
        // are in their own directory, kept until the master's verdict.
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
    /// `result` to forward with the directory holding its files, which is
    /// kept until the master's verdict (§3.5).
    ///
    /// Trust decision: the job child is untrusted and owns its output
    /// directory, so the agent -- root, in the runtime image -- must not
    /// read whatever path the child reports. `worker_result::adopt` moves
    /// the output into a directory only the agent can write and checks each
    /// expected file there (regular, one link, owned by the worker uid)
    /// before anything is read, exactly as local mode does before it stores
    /// a map, including the files/districts-vs-map-document check
    /// (`worker_result::check_counts`). The commit must be the one this
    /// agent's own checkout resolved -- which, since #97 phase 2,
    /// `executor::execute` has already pinned to the commit `self.job`
    /// carries (docs/WORKER_TIER.md §3.3), so this is also the master's own
    /// admitted commit by construction, and `check_result` on the master
    /// checks it again independently against `self.job.commit` once this
    /// agent is not the only thing standing between an untrusted child and
    /// the store.
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
        match self.adopt_and_upload(http, executed, &staging, probe) {
            Ok(event) => Outcome::Result(event, staging),
            Err(error) => {
                let _ = std::fs::remove_dir_all(&staging);
                Outcome::Failed(error)
            }
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
                files: output.files,
                districts: output.districts,
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
        let deadline = Instant::now() + self.hold;
        let mut artifacts = Vec::with_capacity(files.len());
        for (name, path) in files {
            artifacts.push(self.put(http, &name, &path, deadline, probe)?);
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
    /// `Content-Length` (§4.2). One that got no answer, or a 5xx, is sent
    /// again with backoff until `deadline`: that is how an agent re-uploads
    /// whatever a drop interrupted, and uploads are content-addressed, so a
    /// repeat of one the master already holds is a no-op. A 4xx is final --
    /// the lease is not this agent's any more, or the upload is wrong.
    fn put(
        &self,
        http: &ureq::Agent,
        name: &str,
        path: &Path,
        deadline: Instant,
        probe: &AgentProbe,
    ) -> Result<Artifact, ErrorBody> {
        let url = format!("{}/{name}", self.outputs);
        if !self.endpoint.owns(&url) {
            return Err(internal(format!(
                "output URL {url} is not on the master this agent dialled"
            )));
        }
        let (sha256, bytes) = std::fs::File::open(path)
            .and_then(|file| sha256_reader(file))
            .map_err(internal)?;
        let mut backoff = REDIAL_FIRST;
        loop {
            if probe.is_cancelled() {
                return Err(executor::cancelled_error());
            }
            let file = std::fs::File::open(path).map_err(internal)?;
            let sent = http
                .put(&url)
                .header("Authorization", &format!("Bearer {}", self.token))
                .header(SHA256_HEADER, &sha256)
                .send(file);
            let failure = match sent {
                Ok(response) if response.status().is_success() => {
                    return Ok(Artifact {
                        name: name.to_owned(),
                        sha256,
                        bytes,
                    })
                }
                Ok(response) if response.status().is_server_error() => {
                    response.status().to_string()
                }
                Ok(response) => {
                    let status = response.status();
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
                Err(error) => error.to_string(),
            };
            if Instant::now() + backoff > deadline {
                return Err(ErrorBody {
                    error: "worker_crashed".to_owned(),
                    message: format!(
                        "upload of {name} kept failing until the result's hold time ran out: \
                         {failure}"
                    ),
                });
            }
            eprintln!(
                "job {}: upload of {name} failed ({failure}); sending it again",
                self.job_id
            );
            sleep_unless_cancelled(jittered(backoff), probe);
            backoff = (backoff * 2).min(UPLOAD_BACKOFF_CAP);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::ProgressValue;

    fn progress(stage: StageId, done: u64) -> WorkerEvent {
        WorkerEvent::Progress {
            v: 1,
            value: ProgressValue {
                stage,
                stage_index: stage.index(),
                stage_count: StageId::ALL.len(),
                label: stage.label().to_owned(),
                unit: stage.unit().to_owned(),
                done,
                total: Some(100),
                rate_per_s: None,
                transfer_bytes: None,
                transfer_rate_bytes_per_s: None,
            },
        }
    }

    fn started(stage: StageId) -> WorkerEvent {
        WorkerEvent::StageStarted { v: 1, stage }
    }

    fn finished(stage: StageId) -> WorkerEvent {
        WorkerEvent::StageFinished {
            v: 1,
            stage,
            duration_s: 1.0,
            success: true,
        }
    }

    fn log(message: &str) -> WorkerEvent {
        WorkerEvent::Log {
            v: 1,
            message: message.to_owned(),
        }
    }

    /// Each entry as `seq:what`, `-` for an unwritten one.
    fn shape(outbox: &Outbox) -> Vec<String> {
        outbox
            .entries
            .iter()
            .map(|entry| {
                let seq = entry.seq.map_or("-".to_owned(), |seq| seq.to_string());
                let what = match &entry.event {
                    WorkerEvent::StageStarted { stage, .. } => format!("start {}", stage.label()),
                    WorkerEvent::StageFinished { stage, .. } => format!("end {}", stage.label()),
                    WorkerEvent::Progress { value, .. } => {
                        format!("{} {}", value.stage.label(), value.done)
                    }
                    WorkerEvent::Log { message, .. } => format!("log {message}"),
                    other => format!("{other:?}"),
                };
                format!("{seq}:{what}")
            })
            .collect()
    }

    /// Marks every entry written, as `flush` would, without a socket.
    fn write_all(outbox: &mut Outbox) {
        for entry in outbox
            .entries
            .iter_mut()
            .filter(|entry| entry.seq.is_none())
        {
            outbox.last_seq += 1;
            entry.seq = Some(outbox.last_seq);
        }
    }

    /// §3.5: progress keeps only its latest value per stage between two
    /// stage boundaries; the boundaries, and a second pass of the same
    /// stage, keep their order and their own progress.
    #[test]
    fn progress_is_coalesced_per_stage_and_never_across_a_boundary() {
        let parse = StageId::Parse.label();
        let resolve = StageId::Resolve.label();
        let mut outbox = Outbox::default();
        outbox.push(started(StageId::Parse), None);
        for done in 1..=50 {
            outbox.push(progress(StageId::Parse, done), None);
            if done == 20 {
                outbox.push(log("halfway"), None);
            }
        }
        outbox.push(finished(StageId::Parse), None);
        // A second pass of the same stage (multi-source extraction).
        outbox.push(started(StageId::Parse), None);
        outbox.push(progress(StageId::Parse, 3), None);
        outbox.push(progress(StageId::Resolve, 9), None);
        outbox.push(progress(StageId::Parse, 4), None);
        assert_eq!(
            shape(&outbox),
            [
                format!("-:start {parse}"),
                "-:log halfway".to_owned(),
                format!("-:{parse} 50"),
                format!("-:end {parse}"),
                format!("-:start {parse}"),
                format!("-:{resolve} 9"),
                format!("-:{parse} 4"),
            ]
        );
    }

    /// §3.5: log lines waiting for a channel are capped, and one marker at
    /// the place dropping began counts the rest.
    #[test]
    fn log_lines_past_the_bound_are_counted_in_one_marker() {
        let mut outbox = Outbox::default();
        for line in 0..LOG_BOUND + 10 {
            outbox.push(log(&line.to_string()), None);
        }
        outbox.push(started(StageId::Parse), None);
        outbox.push(log("one more"), None);
        let shape = shape(&outbox);
        assert_eq!(shape.len(), LOG_BOUND + 2, "{shape:?}");
        assert_eq!(
            shape[LOG_BOUND],
            "-:log 11 log line(s) dropped while the channel to the master was down"
        );
        assert!(shape[LOG_BOUND + 1].contains("start"), "{shape:?}");
    }

    /// §6: only a SIGKILL the agent did not send is an out-of-memory kill,
    /// and where the cgroup's count was read it must have risen.
    #[test]
    fn only_an_unasked_sigkill_is_an_out_of_memory_kill() {
        const KILL: Option<i32> = Some(9);
        // No cgroup to read: an unasked SIGKILL is taken.
        assert!(killed_by_oom(KILL, false, false, None, None));
        assert!(killed_by_oom(KILL, false, false, Some(3), None));
        // A cgroup read both times: only a raised count.
        assert!(killed_by_oom(KILL, false, false, Some(3), Some(4)));
        assert!(!killed_by_oom(KILL, false, false, Some(3), Some(3)));
        // The agent's own kills, and anything but SIGKILL.
        assert!(!killed_by_oom(KILL, true, false, None, None));
        assert!(!killed_by_oom(KILL, false, true, Some(3), Some(4)));
        assert!(!killed_by_oom(Some(15), false, false, Some(3), Some(4)));
        assert!(!killed_by_oom(None, false, false, Some(3), Some(4)));
    }

    #[test]
    fn the_oom_kill_count_and_the_memory_cgroup_are_read_from_their_files() {
        let v2 = "low 0\nhigh 0\nmax 12\noom 2\noom_kill 2\noom_group_kill 0\n";
        assert_eq!(parse_oom_kills(v2), Some(2));
        let v1 = "oom_kill_disable 0\nunder_oom 0\noom_kill 7\n";
        assert_eq!(parse_oom_kills(v1), Some(7));
        assert_eq!(parse_oom_kills("oom 1\n"), None);
        assert_eq!(
            memory_events_candidates("0::/system.slice/runner.service\n"),
            [PathBuf::from(
                "/sys/fs/cgroup/system.slice/runner.service/memory.events"
            )]
        );
        assert_eq!(
            memory_events_candidates("12:cpu,cpuacct:/a\n9:memory:/docker/x\n0::/\n"),
            [
                PathBuf::from("/sys/fs/cgroup/memory.events"),
                PathBuf::from("/sys/fs/cgroup/memory/docker/x/memory.oom_control"),
            ]
        );
    }

    #[test]
    fn a_process_group_and_its_resident_pages_are_read_from_proc_stat() {
        // `comm` with a space and a parenthesis in it, as a script may have.
        let stat = "4242 (my (odd) job) S 1 4240 4240 0 -1 4194304 100 0 0 0 1 2 0 0 20 0 \
                    1 0 123 45678 321 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 3";
        assert_eq!(stat_group_and_rss(stat), Some((4240, 321)));
        assert_eq!(stat_group_and_rss("garbage"), None);
    }

    /// Written entries are dropped as the master acknowledges them, and a
    /// `continue` renumbers whatever it had not applied from its `acked_seq`
    /// on: one gapless sequence (invariant: seq ordering).
    #[test]
    fn a_rewind_drops_what_the_master_applied_and_renumbers_the_rest() {
        let parse = StageId::Parse.label();
        let mut outbox = Outbox::default();
        outbox.push(started(StageId::Parse), None);
        outbox.push(progress(StageId::Parse, 1), None);
        outbox.push(log("a"), None);
        write_all(&mut outbox);
        assert_eq!(outbox.last_seq, 3);
        outbox.ack(1);
        assert_eq!(
            shape(&outbox),
            [format!("2:{parse} 1"), "3:log a".to_owned()]
        );
        // The channel drops; seqs 2 and 3 may or may not have arrived.
        outbox.push(progress(StageId::Parse, 7), None);
        outbox.push(finished(StageId::Parse), None);
        // The later value superseded the written one.
        assert_eq!(
            shape(&outbox),
            [
                "3:log a".to_owned(),
                format!("-:{parse} 7"),
                format!("-:end {parse}")
            ]
        );
        // The master applied up to 2 (the superseded progress): the log is
        // written again as 3, then the rest.
        outbox.rewind(2);
        assert_eq!(outbox.last_seq, 2);
        write_all(&mut outbox);
        assert_eq!(
            shape(&outbox),
            [
                "3:log a".to_owned(),
                format!("4:{parse} 7"),
                format!("5:end {parse}")
            ]
        );
        // Everything applied: nothing is written again.
        outbox.rewind(5);
        assert!(outbox.entries.is_empty());
        assert_eq!(outbox.last_seq, 5);
    }
}
