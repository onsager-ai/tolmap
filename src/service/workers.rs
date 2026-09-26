//! The worker tier's master side for #97 phase 1 (docs/WORKER_TIER.md §2,
//! §2.3, §2.4, §3, §4, §5.1, §5.6): the private worker listener, the
//! channel sessions with agents, in-memory leases, the lease-scoped
//! artifact endpoints, the job runner loopback mode uses in place of the
//! in-process executor, and the supervisor that keeps
//! `TOLMAP_WORKERS=loopback:N`'s agents running.
//!
//! **What phase 1 is.** State is in memory, as local mode's is. A lost
//! agent or an expired lease fails its job with `worker_crashed`; nothing
//! is re-queued, resumed or retried (phase 2), so `resume` is never
//! advertised and a `hello.resume` is answered with `cancel`. Only loopback
//! agents exist: the listener binds to loopback only, the agents are this
//! binary started by this process, and their tokens are minted here at
//! startup.
//!
//! **Trust decisions**, each also stated where the code makes it:
//! - The listener is separate from the public router and loopback-only
//!   (§5.6). The public router has no `/workers` route.
//! - Every channel upgrade and artifact request is authenticated by a
//!   bearer token in the `Authorization` header, never a query string,
//!   compared as SHA-256 digests in constant time (§5.1). The token is the
//!   agent's identity; `hello.worker_id` is only a label for logs.
//! - An agent is assigned work only if its `hello.build` equals this
//!   master's (§3.6), and a `local/<name>` job only if it advertised
//!   `local_paths` (§3.3).
//! - An artifact URL is honoured only for the token holding that job's
//!   lease at that epoch, while the lease is live (§4.2). Inputs are served
//!   by lookup in a table the master built, never by joining a requested
//!   name onto a path.
//! - Uploads are streamed to a master-owned `0700` directory, hashed on the
//!   way, and kept only if their size and SHA-256 match what the agent
//!   declared (§4.2). A result is registered only if its path fields are
//!   `artifact:` names and every artifact it lists was uploaded with the
//!   digest and size it lists (§3.4, §5.4); the master then writes every
//!   file it registers itself, under names it chooses, and hands them to
//!   the same `jobs::register_owned` local mode uses.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use axum::body::{Body, Bytes};
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxPath, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::progress::StageId;
use crate::service::clone::{self, RepoRef};
use crate::service::error::{ApiError, ErrorBody};
use crate::service::executor::{self, EventSink, JobInputs, WorkerOutput};
use crate::service::jobs::{self, JobSnapshot, SnapshotSink};
use crate::service::worker_result;
use crate::service::AppState;
use crate::worker::{
    is_valid_artifact_name, negotiate, AssignInputs, CancelReason, JobSpec, MasterMessage,
    PreviousMapUrl, ReleasedReason, ResumeAction, ShutdownMode, WelcomeResume, WorkerBuild,
    WorkerEvent, WorkerMessage, FEATURE_LOCAL_PATHS, MAX_CONTROL_FRAME_BYTES, PROTO,
};

/// §2.2's defaults: a heartbeat every 15 s, the SSE heartbeat's interval,
/// and a 60 s lease. Both configurable, so tests can shorten the lease.
pub const DEFAULT_HEARTBEAT_S: u64 = 15;
pub const DEFAULT_LEASE_TTL_S: u64 = 60;

/// The header an upload declares its SHA-256 in (§4.2), as 64 lowercase hex
/// digits. Its size is the request's `Content-Length`.
pub const SHA256_HEADER: &str = "x-tolmap-sha256";

/// Phase 1 never reassigns a job, so every lease is epoch 1 (§2.2).
const EPOCH: u64 = 1;

/// How often a waiting runner re-checks cancellation and lease expiry.
const POLL: Duration = Duration::from_millis(250);

/// An agent that upgrades and then says nothing is closed after this.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a stopping master waits for its agents to exit after `shutdown
/// now` before it kills them.
const REAP_GRACE: Duration = Duration::from_secs(10);

/// The largest text frame axum itself accepts on the channel. Above
/// `MAX_CONTROL_FRAME_BYTES` on purpose: a frame between the two reaches
/// `serve_agent`, which answers it with an `error` frame before closing, as
/// §4.3 asks. Only a frame beyond this is dropped by the WebSocket layer
/// without that courtesy, and the channel closes either way.
const WS_MESSAGE_LIMIT: usize = 4 * MAX_CONTROL_FRAME_BYTES;

// ---- configuration --------------------------------------------------------

/// `TOLMAP_WORKERS`: where jobs run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkersMode {
    /// Unset or `local`: today's single-process mode, unchanged. No
    /// listener is started.
    Local,
    /// `loopback:N`: N agents on this host, dialling a loopback listener.
    Loopback(usize),
}

impl WorkersMode {
    /// Anything but the three documented forms is a startup error rather
    /// than a fallback: this variable decides whether a listener opens and
    /// child processes start, so a typo must not quietly pick either mode.
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        let Some(value) = value.map(str::trim) else {
            return Ok(WorkersMode::Local);
        };
        if value.is_empty() || value == "local" {
            return Ok(WorkersMode::Local);
        }
        if let Some(count) = value
            .strip_prefix("loopback:")
            .and_then(|count| count.parse::<usize>().ok())
            .filter(|count| *count >= 1)
        {
            return Ok(WorkersMode::Loopback(count));
        }
        Err(format!(
            "TOLMAP_WORKERS must be unset, `local`, or `loopback:N` with N at least 1; got {value:?}"
        ))
    }

    pub fn from_env() -> anyhow::Result<Self> {
        let value = std::env::var("TOLMAP_WORKERS").ok();
        Self::parse(value.as_deref()).map_err(anyhow::Error::msg)
    }
}

/// The listener and lease settings loopback mode reads at startup.
pub struct LoopbackSettings {
    pub listen: SocketAddr,
    pub heartbeat_s: u64,
    pub lease_ttl: Duration,
}

impl LoopbackSettings {
    pub fn from_env() -> anyhow::Result<Self> {
        let listen = match std::env::var("TOLMAP_WORKER_LISTEN") {
            Ok(value) => value.trim().parse::<SocketAddr>().with_context(|| {
                format!("TOLMAP_WORKER_LISTEN {value:?} is not an address:port")
            })?,
            Err(_) => SocketAddr::from(([127, 0, 0, 1], 0)),
        };
        // Trust decision (§5.6): in phase 1 the only agents are this
        // process's own children, so the worker endpoint has no reason to
        // be reachable from anywhere but this host. A non-loopback address
        // is refused rather than honoured: exposing it is the owner's
        // phase 3 decision (private network, TLS), not a setting.
        if !listen.ip().is_loopback() {
            bail!(
                "TOLMAP_WORKER_LISTEN {listen} is not a loopback address; loopback agents need \
                 the worker listener on loopback only"
            );
        }
        Ok(LoopbackSettings {
            listen,
            heartbeat_s: positive_env("TOLMAP_WORKER_HEARTBEAT_S", DEFAULT_HEARTBEAT_S),
            lease_ttl: Duration::from_secs(positive_env(
                "TOLMAP_WORKER_LEASE_TTL_S",
                DEFAULT_LEASE_TTL_S,
            )),
        })
    }
}

/// A positive whole number of seconds, or `default` when unset or not one
/// (as every other `TOLMAP_*` tunable falls back).
fn positive_env(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

// ---- build identity and tokens ---------------------------------------------

/// This binary's identity for `hello.build` (§3.6). There is no git commit
/// compiled into the binary, so `commit` is the SHA-256 of the executable
/// itself: the same file on both sides is the same build, and a replaced
/// binary is not. `indexers` stays empty in phase 1: a loopback agent is
/// this binary on this host, running the indexers this process would, so
/// there is nothing to compare yet. Remote workers (phase 3) will need
/// their indexer versions here.
pub fn own_build() -> WorkerBuild {
    static BUILD: OnceLock<WorkerBuild> = OnceLock::new();
    BUILD
        .get_or_init(|| WorkerBuild {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            commit: executable_digest(),
            indexers: BTreeMap::new(),
        })
        .clone()
}

fn executable_digest() -> String {
    let digest = std::env::current_exe()
        .and_then(|path| std::fs::File::open(path))
        .and_then(|file| sha256_reader(file));
    match digest {
        Ok((hex, _)) => format!("sha256:{hex}"),
        // Unique per process, so it matches nothing: an agent whose build
        // cannot be established is idle, never trusted by default.
        Err(error) => {
            eprintln!("worker build identity: could not hash this binary: {error}");
            format!("unreadable:{}", std::process::id())
        }
    }
}

fn same_build(left: &WorkerBuild, right: &WorkerBuild) -> bool {
    left.version == right.version && left.commit == right.commit && left.indexers == right.indexers
}

/// SHA-256 of everything `reader` yields, as lowercase hex, and its length.
pub(crate) fn sha256_reader(mut reader: impl Read) -> std::io::Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        hasher.update(&buffer[..count]);
        total += count as u64;
    }
    Ok((format!("{:x}", hasher.finalize()), total))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// A random 256-bit bearer token as 64 hex digits (§5.1). `rand::rng()` is
/// rand's cryptographically secure generator (ChaCha12, seeded and reseeded
/// from the operating system). The map's `SEED = 7` governs map output and
/// has nothing to do with this.
fn new_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn token_digest(token: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(token);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Constant time in the contents: every byte is compared whatever the
/// earlier ones held. Digests, not tokens, so the length is fixed too.
fn digests_equal(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

// ---- the hub ---------------------------------------------------------------

/// A protocol violation: `code` and `message` go into the `error` frame the
/// master sends before it closes the channel (§3.2).
#[derive(Debug)]
struct Violation {
    code: &'static str,
    message: String,
}

impl Violation {
    fn protocol(message: impl Into<String>) -> Self {
        Violation {
            code: "protocol_error",
            message: message.into(),
        }
    }
}

enum Outgoing {
    Message(MasterMessage),
    Close,
}

/// One live channel.
struct Conn {
    /// The token that authenticated it: the agent's identity.
    agent: usize,
    /// Only for logs; nothing is decided on it.
    worker_id: String,
    /// `hello.build` equals this master's (§3.6). An agent that is not is
    /// kept connected and never assigned work.
    eligible: bool,
    local_paths: bool,
    ready: bool,
    draining: bool,
    /// The job whose lease this channel holds, including a cancelled job
    /// until `released` arrives or the lease expires (§2.4).
    holding: Option<Uuid>,
    out: tokio::sync::mpsc::UnboundedSender<Outgoing>,
}

/// An uploaded artifact, stored content-addressed at `<lease dir>/blobs/
/// <sha256>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Upload {
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
}

/// What the channel delivers to the job's runner (`run_remote`).
pub(crate) enum LeaseEvent {
    /// One `job_event`, in `seq` order.
    Event {
        event: WorkerEvent,
        peak_rss_bytes: Option<u64>,
    },
    /// A heartbeat renewed the lease.
    Tick,
    Released {
        reason: ReleasedReason,
        peak_rss_bytes: Option<u64>,
    },
    /// The channel holding the lease closed.
    Lost(String),
}

/// The master's record that one agent holds one job (§2.2), in memory in
/// phase 1.
struct Lease {
    conn: u64,
    agent: usize,
    epoch: u64,
    deadline: Instant,
    next_seq: u64,
    cancelled: bool,
    /// A terminal event (`result`, `error`) or `released` has arrived;
    /// anything after it is dropped.
    finished: bool,
    events: std_mpsc::Sender<LeaseEvent>,
    /// The only files a GET may return for this lease, by name.
    inputs: BTreeMap<String, PathBuf>,
    uploads: BTreeMap<String, Upload>,
    /// Master-owned `0700`: the names-cache input, uploads and blobs.
    dir: PathBuf,
}

#[derive(Default)]
struct HubInner {
    conns: BTreeMap<u64, Conn>,
    leases: BTreeMap<Uuid, Lease>,
    next_conn: u64,
    stopping: bool,
}

/// What `claim` hands the runner.
pub(crate) struct Claimed {
    pub(crate) events: std_mpsc::Receiver<LeaseEvent>,
    pub(crate) dir: PathBuf,
}

/// Agents, leases and uploads. One per master in loopback mode, shared by
/// the listener's handlers and the job runners.
///
/// Lock discipline: `inner` is held only for bookkeeping, never across I/O
/// on a socket or a file, and nothing that holds it takes the job
/// registry's lock (runners check the registry before taking this one).
pub struct WorkerHub {
    inner: Mutex<HubInner>,
    changed: Condvar,
    /// SHA-256 of each agent's token; an agent is its index here.
    tokens: Vec<[u8; 32]>,
    build: WorkerBuild,
    heartbeat_s: u64,
    lease_ttl: Duration,
    staging: PathBuf,
    /// `http://<listener address>`, the base of every artifact URL.
    base_url: String,
}

impl WorkerHub {
    pub(crate) fn new(
        token_digests: Vec<[u8; 32]>,
        build: WorkerBuild,
        heartbeat_s: u64,
        lease_ttl: Duration,
        staging: PathBuf,
        base_url: String,
    ) -> Self {
        WorkerHub {
            inner: Mutex::new(HubInner::default()),
            changed: Condvar::new(),
            tokens: token_digests,
            build,
            heartbeat_s,
            lease_ttl,
            staging,
            base_url,
        }
    }

    fn lock(&self) -> MutexGuard<'_, HubInner> {
        self.inner.lock().expect("worker hub mutex poisoned")
    }

    fn lease_ttl_s(&self) -> u64 {
        self.lease_ttl.as_secs().max(1)
    }

    /// The agent a request's bearer token names, if any. Trust decision
    /// (§5.1): only the `Authorization` header is read -- a token in a URL
    /// would land in logs -- and the presented token is hashed and compared
    /// with every stored digest without stopping early.
    pub(crate) fn authenticate(&self, headers: &HeaderMap) -> Option<usize> {
        let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
        let token = value.strip_prefix("Bearer ")?.trim();
        if token.is_empty() {
            return None;
        }
        let presented = token_digest(token.as_bytes());
        let mut found = None;
        for (agent, digest) in self.tokens.iter().enumerate() {
            if digests_equal(&presented, digest) {
                found = Some(agent);
            }
        }
        found
    }

    fn add_conn(
        &self,
        agent: usize,
        worker_id: String,
        eligible: bool,
        local_paths: bool,
        out: tokio::sync::mpsc::UnboundedSender<Outgoing>,
    ) -> u64 {
        let mut inner = self.lock();
        let id = inner.next_conn;
        inner.next_conn += 1;
        inner.conns.insert(
            id,
            Conn {
                agent,
                worker_id,
                eligible,
                local_paths,
                ready: false,
                draining: false,
                holding: None,
                out,
            },
        );
        self.changed.notify_all();
        id
    }

    /// Phase 1 has no resume, so a closed channel loses its job at once:
    /// waiting out the lease would only delay the same failure.
    fn remove_conn(&self, conn_id: u64) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        if let Some(conn) = inner.conns.remove(&conn_id) {
            eprintln!(
                "worker agent {} ({}) disconnected",
                conn.agent, conn.worker_id
            );
        }
        for lease in inner.leases.values() {
            if lease.conn == conn_id {
                let _ = lease.events.send(LeaseEvent::Lost(
                    "worker lost: the agent's channel closed".to_owned(),
                ));
            }
        }
        self.changed.notify_all();
    }

    fn handle(&self, conn_id: u64, message: WorkerMessage) -> Result<(), Violation> {
        let mut guard = self.lock();
        let inner = &mut *guard;
        match message {
            WorkerMessage::Hello { .. } => {
                Err(Violation::protocol("a second hello on one channel"))
            }
            WorkerMessage::Ready { slots_free } => {
                // Recorded even while the channel still holds a lease: an
                // agent sends `ready` right after its terminal event, and the
                // runner ends the lease a moment later. `claim` requires both.
                if let Some(conn) = inner.conns.get_mut(&conn_id) {
                    conn.ready = slots_free > 0;
                }
                self.changed.notify_all();
                Ok(())
            }
            WorkerMessage::Draining => {
                if let Some(conn) = inner.conns.get_mut(&conn_id) {
                    conn.draining = true;
                }
                Ok(())
            }
            WorkerMessage::Heartbeat { jobs, .. } => {
                let now = Instant::now();
                for held in jobs {
                    let Ok(id) = Uuid::parse_str(&held.job_id) else {
                        continue;
                    };
                    let Some(lease) = inner.leases.get_mut(&id) else {
                        continue;
                    };
                    if lease.conn != conn_id || lease.epoch != held.epoch {
                        continue;
                    }
                    lease.deadline = now + self.lease_ttl;
                    let _ = lease.events.send(LeaseEvent::Tick);
                    let renewed = MasterMessage::LeaseRenewed {
                        job_id: held.job_id.clone(),
                        epoch: lease.epoch,
                        expires_in_s: self.lease_ttl_s(),
                        acked_seq: lease.next_seq - 1,
                    };
                    if let Some(conn) = inner.conns.get(&conn_id) {
                        let _ = conn.out.send(Outgoing::Message(renewed));
                    }
                }
                Ok(())
            }
            WorkerMessage::JobEvent {
                job_id,
                epoch,
                seq,
                event,
                peak_rss_bytes,
            } => {
                if event.version() != 1 {
                    return Err(Violation {
                        code: "unsupported_event_version",
                        message: format!("job event version {} is not 1", event.version()),
                    });
                }
                // Events for a job this channel does not hold at this epoch
                // are dropped (§2.4, §3.5): a stale epoch, a job whose lease
                // already ended, or one it never held.
                let Ok(id) = Uuid::parse_str(&job_id) else {
                    return Ok(());
                };
                let Some(lease) = inner.leases.get_mut(&id) else {
                    return Ok(());
                };
                if lease.conn != conn_id || lease.epoch != epoch {
                    return Ok(());
                }
                if seq < lease.next_seq {
                    return Ok(());
                }
                if seq > lease.next_seq {
                    return Err(Violation {
                        code: "bad_sequence",
                        message: format!(
                            "job {job_id}: expected seq {}, got {seq}",
                            lease.next_seq
                        ),
                    });
                }
                lease.next_seq += 1;
                // Any job event also renews the lease (§2.2).
                lease.deadline = Instant::now() + self.lease_ttl;
                if lease.finished {
                    return Ok(());
                }
                let terminal = matches!(
                    event,
                    WorkerEvent::Result { .. } | WorkerEvent::Error { .. }
                );
                if lease.cancelled {
                    // The cancel is terminal the moment it lands (§2.4): a
                    // result in flight is refused, never registered.
                    if terminal {
                        lease.finished = true;
                    }
                    if matches!(event, WorkerEvent::Result { .. }) {
                        if let Some(conn) = inner.conns.get(&conn_id) {
                            let _ =
                                conn.out
                                    .send(Outgoing::Message(MasterMessage::ResultRejected {
                                        job_id,
                                        epoch,
                                        reason: "cancelled".to_owned(),
                                    }));
                        }
                    }
                    return Ok(());
                }
                if terminal {
                    lease.finished = true;
                }
                let _ = lease.events.send(LeaseEvent::Event {
                    event,
                    peak_rss_bytes,
                });
                Ok(())
            }
            WorkerMessage::Released {
                job_id,
                epoch,
                reason,
                peak_rss_bytes,
            } => {
                let Ok(id) = Uuid::parse_str(&job_id) else {
                    return Ok(());
                };
                let Some(lease) = inner.leases.get_mut(&id) else {
                    return Ok(());
                };
                if lease.conn != conn_id || lease.epoch != epoch {
                    return Ok(());
                }
                lease.finished = true;
                let _ = lease.events.send(LeaseEvent::Released {
                    reason,
                    peak_rss_bytes,
                });
                Ok(())
            }
        }
    }

    /// Waits for an agent to take `job`, then leases it and sends `assign`
    /// (§2.3). `None` when the job was cancelled, or the master began
    /// stopping, before any agent was free.
    pub(crate) fn claim(
        &self,
        job_id: Uuid,
        job: &JobSpec,
        inputs: &JobInputs,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Claimed>, ErrorBody> {
        let internal = |error: std::io::Error| ApiError::internal(error.to_string()).body;
        let dir = self.staging.join(job_id.to_string());
        let _ = std::fs::remove_dir_all(&dir);
        worker_result::create_private_dir(&dir).map_err(internal)?;
        worker_result::create_private_dir(&dir.join("blobs")).map_err(internal)?;
        let names = dir.join("names.json");
        crate::naming::save_cache(&names, &inputs.names).map_err(internal)?;
        let base = format!("{}/workers/artifacts/{job_id}/{EPOCH}", self.base_url);
        let mut input_files = BTreeMap::new();
        input_files.insert("inputs/names".to_owned(), names);
        // Only the maps the job child could choose: the newest on each
        // branch (`executor::stage_previous_map` takes the newest on the
        // checked-out branch, else the newest overall, which is the newest
        // on its own branch). The agent learns its branch only after it
        // clones, so it gets one per branch; the files stay in the store
        // and are read at GET time, by the master, under its own uid.
        let mut previous_maps = Vec::new();
        let mut branches = BTreeSet::new();
        for row in &inputs.previous_maps {
            if !branches.insert(row.branch.clone()) {
                continue;
            }
            let name = format!("inputs/previous/{}.json", previous_maps.len());
            previous_maps.push(PreviousMapUrl {
                branch: row.branch.clone(),
                commit: row.commit.clone(),
                url: format!("{base}/{name}"),
            });
            input_files.insert(name, row.path.clone());
        }
        let assign = MasterMessage::Assign {
            job_id: job_id.to_string(),
            epoch: EPOCH,
            lease_ttl_s: self.lease_ttl_s(),
            job: job.clone(),
            inputs: AssignInputs {
                names_cache: Some(format!("{base}/inputs/names")),
                previous_maps,
            },
            outputs: base,
        };
        loop {
            // The registry's lock is never taken under this one.
            if is_cancelled() {
                let _ = std::fs::remove_dir_all(&dir);
                return Ok(None);
            }
            let mut guard = self.lock();
            if guard.stopping {
                drop(guard);
                let _ = std::fs::remove_dir_all(&dir);
                return Ok(None);
            }
            let inner = &mut *guard;
            // Lowest channel first: the oldest connected agent, so the
            // choice is deterministic for a given sequence of connections.
            // Trust decision (§3.3): a `local/<name>` job names a path on
            // this host, so it goes only to an agent that said it shares
            // this host's paths.
            let free = inner
                .conns
                .iter()
                .find(|(_, conn)| {
                    conn.eligible
                        && conn.ready
                        && !conn.draining
                        && conn.holding.is_none()
                        && (!job.local || conn.local_paths)
                })
                .map(|(id, _)| *id);
            if let Some(conn_id) = free {
                let (events, receiver) = std_mpsc::channel();
                let conn = inner.conns.get_mut(&conn_id).expect("found above");
                conn.ready = false;
                conn.holding = Some(job_id);
                let agent = conn.agent;
                let _ = conn.out.send(Outgoing::Message(assign.clone()));
                inner.leases.insert(
                    job_id,
                    Lease {
                        conn: conn_id,
                        agent,
                        epoch: EPOCH,
                        deadline: Instant::now() + self.lease_ttl,
                        next_seq: 1,
                        cancelled: false,
                        finished: false,
                        events,
                        inputs: input_files,
                        uploads: BTreeMap::new(),
                        dir: dir.clone(),
                    },
                );
                eprintln!("job {job_id}: assigned to worker agent {agent} (epoch {EPOCH})");
                return Ok(Some(Claimed {
                    events: receiver,
                    dir,
                }));
            }
            let _ = self
                .changed
                .wait_timeout(guard, POLL)
                .expect("worker hub mutex poisoned");
        }
    }

    /// Sends `cancel` for a leased job (§2.4). The lease stays, and the
    /// agent counts as busy, until `released` arrives or the lease expires.
    pub(crate) fn cancel(&self, job_id: Uuid, reason: CancelReason) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let Some(lease) = inner.leases.get_mut(&job_id) else {
            return;
        };
        lease.cancelled = true;
        if let Some(conn) = inner.conns.get(&lease.conn) {
            let _ = conn.out.send(Outgoing::Message(MasterMessage::Cancel {
                job_id: job_id.to_string(),
                epoch: lease.epoch,
                reason,
            }));
        }
    }

    pub(crate) fn expired(&self, job_id: Uuid) -> bool {
        self.lock()
            .leases
            .get(&job_id)
            .is_some_and(|lease| Instant::now() > lease.deadline)
    }

    pub(crate) fn uploads(&self, job_id: Uuid) -> BTreeMap<String, Upload> {
        self.lock()
            .leases
            .get(&job_id)
            .map(|lease| lease.uploads.clone())
            .unwrap_or_default()
    }

    /// `result_accepted` or `result_rejected` for the lease's holder.
    pub(crate) fn verdict(&self, job_id: Uuid, accepted: bool, reason: String) {
        let inner = self.lock();
        let Some(lease) = inner.leases.get(&job_id) else {
            return;
        };
        let job_id = job_id.to_string();
        let epoch = lease.epoch;
        let message = if accepted {
            MasterMessage::ResultAccepted {
                job_id,
                epoch,
                reason,
            }
        } else {
            MasterMessage::ResultRejected {
                job_id,
                epoch,
                reason,
            }
        };
        if let Some(conn) = inner.conns.get(&lease.conn) {
            let _ = conn.out.send(Outgoing::Message(message));
        }
    }

    /// Ends a lease: the agent's slot is free for its next `ready`, the
    /// lease's inputs and uploads are deleted (phase 1 keeps nothing for a
    /// resume that does not exist), and, for an expired lease, the channel
    /// is closed so the agent kills whatever it still runs.
    pub(crate) fn end_lease(&self, job_id: Uuid, close_channel: bool) {
        let dir = {
            let mut guard = self.lock();
            let inner = &mut *guard;
            let lease = inner.leases.remove(&job_id);
            if let Some(lease) = &lease {
                if let Some(conn) = inner.conns.get_mut(&lease.conn) {
                    if conn.holding == Some(job_id) {
                        conn.holding = None;
                    }
                    if close_channel {
                        let _ = conn.out.send(Outgoing::Close);
                    }
                }
            }
            self.changed.notify_all();
            lease.map(|lease| lease.dir)
        };
        if let Some(dir) = dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// `shutdown now` to every agent (§3.2); no job is assigned after this.
    pub fn shutdown_now(&self) {
        let mut inner = self.lock();
        inner.stopping = true;
        for conn in inner.conns.values() {
            let _ = conn.out.send(Outgoing::Message(MasterMessage::Shutdown {
                mode: ShutdownMode::Now,
                reason: "server_stopping".to_owned(),
            }));
        }
        self.changed.notify_all();
    }

    /// The lease an artifact request may act on. Trust decision (§4.2): a
    /// URL is honoured only for the token that holds the job's lease, at
    /// the lease's epoch, while the lease is live and neither cancelled nor
    /// finished. Every refusal is the same 403, so a token learns nothing
    /// about jobs it does not hold.
    fn with_lease<T>(
        &self,
        agent: usize,
        job_id: Uuid,
        epoch: u64,
        act: impl FnOnce(&mut Lease) -> Result<T, StatusCode>,
    ) -> Result<T, StatusCode> {
        let mut inner = self.lock();
        let lease = inner.leases.get_mut(&job_id).ok_or(StatusCode::FORBIDDEN)?;
        if lease.agent != agent || lease.epoch != epoch || lease.cancelled || lease.finished {
            return Err(StatusCode::FORBIDDEN);
        }
        act(lease)
    }
}

// ---- the listener ----------------------------------------------------------

/// The worker listener's routes (§4.2, §5.6): the channel and the artifact
/// URLs, nothing else. Served on its own socket, never merged into
/// `http::router`.
pub fn router(hub: Arc<WorkerHub>) -> Router {
    Router::new()
        .route("/workers/connect", get(connect))
        .route(
            "/workers/artifacts/{job}/{epoch}/{*name}",
            get(get_artifact).put(put_artifact),
        )
        .with_state(hub)
}

fn refuse(status: StatusCode, message: impl Into<String>) -> Response {
    (status, message.into()).into_response()
}

fn unauthorized() -> Response {
    refuse(
        StatusCode::UNAUTHORIZED,
        "a worker token is required in the Authorization header",
    )
}

/// Trust decision (§5.1): the token is checked before anything else about
/// the request, and before the upgrade -- a missing or wrong token gets a
/// 401 and never a channel.
async fn connect(
    State(hub): State<Arc<WorkerHub>>,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let Some(agent) = hub.authenticate(&headers) else {
        return unauthorized();
    };
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };
    upgrade
        .max_message_size(WS_MESSAGE_LIMIT)
        .max_frame_size(WS_MESSAGE_LIMIT)
        .on_upgrade(move |socket| serve_agent(hub, agent, socket))
}

async fn send_message(socket: &mut WebSocket, message: &MasterMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(message).expect("a master message serializes");
    socket.send(Message::Text(text.into())).await
}

/// `error`, then close (§3.2: "the master closes the channel after sending
/// it").
async fn close_with_error(socket: &mut WebSocket, code: &str, message: String) {
    let _ = send_message(
        socket,
        &MasterMessage::Error {
            code: code.to_owned(),
            message,
        },
    )
    .await;
    let _ = socket.send(Message::Close(None)).await;
}

/// Reads one frame as a `WorkerMessage`, or says why not.
fn parse_frame(text: &str) -> Result<WorkerMessage, Violation> {
    if text.len() > MAX_CONTROL_FRAME_BYTES {
        return Err(Violation {
            code: "frame_too_large",
            message: format!(
                "a control frame of {} bytes is over the {MAX_CONTROL_FRAME_BYTES}-byte bound",
                text.len()
            ),
        });
    }
    // An unknown `type` fails here too: no message type is gated on a
    // feature this master offers, so none is acceptable (§3.6).
    serde_json::from_str(text)
        .map_err(|error| Violation::protocol(format!("unreadable or unknown message: {error}")))
}

async fn serve_agent(hub: Arc<WorkerHub>, agent: usize, mut socket: WebSocket) {
    let first = match tokio::time::timeout(HELLO_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => text,
        Ok(Some(Ok(_))) => {
            close_with_error(
                &mut socket,
                "protocol_error",
                "the first frame must be a hello text frame".to_owned(),
            )
            .await;
            return;
        }
        _ => return,
    };
    let hello = match parse_frame(first.as_str()) {
        Ok(message) => message,
        Err(violation) => {
            close_with_error(&mut socket, violation.code, violation.message).await;
            return;
        }
    };
    let WorkerMessage::Hello {
        proto_min,
        proto_max,
        worker_id,
        build,
        class,
        slots,
        features,
        resume,
    } = hello
    else {
        close_with_error(
            &mut socket,
            "protocol_error",
            "the first message must be hello".to_owned(),
        )
        .await;
        return;
    };
    let proto = match negotiate((PROTO, PROTO), (proto_min, proto_max)) {
        Ok(proto) => proto,
        Err(unsupported) => {
            close_with_error(
                &mut socket,
                &unsupported.to_string(),
                format!(
                    "this master speaks proto {PROTO}; the agent offered {proto_min}..={proto_max}"
                ),
            )
            .await;
            return;
        }
    };
    // Trust decision (§3.6): a different build can produce a different map
    // for the same `(slug, commit)`, so it never gets work. It stays
    // connected, and this line is how an operator finds out why it idles.
    let eligible = same_build(&build, &hub.build);
    if !eligible {
        eprintln!(
            "worker agent {agent} ({worker_id}): its build {} {} differs from this master's {} {}; \
             it stays connected but is never assigned work",
            build.version, build.commit, hub.build.version, hub.build.commit
        );
    }
    if slots != 1 {
        eprintln!("worker agent {agent} ({worker_id}): advertised {slots} slots; phase 1 runs one job per agent");
    }
    let local_paths = features
        .iter()
        .any(|feature| feature == FEATURE_LOCAL_PATHS);
    let (out, mut outgoing) = tokio::sync::mpsc::unbounded_channel();
    let conn = hub.add_conn(agent, worker_id.clone(), eligible, local_paths, out);
    eprintln!(
        "worker agent {agent} ({worker_id}) connected: {} MiB usable, {} CPUs, proto {proto}",
        class.memory_bytes / (1024 * 1024),
        class.cpus
    );
    // No resume in phase 1 (§3.5 is phase 2): anything an agent still
    // holds from before is to be dropped.
    let welcome = MasterMessage::Welcome {
        proto,
        heartbeat_s: hub.heartbeat_s,
        lease_ttl_s: hub.lease_ttl_s(),
        resume: resume
            .into_iter()
            .map(|entry| WelcomeResume {
                job_id: entry.job_id,
                action: ResumeAction::Cancel,
                acked_seq: 0,
            })
            .collect(),
    };
    if send_message(&mut socket, &welcome).await.is_ok() {
        loop {
            tokio::select! {
                incoming = socket.recv() => {
                    let text = match incoming {
                        Some(Ok(Message::Text(text))) => text,
                        Some(Ok(Message::Binary(_))) => {
                            close_with_error(
                                &mut socket,
                                "protocol_error",
                                "binary frames are not part of the protocol".to_owned(),
                            )
                            .await;
                            break;
                        }
                        Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    };
                    let handled = parse_frame(text.as_str())
                        .and_then(|message| hub.handle(conn, message));
                    if let Err(violation) = handled {
                        close_with_error(&mut socket, violation.code, violation.message).await;
                        break;
                    }
                }
                message = outgoing.recv() => match message {
                    Some(Outgoing::Message(message)) => {
                        if send_message(&mut socket, &message).await.is_err() {
                            break;
                        }
                    }
                    Some(Outgoing::Close) | None => {
                        let _ = socket.send(Message::Close(None)).await;
                        break;
                    }
                },
            }
        }
    }
    hub.remove_conn(conn);
}

/// `GET` on an input URL from `assign` (§4.2).
async fn get_artifact(
    State(hub): State<Arc<WorkerHub>>,
    AxPath((job, epoch, name)): AxPath<(String, u64, String)>,
    headers: HeaderMap,
) -> Response {
    let Some(agent) = hub.authenticate(&headers) else {
        return unauthorized();
    };
    let Ok(job_id) = Uuid::parse_str(&job) else {
        return refuse(StatusCode::FORBIDDEN, "no lease on that job");
    };
    // Looked up, never joined onto a path: only files the master put in
    // this lease's table can be returned.
    let path = match hub.with_lease(agent, job_id, epoch, |lease| {
        lease
            .inputs
            .get(&name)
            .cloned()
            .ok_or(StatusCode::NOT_FOUND)
    }) {
        Ok(path) => path,
        Err(status) => return refuse(status, "no such input on a lease this token holds"),
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/json")], bytes).into_response(),
        Err(error) => refuse(StatusCode::NOT_FOUND, format!("input unavailable: {error}")),
    }
}

/// Writes what arrives on `chunks` to a new file at `path`, hashing it on
/// the way. Blocking: runs on the blocking pool.
fn write_hashed(
    path: &Path,
    mut chunks: tokio::sync::mpsc::Receiver<Bytes>,
) -> std::io::Result<(String, u64)> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    while let Some(chunk) = chunks.blocking_recv() {
        hasher.update(&chunk);
        file.write_all(&chunk)?;
        total += chunk.len() as u64;
    }
    file.flush()?;
    Ok((format!("{:x}", hasher.finalize()), total))
}

/// `PUT` of one result artifact (§4.2): streamed to a temporary file in the
/// lease's master-owned directory while hashed, never buffered whole, then
/// kept under its digest only if size and digest match the declaration.
async fn put_artifact(
    State(hub): State<Arc<WorkerHub>>,
    AxPath((job, epoch, name)): AxPath<(String, u64, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let Some(agent) = hub.authenticate(&headers) else {
        return unauthorized();
    };
    // Trust decision (§3.4): the four fixed names and `symbols_dir/
    // <digits>.json` only. The name never becomes a path here; it is a key.
    if !is_valid_artifact_name(&name) {
        return refuse(
            StatusCode::BAD_REQUEST,
            format!("{name:?} is not an artifact name"),
        );
    }
    let Ok(job_id) = Uuid::parse_str(&job) else {
        return refuse(StatusCode::FORBIDDEN, "no lease on that job");
    };
    let Some(declared) = headers
        .get(SHA256_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| is_sha256_hex(value))
        .map(str::to_owned)
    else {
        return refuse(
            StatusCode::BAD_REQUEST,
            format!("{SHA256_HEADER} must be the body's SHA-256 as 64 lowercase hex digits"),
        );
    };
    let Some(length) = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
    else {
        return refuse(StatusCode::LENGTH_REQUIRED, "a Content-Length is required");
    };
    let dir = match hub.with_lease(agent, job_id, epoch, |lease| Ok(lease.dir.clone())) {
        Ok(dir) => dir,
        Err(status) => {
            return refuse(
                status,
                "this token holds no live lease on that job and epoch",
            )
        }
    };
    let temp = dir.join(format!("upload-{}", Uuid::new_v4()));
    let (chunks, receiver) = tokio::sync::mpsc::channel::<Bytes>(8);
    let writer = {
        let temp = temp.clone();
        tokio::task::spawn_blocking(move || write_hashed(&temp, receiver))
    };
    let mut stream = body.into_data_stream();
    let mut received = 0u64;
    let mut problem: Option<String> = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                received += bytes.len() as u64;
                if received > length {
                    problem = Some("the body is longer than its Content-Length".to_owned());
                    break;
                }
                if chunks.send(bytes).await.is_err() {
                    break;
                }
            }
            Err(error) => {
                problem = Some(format!("the body ended early: {error}"));
                break;
            }
        }
    }
    drop(chunks);
    let written = match writer.await {
        Ok(Ok(written)) => Some(written),
        Ok(Err(error)) => {
            problem.get_or_insert(format!("could not store the body: {error}"));
            None
        }
        Err(error) => {
            problem.get_or_insert(format!("could not store the body: {error}"));
            None
        }
    };
    let bad = |status: StatusCode, message: String| {
        let _ = std::fs::remove_file(&temp);
        refuse(status, message)
    };
    if let Some(problem) = problem {
        return bad(StatusCode::BAD_REQUEST, problem);
    }
    let Some((sha256, bytes)) = written else {
        return bad(
            StatusCode::INTERNAL_SERVER_ERROR,
            "no body stored".to_owned(),
        );
    };
    if bytes != length {
        return bad(
            StatusCode::BAD_REQUEST,
            format!("size mismatch: declared {length} bytes, received {bytes}"),
        );
    }
    if sha256 != declared {
        return bad(
            StatusCode::BAD_REQUEST,
            format!("digest mismatch: declared {declared}, received {sha256}"),
        );
    }
    // Content-addressed (§4.2): a repeat upload of the same bytes is a
    // no-op.
    let blob = dir.join("blobs").join(&sha256);
    if blob.exists() {
        let _ = std::fs::remove_file(&temp);
    } else if let Err(error) = std::fs::rename(&temp, &blob) {
        return bad(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not keep the upload: {error}"),
        );
    }
    let recorded = hub.with_lease(agent, job_id, epoch, |lease| {
        lease.uploads.insert(name.clone(), Upload { sha256, bytes });
        Ok(())
    });
    match recorded {
        Ok(()) => StatusCode::OK.into_response(),
        Err(status) => refuse(status, "the lease ended during the upload"),
    }
}

// ---- the runner ------------------------------------------------------------

fn worker_crashed(message: impl Into<String>) -> ErrorBody {
    ErrorBody {
        error: "worker_crashed".to_owned(),
        message: message.into(),
    }
}

fn invalid_result(message: impl Into<String>) -> ErrorBody {
    ErrorBody {
        error: "invalid_worker_result".to_owned(),
        message: message.into(),
    }
}

/// Where the agent's executor is in its own clone, which it reports with
/// the two events local mode's `SnapshotSink::clone_started` and
/// `clone_finished` stand for (see `agent::AgentSink`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ExecutorClone {
    NotStarted,
    Running,
    Finished,
}

/// Runs one job in loopback mode: `jobs::prepare`, an agent's executor over
/// the channel, then `jobs::register_owned` -- the three parts of
/// docs/WORKER_TIER.md §2 with execute moved behind the channel. Events go
/// through local mode's own `SnapshotSink`, so the job's snapshots, and the
/// SSE frames made from them, are what local mode makes from the same
/// events. The worker slot `worker_loop` gave this job is held until this
/// returns, which is when the lease has ended.
pub(crate) fn run_remote(
    state: Arc<AppState>,
    hub: &WorkerHub,
    repo_ref: RepoRef,
    tx: watch::Sender<JobSnapshot>,
) {
    let started = Instant::now();
    let job_id = tx.borrow().job_id;
    let registry = &state.jobs;
    if registry.is_cancelled(job_id) {
        return;
    }
    let (job, inputs) = match jobs::prepare(&state, &repo_ref, &tx) {
        Ok(prepared) => prepared,
        Err(error) => return jobs::finish_failed(&tx, error),
    };
    let lease = match hub.claim(job_id, &job, &inputs, &|| registry.is_cancelled(job_id)) {
        Ok(Some(lease)) => lease,
        // Cancelled (or the master is stopping) before any agent took it:
        // the registry has already failed the job.
        Ok(None) => return,
        Err(error) => return jobs::finish_failed(&tx, error),
    };
    let mut sink = SnapshotSink::new(&tx, started, Some(registry));
    let mut clone = ExecutorClone::NotStarted;
    let mut cancel_sent = false;
    loop {
        match lease.events.recv_timeout(POLL) {
            Ok(LeaseEvent::Event {
                event,
                peak_rss_bytes,
            }) => {
                if let Some(peak) = peak_rss_bytes {
                    sink.peak_rss(peak);
                }
                match event {
                    // The agent's executor brackets its own clone with these
                    // two events before the job child exists, and the child
                    // then reports its own near-instant clone stage the same
                    // way; only the first pair is the executor's.
                    WorkerEvent::StageStarted {
                        stage: StageId::Clone,
                        ..
                    } if clone == ExecutorClone::NotStarted => {
                        clone = ExecutorClone::Running;
                        sink.clone_started();
                    }
                    WorkerEvent::StageFinished {
                        stage: StageId::Clone,
                        duration_s,
                        success,
                        ..
                    } if clone == ExecutorClone::Running => {
                        clone = ExecutorClone::Finished;
                        sink.clone_finished(duration_s, success);
                    }
                    WorkerEvent::Result { .. } => {
                        let registered = register_result(
                            &state, hub, &repo_ref, &tx, started, job_id, &lease.dir, &event,
                        );
                        match registered {
                            Ok(()) => hub.verdict(job_id, true, "registered".to_owned()),
                            Err(error) => hub.verdict(job_id, false, error.message),
                        }
                        return hub.end_lease(job_id, false);
                    }
                    WorkerEvent::Error { code, message, .. } => {
                        jobs::finish_failed(
                            &tx,
                            ErrorBody {
                                error: code,
                                message,
                            },
                        );
                        return hub.end_lease(job_id, false);
                    }
                    event => sink.event(event),
                }
            }
            // A heartbeat keeps the elapsed time moving through a long quiet
            // stage, as `install_tick` does for local mode's installs.
            Ok(LeaseEvent::Tick) => sink.install_tick(),
            Ok(LeaseEvent::Released {
                reason,
                peak_rss_bytes,
            }) => {
                if let Some(peak) = peak_rss_bytes {
                    sink.peak_rss(peak);
                }
                if !cancel_sent {
                    jobs::finish_failed(
                        &tx,
                        worker_crashed(format!(
                            "the worker released the job without being asked ({reason:?})"
                        )),
                    );
                }
                return hub.end_lease(job_id, false);
            }
            Ok(LeaseEvent::Lost(message)) => {
                jobs::finish_failed(&tx, worker_crashed(message));
                return hub.end_lease(job_id, false);
            }
            Err(std_mpsc::RecvTimeoutError::Timeout) => {}
            Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                jobs::finish_failed(&tx, worker_crashed("worker lost"));
                return hub.end_lease(job_id, false);
            }
        }
        // The registry already made the job terminal (§2.4); the agent is
        // told, and this slot stays busy until it has released the job.
        if !cancel_sent && registry.is_cancelled(job_id) {
            let reason = if registry.is_stopping() {
                CancelReason::ServerStopping
            } else {
                CancelReason::Cancelled
            };
            hub.cancel(job_id, reason);
            cancel_sent = true;
        }
        if hub.expired(job_id) {
            jobs::finish_failed(&tx, worker_crashed("worker lost: lease expired"));
            return hub.end_lease(job_id, true);
        }
    }
}

/// Checks a forwarded `result` against the uploads, writes the files the
/// master will register, and registers them. `Ok` only once the map row
/// exists (§2.3: `result_accepted` follows registration).
#[allow(clippy::too_many_arguments)]
fn register_result(
    state: &AppState,
    hub: &WorkerHub,
    repo_ref: &RepoRef,
    tx: &watch::Sender<JobSnapshot>,
    started: Instant,
    job_id: Uuid,
    dir: &Path,
    event: &WorkerEvent,
) -> Result<(), ErrorBody> {
    // A cancel that landed between the result and here still wins (§2.4).
    if state.jobs.is_cancelled(job_id) {
        return Err(executor::cancelled_error());
    }
    let checked = check_result(event, &hub.uploads(job_id))
        .and_then(|checked| assemble(dir, &repo_ref.repo, checked));
    let executed = match checked {
        Ok(executed) => executed,
        Err(error) => {
            jobs::finish_failed(tx, error.clone());
            return Err(error);
        }
    };
    // The files are the master's own now, written from verified uploads,
    // so the owner `adopt` checks is this process's uid.
    jobs::register_owned(
        state,
        repo_ref,
        tx,
        started,
        executed,
        Some(executor::current_uid()),
    )
}

/// A `result` whose artifacts all check out.
#[derive(Debug)]
pub(crate) struct CheckedResult {
    commit: String,
    branch: Option<String>,
    lang: String,
    files: usize,
    districts: usize,
    modularity: f64,
    artifacts: Vec<(String, Upload)>,
}

/// Trust decision (§3.4, §5.3, §5.4): a remote result names artifacts,
/// never paths -- the four path fields must be exactly the `artifact:`
/// names -- and every artifact it lists must be a valid name, listed once,
/// and uploaded under this lease with the SHA-256 and size it states. The
/// map and the symbols document are required; the names cache is optional,
/// as a missing one is an empty one in local mode.
pub(crate) fn check_result(
    event: &WorkerEvent,
    uploads: &BTreeMap<String, Upload>,
) -> Result<CheckedResult, ErrorBody> {
    let WorkerEvent::Result {
        v,
        map_path,
        symbols_path,
        symbols_dir,
        names_cache,
        commit,
        branch,
        lang,
        files,
        districts,
        modularity,
        artifacts,
    } = event
    else {
        return Err(invalid_result("not a result event"));
    };
    if *v != 1 {
        return Err(invalid_result(format!("result version {v} is not 1")));
    }
    for (field, value, expected) in [
        ("map_path", map_path, "artifact:map"),
        ("symbols_path", symbols_path, "artifact:symbols"),
        ("symbols_dir", symbols_dir, "artifact:symbols_dir"),
        ("names_cache", names_cache, "artifact:names"),
    ] {
        if value.as_str() != expected {
            return Err(invalid_result(format!(
                "the result's {field} is {value:?}, not {expected:?}: a remote result names \
                 artifacts, never paths"
            )));
        }
    }
    let mut seen = BTreeSet::new();
    let mut checked = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        if !is_valid_artifact_name(&artifact.name) {
            return Err(invalid_result(format!(
                "{:?} is not an artifact name",
                artifact.name
            )));
        }
        if !seen.insert(artifact.name.as_str()) {
            return Err(invalid_result(format!(
                "artifact {:?} is listed twice",
                artifact.name
            )));
        }
        let expected = Upload {
            sha256: artifact.sha256.clone(),
            bytes: artifact.bytes,
        };
        if uploads.get(&artifact.name) != Some(&expected) {
            return Err(invalid_result(format!(
                "artifact {:?} was not uploaded with the SHA-256 and size the result lists",
                artifact.name
            )));
        }
        checked.push((artifact.name.clone(), expected));
    }
    for required in ["map", "symbols"] {
        if !seen.contains(&required) {
            return Err(invalid_result(format!(
                "the result lists no {required:?} artifact"
            )));
        }
    }
    Ok(CheckedResult {
        commit: commit.clone(),
        branch: branch.clone(),
        lang: lang.clone(),
        files: *files,
        districts: *districts,
        modularity: *modularity,
        artifacts: checked,
    })
}

/// Lays the checked uploads out as a job child's output directory -- the
/// shape `jobs::store_worker_result` adopts -- under names the master
/// chooses, copied from its own blobs into a fresh master-owned directory.
/// Copies, not links: `worker_result::adopt` refuses a file with a second
/// link, and two district files with equal bytes share one blob.
fn assemble(
    dir: &Path,
    repo: &str,
    checked: CheckedResult,
) -> Result<executor::Executed, ErrorBody> {
    let internal = |error: std::io::Error| ApiError::internal(error.to_string()).body;
    // `repo` comes from the master's own parse of the request, but it is
    // about to name files, so it must be one plain component.
    if repo.is_empty() || repo == "." || repo == ".." || repo.contains(['/', '\\']) {
        return Err(invalid_result(format!(
            "repository name {repo:?} is not a file name"
        )));
    }
    let output = dir.join("output");
    worker_result::create_private_dir(&output).map_err(internal)?;
    // Derived the way the job child and `worker_result` derive them.
    let map = output.join(format!("{repo}.json"));
    let symbols = map.with_extension("symbols.json");
    let symbols_dir = map.with_extension("symbols");
    let names = output.join(format!("{repo}.names.json"));
    worker_result::create_private_dir(&symbols_dir).map_err(internal)?;
    let blobs = dir.join("blobs");
    for (name, upload) in &checked.artifacts {
        let dest = match name.as_str() {
            "map" => map.clone(),
            "symbols" => symbols.clone(),
            "names" => names.clone(),
            other => match other.strip_prefix("symbols_dir/") {
                Some(entry) => symbols_dir.join(entry),
                None => return Err(invalid_result(format!("{other:?} is not an artifact name"))),
            },
        };
        worker_result::copy_for_worker(&blobs.join(&upload.sha256), &dest).map_err(internal)?;
    }
    let text = |path: &Path| path.to_string_lossy().into_owned();
    Ok(executor::Executed {
        job_dir: output.clone(),
        output_dir: output.clone(),
        // The agent's executor checked the child's commit against its own
        // checkout before uploading (`agent::JobContext::upload`), as local
        // mode's `store_worker_result` does against the master's.
        checkout: clone::Materialized {
            path: dir.to_path_buf(),
            commit: checked.commit.clone(),
            branch: checked.branch,
        },
        output: WorkerOutput {
            map_path: text(&map),
            symbols_path: text(&symbols),
            symbols_dir: text(&symbols_dir),
            names_cache: text(&names),
            commit: checked.commit,
            lang: checked.lang,
            files: checked.files,
            districts: checked.districts,
            modularity: checked.modularity,
        },
    })
}

// ---- loopback: startup, agents, shutdown -----------------------------------

type AgentCommand = Arc<dyn Fn(usize) -> Command + Send + Sync>;

/// Keeps N agent processes running, restarting one that exits with backoff
/// (1 s doubling to 30 s, back to 1 s after a minute's uptime), until told
/// to stop.
pub(crate) struct Supervisor {
    stopping: AtomicBool,
    children: Mutex<Vec<Option<u32>>>,
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl Supervisor {
    pub(crate) fn start(count: usize, command: AgentCommand) -> Arc<Self> {
        let supervisor = Arc::new(Supervisor {
            stopping: AtomicBool::new(false),
            children: Mutex::new(vec![None; count]),
            threads: Mutex::new(Vec::new()),
        });
        for agent in 0..count {
            let this = supervisor.clone();
            let command = command.clone();
            let thread = std::thread::Builder::new()
                .name(format!("tolmap-agent-{agent}"))
                .spawn(move || this.keep_running(agent, &*command))
                .expect("spawn an agent supervisor thread");
            supervisor
                .threads
                .lock()
                .expect("supervisor mutex poisoned")
                .push(thread);
        }
        supervisor
    }

    fn stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    fn keep_running(&self, agent: usize, command: &(dyn Fn(usize) -> Command + Send + Sync)) {
        let mut backoff = Duration::from_secs(1);
        while !self.stopping() {
            let started = Instant::now();
            match command(agent).spawn() {
                Ok(mut child) => {
                    self.children.lock().expect("supervisor mutex poisoned")[agent] =
                        Some(child.id());
                    let status = child.wait();
                    self.children.lock().expect("supervisor mutex poisoned")[agent] = None;
                    if self.stopping() {
                        break;
                    }
                    let status = match status {
                        Ok(status) => status.to_string(),
                        Err(error) => error.to_string(),
                    };
                    eprintln!(
                        "worker agent {agent} exited ({status}); restarting it in {}s",
                        backoff.as_secs()
                    );
                }
                Err(error) => {
                    if self.stopping() {
                        break;
                    }
                    eprintln!(
                        "worker agent {agent} could not start: {error}; retrying in {}s",
                        backoff.as_secs()
                    );
                }
            }
            if started.elapsed() > Duration::from_secs(60) {
                backoff = Duration::from_secs(1);
            }
            let until = Instant::now() + backoff;
            while Instant::now() < until && !self.stopping() {
                std::thread::sleep(Duration::from_millis(100));
            }
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }

    pub(crate) fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
    }

    /// Waits up to `grace` for every agent to exit, kills the rest, and
    /// joins the supervising threads.
    pub(crate) fn reap(&self, grace: Duration) {
        self.stop();
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline
            && self
                .children
                .lock()
                .expect("supervisor mutex poisoned")
                .iter()
                .any(Option::is_some)
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        for pid in self
            .children
            .lock()
            .expect("supervisor mutex poisoned")
            .iter()
            .flatten()
        {
            eprintln!("worker agent pid {pid} did not stop in time; killing it");
            // Each agent leads its own process group (see `start_loopback`).
            executor::kill_worker_group(*pid);
        }
        let threads = std::mem::take(&mut *self.threads.lock().expect("supervisor mutex poisoned"));
        for thread in threads {
            let _ = thread.join();
        }
    }
}

/// A running loopback worker tier: the hub and the agents' supervisor.
pub struct Loopback {
    hub: Arc<WorkerHub>,
    supervisor: Arc<Supervisor>,
}

impl Loopback {
    /// The master's half of graceful shutdown in loopback mode, after
    /// `JobRegistry::shutdown` has failed every job with `server_stopping`
    /// as in local mode: `shutdown now` to every agent, then reap them.
    pub async fn shutdown(self) {
        self.supervisor.stop();
        self.hub.shutdown_now();
        let supervisor = self.supervisor.clone();
        let _ = tokio::task::spawn_blocking(move || supervisor.reap(REAP_GRACE)).await;
    }
}

/// Removes whatever a previous process left at `path` and makes it anew,
/// private. Phase 1 keeps no state across a restart, so nothing there is
/// anyone's any more.
fn fresh_private_dir(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("clear {}", path.display()));
        }
    }
    worker_result::create_private_dir(path).with_context(|| format!("create {}", path.display()))
}

/// Creates `path` readable and writable by its owner only.
fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

/// `TOLMAP_WORKERS=loopback:N`: opens the worker listener on loopback,
/// mints one token per agent, points the registry at the hub, and starts
/// the agents.
pub async fn start_loopback(state: &Arc<AppState>, agents: usize) -> anyhow::Result<Loopback> {
    let settings = LoopbackSettings::from_env()?;
    let cache_dir = state.config.cache_dir.clone();
    std::fs::create_dir_all(&cache_dir)
        .with_context(|| format!("create {}", cache_dir.display()))?;
    // Trust decision (§5.1, §8 phase 1): each agent gets its own random
    // token, minted here and never logged. The master keeps only their
    // SHA-256 digests; each agent reads its own from a root-only `0600`
    // file in a `0700` directory outside every agent's cache directory, so
    // no job directory is ever beneath it. The job child runs as the
    // worker uid with an empty environment and cannot read it. (Where the
    // service is not root -- CI, a developer's machine -- the child runs as
    // the same user, as it always has there.)
    let token_dir = cache_dir.join("worker-tokens");
    fresh_private_dir(&token_dir)?;
    let mut digests = Vec::with_capacity(agents);
    let mut token_files = Vec::with_capacity(agents);
    for agent in 0..agents {
        let token = new_token();
        let path = token_dir.join(format!("{agent}.token"));
        write_private_file(&path, token.as_bytes())
            .with_context(|| format!("write {}", path.display()))?;
        digests.push(token_digest(token.as_bytes()));
        token_files.push(path);
    }
    // Uploads land here, never in the store, until registration copies
    // them out; `0700`, the master's own.
    let staging = cache_dir.join("worker-artifacts");
    fresh_private_dir(&staging)?;
    let build = tokio::task::spawn_blocking(own_build)
        .await
        .context("hash this binary for its build identity")?;
    let listener = tokio::net::TcpListener::bind(settings.listen)
        .await
        .with_context(|| format!("bind the worker listener on {}", settings.listen))?;
    let address = listener.local_addr()?;
    let hub = Arc::new(WorkerHub::new(
        digests,
        build,
        settings.heartbeat_s,
        settings.lease_ttl,
        staging,
        format!("http://{address}"),
    ));
    let app = router(hub.clone());
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            eprintln!("tolmap serve: the worker listener stopped: {error}");
        }
    });
    state.jobs.set_remote(hub.clone(), agents);
    let exe = std::env::current_exe().context("locate this binary to start the agents")?;
    let connect = format!("ws://{address}/workers/connect");
    let agents_dir = cache_dir.join("agents");
    let command: AgentCommand = Arc::new(move |agent: usize| {
        let mut command = Command::new(&exe);
        command
            .arg("worker")
            .arg("--connect")
            .arg(&connect)
            .arg("--token-file")
            .arg(&token_files[agent])
            .arg("--cache-dir")
            .arg(agents_dir.join(agent.to_string()))
            .stdin(Stdio::null());
        // Its own process group, so a terminal's Ctrl+C reaches only the
        // master, which then stops the agents in order; and so `reap` can
        // kill one agent's group without touching the master's.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        // The agent inherits this process's environment: it is this binary
        // on this host, in the role this process plays for its job children
        // in local mode. The executor still hands each job child an empty
        // environment plus its allowlist (`executor::run_child`).
        command
    });
    let supervisor = Supervisor::start(agents, command);
    eprintln!(
        "tolmap serve: worker listener on http://{address} (loopback only), {agents} agent(s)"
    );
    Ok(Loopback { hub, supervisor })
}
