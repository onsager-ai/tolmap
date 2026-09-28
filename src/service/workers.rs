//! The worker tier's master side for #97 phases 1 to 3 (docs/WORKER_TIER.md
//! §2, §2.2–§2.5, §3, §4, §5.1, §5.6, §6): the private worker listener, the
//! channel sessions with agents, leases, the lease-scoped artifact
//! endpoints, the job runner the worker modes use in place of the
//! in-process executor, the supervisor that keeps `TOLMAP_WORKERS=
//! loopback:N`'s agents running, and remote mode's TLS listener and token
//! file.
//!
//! **Durable jobs (phase 2, step 2).** Each job has a row in the store's
//! `jobs` table (`store::JobRow`), and the table is the truth for its state,
//! attempt and epoch; the job registry and the hub's leases are caches of
//! it. A lease that runs out -- the agent died, its channel dropped, or it
//! went quiet -- puts the job back at the head of its class queue with a new
//! epoch to come, counting one lost worker; past the retry bound (§10.6)
//! the job fails `worker_crashed`. A restart or a graceful stop re-queues
//! without counting.
//!
//! **Remote mode (phase 3; §5.1, §5.6).** `TOLMAP_WORKERS=remote` starts no
//! agents: agents on other hosts dial a listener that has TLS unless it is
//! on loopback, holding tokens the owner issued, whose hashes the owner's
//! token file binds to worker ids (`TokenFile`). A lease's holder is then a
//! worker id, so a restarted master hands an adopted lease back to that
//! worker (`WorkerHub::adopt`), and a graceful stop closes the channels
//! without stopping the agents (`Remote::shutdown`). Loopback mode keeps
//! its own rules: the listener on loopback only, the agents this binary
//! started by this process, their tokens minted here at startup.
//!
//! **Resume (phase 2, step 3; §2.5, §3.5).** A dropped channel only
//! detaches its lease. An agent that advertised `resume` and comes back
//! within the TTL names the job in `hello.resume`; if it still holds that
//! lease -- same token, same epoch, not cancelled, not run out -- the lease
//! moves to the new channel and `welcome` answers `continue` with the
//! master's `acked_seq`, and the agent replays what came after. Anything
//! else is answered `cancel`, and the agent kills the job. A lease a
//! restarted master adopted from the store has no holder it could check a
//! token against, so it is never resumed in this step (loopback agents are
//! this process's children and hold nothing after a restart anyway).
//!
//! **Classes (phase 2, step 4; §2.1, §6, §7).** `TOLMAP_LOOPBACK_CLASSES`
//! starts loopback agents that advertise configured classes, and each
//! agent is placed in a class by the usable memory its `hello` advertises.
//! A runner asks for an agent of its slot's class. A job whose `features`
//! predict more than its class holds is stopped with `cancel` `reroute`
//! and moved, uncounted, to the smallest class that holds it; a job child
//! killed for memory (`released` `oom`) moves, uncounted, to the next class
//! up, and fails on the largest; a lost worker whose last heartbeat was
//! within 10% of its class's memory moves to the next class too. A move
//! starts the job's lost-worker count on its new class at 0 (§10.6).
//!
//! **Invariants**, each also stated where the code keeps it:
//! - *Epoch fencing.* An epoch is raised in the store before the `assign`
//!   carrying it is sent (`Store::begin_attempt`), so no epoch is ever
//!   handed out twice, even across a crash. Anything carrying another
//!   epoch than the live lease's -- an event, a heartbeat, an upload, a
//!   result -- is refused; a result gets `result_rejected` and is never
//!   registered. Each epoch uploads into its own directory, removed when
//!   its lease ends. Only the live epoch's holder can resume a lease, and
//!   a lease the runner has found expired (`Lease::lost`) is never resumed
//!   or renewed, so a resume and the runner's re-queue cannot both win.
//! - *Seq ordering.* A job's events are applied in `seq` order: `seq ==
//!   next_seq` is applied, a lower one is a repeat (a replay after a
//!   resume) and ignored, a higher one is a protocol violation. So
//!   `acked_seq = next_seq - 1` is exactly what was applied, and a resumed
//!   agent that writes everything after it gets one gapless sequence.
//! - *Settled results.* A result, once registered or refused, is
//!   remembered (`HubInner::settled`, bounded) with its artifacts and
//!   verdict: a repeat for that epoch with the same artifacts gets the same
//!   verdict again and a different one `result_rejected` (§6 "duplicate or
//!   stale result"), and neither is registered. A resume of that job is
//!   answered `continue` and the verdict is sent again, since the agent
//!   lost it with the old channel.
//! - *Terminal is final.* No row leaves `done` or `failed` (every update is
//!   guarded), and terminal rows are written from the in-memory snapshot,
//!   which is itself terminal once and for all, so a cancel racing a result
//!   or a re-queue leaves memory and table agreeing.
//! - *Lock order.* The registry's lock is never taken under the hub's, and
//!   neither is held across SQLite I/O or a socket write: the runner writes
//!   the store between its calls into either.
//! - *Restart order.* `jobs::restore` reloads the table before the public
//!   listener opens and before any agent can connect: orphaned leases take
//!   their slots first, then queued jobs in admission order.
//!
//! **Trust decisions**, each also stated where the code makes it:
//! - The listener is separate from the public router (§5.6), loopback-only
//!   in loopback mode, and TLS-only off loopback in remote mode with no
//!   plaintext fallback (`remote_tls_files`); its key must be private. The
//!   public router has no `/workers` route.
//! - Every channel upgrade and artifact request is authenticated by a
//!   bearer token in the `Authorization` header, never a query string,
//!   compared as SHA-256 digests in constant time (§5.1). The token is the
//!   agent's identity. In loopback mode `hello.worker_id` is only a label
//!   for logs; in remote mode it must be the worker id the token file
//!   binds the token to, and a token whose line is deleted is refused at
//!   its next request and its channel closed at its next heartbeat. The
//!   token file fails closed.
//! - Each channel has a frame budget (§4.3, `FrameBudget`), past which it
//!   is closed with `rate_limited`.
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

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use axum::body::Bytes;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path as AxPath, Request, State};
use axum::handler::Handler;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, RequestExt, Router};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::progress::StageId;
use crate::service::clone::{self, RepoRef};
use crate::service::error::{ApiError, ErrorBody};
use crate::service::executor::{self, EventSink, JobInputs, WorkerOutput};
use crate::service::jobs::{self, JobSnapshot, JobStatus, SnapshotSink};
use crate::service::schedule::{self, Class};
use crate::service::store::Requeued;
use crate::service::worker_result;
use crate::service::AppState;
use crate::worker::{
    is_valid_artifact_name, negotiate, Artifact, AssignInputs, CancelReason, JobSpec,
    MasterMessage, PreviousMapUrl, ReleasedReason, ResumeAction, ResumeEntry, ShutdownMode,
    WelcomeResume, WorkerBuild, WorkerEvent, WorkerMessage, FEATURE_LOCAL_PATHS, FEATURE_RESUME,
    MAX_CONTROL_FRAME_BYTES, PROTO,
};

/// §2.2's defaults: a heartbeat every 15 s, the SSE heartbeat's interval,
/// and a 60 s lease. Both configurable, so tests can shorten the lease.
pub const DEFAULT_HEARTBEAT_S: u64 = 15;
pub const DEFAULT_LEASE_TTL_S: u64 = 60;

/// The header an upload declares its SHA-256 in (§4.2), as 64 lowercase hex
/// digits. Its size is the request's `Content-Length`.
pub const SHA256_HEADER: &str = "x-tolmap-sha256";

/// One lease may keep at most 1 GiB of unique artifact blobs, with pending
/// uploads reserved against the same total. This is four map allowances:
/// enough for the map, full symbols sibling and district files, while
/// bounding what one worker can put on the master's shared cache volume.
const WORKER_LEASE_MAX_ARTIFACT_BYTES: u64 = 4 * worker_result::EARLY_MAP_MAX_BYTES;

/// A stalled upload owns one blocking writer while its body is being hashed.
/// Keep one lease from occupying an unbounded share of Tokio's process-wide
/// blocking pool, while still allowing a small batch of artifact writes.
const WORKER_LEASE_MAX_IN_FLIGHT_UPLOADS: usize = 8;

/// A body that makes no progress for this long is abandoned so its upload
/// writer, reservation and temporary file can be released.
const WORKER_ARTIFACT_BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

const fn max_upload_bytes(left: u64, right: u64) -> u64 {
    if left > right {
        left
    } else {
        right
    }
}

/// The route-level streaming ceiling is the largest per-artifact cap. Keep
/// this layer on PUT alone; GET artifacts and the worker channel have separate
/// body semantics. The map and full-symbols caps are both currently 256 MiB.
const WORKER_ARTIFACT_ROUTE_BODY_LIMIT: u64 = max_upload_bytes(
    max_upload_bytes(
        worker_result::EARLY_MAP_MAX_BYTES,
        worker_result::FULL_SYMBOLS_MAX_BYTES,
    ),
    max_upload_bytes(
        worker_result::DISTRICT_SYMBOLS_MAX_BYTES,
        worker_result::NAMES_CACHE_MAX_BYTES,
    ),
);

/// The lease may name up to 10,000 distinct artifacts, including its fixed
/// outputs. That leaves room for nearly 10,000 district files while keeping
/// a worker from creating unbounded tiny files and registry entries.
const WORKER_LEASE_MAX_ARTIFACTS: usize = 10_000;

/// §10.6: two lost-worker retries per class by default.
pub const DEFAULT_RETRIES: u32 = 2;

/// How often a running job's snapshot is written to the store between
/// stage boundaries (§3.5: "at stage boundaries and every few seconds").
const SNAPSHOT_EVERY: Duration = Duration::from_secs(3);

/// `stage` of a job re-queued after its lease ran out (§2.2).
const WORKER_LOST: &str = "worker lost, retrying on another worker";

/// `stage` of a job its agent released for a graceful stop.
const STOPPED: &str = "the service stopped; the job runs again when it is back";

/// `stage` of a job its agent released because the worker is stopping.
const WORKER_STOPPED: &str = "the worker stopped; retrying on another worker";

/// `stage` of a job moved to a larger class after its `features` (§2.1
/// step 3).
const REROUTED: &str =
    "the repository needs more memory than its worker class holds; moving it to a larger one";

/// `stage` of a job whose job child was killed for memory (§6).
const OUT_OF_MEMORY: &str = "the worker ran out of memory; retrying on a larger worker class";

/// `stage` of a job whose lost worker was near its memory limit (§6).
const LOST_TO_MEMORY: &str = "worker lost near its memory limit; retrying on a larger worker class";

/// The `worker_crashed` message of a job killed for memory on the largest
/// class (§6): there is nowhere larger to go.
const OOM_ON_LARGEST: &str = "out of memory on the largest worker class";

/// How often a waiting runner re-checks cancellation and lease expiry.
const POLL: Duration = Duration::from_millis(250);

/// An agent that upgrades and then says nothing is closed after this.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a stopping master waits for its agents to exit after `shutdown
/// now` before it kills them.
const REAP_GRACE: Duration = Duration::from_secs(10);

/// Settled results remembered for a repeat or a resume (`HubInner::settled`).
/// An agent that lost its verdict asks again within a lease TTL or so; one
/// that asks after its entry is gone gets `cancel` and discards a result the
/// store already has, which costs nothing.
const SETTLED_KEPT: usize = 256;

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
    /// `remote` or `remote:N` (#97 phase 3): the worker listener, with TLS
    /// and the owner's token file, and no agents of its own; agents dial in
    /// from other hosts. N is how many jobs run at once (one slot per
    /// expected worker, 1 by default: §10.1's fleet is one worker).
    Remote(usize),
}

impl WorkersMode {
    /// Anything but the documented forms is a startup error rather than a
    /// fallback: this variable decides whether a listener opens and child
    /// processes start, so a typo must not quietly pick either mode.
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        let Some(value) = value.map(str::trim) else {
            return Ok(WorkersMode::Local);
        };
        if value.is_empty() || value == "local" {
            return Ok(WorkersMode::Local);
        }
        let count = |prefix: &str| {
            value
                .strip_prefix(prefix)
                .and_then(|count| count.parse::<usize>().ok())
                .filter(|count| *count >= 1)
        };
        if let Some(count) = count("loopback:") {
            return Ok(WorkersMode::Loopback(count));
        }
        if value == "remote" {
            return Ok(WorkersMode::Remote(1));
        }
        if let Some(count) = count("remote:") {
            return Ok(WorkersMode::Remote(count));
        }
        Err(format!(
            "TOLMAP_WORKERS must be unset, `local`, `loopback:N`, `remote` or `remote:N` with N \
             at least 1; got {value:?}"
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
    /// `TOLMAP_WORKER_RETRIES`: lost-worker retries before a job fails.
    pub retries: u32,
}

impl LoopbackSettings {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::read(false)
    }

    /// `remote`: whether the listener may take a non-loopback address.
    /// Remote mode checks TLS for one itself (`RemoteSettings::from_env`).
    fn read(remote: bool) -> anyhow::Result<Self> {
        let listen = match std::env::var("TOLMAP_WORKER_LISTEN") {
            Ok(value) => value.trim().parse::<SocketAddr>().with_context(|| {
                format!("TOLMAP_WORKER_LISTEN {value:?} is not an address:port")
            })?,
            Err(_) => SocketAddr::from(([127, 0, 0, 1], 0)),
        };
        // Trust decision (§5.6): in loopback mode the only agents are this
        // process's own children, so the worker endpoint has no reason to
        // be reachable from anywhere but this host. A non-loopback address
        // is refused rather than honoured: exposing it is remote mode's
        // job, which requires TLS for it (`remote_tls_files`).
        if !remote && !listen.ip().is_loopback() {
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
            // Zero is a valid bound (no retries), unlike the two above.
            retries: std::env::var("TOLMAP_WORKER_RETRIES")
                .ok()
                .and_then(|value| value.trim().parse::<u32>().ok())
                .unwrap_or(DEFAULT_RETRIES),
        })
    }
}

/// `TOLMAP_LOOPBACK_CLASSES` (#97 phase 2, step 4; docs/WORKER_TIER.md §8
/// "Phase 2"): the classes loopback agents advertise, as `<usable
/// memory>:<agents>` pairs separated by commas, e.g. `2GiB:1,16GiB:1`. The
/// memory is a whole number of bytes, optionally with a `KiB`, `MiB`, `GiB`
/// or `TiB` suffix; the counts must add up to `TOLMAP_WORKERS`'s N. Unset
/// or empty: one class of unknown size holding all N agents, as in phase 1,
/// each advertising its host's memory. Returned smallest first
/// (`schedule::order_classes`), which is also how agents are numbered: agent
/// `i` is `schedule::worker_classes(classes)[i]`'s agent, as slot `i` is.
///
/// Anything malformed stops startup, like `TOLMAP_WORKERS`: a typo must not
/// quietly start the wrong agents. A class named twice is refused rather
/// than merged, so the configuration reads as what it starts.
pub fn loopback_classes(value: Option<&str>, agents: usize) -> Result<Vec<Class>, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(vec![Class {
            usable_memory: None,
            slots: agents,
        }]);
    };
    let bad = |why: String| {
        format!(
            "TOLMAP_LOOPBACK_CLASSES {value:?}: {why}; expected <memory>:<agents>[,...], \
             e.g. 2GiB:1,16GiB:1"
        )
    };
    let mut classes: Vec<Class> = Vec::new();
    for part in value.split(',').map(str::trim) {
        let (memory, count) = part
            .split_once(':')
            .ok_or_else(|| bad(format!("{part:?} is not <memory>:<agents>")))?;
        let usable = parse_bytes(memory.trim()).ok_or_else(|| {
            bad(format!(
                "{memory:?} is not a size such as 512MiB, 16GiB or a number of bytes"
            ))
        })?;
        let slots = count
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|slots| *slots >= 1)
            .ok_or_else(|| bad(format!("{count:?} is not an agent count of at least 1")))?;
        if classes
            .iter()
            .any(|class| class.usable_memory == Some(usable))
        {
            return Err(bad(format!("{memory:?} is named twice")));
        }
        classes.push(Class {
            usable_memory: Some(usable),
            slots,
        });
    }
    let total: usize = classes.iter().map(|class| class.slots).sum();
    if total != agents {
        return Err(bad(format!(
            "it names {total} agent(s), but TOLMAP_WORKERS starts {agents}"
        )));
    }
    schedule::order_classes(&mut classes);
    Ok(classes)
}

/// A byte count: digits, then nothing, `B`, `KiB`, `MiB`, `GiB` or `TiB`
/// (powers of 1024). Decimal units (`GB`) are refused, not guessed at: the
/// two differ by 7% at a GiB, enough to put a job in the wrong class.
fn parse_bytes(text: &str) -> Option<u64> {
    let digits = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(digits);
    let number: u64 = number.parse().ok()?;
    let shift = match unit.trim() {
        "" | "B" => 0,
        "KiB" => 10,
        "MiB" => 20,
        "GiB" => 30,
        "TiB" => 40,
        _ => return None,
    };
    number.checked_mul(1u64 << shift).filter(|bytes| *bytes > 0)
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

// ---- remote mode: TLS, the token file, frame limits (#97 phase 3) ----------

/// §4.3: the default bound on an agent's control frames, per channel, as a
/// sustained rate per second and a burst. It bounds the protocol, not jobs:
/// a job child's progress is throttled to four events a second per stage
/// (`progress::StageCounter`), log lines are few, and the largest honest
/// burst is a resumed agent replaying its buffer, which `agent::Outbox`
/// bounds by the stages a job runs. So an agent over this is broken or
/// hostile, never a large repository.
pub const DEFAULT_FRAME_RATE: u32 = 200;
pub const DEFAULT_FRAME_BURST: u32 = 1000;

/// A TLS handshake on the worker listener that takes longer than this is
/// dropped, so a peer that connects and stalls holds nothing for long.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// `TOLMAP_WORKER_FRAME_RATE` and `TOLMAP_WORKER_FRAME_BURST`.
fn frame_limit_from_env() -> (u32, u32) {
    let read = |key: &str, default: u32| {
        u32::try_from(positive_env(key, u64::from(default))).unwrap_or(default)
    };
    (
        read("TOLMAP_WORKER_FRAME_RATE", DEFAULT_FRAME_RATE),
        read("TOLMAP_WORKER_FRAME_BURST", DEFAULT_FRAME_BURST),
    )
}

/// What remote mode reads at startup, on top of the lease settings loopback
/// mode reads too.
pub struct RemoteSettings {
    pub lease: LoopbackSettings,
    /// `TOLMAP_WORKER_TLS_CERT` and `TOLMAP_WORKER_TLS_KEY`, both PEM.
    pub tls: Option<(PathBuf, PathBuf)>,
    /// `TOLMAP_WORKER_TOKENS`: the owner's token file.
    pub tokens: PathBuf,
}

impl RemoteSettings {
    pub fn from_env() -> anyhow::Result<Self> {
        let lease = LoopbackSettings::read(true)?;
        let var = |key: &str| {
            std::env::var(key)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let tls = remote_tls_files(
            lease.listen,
            var("TOLMAP_WORKER_TLS_CERT"),
            var("TOLMAP_WORKER_TLS_KEY"),
        )
        .map_err(anyhow::Error::msg)?;
        let tokens = var("TOLMAP_WORKER_TOKENS").map(PathBuf::from).context(
            "TOLMAP_WORKERS=remote needs TOLMAP_WORKER_TOKENS: the file of `<sha256> <worker_id>` \
             lines naming the workers that may connect",
        )?;
        Ok(RemoteSettings { lease, tls, tokens })
    }
}

/// Trust decision (§5.1, §5.6): every bearer token crosses TLS except on
/// loopback. A listener on any other address -- the private network's, or
/// every interface -- starts only with both a certificate and a key, and
/// there is no fallback to plaintext: a missing file stops startup rather
/// than opening the listener without TLS. A loopback listener may run
/// without TLS, since nothing leaves the host (CI runs the protocol so).
/// Half a configuration is refused too, so a typo in one name cannot
/// quietly leave the other unused.
pub fn remote_tls_files(
    listen: SocketAddr,
    cert: Option<String>,
    key: Option<String>,
) -> Result<Option<(PathBuf, PathBuf)>, String> {
    match (cert, key) {
        (Some(cert), Some(key)) => Ok(Some((PathBuf::from(cert), PathBuf::from(key)))),
        (None, None) if listen.ip().is_loopback() => Ok(None),
        (None, None) => Err(format!(
            "TOLMAP_WORKER_LISTEN {listen} is not a loopback address, so the worker listener \
             needs TLS: set TOLMAP_WORKER_TLS_CERT and TOLMAP_WORKER_TLS_KEY (worker tokens never \
             cross a network in plaintext)"
        )),
        _ => Err(
            "TOLMAP_WORKER_TLS_CERT and TOLMAP_WORKER_TLS_KEY go together: set both, or \
                  neither on a loopback listener"
                .to_owned(),
        ),
    }
}

/// Refuses a file that its group or anyone else may read, write or run:
/// the listener's private key (and, on a worker host, the agent's token).
/// Trust decision: a key others can read is a key others hold, and one they
/// can write is a listener they can impersonate. Refused at startup with
/// the fix, never repaired behind the owner's back.
pub(crate) fn check_private_file(path: &Path, what: &str) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(path).with_context(|| format!("{what} {}", path.display()))?;
    if !metadata.is_file() {
        bail!("{what} {} is not a regular file", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            bail!(
                "{what} {} has mode {mode:o}, so its group or others can use it; make it 0600 \
                 (`chmod 600`), owned by the user tolmap runs as",
                path.display()
            );
        }
    }
    Ok(())
}

/// The worker listener's TLS configuration (§5.1): the certificate chain
/// and key from PEM files, TLS 1.3 only, no client certificates (workers
/// authenticate with their bearer token, §10.4 rules out mutual TLS).
/// Trust decisions: rustls on the `ring` provider, the only one this build
/// has; TLS 1.3 only, since both ends are this binary and nothing older
/// needs to connect; and the key file must be private
/// (`check_private_file`).
pub(crate) fn server_tls(
    cert: &Path,
    key: &Path,
) -> anyhow::Result<Arc<tokio_rustls::rustls::ServerConfig>> {
    use tokio_rustls::rustls;
    use tokio_rustls::rustls::pki_types::pem::PemObject;
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
    check_private_file(key, "TOLMAP_WORKER_TLS_KEY")?;
    let chain = CertificateDer::pem_file_iter(cert)
        .and_then(|certs| certs.collect::<Result<Vec<_>, _>>())
        .with_context(|| format!("read TOLMAP_WORKER_TLS_CERT {}", cert.display()))?;
    if chain.is_empty() {
        bail!(
            "TOLMAP_WORKER_TLS_CERT {} holds no PEM certificate",
            cert.display()
        );
    }
    let private = PrivateKeyDer::from_pem_file(key)
        .with_context(|| format!("read TOLMAP_WORKER_TLS_KEY {}", key.display()))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("configure TLS 1.3 for the worker listener")?
        .with_no_client_auth()
        .with_single_cert(chain, private)
        .context("the worker listener's certificate and key do not make a usable pair")?;
    Ok(Arc::new(config))
}

/// The worker listener with TLS, for `axum::serve`. Handshakes run in
/// tasks of their own, each bounded by `TLS_HANDSHAKE_TIMEOUT`, so one
/// slow or silent peer never holds up the next connection; only a finished
/// handshake is handed to axum. A failed one is logged (that line is how an
/// operator finds an agent that does not trust the certificate) and
/// dropped.
struct TlsListener {
    accepted: tokio::sync::mpsc::Receiver<(
        tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
        SocketAddr,
    )>,
    local: SocketAddr,
}

impl TlsListener {
    fn start(
        listener: tokio::net::TcpListener,
        config: Arc<tokio_rustls::rustls::ServerConfig>,
    ) -> std::io::Result<Self> {
        let local = listener.local_addr()?;
        let acceptor = tokio_rustls::TlsAcceptor::from(config);
        let (sender, accepted) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            loop {
                let (tcp, peer) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        eprintln!("worker listener: accept failed: {error}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                if sender.is_closed() {
                    return;
                }
                let (acceptor, sender) = (acceptor.clone(), sender.clone());
                tokio::spawn(async move {
                    match tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
                        Ok(Ok(stream)) => {
                            let _ = sender.send((stream, peer)).await;
                        }
                        Ok(Err(error)) => {
                            eprintln!("worker listener: TLS handshake with {peer} failed: {error}")
                        }
                        Err(_) => eprintln!("worker listener: TLS handshake with {peer} timed out"),
                    }
                });
            }
        });
        Ok(TlsListener { accepted, local })
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.accepted.recv().await {
            Some(accepted) => accepted,
            // The accept loop holds a sender for as long as it runs, and it
            // runs for the process's life.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// A worker id (§5.1): what a token file line binds a token to, and what an
/// agent must name in `hello.worker_id`. 1 to 64 ASCII letters, digits,
/// `.`, `_` or `-`: it lands in logs and in the store's `lease_holder`, so
/// it is kept to characters that cannot break either.
pub fn is_valid_worker_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// `tolmap worker-token new --id <id>` (§5.1): a fresh random 256-bit token
/// and the line binding its SHA-256 to `id` in the master's token file. The
/// caller prints both once; nothing is written or logged here.
pub fn issue_worker_token(id: &str) -> Result<(String, String), String> {
    if !is_valid_worker_id(id) {
        return Err(format!(
            "worker id {id:?} must be 1 to 64 ASCII letters, digits, `.`, `_` or `-`"
        ));
    }
    let token = new_token();
    let line = format!("{} {id}", hex(&token_digest(token.as_bytes())));
    Ok((token, line))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 64 hex digits as the 32 bytes they spell.
fn unhex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// A token file (§5.1): one `<sha256 hex> <worker_id>` per line, blank
/// lines and `#` comments allowed. A worker id may appear on several lines
/// (rotation adds the new token's line before the old one goes); a hash may
/// not, since it would bind one token to two workers. Anything malformed
/// refuses the whole file. Messages never echo a line's first field: an
/// owner who pasted the token itself instead of its hash would otherwise
/// find it in a log.
fn parse_token_file(text: &str) -> Result<Vec<([u8; 32], String)>, String> {
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(hash), Some(id), None) = (fields.next(), fields.next(), fields.next()) else {
            return Err(format!(
                "line {number}: expected `<sha256 hex> <worker_id>`"
            ));
        };
        let Some(digest) = unhex32(&hash.to_ascii_lowercase()) else {
            return Err(format!(
                "line {number}: the first field is not a SHA-256 as 64 hex digits"
            ));
        };
        if !is_valid_worker_id(id) {
            return Err(format!(
                "line {number}: worker id {id:?} must be 1 to 64 ASCII letters, digits, `.`, `_` \
                 or `-`"
            ));
        }
        if !seen.insert(digest) {
            return Err(format!(
                "line {number}: the same token hash is on an earlier line"
            ));
        }
        entries.push((digest, id.to_owned()));
    }
    Ok(entries)
}

/// What a token file looked like when it was last read: a change in any of
/// these re-reads it. A rename over it (the safe way to edit it) changes
/// the inode; an edit in place changes the size or the modification time;
/// a `chmod` changes the mode, which `TokenFile::read` checks.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    inode: u64,
    device: u64,
    mode: u32,
}

fn stamp_of(metadata: &std::fs::Metadata) -> FileStamp {
    #[cfg(unix)]
    let (inode, device, mode) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.ino(), metadata.dev(), metadata.mode())
    };
    #[cfg(not(unix))]
    let (inode, device, mode) = (0, 0, 0);
    FileStamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        inode,
        device,
        mode,
    }
}

/// Remote mode's tokens (§5.1): the owner's token file, holding only
/// hashes. It is checked for a change (one `stat`) on every authenticated
/// request -- each channel upgrade, each artifact request -- and on every
/// heartbeat, and re-read when it changed, so deleting a line revokes that
/// worker on its next connection, its next artifact request and its next
/// heartbeat, with no restart.
///
/// Trust decision: it fails closed. A file that cannot be read, is
/// malformed, or can be written by its group or others refuses every token
/// until it is fixed, rather than keeping the tokens it held before: an
/// edit meant to revoke a worker must never leave that worker in. Replace
/// the file by renaming a new one over it, so no request reads it half
/// written.
pub(crate) struct TokenFile {
    path: PathBuf,
    state: Mutex<TokenFileState>,
}

#[derive(Default)]
struct TokenFileState {
    /// The file as last read; `None` while it could not be.
    stamp: Option<FileStamp>,
    /// Its entries; `None` while it is unusable, when every token is
    /// refused.
    entries: Option<Vec<([u8; 32], String)>>,
    /// The last problem logged, so an unusable file is reported once per
    /// problem, not once per request.
    problem: Option<String>,
    /// A number per worker id for this process's life: the `agent` a
    /// channel, a lease and a settled result record (`Conn::agent`). Two
    /// tokens for one worker id -- mid-rotation -- are the same agent, and
    /// an adopted lease names its holder by worker id, so a restarted
    /// master knows which agent may resume it.
    ids: BTreeMap<String, usize>,
}

impl TokenFileState {
    fn agent(&mut self, id: &str) -> usize {
        let next = self.ids.len();
        *self.ids.entry(id.to_owned()).or_insert(next)
    }
}

impl TokenFile {
    /// Reads the file at startup, where a problem stops the service instead
    /// of refusing every worker.
    pub(crate) fn open(path: &Path) -> anyhow::Result<Self> {
        let (stamp, entries) = Self::read(path).map_err(anyhow::Error::msg)?;
        if entries.is_empty() {
            eprintln!(
                "worker tokens: {} lists no worker yet; no agent can connect until a line is added",
                path.display()
            );
        }
        Ok(TokenFile {
            path: path.to_path_buf(),
            state: Mutex::new(TokenFileState {
                stamp: Some(stamp),
                entries: Some(entries),
                ..TokenFileState::default()
            }),
        })
    }

    fn read(path: &Path) -> Result<(FileStamp, Vec<([u8; 32], String)>), String> {
        let shown = path.display();
        let metadata = std::fs::metadata(path)
            .map_err(|error| format!("TOLMAP_WORKER_TOKENS {shown}: {error}"))?;
        // Trust decision: a file others can write is a file others can add
        // a worker to. Its contents are hashes, so reading is harmless.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o022 != 0 {
                return Err(format!(
                    "TOLMAP_WORKER_TOKENS {shown} has mode {mode:o}, so its group or others can \
                     write it; make it writable by its owner only (`chmod go-w`)"
                ));
            }
        }
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("TOLMAP_WORKER_TOKENS {shown}: {error}"))?;
        let entries = parse_token_file(&text)
            .map_err(|why| format!("TOLMAP_WORKER_TOKENS {shown}: {why}"))?;
        Ok((stamp_of(&metadata), entries))
    }

    /// Re-reads the file if it changed since it was last read.
    fn refresh(&self, state: &mut TokenFileState) {
        let stamp = std::fs::metadata(&self.path)
            .ok()
            .map(|metadata| stamp_of(&metadata));
        if stamp.is_some() && stamp == state.stamp && state.entries.is_some() {
            return;
        }
        match Self::read(&self.path) {
            Ok((stamp, entries)) => {
                eprintln!(
                    "worker tokens: re-read {} ({} line(s))",
                    self.path.display(),
                    entries.len()
                );
                state.stamp = Some(stamp);
                state.entries = Some(entries);
                state.problem = None;
            }
            Err(problem) => {
                if state.problem.as_deref() != Some(problem.as_str()) {
                    eprintln!(
                        "worker tokens: {problem}; refusing every worker token until it is fixed"
                    );
                }
                state.stamp = stamp;
                state.entries = None;
                state.problem = Some(problem);
            }
        }
    }

    /// The agent and worker id `presented` (a token's digest) is bound to
    /// now, compared with every line without stopping early.
    fn lookup(&self, presented: &[u8; 32]) -> Option<(usize, String)> {
        let mut state = self.state.lock().expect("token file mutex poisoned");
        self.refresh(&mut state);
        let mut found = None;
        for (digest, id) in state.entries.as_deref().unwrap_or_default() {
            if digests_equal(presented, digest) {
                found = Some(id.clone());
            }
        }
        let id = found?;
        Some((state.agent(&id), id))
    }

    /// The agent number of worker `id`, whether or not it holds a token now.
    fn agent(&self, id: &str) -> usize {
        self.state
            .lock()
            .expect("token file mutex poisoned")
            .agent(id)
    }
}

/// How the hub authenticates agents.
enum Tokens {
    /// Loopback mode: one token per agent, minted at startup; an agent is
    /// its index here.
    Minted(Vec<[u8; 32]>),
    /// Remote mode: the owner's token file.
    File(TokenFile),
}

/// Who a request's bearer token says it is.
#[derive(Clone, Debug)]
pub(crate) struct Identity {
    /// `Conn::agent`: the minted token's index, or the worker id's number.
    agent: usize,
    /// The presented token's SHA-256, to check it is still listed.
    digest: [u8; 32],
    /// The token file's worker id for it; `None` for a minted token.
    worker_id: Option<String>,
}

/// §4.3's per-channel frame bound: a token bucket holding up to `burst`
/// frames, refilled at `rate` a second.
struct FrameBudget {
    rate: f64,
    burst: f64,
    tokens: f64,
    at: Instant,
}

impl FrameBudget {
    fn new(rate: u32, burst: u32) -> Self {
        let burst = f64::from(burst.max(1));
        FrameBudget {
            rate: f64::from(rate.max(1)),
            burst,
            tokens: burst,
            at: Instant::now(),
        }
    }

    /// Spends one frame; `false` when the budget is gone.
    fn take(&mut self) -> bool {
        let now = Instant::now();
        let refill = now.duration_since(self.at).as_secs_f64() * self.rate;
        self.tokens = (self.tokens + refill).min(self.burst);
        self.at = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

// ---- build identity and tokens ---------------------------------------------

/// This binary's identity for `hello.build` (§3.6): the crate version, the
/// git commit it was built from, and the SCIP indexer versions this host's
/// image pins. Phase 1 used the executable's own SHA-256 for `commit`
/// (#153's departure 5) because no git commit was compiled in and no
/// indexer versions file existed yet; both now do -- see `build_commit` and
/// `read_indexer_versions`.
pub fn own_build() -> WorkerBuild {
    static BUILD: OnceLock<WorkerBuild> = OnceLock::new();
    BUILD
        .get_or_init(|| WorkerBuild {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            commit: build_commit(),
            indexers: read_indexer_versions(),
        })
        .clone()
}

/// The git commit this binary was built from (§3.6, `build.rs`'s
/// `emit_build_commit`): `TOLMAP_BUILD_COMMIT`, baked in at compile time --
/// from CI's or the Dockerfile's build-arg env var if one was set, else
/// `git rev-parse HEAD` run at compile time, in that order. `"unknown"`
/// when build.rs found neither (a source tarball with no `.git` and no
/// build arg): `same_build` never matches an `"unknown"` commit, including
/// to another `"unknown"`, so a build that cannot name its own commit is
/// idle rather than trusted by default, same stance #153 took for a binary
/// whose hash it could not read.
///
/// Tests get a matching identity for free: `own_build()` is memoized once
/// per process (`OnceLock`), so every call inside one `cargo test` binary
/// returns the identical `WorkerBuild` -- version, commit and indexers all
/// equal by construction, whatever `TOLMAP_BUILD_COMMIT` happened to be at
/// compile time. A test that wants a *different* build (to exercise a
/// mismatch) constructs one explicitly instead, as `a_build_mismatch_
/// leaves_the_agent_idle` already does.
fn build_commit() -> String {
    match option_env!("TOLMAP_BUILD_COMMIT") {
        Some(commit) if !commit.trim().is_empty() => commit.trim().to_owned(),
        _ => "unknown".to_owned(),
    }
}

/// Path `own_build` reads the SCIP indexer versions from, unless
/// `TOLMAP_INDEXER_VERSIONS` overrides it: the runtime image writes this at
/// build time (`.github/workflows/scip-image-build.yml`, `Dockerfile`),
/// next to the indexers themselves, so a worker never has to run each one
/// just to ask its version at agent start.
const DEFAULT_INDEXER_VERSIONS_PATH: &str = "/usr/local/share/tolmap/indexers.json";

/// The SCIP indexer versions this host's image pins (§3.6, §8 phase 3):
/// read once from a small `{"scip-python": "0.6.6", ...}` file, never by
/// running the indexers themselves (slow, and it would execute untrusted
/// tools at agent start for no reason). No file -- a loopback agent, which
/// is this binary on this host and has nothing to compare yet, or an image
/// that predates this file -- is an empty map, as phase 1 always was. A
/// file that exists but does not parse is also an empty map, logged rather
/// than a startup failure: a worker with a broken versions file should stay
/// connected and report what it can, not refuse to start.
fn read_indexer_versions() -> BTreeMap<String, String> {
    let path = std::env::var_os("TOLMAP_INDEXER_VERSIONS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_INDEXER_VERSIONS_PATH));
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return BTreeMap::new(),
        Err(error) => {
            eprintln!(
                "worker build identity: could not read {}: {error}",
                path.display()
            );
            return BTreeMap::new();
        }
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        eprintln!(
            "worker build identity: {} did not parse as {{\"indexer\": \"version\"}}: {error}",
            path.display()
        );
        BTreeMap::new()
    })
}

/// §3.6: an `"unknown"` commit identifies nothing, not even itself -- two
/// binaries that could not name their own commit are not proven to be the
/// same build, so trusting them by default would silently reopen the
/// determinism gap #153 closed for a hashable one. Tests get a matching
/// identity through `own_build()`'s per-process memoization (its doc
/// comment), never by relying on two `"unknown"`s matching each other.
fn same_build(left: &WorkerBuild, right: &WorkerBuild) -> bool {
    left.commit != "unknown"
        && right.commit != "unknown"
        && left.version == right.version
        && left.commit == right.commit
        && left.indexers == right.indexers
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
    /// `hello.build` equals this master's (§3.6), and its advertised memory
    /// places it in a class. An agent that is not is kept connected and
    /// never assigned work.
    eligible: bool,
    /// The class its `hello.class.memory_bytes` places it in
    /// (`WorkerHub::class_of`); `None` for one smaller than every class.
    class: Option<usize>,
    local_paths: bool,
    ready: bool,
    draining: bool,
    /// The job whose lease this channel holds, including a cancelled job
    /// until `released` arrives or the lease expires (§2.4).
    holding: Option<Uuid>,
    /// The origin the agent dialled (`WorkerHub::origin`): the base of the
    /// artifact URLs in its `assign`s.
    base_url: String,
    out: tokio::sync::mpsc::UnboundedSender<Outgoing>,
}

/// An uploaded artifact, stored content-addressed at `<lease dir>/blobs/
/// <sha256>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Upload {
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
}

/// An upload's declared size and name, reserved before its body is read.
struct UploadReservation {
    name: String,
    bytes: u64,
}

/// The unique bytes already represented by this lease's upload names and
/// any old blobs awaiting unlink. Returns `None` only for inconsistent
/// metadata or arithmetic overflow, both treated as over the lease limit.
fn lease_blob_bytes(lease: &Lease) -> Option<u64> {
    let mut blobs = BTreeMap::<&str, u64>::new();
    for upload in lease.uploads.values() {
        if let Some(previous) = blobs.get(upload.sha256.as_str()) {
            if *previous != upload.bytes {
                return None;
            }
        } else {
            blobs.insert(upload.sha256.as_str(), upload.bytes);
        }
    }
    for (sha256, bytes) in &lease.orphaned_blobs {
        if let Some(previous) = blobs.get(sha256.as_str()) {
            if *previous != *bytes {
                return None;
            }
        } else {
            blobs.insert(sha256.as_str(), *bytes);
        }
    }
    blobs
        .values()
        .try_fold(0u64, |total, bytes| total.checked_add(*bytes))
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
}

/// The master's record that one agent holds one job (§2.2), a cache of the
/// job's row: the row holds its status, epoch and holder.
struct Lease {
    /// The channel holding it; `None` once that channel closed (the lease
    /// then runs out unless its agent resumes it) and for a lease a
    /// restarted master adopted from the store.
    conn: Option<u64>,
    /// The agent (token) holding it; `None` for an adopted lease.
    agent: Option<usize>,
    epoch: u64,
    deadline: Instant,
    /// The next `seq` to apply (invariant: seq ordering).
    next_seq: u64,
    cancelled: bool,
    /// The runner found it expired and is re-queueing the job: never
    /// renewed or resumed again (invariant: epoch fencing).
    lost: bool,
    /// A terminal event (`result`, `error`) or `released` has arrived;
    /// anything after it is dropped.
    finished: bool,
    /// `cancel` `reroute` was sent (§2.1 step 3). The job stays live, so a
    /// result already on its way is still registered; `resume` answers an
    /// agent whose channel dropped meanwhile with the same `cancel`.
    rerouting: bool,
    /// The agent's last heartbeat's `rss_bytes`: what the job held when its
    /// agent was last heard from, which tells a worker that died of memory
    /// (§6 "worker host dies").
    last_rss: Option<u64>,
    /// The artifacts the first `result` listed, to tell a repeat of it from
    /// a different one.
    result: Option<Vec<Artifact>>,
    events: std_mpsc::Sender<LeaseEvent>,
    /// The only files a GET may return for this lease, by name.
    inputs: BTreeMap<String, PathBuf>,
    uploads: BTreeMap<String, Upload>,
    /// Declared sizes held while their bodies stream, plus names not yet
    /// present in `uploads` for the artifact-count bound.
    reservations: BTreeMap<Uuid, UploadReservation>,
    /// Blobs whose last name was replaced but whose unlink failed. They
    /// remain on disk and count against the lease until cleanup or expiry.
    orphaned_blobs: BTreeMap<String, u64>,
    /// Serializes the short content-addressed blob install/remove phase.
    /// Bodies stream with no hub lock and no blob-operation lock held.
    blob_ops: Arc<Mutex<()>>,
    /// Master-owned `0700`: the names-cache input, uploads and blobs.
    dir: PathBuf,
}

/// A result that was registered or refused, remembered after its lease
/// ended (invariant: settled results).
struct Settled {
    epoch: u64,
    agent: Option<usize>,
    artifacts: Vec<Artifact>,
    accepted: bool,
    reason: String,
    /// The master's `acked_seq` for the job when it settled.
    last_seq: u64,
    /// Insertion number, so evicting an old entry never drops a newer one
    /// for the same job.
    order: u64,
}

impl Settled {
    fn verdict(&self, job_id: Uuid) -> MasterMessage {
        let (job_id, epoch, reason) = (job_id.to_string(), self.epoch, self.reason.clone());
        if self.accepted {
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
        }
    }
}

/// Whether two `result`s list the same artifacts, whatever their order.
fn same_artifacts(left: &[Artifact], right: &[Artifact]) -> bool {
    let set = |artifacts: &[Artifact]| {
        artifacts
            .iter()
            .map(|artifact| {
                (
                    artifact.name.clone(),
                    artifact.sha256.clone(),
                    artifact.bytes,
                )
            })
            .collect::<BTreeSet<_>>()
    };
    left.len() == right.len() && set(left) == set(right)
}

fn result_artifacts(event: &WorkerEvent) -> &[Artifact] {
    match event {
        WorkerEvent::Result { artifacts, .. } => artifacts,
        _ => &[],
    }
}

#[derive(Default)]
struct HubInner {
    conns: BTreeMap<u64, Conn>,
    leases: BTreeMap<Uuid, Lease>,
    /// Runners waiting in `claim`, by admission order: whether each job is
    /// a `local/<name>` one, and the class of agent it needs. Free agents
    /// go to the earliest first, so a re-queued job, which keeps its
    /// admission number, is the head of the queue here too.
    waiting: BTreeMap<(i64, Uuid), (bool, usize)>,
    /// The last `SETTLED_KEPT` settled results, by job, and their order.
    settled: BTreeMap<Uuid, Settled>,
    settled_order: VecDeque<(u64, Uuid)>,
    next_settled: u64,
    next_conn: u64,
    stopping: bool,
}

impl HubInner {
    fn settle(&mut self, job_id: Uuid, mut settled: Settled) {
        settled.order = self.next_settled;
        self.next_settled += 1;
        self.settled_order.push_back((settled.order, job_id));
        self.settled.insert(job_id, settled);
        while self.settled_order.len() > SETTLED_KEPT {
            if let Some((order, old)) = self.settled_order.pop_front() {
                if self
                    .settled
                    .get(&old)
                    .is_some_and(|entry| entry.order == order)
                {
                    self.settled.remove(&old);
                }
            }
        }
    }
}

/// What `claim` hands the runner.
pub(crate) struct Claimed {
    pub(crate) events: std_mpsc::Receiver<LeaseEvent>,
    pub(crate) dir: PathBuf,
    /// Who holds it, for the store's `lease_holder` and the logs.
    pub(crate) holder: String,
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
    /// Loopback mode: SHA-256 of each agent's token, an agent being its
    /// index there. Remote mode: the owner's token file.
    tokens: Tokens,
    /// `http`, or `https` behind the TLS listener: the scheme of every
    /// artifact URL.
    scheme: &'static str,
    /// §4.3: each channel's frame budget, per second and as a burst.
    frame_rate: u32,
    frame_burst: u32,
    build: WorkerBuild,
    heartbeat_s: u64,
    lease_ttl: Duration,
    #[allow(dead_code)] // Read by the streaming handler in the implementation commit.
    artifact_body_idle_timeout: Duration,
    /// Lost-worker retries before a job fails (§10.6).
    retries: u32,
    staging: PathBuf,
    /// `<scheme>://<listener address>`: the base of the artifact URLs of an
    /// agent whose request named no usable `Host` (`WorkerHub::origin`).
    base_url: String,
    /// Each class's usable memory, smallest first, the registry's classes
    /// in the registry's order (`JobRegistry::set_remote`); `None` is one
    /// of unknown size, which every agent fits.
    classes: Vec<Option<u64>>,
    /// Called, off this hub's lock, whenever an agent connects or goes away
    /// (`jobs::agents_changed`), so the registry can start what a new agent
    /// can take and quote queued jobs against the agents there are.
    on_agents: OnceLock<Box<dyn Fn() + Send + Sync>>,
}

impl WorkerHub {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        token_digests: Vec<[u8; 32]>,
        build: WorkerBuild,
        heartbeat_s: u64,
        lease_ttl: Duration,
        retries: u32,
        staging: PathBuf,
        base_url: String,
        classes: Vec<Option<u64>>,
    ) -> Self {
        WorkerHub {
            inner: Mutex::new(HubInner::default()),
            changed: Condvar::new(),
            tokens: Tokens::Minted(token_digests),
            scheme: "http",
            frame_rate: DEFAULT_FRAME_RATE,
            frame_burst: DEFAULT_FRAME_BURST,
            build,
            heartbeat_s,
            lease_ttl,
            artifact_body_idle_timeout: WORKER_ARTIFACT_BODY_IDLE_TIMEOUT,
            retries,
            staging,
            base_url,
            classes,
            on_agents: OnceLock::new(),
        }
    }

    #[cfg(test)]
    fn with_artifact_body_idle_timeout(mut self, timeout: Duration) -> Self {
        self.artifact_body_idle_timeout = timeout;
        self
    }

    /// Remote mode (#97 phase 3): agents authenticate against the owner's
    /// token file instead of minted tokens, a token binds its agent to a
    /// worker id (`hello.worker_id` must name it), and an adopted lease
    /// remembers its holder by that id so its agent can resume it after a
    /// restart. `tls`: the listener terminates TLS, so artifact URLs are
    /// `https`.
    pub(crate) fn with_token_file(mut self, tokens: TokenFile, tls: bool) -> Self {
        self.tokens = Tokens::File(tokens);
        self.scheme = if tls { "https" } else { "http" };
        self
    }

    /// §4.3: the frame budget of each channel.
    pub(crate) fn with_frame_limit(mut self, rate: u32, burst: u32) -> Self {
        self.frame_rate = rate;
        self.frame_burst = burst;
        self
    }

    /// Whether agents are remote workers holding the owner's tokens.
    fn remote(&self) -> bool {
        matches!(self.tokens, Tokens::File(_))
    }

    /// Whether this master has begun stopping (`shutdown_now`,
    /// `detach_all`).
    fn is_stopping(&self) -> bool {
        self.lock().stopping
    }

    /// Sets what `agents_changed` calls; once, before the listener serves.
    pub(crate) fn on_agents_changed(&self, callback: Box<dyn Fn() + Send + Sync>) {
        let _ = self.on_agents.set(callback);
    }

    /// Lock order: called with neither this hub's lock nor the registry's
    /// held, since the callback takes the registry's, which then takes this
    /// one's (`JobRegistry` reads `live_by_class` under its own lock).
    fn agents_changed(&self) {
        if let Some(callback) = self.on_agents.get() {
            callback();
        }
    }

    /// The class an agent advertising `memory_bytes` of usable memory
    /// belongs to: the largest whose usable memory it covers, so it can hold
    /// whatever that class promises. A class of unknown size (`None`, plain
    /// `loopback:N`) takes every agent. `None` when the agent covers no
    /// class: it is kept connected and never assigned work.
    fn class_of(&self, memory_bytes: u64) -> Option<usize> {
        self.classes
            .iter()
            .rposition(|usable| usable.is_none_or(|usable| memory_bytes >= usable))
    }

    /// Each configured class's usable memory, `None` for one of unknown
    /// size, smallest first -- the same order and length as `live_by_class`
    /// (§7.4, `GET /workers/capacity`).
    pub(crate) fn usable_memory_by_class(&self) -> &[Option<u64>] {
        &self.classes
    }

    /// The agents connected now in each class, by class index: every
    /// eligible agent counted once, however many channels it has open (an
    /// old one not yet noticed dead, and a resumed one).
    pub(crate) fn live_by_class(&self) -> Vec<usize> {
        let inner = self.lock();
        let agents: BTreeSet<(usize, usize)> = inner
            .conns
            .values()
            .filter(|conn| conn.eligible)
            .filter_map(|conn| conn.class.map(|class| (class, conn.agent)))
            .collect();
        let mut live = vec![0usize; self.classes.len()];
        for (class, _) in agents {
            if let Some(count) = live.get_mut(class) {
                *count += 1;
            }
        }
        live
    }

    fn lock(&self) -> MutexGuard<'_, HubInner> {
        self.inner.lock().expect("worker hub mutex poisoned")
    }

    fn lease_ttl_s(&self) -> u64 {
        self.lease_ttl.as_secs().max(1)
    }

    /// The agent a request's bearer token names, if any.
    pub(crate) fn authenticate(&self, headers: &HeaderMap) -> Option<usize> {
        self.identify(headers).map(|identity| identity.agent)
    }

    /// Who a request's bearer token says it is. Trust decision (§5.1): only
    /// the `Authorization` header is read -- a token in a URL would land in
    /// logs -- and the presented token is hashed and compared with every
    /// stored digest without stopping early. In remote mode the token file
    /// is checked for a change first, so a line deleted a moment ago
    /// refuses its token now.
    pub(crate) fn identify(&self, headers: &HeaderMap) -> Option<Identity> {
        let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
        let token = value.strip_prefix("Bearer ")?.trim();
        if token.is_empty() {
            return None;
        }
        let presented = token_digest(token.as_bytes());
        match &self.tokens {
            Tokens::Minted(digests) => {
                let mut found = None;
                for (agent, digest) in digests.iter().enumerate() {
                    if digests_equal(&presented, digest) {
                        found = Some(agent);
                    }
                }
                found.map(|agent| Identity {
                    agent,
                    digest: presented,
                    worker_id: None,
                })
            }
            Tokens::File(file) => file.lookup(&presented).map(|(agent, id)| Identity {
                agent,
                digest: presented,
                worker_id: Some(id),
            }),
        }
    }

    /// Whether `identity`'s token is still in the token file, for the same
    /// worker (§5.1: revoking a worker is deleting its line). Minted tokens
    /// live as long as the process.
    fn still_authorized(&self, identity: &Identity) -> bool {
        match &self.tokens {
            Tokens::Minted(_) => true,
            Tokens::File(file) => file
                .lookup(&identity.digest)
                .is_some_and(|(_, id)| Some(&id) == identity.worker_id.as_ref()),
        }
    }

    /// The origin an agent dialled, from its upgrade request's `Host`: the
    /// base of the artifact URLs its `assign`s name. The agent sends its
    /// token only to the origin it dialled (`agent::Endpoint::owns`), and a
    /// remote master is dialled by a name or address of the private
    /// network that its listener's own address (often every interface)
    /// does not spell. Trust decision: `Host` is the agent's own word, and
    /// the URLs made from it go back to that agent only, so an agent that
    /// lies misdirects itself and no one else; it must still be a bare
    /// authority, with no user info, path or query, or the listener's own
    /// address is used.
    fn origin(&self, headers: &HeaderMap) -> String {
        headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .filter(|host| !host.contains('@'))
            .and_then(|host| host.parse::<axum::http::uri::Authority>().ok())
            .map(|authority| format!("{}://{authority}", self.scheme))
            .unwrap_or_else(|| self.base_url.clone())
    }

    #[allow(clippy::too_many_arguments)]
    fn add_conn(
        &self,
        agent: usize,
        worker_id: String,
        eligible: bool,
        class: Option<usize>,
        local_paths: bool,
        base_url: String,
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
                class,
                local_paths,
                ready: false,
                draining: false,
                holding: None,
                base_url,
                out,
            },
        );
        self.changed.notify_all();
        id
    }

    /// A closed channel does not end its lease (§6 "channel lost, worker
    /// alive"): the lease is detached, and either its agent resumes it on a
    /// new channel within the TTL (`resume`) or it runs out at its deadline
    /// and the runner re-queues the job.
    fn remove_conn(&self, conn_id: u64) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        if let Some(conn) = inner.conns.remove(&conn_id) {
            eprintln!(
                "worker agent {} ({}) disconnected",
                conn.agent, conn.worker_id
            );
        }
        for (job_id, lease) in inner.leases.iter_mut() {
            if lease.conn == Some(conn_id) {
                lease.conn = None;
                eprintln!(
                    "job {job_id}: its agent's channel closed; the lease (epoch {}) runs out \
                     within {} s unless the agent resumes it",
                    lease.epoch,
                    self.lease_ttl_s()
                );
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
            WorkerMessage::Heartbeat { jobs, rss_bytes } => {
                let now = Instant::now();
                for held in jobs {
                    let Ok(id) = Uuid::parse_str(&held.job_id) else {
                        continue;
                    };
                    let Some(lease) = inner.leases.get_mut(&id) else {
                        continue;
                    };
                    if lease.conn != Some(conn_id) || lease.epoch != held.epoch || lease.lost {
                        continue;
                    }
                    lease.deadline = now + self.lease_ttl;
                    // One slot per agent: its resident memory is its job's.
                    if rss_bytes.is_some() {
                        lease.last_rss = rss_bytes;
                    }
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
                // Epoch fencing (§2.4, §3.5, §6 "duplicate or stale
                // result"): events for a job this channel does not hold at
                // this epoch are dropped -- a stale epoch, a job whose lease
                // already ended, or one it never held -- and a result among
                // them is answered `result_rejected`, never registered.
                let is_result = matches!(event, WorkerEvent::Result { .. });
                let reject = |reason: &str| {
                    if is_result {
                        if let Some(conn) = inner.conns.get(&conn_id) {
                            let _ =
                                conn.out
                                    .send(Outgoing::Message(MasterMessage::ResultRejected {
                                        job_id: job_id.clone(),
                                        epoch,
                                        reason: reason.to_owned(),
                                    }));
                        }
                    }
                };
                let Ok(id) = Uuid::parse_str(&job_id) else {
                    reject("not a job id");
                    return Ok(());
                };
                let agent = inner.conns.get(&conn_id).map(|conn| conn.agent);
                let Some(lease) = inner.leases.get_mut(&id) else {
                    // §6 "duplicate or stale result", after the lease
                    // ended: a repeat of the result this epoch settled gets
                    // the same verdict again, a different one is refused,
                    // and neither is registered (invariant: settled
                    // results).
                    let settled = inner
                        .settled
                        .get(&id)
                        .filter(|settled| settled.epoch == epoch && settled.agent == agent);
                    match settled {
                        Some(settled) if is_result => {
                            if same_artifacts(result_artifacts(&event), &settled.artifacts) {
                                if let Some(conn) = inner.conns.get(&conn_id) {
                                    let _ = conn.out.send(Outgoing::Message(settled.verdict(id)));
                                }
                            } else {
                                reject(
                                    "a different result for an epoch that already delivered one",
                                );
                            }
                        }
                        _ => reject("no live lease on this job: stale epoch, or the job ended"),
                    }
                    return Ok(());
                };
                // Epoch fencing: only the channel holding the live epoch,
                // and never a lease the runner already found expired.
                if lease.conn != Some(conn_id) || lease.epoch != epoch || lease.lost {
                    reject("stale epoch: the job's lease is held at another epoch");
                    return Ok(());
                }
                // Invariant (seq ordering): a repeat -- the replay a
                // resumed agent sends of what it could not know arrived --
                // is ignored; a gap is a violation.
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
                    // A second result for this epoch before its lease ended.
                    // A repeat of the first gets the first one's verdict:
                    // again from `settled` if it was given already, or, while
                    // the first is still being registered, the verdict still
                    // to come on this channel answers both. A different one,
                    // or anything after an error or `released`, is refused.
                    // Only the first is ever registered.
                    let repeat = lease
                        .result
                        .as_deref()
                        .is_some_and(|first| same_artifacts(result_artifacts(&event), first));
                    if !(is_result && repeat) {
                        reject("a different result for an epoch that already delivered one");
                    } else if let Some(settled) = inner
                        .settled
                        .get(&id)
                        .filter(|settled| settled.epoch == epoch)
                    {
                        if let Some(conn) = inner.conns.get(&conn_id) {
                            let _ = conn.out.send(Outgoing::Message(settled.verdict(id)));
                        }
                    }
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
                if is_result {
                    lease.result = Some(result_artifacts(&event).to_vec());
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
                let agent = inner.conns.get(&conn_id).map(|conn| conn.agent);
                let Some(lease) = inner.leases.get_mut(&id) else {
                    return Ok(());
                };
                // From the lease's agent on any of its channels: an agent
                // that killed a job while its channel was down, or after a
                // resume was answered `cancel`, releases it on the next
                // channel, which does not hold the lease. Never for a lease
                // already found expired, whose re-queue is under way.
                if agent.is_none() || lease.agent != agent || lease.epoch != epoch || lease.lost {
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

    /// The directory a lease at `epoch` keeps its inputs and uploads in.
    /// One per epoch, so an upload still in flight from a superseded epoch
    /// can never land beside the current epoch's files (§6 "stale result").
    fn lease_dir(&self, job_id: Uuid, epoch: u64) -> PathBuf {
        self.staging.join(format!("{job_id}-e{epoch}"))
    }

    /// Waits for an agent of `class` to take `job`, then leases it at
    /// `epoch` and sends `assign` (§2.3). `epoch` must already be in the
    /// store (`Store::begin_attempt`): that write is what keeps an epoch
    /// from ever being handed out twice. `order` is the job's admission
    /// number: a free agent goes to the waiting runner with the lowest one
    /// that it may take. `class` is the runner's slot's class, which may be
    /// larger than the job's own when the slot spilled down to it (§7.1).
    /// `None` when the job was cancelled, or the master began stopping,
    /// before any agent was free.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn claim(
        &self,
        job_id: Uuid,
        epoch: u64,
        order: i64,
        class: usize,
        job: &JobSpec,
        inputs: &JobInputs,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Claimed>, ErrorBody> {
        let internal = |error: std::io::Error| ApiError::internal(error.to_string()).body;
        let dir = self.lease_dir(job_id, epoch);
        let _ = std::fs::remove_dir_all(&dir);
        worker_result::create_private_dir(&dir).map_err(internal)?;
        worker_result::create_private_dir(&dir.join("blobs")).map_err(internal)?;
        let names = dir.join("names.json");
        crate::naming::save_cache(&names, &inputs.names).map_err(internal)?;
        let mut input_files = BTreeMap::new();
        input_files.insert("inputs/names".to_owned(), names);
        // Only the maps the job child could choose: the newest on each
        // branch (`executor::stage_previous_map` takes the newest on the
        // checked-out branch, else the newest overall, which is the newest
        // on its own branch). The agent learns its branch only after it
        // clones, so it gets one per branch; the files stay in the store
        // and are read at GET time, by the master, under its own uid.
        let mut previous = Vec::new();
        let mut branches = BTreeSet::new();
        for row in &inputs.previous_maps {
            if !branches.insert(row.branch.clone()) {
                continue;
            }
            let name = format!("inputs/previous/{}.json", previous.len());
            previous.push((row.branch.clone(), row.commit.clone(), name.clone()));
            input_files.insert(name, row.path.clone());
        }
        // The URLs name the origin the chosen agent dialled
        // (`Conn::base_url`), so the `assign` is made once it is chosen.
        let assign = |origin: &str| {
            let base = format!("{origin}/workers/artifacts/{job_id}/{epoch}");
            MasterMessage::Assign {
                job_id: job_id.to_string(),
                epoch,
                lease_ttl_s: self.lease_ttl_s(),
                job: job.clone(),
                inputs: AssignInputs {
                    names_cache: Some(format!("{base}/inputs/names")),
                    previous_maps: previous
                        .iter()
                        .map(|(branch, commit, name)| PreviousMapUrl {
                            branch: branch.clone(),
                            commit: commit.clone(),
                            url: format!("{base}/{name}"),
                        })
                        .collect(),
                },
                outputs: base,
            }
        };
        let me = (order, job_id);
        self.lock().waiting.insert(me, (job.local, class));
        let give_up = |guard: MutexGuard<'_, HubInner>| -> Result<Option<Claimed>, ErrorBody> {
            drop(guard);
            self.lock().waiting.remove(&me);
            let _ = std::fs::remove_dir_all(&dir);
            self.changed.notify_all();
            Ok(None)
        };
        loop {
            // The registry's lock is never taken under this one.
            if is_cancelled() {
                return give_up(self.lock());
            }
            let mut guard = self.lock();
            if guard.stopping {
                return give_up(guard);
            }
            let inner = &mut *guard;
            if let Some(conn_id) = pick_agent(inner, me) {
                inner.waiting.remove(&me);
                let (events, receiver) = std_mpsc::channel();
                let conn = inner.conns.get_mut(&conn_id).expect("picked from conns");
                conn.ready = false;
                conn.holding = Some(job_id);
                let agent = conn.agent;
                // The store's `lease_holder`. In remote mode it is the
                // worker id the agent's token is bound to (`serve_agent`
                // checked `hello.worker_id` against it), which is what lets
                // a restarted master hand the lease back to that worker
                // (`adopt`). A loopback agent's is only a label.
                let holder = if self.remote() {
                    conn.worker_id.clone()
                } else {
                    format!("agent {agent} ({})", conn.worker_id)
                };
                let message = assign(&conn.base_url);
                let _ = conn.out.send(Outgoing::Message(message));
                inner.leases.insert(
                    job_id,
                    Lease {
                        conn: Some(conn_id),
                        agent: Some(agent),
                        epoch,
                        deadline: Instant::now() + self.lease_ttl,
                        next_seq: 1,
                        cancelled: false,
                        lost: false,
                        finished: false,
                        rerouting: false,
                        last_rss: None,
                        result: None,
                        events,
                        inputs: input_files,
                        uploads: BTreeMap::new(),
                        reservations: BTreeMap::new(),
                        orphaned_blobs: BTreeMap::new(),
                        blob_ops: Arc::new(Mutex::new(())),
                        dir: dir.clone(),
                    },
                );
                self.changed.notify_all();
                eprintln!("job {job_id}: assigned to worker agent {agent} (epoch {epoch})");
                return Ok(Some(Claimed {
                    events: receiver,
                    dir,
                    holder,
                }));
            }
            let _ = self
                .changed
                .wait_timeout(guard, POLL)
                .expect("worker hub mutex poisoned");
        }
    }

    /// Takes over the lease a restarted master found in the store (§6
    /// "master restarts mid-job"): the same epoch, no channel, and a
    /// deadline one TTL from now, so the agent that held it has that long
    /// to come back. `holder` is the store's `lease_holder`.
    ///
    /// Remote mode (#97 phase 3, lifting #156's first departure): the
    /// holder is a worker id bound to a token in the owner's file, so the
    /// lease records that worker as its agent, and `resume` hands it back
    /// to an agent presenting a token for the same worker id at the same
    /// epoch -- the job carries on, no re-run, no attempt counted. Loopback
    /// mode: no holder (`agent: None`), so `resume` never hands it back: a
    /// minted token names no one after a restart, and loopback agents, this
    /// process's children, are new processes after one and hold nothing.
    /// Either way, a lease no one resumes runs out and the runner re-queues
    /// the job, uncounted.
    pub(crate) fn adopt(&self, job_id: Uuid, epoch: u64, holder: Option<&str>) -> Claimed {
        let agent = match (&self.tokens, holder) {
            (Tokens::File(file), Some(holder)) if is_valid_worker_id(holder) => {
                Some(file.agent(holder))
            }
            _ => None,
        };
        let dir = self.lease_dir(job_id, epoch);
        let _ = std::fs::remove_dir_all(&dir);
        // `blobs` too, as `claim` makes it: a lease resumed in remote mode
        // takes uploads, which land there.
        let made = worker_result::create_private_dir(&dir)
            .and_then(|()| worker_result::create_private_dir(&dir.join("blobs")));
        if let Err(error) = made {
            eprintln!("job {job_id}: could not create {}: {error}", dir.display());
        }
        let (events, receiver) = std_mpsc::channel();
        let mut inner = self.lock();
        inner.leases.insert(
            job_id,
            Lease {
                conn: None,
                agent,
                epoch,
                deadline: Instant::now() + self.lease_ttl,
                next_seq: 1,
                cancelled: false,
                lost: false,
                finished: false,
                rerouting: false,
                last_rss: None,
                result: None,
                events,
                inputs: BTreeMap::new(),
                uploads: BTreeMap::new(),
                reservations: BTreeMap::new(),
                orphaned_blobs: BTreeMap::new(),
                blob_ops: Arc::new(Mutex::new(())),
                dir: dir.clone(),
            },
        );
        eprintln!(
            "job {job_id}: its lease (epoch {epoch}) survived a restart; it runs out within {} s",
            self.lease_ttl_s()
        );
        Claimed {
            events: receiver,
            dir,
            holder: match (agent, holder) {
                (Some(_), Some(holder)) => holder.to_owned(),
                _ => "none since the restart".to_owned(),
            },
        }
    }

    /// The lease exists and no channel holds it.
    pub(crate) fn detached(&self, job_id: Uuid) -> bool {
        self.lock()
            .leases
            .get(&job_id)
            .is_some_and(|lease| lease.conn.is_none())
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
        if let Some(conn) = lease.conn.and_then(|conn| inner.conns.get(&conn)) {
            let _ = conn.out.send(Outgoing::Message(MasterMessage::Cancel {
                job_id: job_id.to_string(),
                epoch: lease.epoch,
                reason,
            }));
        }
    }

    /// §2.1 step 3: asks the lease's agent to stop the job so it can move to
    /// a larger class. Unlike `cancel` the job stays live: a result already
    /// on its way is still registered, and the lease ends with the agent's
    /// `released` (`reroute`). An agent whose channel is down meanwhile
    /// hears it from `resume` instead.
    pub(crate) fn reroute(&self, job_id: Uuid) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let Some(lease) = inner.leases.get_mut(&job_id) else {
            return;
        };
        lease.rerouting = true;
        if let Some(conn) = lease.conn.and_then(|conn| inner.conns.get(&conn)) {
            let _ = conn.out.send(Outgoing::Message(MasterMessage::Cancel {
                job_id: job_id.to_string(),
                epoch: lease.epoch,
                reason: CancelReason::Reroute,
            }));
        }
    }

    /// The `rss_bytes` of the last heartbeat that renewed the lease.
    pub(crate) fn last_rss(&self, job_id: Uuid) -> Option<u64> {
        self.lock()
            .leases
            .get(&job_id)
            .and_then(|lease| lease.last_rss)
    }

    /// Whether the lease has run out. Once it has, it is `lost` for good:
    /// no heartbeat renews it and no resume takes it back, so the re-queue
    /// the runner starts on this answer cannot race a resume (invariant:
    /// epoch fencing).
    pub(crate) fn expired(&self, job_id: Uuid) -> bool {
        let mut inner = self.lock();
        let Some(lease) = inner.leases.get_mut(&job_id) else {
            return false;
        };
        if Instant::now() > lease.deadline {
            lease.lost = true;
        }
        lease.lost
    }

    /// Answers `hello.resume` (§2.5, §3.5) for the channel `conn_id` of
    /// `agent`. `continue`, with the master's own `acked_seq`, for a lease
    /// this agent still holds -- the same token, the same epoch, neither
    /// cancelled nor run out -- which moves to the new channel with a fresh
    /// deadline, closing the old channel if the master had not noticed it
    /// die. `continue` too for a result this epoch already settled, whose
    /// verdict is then sent again after the `welcome`. `cancel` for
    /// everything else: `cancelled` for a job the user cancelled meanwhile,
    /// `lease_lost` otherwise -- another epoch holds the job, its lease ran
    /// out, it ended, or a restarted loopback master adopted it (an adopted
    /// lease has a holder only in remote mode, where it is a worker id:
    /// `adopt`).
    fn resume(&self, conn_id: u64, agent: usize, entries: Vec<ResumeEntry>) -> Vec<WelcomeResume> {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let now = Instant::now();
        let mut answers = Vec::with_capacity(entries.len());
        for entry in entries {
            let answer = |action, acked_seq, reason| WelcomeResume {
                job_id: entry.job_id.clone(),
                action,
                acked_seq,
                reason,
            };
            let cancel = |reason| answer(ResumeAction::Cancel, 0, Some(reason));
            let Ok(id) = Uuid::parse_str(&entry.job_id) else {
                answers.push(cancel(CancelReason::LeaseLost));
                continue;
            };
            // Checked before the lease: a result is settled a moment before
            // its lease ends, and the verdict sent then went to whichever
            // channel held the lease, possibly the dead one.
            if let Some(settled) = inner
                .settled
                .get(&id)
                .filter(|settled| settled.epoch == entry.epoch && settled.agent == Some(agent))
            {
                if let Some(conn) = inner.conns.get(&conn_id) {
                    let _ = conn.out.send(Outgoing::Message(settled.verdict(id)));
                }
                answers.push(answer(ResumeAction::Continue, settled.last_seq, None));
                continue;
            }
            let Some(lease) = inner.leases.get_mut(&id) else {
                answers.push(cancel(CancelReason::LeaseLost));
                continue;
            };
            // Run out is run out, whether or not the runner has polled yet:
            // marked here as `expired` would mark it, so the `released` this
            // `cancel` brings back is ignored rather than read as a job the
            // agent gave up unasked.
            if now > lease.deadline {
                lease.lost = true;
            }
            if lease.agent != Some(agent) || lease.epoch != entry.epoch || lease.lost {
                answers.push(cancel(CancelReason::LeaseLost));
                continue;
            }
            if lease.cancelled {
                answers.push(cancel(CancelReason::Cancelled));
                continue;
            }
            // A reroute's `cancel` that went to the dead channel (§2.1 step
            // 3): the agent hears it now. Its `released` comes back on the
            // new channel, and the fresh deadline leaves it time to.
            if lease.rerouting {
                lease.deadline = now + self.lease_ttl;
                answers.push(cancel(CancelReason::Reroute));
                continue;
            }
            if let Some(old) = lease.conn.filter(|old| *old != conn_id) {
                if let Some(conn) = inner.conns.get_mut(&old) {
                    if conn.holding == Some(id) {
                        conn.holding = None;
                    }
                    let _ = conn.out.send(Outgoing::Close);
                }
            }
            lease.conn = Some(conn_id);
            lease.deadline = now + self.lease_ttl;
            if let Some(conn) = inner.conns.get_mut(&conn_id) {
                conn.holding = Some(id);
            }
            let acked_seq = lease.next_seq - 1;
            eprintln!(
                "job {id}: worker agent {agent} resumed its lease (epoch {}) on a new channel; \
                 events up to seq {acked_seq} of its {} are applied",
                lease.epoch, entry.last_seq
            );
            answers.push(answer(ResumeAction::Continue, acked_seq, None));
        }
        self.changed.notify_all();
        answers
    }

    pub(crate) fn uploads(&self, job_id: Uuid) -> BTreeMap<String, Upload> {
        self.lock()
            .leases
            .get(&job_id)
            .map(|lease| lease.uploads.clone())
            .unwrap_or_default()
    }

    /// `result_accepted` or `result_rejected` for the lease's holder, on
    /// whichever channel holds the lease now, and remembered for a repeat
    /// or a resume (invariant: settled results).
    pub(crate) fn verdict(&self, job_id: Uuid, accepted: bool, reason: String) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let Some(lease) = inner.leases.get(&job_id) else {
            return;
        };
        let settled = Settled {
            epoch: lease.epoch,
            agent: lease.agent,
            artifacts: lease.result.clone().unwrap_or_default(),
            accepted,
            reason,
            last_seq: lease.next_seq - 1,
            order: 0,
        };
        if let Some(conn) = lease.conn.and_then(|conn| inner.conns.get(&conn)) {
            let _ = conn.out.send(Outgoing::Message(settled.verdict(job_id)));
        }
        inner.settle(job_id, settled);
    }

    /// Ends a lease: the agent's slot is free for its next `ready`, the
    /// lease's inputs and uploads are deleted (an epoch's files are never
    /// used by another epoch), and, for an expired lease, the channel is
    /// closed so the agent kills whatever it still runs.
    pub(crate) fn end_lease(&self, job_id: Uuid, close_channel: bool) {
        let dir = {
            let mut guard = self.lock();
            let inner = &mut *guard;
            let lease = inner.leases.remove(&job_id);
            if let Some(lease) = &lease {
                if let Some(conn) = lease.conn.and_then(|conn| inner.conns.get_mut(&conn)) {
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

    /// Remote mode's graceful stop (§6 "master graceful stop"): no job is
    /// assigned after this, and every channel is closed without `shutdown`,
    /// so remote agents keep running their jobs and redial. A closed channel
    /// only detaches its lease; the stopping runner then lets it go and the
    /// job's row stays `leased`/`running` (`run_remote`), for the next
    /// process to adopt and the agent to resume. `connect` refuses new
    /// channels meanwhile, so no agent resumes a lease here that this
    /// process is about to let go.
    pub fn detach_all(&self) {
        let mut inner = self.lock();
        inner.stopping = true;
        for conn in inner.conns.values() {
            let _ = conn.out.send(Outgoing::Close);
        }
        self.changed.notify_all();
    }

    /// Waits up to `grace` for every lease to end.
    fn wait_for_no_leases(&self, grace: Duration) {
        let deadline = Instant::now() + grace;
        while !self.lock().leases.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
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
        if lease.agent != Some(agent) || lease.epoch != epoch || lease.cancelled || lease.finished {
            return Err(StatusCode::FORBIDDEN);
        }
        act(lease)
    }

    /// Drops a reservation even if the lease became cancelled or finished
    /// while its body was streaming. A removed lease has already discarded
    /// the whole accounting record.
    fn release_upload_reservation(
        &self,
        agent: usize,
        job_id: Uuid,
        epoch: u64,
        reservation_id: Uuid,
    ) {
        let mut inner = self.lock();
        if let Some(lease) = inner.leases.get_mut(&job_id) {
            if lease.agent == Some(agent) && lease.epoch == epoch {
                lease.reservations.remove(&reservation_id);
            }
        }
    }

    /// A replaced blob was counted as orphaned while unlink ran without the
    /// hub lock. Forget that accounting entry once it is gone from disk.
    fn forget_orphaned_blob(&self, job_id: Uuid, epoch: u64, sha256: &str, bytes: u64) {
        let mut inner = self.lock();
        if let Some(lease) = inner.leases.get_mut(&job_id) {
            if lease.epoch == epoch && lease.orphaned_blobs.get(sha256) == Some(&bytes) {
                lease.orphaned_blobs.remove(sha256);
            }
        }
    }
}

/// Releases both the reservation and temporary file on every return path,
/// including a client disconnect that drops the handler future mid-stream.
struct UploadGuard<'a> {
    hub: &'a WorkerHub,
    agent: usize,
    job_id: Uuid,
    epoch: u64,
    reservation_id: Uuid,
    temp: PathBuf,
    active: bool,
}

impl Drop for UploadGuard<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.temp);
        if self.active {
            self.hub.release_upload_reservation(
                self.agent,
                self.job_id,
                self.epoch,
                self.reservation_id,
            );
        }
    }
}

/// The agent a waiting runner `me` may take now: free agents are handed to
/// waiting runners in admission order, each the lowest-numbered free agent
/// it may use, and `me` gets whatever is left for it. Lowest channel first
/// is the oldest connected agent, so the choice is deterministic for a
/// given sequence of connections.
fn pick_agent(inner: &HubInner, me: (i64, Uuid)) -> Option<u64> {
    let mut taken = BTreeSet::new();
    for (&waiter, &(local, class)) in &inner.waiting {
        // Trust decision (§3.3): a `local/<name>` job names a path on this
        // host, so it goes only to an agent that said it shares this
        // host's paths.
        let free = inner
            .conns
            .iter()
            .find(|(id, conn)| {
                !taken.contains(*id)
                    && conn.eligible
                    && conn.ready
                    && !conn.draining
                    && conn.holding.is_none()
                    && conn.class == Some(class)
                    && (!local || conn.local_paths)
            })
            .map(|(id, _)| *id);
        if waiter == me {
            return free;
        }
        if let Some(id) = free {
            taken.insert(id);
        }
    }
    None
}

// ---- the listener ----------------------------------------------------------

/// The worker listener's routes (§4.2, §5.6, §7.4): the channel, the
/// artifact URLs and the read-only desired-capacity route, nothing else.
/// Served on its own socket, never merged into `http::router`.
///
/// Two sub-routers, not one: the channel and artifact handlers only need
/// the hub, as before, but `GET /workers/capacity` also needs the job
/// registry's running and queued counts, which only `AppState` has --
/// `state.jobs.remote()` is the hub `set_remote` already gave it. Axum
/// resolves one state type per `Router`, so each half calls `with_state`
/// on its own state before the two are merged, rather than threading a
/// combined state type through handlers that do not need half of it.
pub fn router(state: Arc<AppState>) -> Router {
    let hub = state
        .jobs
        .remote()
        .expect("the worker listener starts only after JobRegistry::set_remote");
    let channel = Router::new()
        .route("/workers/connect", get(connect))
        .route(
            "/workers/artifacts/{job}/{epoch}/{*name}",
            get(get_artifact).put(put_artifact.layer(DefaultBodyLimit::max(
                WORKER_ARTIFACT_ROUTE_BODY_LIMIT as usize,
            ))),
        )
        .with_state(hub);
    let capacity = Router::new()
        .route("/workers/capacity", get(get_capacity))
        .with_state(state);
    channel.merge(capacity)
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
/// the request, and before the upgrade -- a missing, wrong or revoked token
/// gets a 401 and never a channel.
async fn connect(
    State(hub): State<Arc<WorkerHub>>,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let Some(identity) = hub.identify(&headers) else {
        return unauthorized();
    };
    // A stopping master takes no new channel (`detach_all`): an agent that
    // resumed a lease here would lose it when this process lets it go. A
    // remote agent reads the 503 as a failed dial and tries again, which
    // finds the next process.
    if hub.is_stopping() {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "the master is stopping; dial again",
        );
    }
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };
    let origin = hub.origin(&headers);
    upgrade
        .max_message_size(WS_MESSAGE_LIMIT)
        .max_frame_size(WS_MESSAGE_LIMIT)
        .on_upgrade(move |socket| serve_agent(hub, identity, origin, socket))
}

/// §7.4: the default per-class maximum a provider-specific autoscaler
/// should ever bring up -- one worker, the decided phase 3 fleet (§10.1).
/// `TOLMAP_WORKER_MAX_PER_CLASS` overrides it uniformly for every class;
/// the design decided one number for the one class there is, not a
/// per-class table, so that is what this reads.
pub const DEFAULT_WORKER_MAX_PER_CLASS: usize = 1;

/// §7.4: how long a class may sit idle (nothing running or queued) before
/// `GET /workers/capacity` reports 0 for it, rather than holding the
/// clamp it last computed. Long enough that back-to-back small jobs do not
/// each pay a worker's cold start. `TOLMAP_WORKER_IDLE_S` overrides it.
pub const DEFAULT_WORKER_IDLE_S: u64 = 600;

/// A whole number, or `default` when `key` is unset or not one -- like
/// `positive_env`, but zero is a valid value (`TOLMAP_WORKER_MAX_PER_CLASS
/// =0` disables a class's autoscaler on purpose, unlike a heartbeat or
/// lease interval, which zero would only break).
fn usize_env(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

/// §7.4's clamp-and-idle-drop arithmetic, factored out so the unit tests
/// below exercise exactly what `get_capacity` computes, without a real
/// clock or a live hub. `running` and `queued` are one class's counts;
/// `idle_for` is `None` while the class is busy (`running + queued > 0`)
/// or has never been busy, `Some(elapsed)` for how long it has sat idle
/// otherwise (`JobRegistry::class_load`).
///
/// Busy: `running + queued`, clamped to `max`. Idle: `max.min(1)` -- "keep
/// the one worker" -- for as long as `idle_for` stays under `idle_after`,
/// then 0. A class that has never been busy is never held: `idle_for` is
/// `None` there too, which this treats the same as "idle past the
/// window" (§7.4 "started when a job is queued" -- nothing starts one
/// before the first job arrives).
fn desired_capacity(
    running: usize,
    queued: usize,
    max: usize,
    idle_for: Option<Duration>,
    idle_after: Duration,
) -> usize {
    let busy = (running + queued).min(max);
    if busy > 0 {
        return busy;
    }
    match idle_for {
        Some(elapsed) if elapsed < idle_after => max.min(1),
        _ => 0,
    }
}

#[derive(Serialize)]
struct ClassCapacity {
    class: usize,
    usable_memory_bytes: Option<u64>,
    running: usize,
    queued: usize,
    connected_agents: usize,
    desired: usize,
}

#[derive(Serialize)]
struct CapacityResponse {
    classes: Vec<ClassCapacity>,
}

/// `GET /workers/capacity` (§7.4): on the worker listener only, never the
/// public router (docs/API.md marks it as such). Answers what a
/// provider-specific starter in the private hosting repository should have
/// running, per configured class: running and queued jobs
/// (`JobRegistry::class_load`), connected agents (`WorkerHub::
/// live_by_class`), and `desired` (`desired_capacity`). Same bearer check
/// as `/workers/connect` -- any configured worker token, not only the one
/// for a connected agent.
async fn get_capacity(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(hub) = state.jobs.remote() else {
        return refuse(StatusCode::NOT_FOUND, "no worker hub is running");
    };
    if hub.authenticate(&headers).is_none() {
        return unauthorized();
    }
    let max = usize_env("TOLMAP_WORKER_MAX_PER_CLASS", DEFAULT_WORKER_MAX_PER_CLASS);
    let idle_after =
        Duration::from_secs(positive_env("TOLMAP_WORKER_IDLE_S", DEFAULT_WORKER_IDLE_S));
    let live = hub.live_by_class();
    let memory = hub.usable_memory_by_class();
    let load = state.jobs.class_load(memory.len());
    let classes = load
        .into_iter()
        .enumerate()
        .map(|(class, load)| ClassCapacity {
            class,
            usable_memory_bytes: memory[class],
            running: load.running,
            queued: load.queued,
            connected_agents: live.get(class).copied().unwrap_or(0),
            desired: desired_capacity(load.running, load.queued, max, load.idle_for, idle_after),
        })
        .collect();
    Json(CapacityResponse { classes }).into_response()
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

async fn serve_agent(
    hub: Arc<WorkerHub>,
    identity: Identity,
    origin: String,
    mut socket: WebSocket,
) {
    let agent = identity.agent;
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
    // Trust decision (§5.1): in remote mode a token is bound to one worker
    // id, and an agent names that id in its `hello` or gets no channel. The
    // id is what the store records as a lease's holder and what a restarted
    // master hands an adopted lease back to, so one worker's token must not
    // pass for another worker.
    if let Some(bound) = &identity.worker_id {
        if worker_id != *bound {
            close_with_error(
                &mut socket,
                "worker_id_mismatch",
                format!("this token is worker {bound:?}'s, and the hello names {worker_id:?}"),
            )
            .await;
            return;
        }
    }
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
    let same = same_build(&build, &hub.build);
    if !same {
        eprintln!(
            "worker agent {agent} ({worker_id}): its build {} {} differs from this master's {} {}; \
             it stays connected but is never assigned work",
            build.version, build.commit, hub.build.version, hub.build.commit
        );
    }
    if slots != 1 {
        eprintln!("worker agent {agent} ({worker_id}): advertised {slots} slots; phase 1 runs one job per agent");
    }
    // #97 phase 2, step 4: its advertised memory places it in a class. One
    // smaller than every class could be sent a job it cannot hold, so, like
    // another build, it stays connected and idle, and this says why.
    let worker_class = hub.class_of(class.memory_bytes);
    if worker_class.is_none() {
        eprintln!(
            "worker agent {agent} ({worker_id}): it advertises {} MiB usable, less than every \
             worker class; it stays connected but is never assigned work",
            class.memory_bytes / (1024 * 1024)
        );
    }
    let eligible = same && worker_class.is_some();
    let local_paths = features
        .iter()
        .any(|feature| feature == FEATURE_LOCAL_PATHS);
    // §3.6: what is gated on a feature comes only from a peer that
    // advertised it. `hello.resume` from an agent that never said it can
    // resume would have it replay events under rules it did not agree to.
    if !resume.is_empty() && !features.iter().any(|feature| feature == FEATURE_RESUME) {
        close_with_error(
            &mut socket,
            "protocol_error",
            format!("hello.resume from an agent that did not advertise {FEATURE_RESUME:?}"),
        )
        .await;
        return;
    }
    let (out, mut outgoing) = tokio::sync::mpsc::unbounded_channel();
    let conn = hub.add_conn(
        agent,
        worker_id.clone(),
        eligible,
        worker_class,
        local_paths,
        origin,
        out,
    );
    eprintln!(
        "worker agent {agent} ({worker_id}) connected: {} MiB usable (worker class {}), {} CPUs, \
         proto {proto}",
        class.memory_bytes / (1024 * 1024),
        worker_class.map_or("none".to_owned(), |class| class.to_string()),
        class.cpus
    );
    // Answered before `welcome` goes out; a verdict `resume` sends again
    // waits in `outgoing`, which the loop below drains only after
    // `welcome`, so the agent reads its answers first.
    let welcome = MasterMessage::Welcome {
        proto,
        heartbeat_s: hub.heartbeat_s,
        lease_ttl_s: hub.lease_ttl_s(),
        resume: hub.resume(conn, agent, resume),
    };
    // Off the hub's lock: a new agent may let a queued job start.
    hub.agents_changed();
    // §4.3: every frame after the `hello` spends from this channel's budget.
    let mut budget = FrameBudget::new(hub.frame_rate, hub.frame_burst);
    if send_message(&mut socket, &welcome).await.is_ok() {
        loop {
            tokio::select! {
                incoming = socket.recv() => {
                    if matches!(incoming, Some(Ok(_))) && !budget.take() {
                        close_with_error(
                            &mut socket,
                            "rate_limited",
                            format!(
                                "more than {} control frames a second (bursts of {}); the \
                                 protocol never needs that many",
                                hub.frame_rate, hub.frame_burst
                            ),
                        )
                        .await;
                        break;
                    }
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
                    let handled = parse_frame(text.as_str()).and_then(|message| {
                        // §5.1: a worker whose token line was deleted while
                        // its channel was up loses the channel at its next
                        // heartbeat (and its next dial gets a 401). Checked
                        // here, off the hub's lock, since it may read the
                        // token file.
                        if matches!(message, WorkerMessage::Heartbeat { .. })
                            && !hub.still_authorized(&identity)
                        {
                            return Err(Violation {
                                code: "token_revoked",
                                message: "this worker's token is no longer in the master's token \
                                          file"
                                    .to_owned(),
                            });
                        }
                        hub.handle(conn, message)
                    });
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
    hub.agents_changed();
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
    mut file: std::fs::File,
    mut chunks: tokio::sync::mpsc::Receiver<Bytes>,
) -> std::io::Result<(String, u64)> {
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

fn create_upload_temp(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// `PUT` of one result artifact (§4.2): streamed to a temporary file in the
/// lease's master-owned directory while hashed, never buffered whole, then
/// kept under its digest only if size and digest match the declaration.
async fn put_artifact(
    State(hub): State<Arc<WorkerHub>>,
    AxPath((job, epoch, name)): AxPath<(String, u64, String)>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    // `Body` extraction itself is intentionally unbounded for streaming
    // handlers. Apply the route's DefaultBodyLimit explicitly so chunked and
    // unknown-size streams are still capped while they are consumed.
    let body = request.into_limited_body();
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
    let kind_limit = match name.as_str() {
        "map" => worker_result::EARLY_MAP_MAX_BYTES,
        "symbols" => worker_result::FULL_SYMBOLS_MAX_BYTES,
        "names" => worker_result::NAMES_CACHE_MAX_BYTES,
        _ => worker_result::DISTRICT_SYMBOLS_MAX_BYTES,
    };
    if length > kind_limit {
        return refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("artifact {name:?} exceeds its {kind_limit}-byte per-artifact limit"),
        );
    }

    let reservation_id = Uuid::new_v4();
    let reservation = hub.with_lease(agent, job_id, epoch, |lease| {
        let used_bytes = lease_blob_bytes(lease);
        let reserved_bytes = lease
            .reservations
            .values()
            .try_fold(0u64, |total, item| total.checked_add(item.bytes));
        let fits_bytes = used_bytes
            .and_then(|used| reserved_bytes.and_then(|reserved| used.checked_add(reserved)))
            .and_then(|reserved| reserved.checked_add(length))
            .is_some_and(|total| total <= WORKER_LEASE_MAX_ARTIFACT_BYTES);
        if !fits_bytes {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }

        let pending_names = lease
            .reservations
            .values()
            .filter(|item| !lease.uploads.contains_key(&item.name))
            .map(|item| item.name.as_str())
            .collect::<BTreeSet<_>>();
        let current_count = lease.uploads.len() + pending_names.len();
        let name_already_counted = lease.uploads.contains_key(&name)
            || lease.reservations.values().any(|item| item.name == name);
        if !name_already_counted && current_count >= WORKER_LEASE_MAX_ARTIFACTS {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }

        lease.reservations.insert(
            reservation_id,
            UploadReservation {
                name: name.clone(),
                bytes: length,
            },
        );
        Ok((lease.dir.clone(), lease.blob_ops.clone()))
    });
    let (dir, blob_ops) = match reservation {
        Ok(reservation) => reservation,
        Err(status) => {
            let message = if status == StatusCode::PAYLOAD_TOO_LARGE {
                "the lease's artifact byte or count limit would be exceeded"
            } else {
                "this token holds no live lease on that job and epoch"
            };
            return refuse(status, message);
        }
    };
    let temp = dir.join(format!("upload-{reservation_id}"));
    let mut guard = UploadGuard {
        hub: &hub,
        agent,
        job_id,
        epoch,
        reservation_id,
        temp: temp.clone(),
        active: true,
    };
    let file = match create_upload_temp(&temp) {
        Ok(file) => file,
        Err(error) => {
            return refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not create the upload temporary file: {error}"),
            )
        }
    };
    let (chunks, receiver) = tokio::sync::mpsc::channel::<Bytes>(8);
    let writer = tokio::task::spawn_blocking(move || write_hashed(file, receiver));
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
    let bad = |status: StatusCode, message: String| refuse(status, message);
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
    // Bodies are complete now. Serialize only this short blob install and
    // replacement phase; the hub lock below covers accounting only, never
    // this stream or filesystem work.
    let _blob_ops = blob_ops
        .lock()
        .expect("lease blob-operation mutex poisoned");
    let blob = dir.join("blobs").join(&sha256);
    let blob_existed = blob.exists();
    if blob_existed {
        if let Err(error) = std::fs::remove_file(&temp) {
            return bad(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not discard the duplicate upload: {error}"),
            );
        }
    } else if let Err(error) = std::fs::rename(&temp, &blob) {
        return bad(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not keep the upload: {error}"),
        );
    }
    let upload = Upload {
        sha256: sha256.clone(),
        bytes,
    };
    let recorded = hub.with_lease(agent, job_id, epoch, |lease| {
        let Some(reservation) = lease.reservations.get(&reservation_id) else {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        };
        if reservation.name != name || reservation.bytes != bytes {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }

        let old = lease.uploads.insert(name.clone(), upload);
        lease.reservations.remove(&reservation_id);
        lease.orphaned_blobs.remove(&sha256);
        let old_blob = old.filter(|previous| {
            previous.sha256 != sha256
                && !lease
                    .uploads
                    .values()
                    .any(|upload| upload.sha256 == previous.sha256)
        });
        if let Some(previous) = &old_blob {
            lease
                .orphaned_blobs
                .insert(previous.sha256.clone(), previous.bytes);
        }
        Ok(old_blob)
    });
    match recorded {
        Ok(old_blob) => {
            if let Some(old_blob) = old_blob {
                let old_path = dir.join("blobs").join(&old_blob.sha256);
                match std::fs::remove_file(&old_path) {
                    Ok(()) => {
                        hub.forget_orphaned_blob(job_id, epoch, &old_blob.sha256, old_blob.bytes)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        hub.forget_orphaned_blob(job_id, epoch, &old_blob.sha256, old_blob.bytes)
                    }
                    Err(error) => eprintln!(
                        "job {job_id}: could not remove replaced artifact blob {}: {error}",
                        old_blob.sha256
                    ),
                }
            }
            guard.active = false;
            StatusCode::OK.into_response()
        }
        Err(status) => {
            if !blob_existed {
                let _ = std::fs::remove_file(&blob);
            }
            refuse(status, "the lease ended during the upload")
        }
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
///
/// Durable (phase 2): the job's row moves `queued` → `leased` (written
/// right after `assign`) → `running` (first event), with the snapshot
/// written at every stage boundary and at most every `SNAPSHOT_EVERY`
/// between. A lease that runs out re-queues the job (`requeue`), counted
/// as a lost worker unless this run adopted a lease a restart left behind.
/// Terminal rows are written by `worker_loop` once this returns.
///
/// Classes (phase 2, step 4): the runner asks for an agent of its slot's
/// class, which is the class the job runs on. When the job's `features`
/// predict more than that class holds and a larger class exists, the agent
/// is told `cancel` `reroute` and, on its `released`, the job moves there
/// uncounted (§2.1 step 3). A `released` `oom` moves it to the next class
/// up, uncounted, or fails it on the largest (§6). A lease that runs out
/// with its last heartbeat within 10% of the class's memory moves the job
/// to the next class, counted as the lost worker it is (§6).
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
    let store = &state.store;
    // The commit the job was admitted against, which `jobs::prepare` puts
    // in the `JobSpec`: a result is checked against it (`check_result`).
    // Read from the snapshot here, so an adopted lease, which skips
    // `prepare`, checks against the same value.
    let admitted_commit = tx.borrow().commit.clone().unwrap_or_default();
    // The class of the agent this job runs on: its slot's, which is larger
    // than the job's own when the slot spilled down to it (§7.1). Rerouting
    // and escalation compare against this, not the job's class: a small job
    // killed for memory on a large agent needs a class larger than that.
    let running_class = registry.slot_class(job_id).unwrap_or(0);
    // A lease a restarted master found in the store (§6), adopted by
    // `jobs::restore` before any agent could connect: not asked for again.
    // Its expiry is a restart, not a lost worker.
    let orphan = registry.take_orphan(job_id);
    let adopted = orphan.is_some();
    let (lease, epoch, mut table) = match orphan {
        Some((epoch, lease)) => (lease, epoch, "running"),
        None => {
            let (job, inputs) = match jobs::prepare(&state, &repo_ref, &tx) {
                Ok(prepared) => prepared,
                Err(error) => return jobs::finish_failed_durable(&state, &tx, error),
            };
            // Invariant (epoch fencing): the epoch is raised in the store
            // before `assign` carries it, so it is never handed out twice.
            // `None`: the row is terminal, so a cancel won.
            let (epoch, order) = match store.begin_attempt(&job_id.to_string()) {
                Ok(Some(attempt)) => attempt,
                Ok(None) => return,
                Err(error) => {
                    return jobs::finish_failed_durable(
                        &state,
                        &tx,
                        ApiError::internal(format!("could not record the attempt: {error:#}")).body,
                    )
                }
            };
            match hub.claim(job_id, epoch, order, running_class, &job, &inputs, &|| {
                registry.is_cancelled(job_id)
            }) {
                Ok(Some(lease)) => {
                    save_state(
                        store,
                        &tx,
                        epoch,
                        "leased",
                        Some((lease.holder.as_str(), hub.lease_ttl)),
                    );
                    (lease, epoch, "leased")
                }
                // Cancelled before any agent took it (the registry has
                // failed the job), or the master is stopping (the row stays
                // `queued` for the next process).
                Ok(None) => return,
                Err(error) => return jobs::finish_failed_durable(&state, &tx, error),
            }
        }
    };
    let mut sink = SnapshotSink::new(&tx, started, Some(registry));
    // An adopted lease that its agent resumes (remote mode) carries on from
    // the snapshot the store kept, which may be past the executor's clone
    // already; the child's own `clone` stage that follows must not be taken
    // for the executor's.
    let mut clone = if adopted {
        executor_clone_of(&tx.borrow())
    } else {
        ExecutorClone::NotStarted
    };
    let mut cancel_sent = false;
    let mut early_map_attempted = false;
    let mut repeated_write_map_logged = false;
    // Set once `cancel` `reroute` is sent: the class the job moves to.
    // Asked at most once per lease, whatever further `features` say.
    let mut reroute_to: Option<usize> = None;
    let mut saved = SavedSnapshot::of(&tx.borrow(), table);
    loop {
        match lease.events.recv_timeout(POLL) {
            Ok(LeaseEvent::Event {
                event,
                peak_rss_bytes,
            }) => {
                if let Some(peak) = peak_rss_bytes {
                    sink.peak_rss(peak);
                }
                // `leased` → `running` on the first event (§2.2).
                table = "running";
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
                        // An adopted lease starts with no uploads: they went
                        // to the previous process's staging, which this one
                        // cleared. A result whose artifacts were uploaded
                        // before the restart is refused and the job runs
                        // again, uncounted, rather than failing for the
                        // restart.
                        if adopted && !uploads_cover(&event, &hub.uploads(job_id)) {
                            hub.verdict(
                                job_id,
                                false,
                                "its artifacts were uploaded before the master restarted"
                                    .to_owned(),
                            );
                            return requeue(
                                &state,
                                hub,
                                &tx,
                                job_id,
                                epoch,
                                false,
                                jobs::RESTARTED,
                                false,
                                None,
                            );
                        }
                        let registered = register_result(
                            &state,
                            hub,
                            &repo_ref,
                            &tx,
                            started,
                            job_id,
                            &lease.dir,
                            &admitted_commit,
                            &event,
                        );
                        match registered {
                            Ok(()) => hub.verdict(job_id, true, "registered".to_owned()),
                            Err(error) => hub.verdict(job_id, false, error.message),
                        }
                        return hub.end_lease(job_id, false);
                    }
                    WorkerEvent::Error { code, message, .. } => {
                        jobs::finish_failed_durable(
                            &state,
                            &tx,
                            ErrorBody {
                                error: code,
                                message,
                            },
                        );
                        return hub.end_lease(job_id, false);
                    }
                    // §2.1 step 3: the files the job holds are known now, so
                    // the prediction is better than the one it was bound
                    // with. The sink records them first; the registry then
                    // says whether they outgrow the class this runs on.
                    event @ WorkerEvent::Features { .. } => {
                        sink.event(event);
                        if reroute_to.is_none() && !cancel_sent {
                            if let Some((target, peak)) =
                                registry.reroute_target(job_id, running_class)
                            {
                                eprintln!(
                                    "job {job_id}: its features predict a peak of {} MiB, more \
                                     than worker class {running_class} holds; rerouting it to \
                                     class {target}",
                                    peak / (1024 * 1024)
                                );
                                hub.reroute(job_id);
                                reroute_to = Some(target);
                            }
                        }
                    }
                    // docs/UX.md §12: the agent uploads the map as its `map`
                    // artifact before it forwards this event
                    // (`agent::JobContext::upload_early_map`), so a map that
                    // made it is in this lease's uploads by now.
                    event @ WorkerEvent::StageFinished {
                        stage: StageId::WriteMap,
                        success: true,
                        ..
                    } => {
                        sink.event(event);
                        if !early_map_attempted {
                            early_map_attempted = true;
                            publish_uploaded_map(registry, hub, &tx, job_id, &lease.dir);
                        } else if !repeated_write_map_logged {
                            eprintln!("job {job_id}: ignoring repeated successful write_map event for the early map");
                            repeated_write_map_logged = true;
                        }
                    }
                    event => sink.event(event),
                }
                saved.save_if_due(store, &tx, epoch, table);
            }
            // A heartbeat keeps the elapsed time moving through a long quiet
            // stage, as `install_tick` does for local mode's installs.
            Ok(LeaseEvent::Tick) => {
                sink.install_tick();
                saved.save_if_due(store, &tx, epoch, table);
            }
            Ok(LeaseEvent::Released {
                reason,
                peak_rss_bytes,
            }) => {
                if let Some(peak) = peak_rss_bytes {
                    sink.peak_rss(peak);
                }
                if cancel_sent {
                    return hub.end_lease(job_id, false);
                }
                // §2.1 step 3: stopped for the class the features call for,
                // and moved there uncounted: a reroute is not a lost worker
                // (§10.6).
                if let Some(target) = reroute_to.filter(|_| reason == ReleasedReason::Reroute) {
                    return requeue(
                        &state,
                        hub,
                        &tx,
                        job_id,
                        epoch,
                        false,
                        REROUTED,
                        false,
                        Some(target),
                    );
                }
                // §6 "job child OOM-killed": the peak is recorded so the
                // memory model learns, then the job moves to the next class
                // up, uncounted -- or fails, on the largest.
                if reason == ReleasedReason::Oom {
                    jobs::record_oom(&state, job_id, epoch, peak_rss_bytes);
                    return match registry.next_class(running_class) {
                        Some(target) => requeue(
                            &state,
                            hub,
                            &tx,
                            job_id,
                            epoch,
                            false,
                            OUT_OF_MEMORY,
                            false,
                            Some(target),
                        ),
                        None => {
                            eprintln!("job {job_id}: {OOM_ON_LARGEST}");
                            jobs::finish_failed_durable(
                                &state,
                                &tx,
                                worker_crashed(OOM_ON_LARGEST),
                            );
                            hub.end_lease(job_id, false)
                        }
                    };
                }
                // §6, §10.6: a job released for a graceful stop (of the
                // master, answering `shutdown now`, or of the worker) or a
                // reroute goes back to its queue without counting.
                let why = match reason {
                    ReleasedReason::ServerStopping => Some(STOPPED),
                    ReleasedReason::WorkerStopping | ReleasedReason::Reroute => {
                        Some(WORKER_STOPPED)
                    }
                    ReleasedReason::Cancelled | ReleasedReason::LeaseLost | ReleasedReason::Oom => {
                        None
                    }
                };
                match why {
                    Some(why) => {
                        return requeue(&state, hub, &tx, job_id, epoch, false, why, false, None)
                    }
                    None => {
                        jobs::finish_failed_durable(
                            &state,
                            &tx,
                            worker_crashed(format!(
                                "the worker released the job without being asked ({reason:?})"
                            )),
                        );
                        return hub.end_lease(job_id, false);
                    }
                }
            }
            Err(std_mpsc::RecvTimeoutError::Timeout) => {}
            Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                jobs::finish_failed_durable(&state, &tx, worker_crashed("worker lost"));
                return hub.end_lease(job_id, false);
            }
        }
        // The registry already made the job terminal (§2.4); the agent is
        // told, and this slot stays busy until it has released the job.
        if !cancel_sent && registry.is_cancelled(job_id) {
            hub.cancel(job_id, CancelReason::Cancelled);
            cancel_sent = true;
        }
        // A graceful stop whose agent went without releasing the job (it
        // was reaped): the row stays `leased`/`running`, and the next
        // process re-queues it without counting (`jobs::restore`). Waiting
        // for the lease to run out would only hold the process's exit.
        if registry.is_stopping() && hub.detached(job_id) {
            return hub.end_lease(job_id, false);
        }
        if hub.expired(job_id) {
            if cancel_sent || registry.is_stopping() {
                return hub.end_lease(job_id, true);
            }
            let (counted, why) = if adopted {
                (false, jobs::RESTARTED)
            } else {
                (true, WORKER_LOST)
            };
            // §6 "worker host dies": a worker whose last heartbeat had its
            // job within 10% of the class's memory most likely died of it,
            // so the retry goes to the next class rather than this one again.
            // A reroute already asked for still happens: the job goes to the
            // larger of the two.
            let near_limit = counted
                && hub
                    .last_rss(job_id)
                    .zip(registry.class_memory(running_class))
                    .is_some_and(|(rss, usable)| {
                        rss.saturating_mul(10) >= usable.saturating_mul(9)
                    });
            let to_memory = near_limit
                .then(|| registry.next_class(running_class))
                .flatten();
            let to_class = to_memory.max(reroute_to);
            let why = if to_memory.is_some() {
                LOST_TO_MEMORY
            } else {
                why
            };
            return requeue(
                &state, hub, &tx, job_id, epoch, counted, why, true, to_class,
            );
        }
    }
}

/// Where a restored snapshot has the executor's clone: the state `run_remote`
/// starts from for a lease it adopted.
fn executor_clone_of(snapshot: &JobSnapshot) -> ExecutorClone {
    let state = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == StageId::Clone)
        .map(|stage| stage.state);
    match state {
        Some(jobs::StageState::Running) => ExecutorClone::Running,
        Some(jobs::StageState::Done | jobs::StageState::Failed) => ExecutorClone::Finished,
        Some(jobs::StageState::Pending) | None => ExecutorClone::NotStarted,
    }
}

/// Whether every artifact a `result` lists was uploaded under this lease
/// with the SHA-256 and size it lists.
fn uploads_cover(event: &WorkerEvent, uploads: &BTreeMap<String, Upload>) -> bool {
    result_artifacts(event).iter().all(|artifact| {
        uploads.get(&artifact.name)
            == Some(&Upload {
                sha256: artifact.sha256.clone(),
                bytes: artifact.bytes,
            })
    })
}

/// Puts a job whose lease ended without a result back at the head of its
/// class queue (§2.2, §2.5 "lease expired first"), or, with `to_class`,
/// of that class's queue (phase 2, step 4: a reroute, an out-of-memory
/// escalation, a lost worker that died of memory). The store decides first,
/// in one transaction: a `counted` re-queue is a lost worker, and past the
/// retry bound on the job's class (§10.6) the job fails `worker_crashed`
/// instead; a move to another class starts that count again and is never
/// refused. Memory follows the store: the snapshot is reset and
/// `worker_loop` re-inserts the job, by its admission order, when this
/// runner returns. The next `assign` raises the epoch, so this epoch's
/// stragglers are refused.
#[allow(clippy::too_many_arguments)]
fn requeue(
    state: &AppState,
    hub: &WorkerHub,
    tx: &watch::Sender<JobSnapshot>,
    job_id: Uuid,
    epoch: u64,
    counted: bool,
    why: &str,
    close_channel: bool,
    to_class: Option<usize>,
) {
    hub.end_lease(job_id, close_channel);
    let reset = jobs::requeued_snapshot(&tx.borrow(), why);
    let json = serde_json::to_string(&reset).expect("a JobSnapshot serializes");
    let id = job_id.to_string();
    let decided = match to_class {
        Some(class) => state.store.rebind_job(&id, epoch, class, counted, &json),
        None => state
            .store
            .requeue_job(&id, epoch, counted, hub.retries, &json),
    };
    match decided {
        Ok(Requeued::Queued { attempt }) => {
            match to_class {
                Some(class) => eprintln!(
                    "job {job_id}: {why} (epoch {epoch} ended; attempt {attempt}; now worker \
                     class {class})"
                ),
                None => eprintln!("job {job_id}: {why} (epoch {epoch} ended; attempt {attempt})"),
            }
            // Never over a terminal snapshot: a cancel that landed since
            // wins, and `worker_loop` then ends the job instead.
            tx.send_modify(|snapshot| {
                if !jobs::is_terminal(snapshot) {
                    *snapshot = reset.clone();
                }
            });
            state.jobs.mark_requeue(job_id, to_class);
        }
        Ok(Requeued::Exhausted { lost }) => {
            eprintln!("job {job_id}: lost {lost} workers; not retrying it again");
            jobs::finish_failed_durable(
                state,
                tx,
                worker_crashed(format!(
                    "lost {lost} workers: the job's worker was lost {lost} times, past the \
                     bound of {} retries",
                    hub.retries
                )),
            );
        }
        // Terminal in the table already: a cancel won.
        Ok(Requeued::Unchanged) => {}
        Err(error) => jobs::finish_failed_durable(
            state,
            tx,
            ApiError::internal(format!("could not re-queue the job: {error:#}")).body,
        ),
    }
}

/// Writes a live job's state and snapshot (`leased` or `running`), with its
/// lease holder and deadline when it has just been leased. A failed write
/// is logged: the epoch is already safe in the store (`begin_attempt`), and
/// the next write or a restart catches up.
fn save_state(
    store: &crate::service::store::Store,
    tx: &watch::Sender<JobSnapshot>,
    epoch: u64,
    status: &str,
    lease: Option<(&str, Duration)>,
) {
    let snapshot = tx.borrow().clone();
    if jobs::is_terminal(&snapshot) {
        return;
    }
    let deadline_ms = lease.map(|(_, ttl)| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        (now + ttl).as_millis() as i64
    });
    let json = serde_json::to_string(&snapshot).expect("a JobSnapshot serializes");
    if let Err(error) = store.save_job_state(
        &snapshot.job_id.to_string(),
        epoch,
        status,
        lease.map(|(holder, _)| holder),
        deadline_ms,
        &json,
    ) {
        eprintln!(
            "job {}: could not record its progress in the store: {error:#}",
            snapshot.job_id
        );
    }
}

/// When a running job's snapshot was last written, and what it showed: it
/// is written again at once when the status, a stage's state or the row's
/// status changes (§3.5 "at stage boundaries"), and otherwise at most every
/// `SNAPSHOT_EVERY` -- never on every progress event.
struct SavedSnapshot {
    at: Instant,
    shape: (JobStatus, Vec<jobs::StageState>, &'static str),
}

impl SavedSnapshot {
    fn of(snapshot: &JobSnapshot, table: &'static str) -> Self {
        SavedSnapshot {
            at: Instant::now(),
            shape: Self::shape(snapshot, table),
        }
    }

    fn shape(
        snapshot: &JobSnapshot,
        table: &'static str,
    ) -> (JobStatus, Vec<jobs::StageState>, &'static str) {
        (
            snapshot.status,
            snapshot.stages.iter().map(|stage| stage.state).collect(),
            table,
        )
    }

    fn save_if_due(
        &mut self,
        store: &crate::service::store::Store,
        tx: &watch::Sender<JobSnapshot>,
        epoch: u64,
        table: &'static str,
    ) {
        let shape = Self::shape(&tx.borrow(), table);
        if shape == self.shape && self.at.elapsed() < SNAPSHOT_EVERY {
            return;
        }
        save_state(store, tx, epoch, table, None);
        self.at = Instant::now();
        self.shape = shape;
    }
}

/// Publishes the `map` artifact an agent uploaded when its job's `write_map`
/// stage finished as the job's early map (docs/UX.md §12). Best effort, like
/// the upload: no upload, or one that is not a map document, leaves the job
/// to open its map when it is done. The blob was stored under the digest the
/// master computed itself while receiving it (`put_artifact`), so it is
/// exactly what the agent sent; `check_map_bytes` is the same check local
/// mode makes before publishing.
fn publish_uploaded_map(
    registry: &jobs::JobRegistry,
    hub: &WorkerHub,
    tx: &watch::Sender<JobSnapshot>,
    job_id: Uuid,
    dir: &Path,
) {
    let Some(upload) = hub.uploads(job_id).remove("map") else {
        eprintln!("job {job_id}: no uploaded map to open early");
        return;
    };
    if upload.bytes > worker_result::EARLY_MAP_MAX_BYTES {
        eprintln!(
            "job {job_id}: the uploaded map is not opened early: {} bytes exceeds the 256 MiB cap",
            upload.bytes
        );
        return;
    }
    let path = dir.join("blobs").join(&upload.sha256);
    let mut map = Vec::new();
    let read = std::fs::File::open(&path).and_then(|file| {
        file.take(worker_result::EARLY_MAP_MAX_BYTES + 1)
            .read_to_end(&mut map)
    });
    if let Err(error) = read {
        eprintln!("job {job_id}: the uploaded map is not opened early: {error}");
        return;
    }
    if map.len() as u64 > worker_result::EARLY_MAP_MAX_BYTES {
        eprintln!(
            "job {job_id}: the uploaded map is not opened early: its size on disk exceeds the 256 MiB cap"
        );
        return;
    }
    if let Err(refused) = worker_result::check_map_bytes(&map) {
        eprintln!("job {job_id}: the uploaded map is not opened early: {refused}");
        return;
    }
    registry.publish_early_map(tx, axum::body::Bytes::from(map));
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
    admitted_commit: &str,
    event: &WorkerEvent,
) -> Result<(), ErrorBody> {
    // A cancel that landed between the result and here still wins (§2.4).
    if state.jobs.is_cancelled(job_id) {
        return Err(executor::cancelled_error());
    }
    let checked = check_result(event, admitted_commit, &hub.uploads(job_id))
        .and_then(|checked| assemble(dir, &repo_ref.repo, checked));
    let executed = match checked {
        Ok(executed) => executed,
        Err(error) => {
            jobs::finish_failed_durable(state, tx, error.clone());
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
///
/// **The commit is checked against the admission, not the agent's say-so**
/// (docs/WORKER_TIER.md §5.4, #97 phase 2): `admitted_commit` is the
/// `JobSpec.commit` this job was assigned with, from the master's own
/// `jobs::prepare`, never the event's own `commit` field. The agent's
/// executor already pins its checkout to that same commit and checks the
/// child's report against it before uploading
/// (`agent::JobContext::upload`), but a compromised or buggy agent (§5.3)
/// controls what it puts in the `result` event it sends over the channel,
/// so the master repeats the check independently against the one value it
/// trusts -- what it assigned -- exactly as local mode's `jobs::register`
/// does against its own pinned checkout.
pub(crate) fn check_result(
    event: &WorkerEvent,
    admitted_commit: &str,
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
    if !worker_result::is_object_id(commit) || commit != admitted_commit {
        return Err(invalid_result(
            "the reported commit is not the commit the job was admitted against",
        ));
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
    /// `JobRegistry::shutdown` has stopped admission (failing nothing: jobs
    /// are durable in this mode): `shutdown now` to every agent, whose
    /// release puts each running job back to `queued` in the store without
    /// counting an attempt (`run_remote`), then reap them. Queued jobs stay
    /// queued for the next process.
    pub async fn shutdown(self) {
        self.supervisor.stop();
        self.hub.shutdown_now();
        let supervisor = self.supervisor.clone();
        let _ = tokio::task::spawn_blocking(move || supervisor.reap(REAP_GRACE)).await;
    }
}

/// Removes whatever a previous process left at `path` and makes it anew,
/// private. Tokens and uploads do not survive a restart -- the agents that
/// held them are gone, and a job whose lease survives re-runs from scratch
/// (`jobs::restore`) -- so nothing there is anyone's any more.
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
    let classes = loopback_classes(
        std::env::var("TOLMAP_LOOPBACK_CLASSES").ok().as_deref(),
        agents,
    )
    .map_err(anyhow::Error::msg)?;
    // Agent `i` advertises the class of slot `i`: both are numbered
    // smallest class first (`schedule::worker_classes`). `None`, plain
    // `loopback:N`, leaves each agent advertising its host.
    let agent_memory: Vec<Option<u64>> = schedule::worker_classes(&classes)
        .into_iter()
        .map(|class| classes[class].usable_memory)
        .collect();
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
    let (frame_rate, frame_burst) = frame_limit_from_env();
    let hub = Arc::new(
        WorkerHub::new(
            digests,
            build,
            settings.heartbeat_s,
            settings.lease_ttl,
            settings.retries,
            staging,
            format!("http://{address}"),
            classes.iter().map(|class| class.usable_memory).collect(),
        )
        .with_frame_limit(frame_rate, frame_burst),
    );
    // Weak: the registry holds the hub, so a strong reference back would
    // keep both alive forever. Set before the listener serves anyone.
    let weak = Arc::downgrade(state);
    hub.on_agents_changed(Box::new(move || {
        if let Some(state) = weak.upgrade() {
            jobs::agents_changed(&state);
        }
    }));
    state.jobs.set_remote(hub.clone(), classes.clone());
    // Restart order (#97 phase 2): the store's live jobs are reloaded
    // before the worker listener serves an agent and before `serve` opens
    // the public listener, so orphaned leases hold their slots before any
    // new admission and queued jobs keep their admission order.
    jobs::restore(state).context("reload jobs from the store")?;
    let app = router(state.clone());
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            eprintln!("tolmap serve: the worker listener stopped: {error}");
        }
    });
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
            // This master's own child: it exits when this master is gone,
            // rather than redialling a successor that minted new tokens.
            .arg("--loopback")
            .stdin(Stdio::null());
        if let Some(memory) = agent_memory[agent] {
            command.arg("--class-memory").arg(memory.to_string());
        }
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
    let described: Vec<String> = classes
        .iter()
        .map(|class| match class.usable_memory {
            Some(bytes) => format!("{} MiB x{}", bytes / (1024 * 1024), class.slots),
            None => format!("host memory x{}", class.slots),
        })
        .collect();
    eprintln!(
        "tolmap serve: worker listener on http://{address} (loopback only), {agents} agent(s) in \
         worker classes {}",
        described.join(", ")
    );
    Ok(Loopback { hub, supervisor })
}

/// A running remote worker tier: the hub. Its agents run on other hosts
/// and are not this process's to stop.
pub struct Remote {
    hub: Arc<WorkerHub>,
}

impl Remote {
    /// The master's half of graceful shutdown in remote mode, after
    /// `JobRegistry::shutdown` has stopped admission (§6 "master graceful
    /// stop": "jobs stay in the store, agents keep running and reconnect
    /// when the master is back"). Unlike loopback mode, no agent is told to
    /// stop: every channel is closed (`WorkerHub::detach_all`), each
    /// running job's row stays `leased`/`running`, and the next process
    /// adopts it and its agent resumes it. Waits a moment for the runners
    /// to let their leases go.
    pub async fn shutdown(self) {
        self.hub.detach_all();
        let hub = self.hub.clone();
        let _ = tokio::task::spawn_blocking(move || hub.wait_for_no_leases(REAP_GRACE)).await;
    }
}

/// `TOLMAP_WORKERS=remote[:N]` (#97 phase 3, docs/WORKER_TIER.md §5.1,
/// §5.6, §8 "Phase 3"): opens the worker listener -- with TLS unless it is
/// on loopback (`remote_tls_files`) -- for agents holding the owner's
/// tokens (`TokenFile`), points the registry at the hub with N slots in one
/// class, and starts no agents. Everything is read and checked before the
/// listener opens, so a bad certificate, key or token file stops startup.
pub async fn start_remote(state: &Arc<AppState>, slots: usize) -> anyhow::Result<Remote> {
    let settings = RemoteSettings::from_env()?;
    let tls = match &settings.tls {
        Some((cert, key)) => Some(server_tls(cert, key)?),
        None => None,
    };
    let tokens = TokenFile::open(&settings.tokens)?;
    // One class of unknown size: every agent fits it. The decided phase 3
    // fleet is one class (§10.1); classes built from remote workers'
    // `hello`s are for a fleet that has more than one.
    let classes = vec![Class {
        usable_memory: None,
        slots,
    }];
    let cache_dir = state.config.cache_dir.clone();
    std::fs::create_dir_all(&cache_dir)
        .with_context(|| format!("create {}", cache_dir.display()))?;
    let staging = cache_dir.join("worker-artifacts");
    fresh_private_dir(&staging)?;
    let build = tokio::task::spawn_blocking(own_build)
        .await
        .context("hash this binary for its build identity")?;
    let listen = settings.lease.listen;
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("bind the worker listener on {listen}"))?;
    let address = listener.local_addr()?;
    let scheme = if tls.is_some() { "https" } else { "http" };
    let (frame_rate, frame_burst) = frame_limit_from_env();
    let hub = Arc::new(
        WorkerHub::new(
            Vec::new(),
            build,
            settings.lease.heartbeat_s,
            settings.lease.lease_ttl,
            settings.lease.retries,
            staging,
            format!("{scheme}://{address}"),
            classes.iter().map(|class| class.usable_memory).collect(),
        )
        .with_token_file(tokens, tls.is_some())
        .with_frame_limit(frame_rate, frame_burst),
    );
    let weak = Arc::downgrade(state);
    hub.on_agents_changed(Box::new(move || {
        if let Some(state) = weak.upgrade() {
            jobs::agents_changed(&state);
        }
    }));
    state.jobs.set_remote(hub.clone(), classes);
    // Restart order, as in loopback mode: the store's live jobs, and the
    // leases a previous process left, are reloaded before any agent can
    // connect, so a remote agent that redials finds its lease adopted and
    // resumes it.
    jobs::restore(state).context("reload jobs from the store")?;
    let app = router(state.clone());
    match tls {
        Some(config) => {
            let listener = TlsListener::start(listener, config)?;
            tokio::spawn(async move {
                if let Err(error) = axum::serve(listener, app).await {
                    eprintln!("tolmap serve: the worker listener stopped: {error}");
                }
            });
        }
        None => {
            tokio::spawn(async move {
                if let Err(error) = axum::serve(listener, app).await {
                    eprintln!("tolmap serve: the worker listener stopped: {error}");
                }
            });
        }
    }
    eprintln!(
        "tolmap serve: worker listener on {scheme}://{address} for remote agents, {slots} \
         slot(s), tokens from {}",
        settings.tokens.display()
    );
    Ok(Remote { hub })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use axum::body::Body;
    use std::net::TcpStream;

    use tokio_tungstenite::tungstenite;

    use crate::service::clone::RepoSource;
    use crate::service::config::{Limits, ServeConfig};
    use crate::service::jobs::JobStatus;
    use crate::service::ratelimit::RateLimiter;
    use crate::service::store::Store;
    use crate::worker::{Artifact, HeartbeatJob, WorkerClass};

    const TOKENS: [&str; 2] = ["test-token-for-agent-0", "test-token-for-agent-1"];
    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    fn test_build() -> WorkerBuild {
        WorkerBuild {
            version: "test".to_owned(),
            commit: "test-build".to_owned(),
            indexers: BTreeMap::new(),
        }
    }

    /// A master with the hub and listener of loopback mode, on a store and
    /// cache of its own, with no agents started: the tests play them.
    struct Fixture {
        state: Arc<AppState>,
        hub: Arc<WorkerHub>,
        port: u16,
        dir: tempfile::TempDir,
        runtime: Option<tokio::runtime::Runtime>,
        /// Set by `relayed`: the relay every artifact URL and the agents'
        /// channel go through.
        relay: Option<Relay>,
    }

    /// One class of unknown size with `slots` agents: plain `loopback:N`.
    fn one_class(slots: usize) -> Vec<Class> {
        vec![Class {
            usable_memory: None,
            slots,
        }]
    }

    const GIB: u64 = 1 << 30;

    /// §9's two classes, one agent each: token 0 is the "small" agent (2
    /// GiB), token 1 the "large" one (16 GiB).
    fn two_classes() -> Vec<Class> {
        vec![
            Class {
                usable_memory: Some(2 * GIB),
                slots: 1,
            },
            Class {
                usable_memory: Some(16 * GIB),
                slots: 1,
            },
        ]
    }

    impl Fixture {
        fn new(lease_ttl: Duration, build: WorkerBuild) -> Self {
            Self::with(tempfile::tempdir().unwrap(), lease_ttl, build, TOKENS.len())
        }

        fn with_upload_idle_timeout(
            lease_ttl: Duration,
            build: WorkerBuild,
            idle_timeout: Duration,
        ) -> Self {
            Self::build_with(
                tempfile::tempdir().unwrap(),
                lease_ttl,
                build,
                one_class(TOKENS.len()),
                Limits::default(),
                false,
                &|hub| hub.with_artifact_body_idle_timeout(idle_timeout),
            )
        }

        /// A master with `classes` (at most `TOKENS.len()` agents in all)
        /// and a per-class queue bound of `max_queued_jobs`.
        fn classed(
            lease_ttl: Duration,
            build: WorkerBuild,
            classes: Vec<Class>,
            max_queued_jobs: usize,
        ) -> Self {
            Self::build(
                tempfile::tempdir().unwrap(),
                lease_ttl,
                build,
                classes,
                Limits {
                    max_queued_jobs,
                    ..Limits::default()
                },
                false,
            )
        }

        /// A master on `dir`'s store and cache, which may hold a previous
        /// master's jobs: it reloads them as `start_loopback` does, before
        /// its listener serves anyone.
        fn with(
            dir: tempfile::TempDir,
            lease_ttl: Duration,
            build: WorkerBuild,
            slots: usize,
        ) -> Self {
            Self::build(
                dir,
                lease_ttl,
                build,
                one_class(slots),
                Limits::default(),
                false,
            )
        }

        /// A master whose listener is reached through a `Relay`: the agent
        /// only sends its token to the origin it dialled, so the artifact
        /// URLs name the relay too.
        fn relayed(lease_ttl: Duration, build: WorkerBuild) -> Self {
            Self::build(
                tempfile::tempdir().unwrap(),
                lease_ttl,
                build,
                one_class(TOKENS.len()),
                Limits::default(),
                true,
            )
        }

        fn build(
            dir: tempfile::TempDir,
            lease_ttl: Duration,
            build: WorkerBuild,
            classes: Vec<Class>,
            limits: Limits,
            relayed: bool,
        ) -> Self {
            Self::build_with(dir, lease_ttl, build, classes, limits, relayed, &|hub| hub)
        }

        /// `build`, with `customize` applied to the hub before it serves:
        /// remote mode's token file, a frame limit.
        #[allow(clippy::too_many_arguments)]
        fn build_with(
            dir: tempfile::TempDir,
            lease_ttl: Duration,
            build: WorkerBuild,
            classes: Vec<Class>,
            limits: Limits,
            relayed: bool,
            customize: &dyn Fn(WorkerHub) -> WorkerHub,
        ) -> Self {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            let config = ServeConfig {
                bind: "127.0.0.1:0".parse().unwrap(),
                db_path: dir.path().join("store.sqlite3"),
                cache_dir: dir.path().join("cache"),
                static_dir: None,
                prune_variant: crate::pipeline::PruneVariant::NodeRelative,
                namer: crate::naming::NamerKind::Idf,
                namer_model: crate::naming::DEFAULT_MODEL.to_owned(),
                refs: crate::extract::RefsMode::Hand,
                scip_install: false,
                limits,
                retain_commits_per_repo: 20,
                worker_uid: executor::current_uid(),
                worker_gid: executor::current_gid(),
            };
            std::fs::create_dir_all(&config.cache_dir).unwrap();
            let staging = config.cache_dir.join("worker-artifacts");
            worker_result::create_private_dir(&staging).unwrap();
            let state = Arc::new(AppState {
                store: Store::open(&config.db_path).unwrap(),
                config,
                jobs: jobs::new_registry(),
                rate_limiter: RateLimiter::new(),
            });
            let listener = runtime
                .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
                .unwrap();
            let address = listener.local_addr().unwrap();
            let relay = relayed.then(|| Relay::start(address.port()));
            let base_url = match &relay {
                Some(relay) => format!("http://127.0.0.1:{}", relay.port),
                None => format!("http://{address}"),
            };
            let hub = Arc::new(customize(WorkerHub::new(
                TOKENS
                    .iter()
                    .map(|token| token_digest(token.as_bytes()))
                    .collect(),
                build,
                // A real agent heartbeats at this interval, so it has to fit
                // a short test lease several times; scripted agents send
                // their own heartbeats or none.
                (lease_ttl.as_secs() / 3).max(1),
                lease_ttl,
                DEFAULT_RETRIES,
                staging,
                base_url,
                classes.iter().map(|class| class.usable_memory).collect(),
            )));
            // As `start_loopback` wires it.
            let weak = Arc::downgrade(&state);
            hub.on_agents_changed(Box::new(move || {
                if let Some(state) = weak.upgrade() {
                    jobs::agents_changed(&state);
                }
            }));
            state.jobs.set_remote(hub.clone(), classes);
            {
                let _entered = runtime.enter();
                jobs::restore(&state).unwrap();
            }
            let app = router(state.clone());
            runtime.spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            Fixture {
                state,
                hub,
                port: address.port(),
                dir,
                runtime: Some(runtime),
                relay,
            }
        }

        fn spawn(&self, repo: RepoRef) -> Uuid {
            self.spawn_at(repo, COMMIT)
        }

        /// For a test whose agent really clones: the executor checks out
        /// the admitted commit, so it must exist in the repository.
        fn spawn_at(&self, repo: RepoRef, commit: &str) -> Uuid {
            let _entered = self.runtime.as_ref().unwrap().enter();
            jobs::spawn_job(self.state.clone(), repo, commit.to_owned()).unwrap()
        }

        fn snapshot(&self, id: Uuid) -> JobSnapshot {
            self.state.jobs.subscribe(id).unwrap().borrow().clone()
        }

        fn wait_for(
            &self,
            id: Uuid,
            what: &str,
            check: impl Fn(&JobSnapshot) -> bool,
        ) -> JobSnapshot {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let snapshot = self.snapshot(id);
                if check(&snapshot) {
                    return snapshot;
                }
                assert!(Instant::now() < deadline, "{what}: {snapshot:?}");
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn leases(&self) -> usize {
            self.hub.lock().leases.len()
        }

        fn wait_for_no_lease(&self) {
            let deadline = Instant::now() + Duration::from_secs(15);
            while self.leases() > 0 {
                assert!(Instant::now() < deadline, "the lease never ended");
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn agent(&self, token: usize) -> FakeAgent {
            FakeAgent::join(self.port, TOKENS[token], test_build())
        }

        /// A scripted agent advertising `memory_bytes` of usable memory.
        fn class_agent(&self, token: usize, memory_bytes: u64) -> FakeAgent {
            FakeAgent::join_as(self.port, TOKENS[token], test_build(), memory_bytes)
        }

        fn row(&self, id: Uuid) -> crate::service::store::JobRow {
            self.state
                .store
                .job(&id.to_string())
                .unwrap()
                .expect("the job has a row")
        }

        fn wait_for_row(
            &self,
            id: Uuid,
            what: &str,
            check: impl Fn(&crate::service::store::JobRow) -> bool,
        ) -> crate::service::store::JobRow {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let row = self.row(id);
                if check(&row) {
                    return row;
                }
                assert!(Instant::now() < deadline, "{what}: {row:?}");
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn nothing_stored(&self, slug: &str) {
            assert!(self.state.store.get(slug, COMMIT).unwrap().is_none());
            let maps = self.state.config.cache_dir.join("maps");
            if let Ok(entries) = std::fs::read_dir(maps.join(slug)) {
                let left: Vec<_> = entries.map(|entry| entry.unwrap().file_name()).collect();
                assert!(left.is_empty(), "left in the store: {left:?}");
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.state.jobs.shutdown();
            self.hub.shutdown_now();
            // Runners leave once their agent's channel is gone (a stopping
            // master does not wait out a lease); give them the moment that
            // takes while the listener still runs.
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.leases() > 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if let Some(runtime) = self.runtime.take() {
                runtime.shutdown_timeout(Duration::from_secs(2));
            }
        }
    }

    fn remote_repo(name: &str) -> RepoRef {
        RepoRef {
            slug: format!("test/{name}"),
            owner: "test".to_owned(),
            repo: name.to_owned(),
            source: RepoSource::Remote("https://example.invalid/test.git".to_owned()),
        }
    }

    fn dial(port: u16, token: &str) -> Result<tungstenite::WebSocket<TcpStream>, String> {
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let uri: tungstenite::http::Uri = format!("ws://127.0.0.1:{port}/workers/connect")
            .parse()
            .unwrap();
        let request = tungstenite::ClientRequestBuilder::new(uri)
            .with_header("Authorization", format!("Bearer {token}"));
        let (socket, _) =
            tungstenite::client(request, stream).map_err(|error| error.to_string())?;
        Ok(socket)
    }

    fn hello(build: WorkerBuild, proto: (u32, u32)) -> WorkerMessage {
        hello_with(build, proto, &[FEATURE_LOCAL_PATHS], Vec::new())
    }

    fn hello_with(
        build: WorkerBuild,
        proto: (u32, u32),
        features: &[&str],
        resume: Vec<ResumeEntry>,
    ) -> WorkerMessage {
        WorkerMessage::Hello {
            proto_min: proto.0,
            proto_max: proto.1,
            worker_id: "fake".to_owned(),
            build,
            class: WorkerClass {
                memory_bytes: 1 << 30,
                cpus: 1,
            },
            slots: 1,
            features: features.iter().map(|feature| feature.to_string()).collect(),
            resume,
        }
    }

    fn resume_entry(job: Uuid, epoch: u64, last_seq: u64) -> ResumeEntry {
        ResumeEntry {
            job_id: job.to_string(),
            epoch,
            last_seq,
        }
    }

    /// A scripted agent on the real channel. `epoch` and `seq` follow the
    /// job it was last assigned.
    struct FakeAgent {
        socket: tungstenite::WebSocket<TcpStream>,
        seq: u64,
        epoch: u64,
    }

    impl FakeAgent {
        fn raw(port: u16, token: &str) -> Self {
            FakeAgent {
                socket: dial(port, token).unwrap(),
                seq: 0,
                epoch: 1,
            }
        }

        fn join(port: u16, token: &str, build: WorkerBuild) -> Self {
            Self::join_as(port, token, build, 1 << 30)
        }

        fn join_as(port: u16, token: &str, build: WorkerBuild, memory_bytes: u64) -> Self {
            let mut agent = FakeAgent::raw(port, token);
            let mut hello = hello(build, (PROTO, PROTO));
            if let WorkerMessage::Hello { class, .. } = &mut hello {
                class.memory_bytes = memory_bytes;
            }
            agent.send(&hello);
            match agent.recv() {
                MasterMessage::Welcome { proto, .. } => assert_eq!(proto, PROTO),
                other => panic!("expected welcome, got {other:?}"),
            }
            agent.send(&WorkerMessage::Ready { slots_free: 1 });
            agent
        }

        /// A new channel for an agent that advertises `resume` and names
        /// `resume` in its `hello`; the `welcome`'s answers. It sends no
        /// `ready`: the test decides.
        fn rejoin(port: u16, token: &str, resume: Vec<ResumeEntry>) -> (Self, Vec<WelcomeResume>) {
            let mut agent = FakeAgent::raw(port, token);
            agent.send(&hello_with(
                test_build(),
                (PROTO, PROTO),
                &[FEATURE_LOCAL_PATHS, FEATURE_RESUME],
                resume,
            ));
            match agent.recv() {
                MasterMessage::Welcome { resume, .. } => (agent, resume),
                other => panic!("expected welcome, got {other:?}"),
            }
        }

        fn send(&mut self, message: &WorkerMessage) {
            self.send_text(serde_json::to_string(message).unwrap());
        }

        fn send_text(&mut self, text: String) {
            self.socket.send(tungstenite::Message::text(text)).unwrap();
        }

        fn event(&mut self, job: Uuid, event: WorkerEvent) {
            self.event_at(job, self.epoch, event);
        }

        /// One `job_event` at `epoch`, whatever this agent holds.
        fn event_at(&mut self, job: Uuid, epoch: u64, event: WorkerEvent) {
            self.seq += 1;
            let seq = self.seq;
            self.send(&WorkerMessage::JobEvent {
                job_id: job.to_string(),
                epoch,
                seq,
                event,
                peak_rss_bytes: None,
            });
        }

        fn recv_text_within(&mut self, wait: Duration) -> Option<String> {
            let deadline = Instant::now() + wait;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return None;
                }
                self.socket.get_ref().set_read_timeout(Some(left)).unwrap();
                match self.socket.read() {
                    Ok(tungstenite::Message::Text(text)) => return Some(text.as_str().to_owned()),
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(error))
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(error) => panic!("the channel failed: {error}"),
                }
            }
        }

        fn recv_within(&mut self, wait: Duration) -> Option<MasterMessage> {
            self.recv_text_within(wait)
                .map(|text| serde_json::from_str(&text).unwrap())
        }

        fn recv_text(&mut self) -> String {
            self.recv_text_within(Duration::from_secs(15))
                .expect("a message from the master")
        }

        fn recv(&mut self) -> MasterMessage {
            serde_json::from_str(&self.recv_text()).unwrap()
        }

        /// The next `assign`; this agent then holds that job at its epoch.
        fn assigned(&mut self) -> Uuid {
            match self.recv() {
                MasterMessage::Assign { job_id, epoch, .. } => {
                    self.epoch = epoch;
                    self.seq = 0;
                    Uuid::parse_str(&job_id).unwrap()
                }
                other => panic!("expected assign, got {other:?}"),
            }
        }

        /// Uploads a whole result for its job at its epoch and sends it.
        fn deliver(&mut self, port: u16, token: &str, job: Uuid) {
            let artifacts = upload_all_at(port, token, job, self.epoch);
            self.event(job, result_event(artifacts));
        }

        /// Whether the master closed the channel within a few seconds.
        fn closed(&mut self) -> bool {
            self.socket
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            loop {
                match self.socket.read() {
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(error))
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        return false
                    }
                    Err(_) => return true,
                }
            }
        }

        fn expect_error(&mut self, code: &str) {
            match self.recv() {
                MasterMessage::Error { code: got, message } => {
                    assert_eq!(got, code, "{message}")
                }
                other => panic!("expected error {code}, got {other:?}"),
            }
            assert!(self.closed(), "the master must close after an error");
        }
    }

    /// One HTTP/1.1 request on its own connection; the status and the whole
    /// response. `length` overrides the `Content-Length` sent, to send a
    /// body shorter than it declares.
    fn request(
        port: u16,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: &[u8],
        length: Option<usize>,
    ) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut head =
            format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
        for (key, value) in headers {
            head.push_str(&format!("{key}: {value}\r\n"));
        }
        head.push_str(&format!(
            "Content-Length: {}\r\n\r\n",
            length.unwrap_or(body.len())
        ));
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        if length.is_some_and(|length| length > body.len()) {
            stream.shutdown(std::net::Shutdown::Write).unwrap();
        }
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        let response = String::from_utf8_lossy(&response).into_owned();
        let status = response
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        (status, response)
    }

    fn bearer(token: &str) -> (&'static str, String) {
        ("Authorization", format!("Bearer {token}"))
    }

    fn sha(body: &[u8]) -> String {
        sha256_reader(body).unwrap().0
    }

    /// The body of `request`'s raw HTTP/1.1 response text (after the blank
    /// line that ends the headers).
    fn response_body(response: &str) -> &str {
        response.split_once("\r\n\r\n").map_or("", |(_, body)| body)
    }

    fn put(port: u16, token: &str, job: Uuid, name: &str, body: &[u8]) -> u16 {
        put_at(port, token, job, 1, name, body)
    }

    fn put_at(port: u16, token: &str, job: Uuid, epoch: u64, name: &str, body: &[u8]) -> u16 {
        request(
            port,
            "PUT",
            &format!("/workers/artifacts/{job}/{epoch}/{name}"),
            &[bearer(token), (SHA256_HEADER, sha(body))],
            body,
            None,
        )
        .0
    }

    /// Declares an upload without sending its body. A prompt response proves
    /// a size refusal happened before the handler tried to read the stream.
    fn put_declared_without_body(
        port: u16,
        token: &str,
        job: Uuid,
        name: &str,
        bytes: u64,
    ) -> (u16, String) {
        use std::io::{Read, Write};

        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        write!(
            stream,
            "PUT /workers/artifacts/{job}/1/{name} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Connection: close\r\n\
             Authorization: Bearer {token}\r\n\
             {SHA256_HEADER}: {}\r\n\
             Content-Length: {bytes}\r\n\r\n",
            sha(MAP)
        )
        .unwrap();
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        let response = String::from_utf8_lossy(&response).into_owned();
        let status = response
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        (status, response)
    }

    /// Opens an artifact PUT and leaves its declared body unfinished. The
    /// connection remains open so the handler sees an idle body, not EOF.
    fn stalled_artifact_put(port: u16, token: &str, job: Uuid, name: &str) -> TcpStream {
        use std::io::Write;

        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(600)))
            .unwrap();
        write!(
            stream,
            "PUT /workers/artifacts/{job}/1/{name} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Connection: close\r\n\
             Authorization: Bearer {token}\r\n\
             {SHA256_HEADER}: {}\r\n\
             Content-Length: 1\r\n\r\n",
            sha(MAP)
        )
        .unwrap();
        stream
    }

    fn response_status(stream: &mut TcpStream) -> std::io::Result<u16> {
        use std::io::Read;

        let mut response = Vec::new();
        stream.read_to_end(&mut response)?;
        Ok(String::from_utf8_lossy(&response)
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0))
    }

    /// Sends an HTTP/1.1 chunked PUT without Content-Length. The handler
    /// should reject the headers before consuming this body; writes may stop
    /// early after the server closes the refused request.
    fn put_chunked_without_length(
        port: u16,
        token: &str,
        job: Uuid,
        name: &str,
        body: &[u8],
    ) -> (u16, String) {
        use std::io::{Read, Write};

        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        write!(
            stream,
            "PUT /workers/artifacts/{job}/1/{name} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Connection: close\r\n\
             Authorization: Bearer {token}\r\n\
             {SHA256_HEADER}: {}\r\n\
             Transfer-Encoding: chunked\r\n\r\n",
            sha(body)
        )
        .unwrap();
        let _ = write!(stream, "{:X}\r\n", body.len());
        let _ = stream.write_all(body);
        let _ = stream.write_all(b"\r\n0\r\n\r\n");

        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        let response = String::from_utf8_lossy(&response).into_owned();
        let status = response
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        (status, response)
    }

    /// Sends `body` verbatim after an HTTP/1.1 request whose Content-Length
    /// covers only its first `declared_bytes` bytes.
    fn put_with_extra_after_content_length(
        port: u16,
        token: &str,
        job: Uuid,
        name: &str,
        body: &[u8],
        declared_bytes: usize,
    ) -> (Vec<u16>, String) {
        use std::io::{Read, Write};

        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let head = format!(
            "PUT /workers/artifacts/{job}/1/{name} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Connection: keep-alive\r\n\
             Authorization: Bearer {token}\r\n\
             {SHA256_HEADER}: {}\r\n\
             Content-Length: {declared_bytes}\r\n\r\n",
            sha(body)
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();

        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        let response = String::from_utf8_lossy(&response).into_owned();
        let mut statuses = Vec::new();
        let mut remaining = response.as_str();
        while let Some((headers, body)) = remaining.split_once("\r\n\r\n") {
            let Some(status) = headers
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|value| value.parse().ok())
            else {
                break;
            };
            statuses.push(status);
            let Some(content_length) = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            else {
                break;
            };
            let Some(next_response) = body.get(content_length..) else {
                break;
            };
            remaining = next_response;
            if remaining.is_empty() {
                break;
            }
        }
        (statuses, response)
    }

    fn lease_blob_stats(fixture: &Fixture, job: Uuid) -> (usize, u64) {
        let dir = fixture
            .hub
            .with_lease(0, job, 1, |lease| Ok(lease.dir.clone()))
            .unwrap();
        let mut count = 0;
        let mut bytes = 0;
        for entry in std::fs::read_dir(dir.join("blobs")).unwrap() {
            let metadata = entry.unwrap().metadata().unwrap();
            count += 1;
            bytes += metadata.len();
        }
        (count, bytes)
    }

    fn lease_upload_state(fixture: &Fixture, job: Uuid) -> (usize, usize, u64) {
        let (dir, reserved_bytes) = fixture
            .hub
            .with_lease(0, job, 1, |lease| {
                let reserved_bytes = lease
                    .reservations
                    .values()
                    .map(|reservation| reservation.bytes)
                    .sum();
                Ok((lease.dir.clone(), reserved_bytes))
            })
            .unwrap();
        let blobs = std::fs::read_dir(dir.join("blobs")).unwrap().count();
        let temp_files = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("upload-"))
            })
            .count();
        (blobs, temp_files, reserved_bytes)
    }

    fn artifact(name: &str, body: &[u8]) -> Artifact {
        Artifact {
            name: name.to_owned(),
            sha256: sha(body),
            bytes: body.len() as u64,
        }
    }

    fn result_event(artifacts: Vec<Artifact>) -> WorkerEvent {
        WorkerEvent::Result {
            v: 1,
            map_path: "artifact:map".to_owned(),
            symbols_path: "artifact:symbols".to_owned(),
            symbols_dir: "artifact:symbols_dir".to_owned(),
            names_cache: "artifact:names".to_owned(),
            commit: COMMIT.to_owned(),
            branch: Some("main".to_owned()),
            lang: "py".to_owned(),
            files: 1,
            districts: 1,
            modularity: 0.5,
            artifacts,
        }
    }

    // `result_event` reports `files: 1, districts: 1` -- this must agree,
    // since #97 phase 2's `check_counts` (`worker_result.rs`) now refuses a
    // result whose counts disagree with the map document it shipped.
    const MAP: &[u8] = br#"{"F": ["a.py"], "districts": {"0": {}}}"#;
    const OTHER_MAP: &[u8] = br#"{"F": ["b.py"], "districts": {"1": {}}}"#;
    const SYMBOLS: &[u8] = b"{\"symbols\":1}";
    const DISTRICT: &[u8] = b"{\"district\":0}";
    const NAMES: &[u8] = b"{}";

    fn upload_all(port: u16, job: Uuid) -> Vec<Artifact> {
        upload_all_at(port, TOKENS[0], job, 1)
    }

    fn upload_all_at(port: u16, token: &str, job: Uuid, epoch: u64) -> Vec<Artifact> {
        let files = [
            ("map", MAP),
            ("symbols", SYMBOLS),
            ("symbols_dir/0.json", DISTRICT),
            ("names", NAMES),
        ];
        files
            .iter()
            .map(|(name, body)| {
                assert_eq!(
                    put_at(port, token, job, epoch, name, body),
                    200,
                    "PUT {name} at epoch {epoch}"
                );
                artifact(name, body)
            })
            .collect()
    }

    /// §7.4's clamp: busy (running + queued > 0) always wins over the idle
    /// rule, and is capped at `max`, whichever of running or queued (or
    /// both) supplies the count.
    #[test]
    fn desired_capacity_clamps_busy_classes_to_the_configured_maximum() {
        let hour = Duration::from_secs(3600);
        assert_eq!(desired_capacity(1, 0, 1, None, hour), 1);
        assert_eq!(desired_capacity(0, 3, 1, None, hour), 1);
        assert_eq!(desired_capacity(5, 5, 1, None, hour), 1);
        assert_eq!(desired_capacity(1, 2, 4, None, hour), 3);
        assert_eq!(desired_capacity(5, 5, 4, None, hour), 4);
    }

    /// §7.4's idle drop: a class that has never been busy is 0 from the
    /// start (nothing starts a worker before the first job); a class that
    /// just went idle holds at `max.min(1)` until `TOLMAP_WORKER_IDLE_S`
    /// passes, then drops to 0; a `max` of 0 disables it outright.
    #[test]
    fn desired_capacity_drops_to_zero_only_after_the_idle_window() {
        let idle_after = Duration::from_secs(600);
        assert_eq!(
            desired_capacity(0, 0, 1, None, idle_after),
            0,
            "never busy must not start a worker"
        );
        assert_eq!(
            desired_capacity(0, 0, 1, Some(Duration::from_secs(0)), idle_after),
            1,
            "just went idle: hold one worker"
        );
        assert_eq!(
            desired_capacity(0, 0, 1, Some(Duration::from_secs(599)), idle_after),
            1
        );
        assert_eq!(
            desired_capacity(0, 0, 1, Some(Duration::from_secs(600)), idle_after),
            0,
            "at the window: drop"
        );
        assert_eq!(
            desired_capacity(0, 0, 1, Some(Duration::from_secs(3600)), idle_after),
            0
        );
        assert_eq!(
            desired_capacity(0, 0, 0, Some(Duration::from_secs(1)), idle_after),
            0,
            "max 0 disables the class even mid-grace"
        );
    }

    #[test]
    fn workers_mode_is_local_unless_loopback_is_asked_for_and_refuses_the_rest() {
        assert_eq!(WorkersMode::parse(None), Ok(WorkersMode::Local));
        assert_eq!(WorkersMode::parse(Some("")), Ok(WorkersMode::Local));
        assert_eq!(WorkersMode::parse(Some("local")), Ok(WorkersMode::Local));
        assert_eq!(WorkersMode::parse(Some(" local ")), Ok(WorkersMode::Local));
        assert_eq!(
            WorkersMode::parse(Some("loopback:1")),
            Ok(WorkersMode::Loopback(1))
        );
        assert_eq!(
            WorkersMode::parse(Some("loopback:3")),
            Ok(WorkersMode::Loopback(3))
        );
        // #97 phase 3: `remote` is one slot, `remote:N` N.
        assert_eq!(
            WorkersMode::parse(Some("remote")),
            Ok(WorkersMode::Remote(1))
        );
        assert_eq!(
            WorkersMode::parse(Some(" remote:2 ")),
            Ok(WorkersMode::Remote(2))
        );
        for bad in [
            "loopback:0",
            "loopback:",
            "loopback",
            "loopback:-1",
            "loopback:two",
            "remote:0",
            "remote:",
            "remote:x",
            "REMOTE",
            "LOCAL",
        ] {
            assert!(WorkersMode::parse(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn tokens_are_distinct_256_bit_values_compared_by_digest() {
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 64);
        assert!(is_sha256_hex(&a));
        assert_ne!(a, b);
        assert!(digests_equal(
            &token_digest(a.as_bytes()),
            &token_digest(a.as_bytes())
        ));
        assert!(!digests_equal(
            &token_digest(a.as_bytes()),
            &token_digest(b.as_bytes())
        ));
    }

    /// §5.1: no token, a wrong one, the right one in the wrong scheme or in
    /// the URL -- each a 401 before any upgrade, and no channel exists.
    #[test]
    fn the_channel_refuses_a_missing_or_wrong_token_before_upgrading() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let upgrade = |path: &str, authorization: Option<String>| -> u16 {
            let mut stream = TcpStream::connect(("127.0.0.1", fixture.port)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut head = format!(
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: Upgrade\r\n\
                 Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n\
                 Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"
            );
            if let Some(authorization) = authorization {
                head.push_str(&format!("Authorization: {authorization}\r\n"));
            }
            head.push_str("\r\n");
            stream.write_all(head.as_bytes()).unwrap();
            // The status line is all this needs; a 101 keeps the socket open.
            let mut line = Vec::new();
            let mut byte = [0u8; 1];
            while !line.ends_with(b"\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                line.push(byte[0]);
            }
            String::from_utf8_lossy(&line)
                .split(' ')
                .nth(1)
                .and_then(|code| code.parse().ok())
                .unwrap_or(0)
        };
        assert_eq!(upgrade("/workers/connect", None), 401);
        assert_eq!(
            upgrade("/workers/connect", Some("Bearer not-a-token".to_owned())),
            401
        );
        assert_eq!(
            upgrade("/workers/connect", Some(format!("Basic {}", TOKENS[0]))),
            401
        );
        assert_eq!(
            upgrade(&format!("/workers/connect?token={}", TOKENS[0]), None),
            401
        );
        assert_eq!(fixture.hub.lock().conns.len(), 0);
        assert!(dial(fixture.port, "not-a-token")
            .unwrap_err()
            .contains("401"));
        assert_eq!(
            upgrade("/workers/connect", Some(format!("Bearer {}", TOKENS[0]))),
            101
        );
    }

    /// §5.6: the worker endpoints live on their own listener only.
    #[test]
    fn the_public_router_routes_no_worker_path() {
        use tower::ServiceExt;
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let runtime = fixture.runtime.as_ref().unwrap();
        for (method, path) in [
            ("GET", "/workers/connect".to_owned()),
            (
                "PUT",
                format!("/workers/artifacts/{}/1/map", Uuid::new_v4()),
            ),
            (
                "GET",
                format!("/workers/artifacts/{}/1/inputs/names", Uuid::new_v4()),
            ),
            ("GET", "/workers/capacity".to_owned()),
        ] {
            let request = axum::http::Request::builder()
                .method(method)
                .uri(&path)
                .header(header::AUTHORIZATION, format!("Bearer {}", TOKENS[0]))
                .body(Body::empty())
                .unwrap();
            let response = runtime
                .block_on(crate::service::http::router(fixture.state.clone()).oneshot(request))
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
        }
    }

    /// §7.4: `GET /workers/capacity` reports one row per configured class,
    /// refuses a missing or wrong token like every other `/workers` route,
    /// and its running/queued counts track admission and dispatch (no
    /// idle-window assertion here -- that arithmetic is
    /// `desired_capacity_*` above; a real elapsed wait would make this test
    /// slow and flaky for no more coverage).
    #[test]
    fn workers_capacity_reports_running_queued_and_connected_agents_per_class() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let get = || {
            request(
                fixture.port,
                "GET",
                "/workers/capacity",
                &[bearer(TOKENS[0])],
                b"",
                None,
            )
        };

        assert_eq!(
            request(fixture.port, "GET", "/workers/capacity", &[], b"", None).0,
            401,
            "no token"
        );
        assert_eq!(
            request(
                fixture.port,
                "GET",
                "/workers/capacity",
                &[bearer("not-a-real-token")],
                b"",
                None,
            )
            .0,
            401,
            "wrong token"
        );

        let (status, response) = get();
        assert_eq!(status, 200, "{response}");
        let body: serde_json::Value = serde_json::from_str(response_body(&response)).unwrap();
        let classes = body["classes"].as_array().unwrap();
        assert_eq!(classes.len(), 1, "one class: plain loopback:N");
        assert_eq!(classes[0]["running"], 0);
        assert_eq!(classes[0]["queued"], 0);
        assert_eq!(classes[0]["connected_agents"], 0);
        assert_eq!(classes[0]["desired"], 0, "never busy: no worker started");

        // Queued, no agent connected yet: desired clamps to the default
        // maximum (1) even though nothing is running.
        let id = fixture.spawn(remote_repo("demo"));
        fixture.wait_for(id, "queued", |snapshot| {
            snapshot.status == JobStatus::Queued
        });
        let (status, response) = get();
        assert_eq!(status, 200, "{response}");
        let body: serde_json::Value = serde_json::from_str(response_body(&response)).unwrap();
        let classes = body["classes"].as_array().unwrap();
        // With no agent connected the job waits in its class's slot, not in
        // the queue (#157, departure 4), so it counts as running here. The
        // starter only reads `desired`, which is the same either way.
        assert_eq!(
            classes[0]["queued"].as_u64().unwrap() + classes[0]["running"].as_u64().unwrap(),
            1
        );
        assert_eq!(classes[0]["connected_agents"], 0);
        assert_eq!(classes[0]["desired"], 1);

        // An agent connects and takes it: running, not queued, one
        // connected agent.
        let mut agent = fixture.agent(0);
        assert_eq!(agent.assigned(), id);
        let (status, response) = get();
        assert_eq!(status, 200, "{response}");
        let body: serde_json::Value = serde_json::from_str(response_body(&response)).unwrap();
        let classes = body["classes"].as_array().unwrap();
        assert_eq!(classes[0]["queued"], 0);
        assert_eq!(classes[0]["running"], 1);
        assert_eq!(classes[0]["connected_agents"], 1);
        assert_eq!(classes[0]["desired"], 1);
    }

    /// The whole path with a scripted agent: `assign` carries URLs and a
    /// `JobSpec`, never a store path; inputs go to the lease holder only;
    /// events reach the snapshot through local mode's sink; per-file
    /// uploads are registered and `result_accepted` follows the map row.
    #[test]
    fn a_result_from_uploaded_artifacts_is_registered_then_accepted() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        let text = agent.recv_text();
        let MasterMessage::Assign {
            job_id,
            job,
            inputs,
            outputs,
            ..
        } = serde_json::from_str::<MasterMessage>(&text).unwrap()
        else {
            panic!("expected assign: {text}");
        };
        assert_eq!(job_id, id.to_string());
        // §5.2, §9: nothing an agent receives names the store or the
        // master's cache directory.
        for secret in [
            fixture.state.config.db_path.to_string_lossy().into_owned(),
            fixture
                .state
                .config
                .cache_dir
                .to_string_lossy()
                .into_owned(),
            fixture.dir.path().to_string_lossy().into_owned(),
        ] {
            assert!(!text.contains(&secret), "assign names {secret}: {text}");
        }
        assert_eq!(job.source, "https://example.invalid/test.git");
        assert_eq!(job.commit, COMMIT);
        let base = format!("http://127.0.0.1:{}/workers/artifacts/{id}/1", fixture.port);
        assert_eq!(outputs, base);
        assert_eq!(inputs.names_cache, Some(format!("{base}/inputs/names")));
        assert!(inputs.previous_maps.is_empty());

        let names = format!("/workers/artifacts/{id}/1/inputs/names");
        let (status, body) = request(fixture.port, "GET", &names, &[bearer(TOKENS[0])], b"", None);
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            request(fixture.port, "GET", &names, &[bearer(TOKENS[1])], b"", None).0,
            403
        );
        assert_eq!(request(fixture.port, "GET", &names, &[], b"", None).0, 401);
        let unknown = format!("/workers/artifacts/{id}/1/inputs/../../store.sqlite3");
        assert_eq!(
            request(
                fixture.port,
                "GET",
                &unknown,
                &[bearer(TOKENS[0])],
                b"",
                None
            )
            .0,
            404
        );

        agent.event(
            id,
            WorkerEvent::StageStarted {
                v: 1,
                stage: StageId::Clone,
            },
        );
        agent.event(
            id,
            WorkerEvent::StageFinished {
                v: 1,
                stage: StageId::Clone,
                duration_s: 0.5,
                success: true,
            },
        );
        agent.event(
            id,
            WorkerEvent::StageStarted {
                v: 1,
                stage: StageId::Parse,
            },
        );
        let snapshot = fixture.wait_for(id, "indexing", |s| s.status == JobStatus::Indexing);
        let clone = &snapshot.stages[StageId::Clone.index() - 1];
        assert_eq!(clone.state, jobs::StageState::Done);
        assert_eq!(clone.duration_s, Some(0.5));

        let artifacts = upload_all(fixture.port, id);
        // Content-addressed: a repeat upload is a no-op, not an error.
        assert_eq!(put(fixture.port, TOKENS[0], id, "map", MAP), 200);
        agent.event(id, result_event(artifacts));
        match agent.recv() {
            MasterMessage::ResultAccepted { job_id, .. } => assert_eq!(job_id, id.to_string()),
            other => panic!("expected result_accepted, got {other:?}"),
        }
        let snapshot = fixture.snapshot(id);
        assert_eq!(snapshot.status, JobStatus::Done, "{snapshot:?}");
        let row = fixture
            .state
            .store
            .get("test/demo", COMMIT)
            .unwrap()
            .expect("a map row before result_accepted");
        assert_eq!(std::fs::read(&row.map_path).unwrap(), MAP);
        assert_eq!(
            std::fs::read(row.map_path.with_extension("symbols.json")).unwrap(),
            SYMBOLS
        );
        assert_eq!(
            std::fs::read(row.map_path.with_extension("symbols").join("0.json")).unwrap(),
            DISTRICT
        );
        fixture.wait_for_no_lease();
    }

    /// docs/UX.md §12 in worker modes: the agent uploads the map as `map`
    /// before forwarding `write_map`'s end, and the master then serves it at
    /// the job's commit while the symbol stages run -- not before, and not
    /// once the job is done and its row is registered.
    #[test]
    fn an_uploaded_map_opens_early_when_write_map_finishes() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        let text = agent.recv_text();
        assert!(
            matches!(
                serde_json::from_str::<MasterMessage>(&text).unwrap(),
                MasterMessage::Assign { .. }
            ),
            "expected assign: {text}"
        );
        agent.event(
            id,
            WorkerEvent::StageStarted {
                v: 1,
                stage: StageId::WriteMap,
            },
        );
        fixture.wait_for(id, "indexing", |s| s.status == JobStatus::Indexing);
        assert_eq!(put(fixture.port, TOKENS[0], id, "map", MAP), 200);
        // Uploaded but its stage not over: not served yet.
        assert!(!fixture.snapshot(id).map_ready);
        assert!(fixture.state.jobs.early_map("test/demo", COMMIT).is_none());
        agent.event(
            id,
            WorkerEvent::StageFinished {
                v: 1,
                stage: StageId::WriteMap,
                duration_s: 0.1,
                success: true,
            },
        );
        fixture.wait_for(id, "map_ready", |s| s.map_ready);
        assert_eq!(
            fixture
                .state
                .jobs
                .early_map("test/demo", COMMIT)
                .unwrap()
                .as_ref(),
            MAP
        );
        assert!(fixture.state.jobs.early_map("test/demo", "other").is_none());

        assert_eq!(put(fixture.port, TOKENS[0], id, "map", OTHER_MAP), 200);
        agent.event(
            id,
            WorkerEvent::StageFinished {
                v: 1,
                stage: StageId::WriteMap,
                duration_s: 0.2,
                success: true,
            },
        );
        agent.event(
            id,
            WorkerEvent::Log {
                v: 1,
                message: "after repeated write_map".to_owned(),
            },
        );
        fixture.wait_for(id, "repeated write_map", |snapshot| {
            snapshot.stage == "after repeated write_map"
        });
        assert_eq!(
            fixture
                .state
                .jobs
                .early_map("test/demo", COMMIT)
                .unwrap()
                .as_ref(),
            MAP,
            "later write_map events must not replace the first early map"
        );

        let artifacts = upload_all(fixture.port, id);
        agent.event(id, result_event(artifacts));
        match agent.recv() {
            MasterMessage::ResultAccepted { job_id, .. } => assert_eq!(job_id, id.to_string()),
            other => panic!("expected result_accepted, got {other:?}"),
        }
        fixture.wait_for(id, "done", |s| s.status == JobStatus::Done);
        let deadline = Instant::now() + Duration::from_secs(15);
        while fixture.state.jobs.early_map("test/demo", COMMIT).is_some() {
            assert!(Instant::now() < deadline, "the early map outlived the job");
            std::thread::sleep(Duration::from_millis(20));
        }
        fixture.wait_for_no_lease();
    }

    #[test]
    fn each_artifact_kind_rejects_content_length_over_its_cap_before_reading() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);

        // Keep the values in step with the per-kind limits in
        // worker_result.rs: symbols use the full-result allowance, each
        // district is fetched as one viewer JSON response, and names already
        // have a 16 MiB adoption bound.
        for (name, limit) in [
            ("symbols", 256 * 1024 * 1024),
            ("names", 16 * 1024 * 1024),
            ("symbols_dir/0.json", 64 * 1024 * 1024),
        ] {
            let (status, response) =
                put_declared_without_body(fixture.port, TOKENS[0], id, name, limit + 1);
            assert_eq!(status, 413, "{name}: {response}");
        }
        assert!(fixture.hub.uploads(id).is_empty());
    }

    #[test]
    fn chunked_upload_without_content_length_is_refused_without_reserving_or_writing() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);

        let body = vec![b'x'; worker_result::NAMES_CACHE_MAX_BYTES as usize + 1];
        assert!(body.len() as u64 > worker_result::NAMES_CACHE_MAX_BYTES);
        let reserved_before = lease_upload_state(&fixture, id).2;
        let (status, response) =
            put_chunked_without_length(fixture.port, TOKENS[0], id, "names", &body);

        assert_eq!(status, 411, "{response}");
        assert_eq!(lease_upload_state(&fixture, id), (0, 0, reserved_before));
        assert!(fixture.hub.uploads(id).is_empty());
    }

    #[test]
    fn a_lease_refuses_the_n_plus_one_stalled_artifact_put() {
        let fixture = Fixture::with_upload_idle_timeout(
            Duration::from_secs(60),
            test_build(),
            Duration::from_millis(150),
        );
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);

        let mut stalled = (0..WORKER_LEASE_MAX_IN_FLIGHT_UPLOADS)
            .map(|_| stalled_artifact_put(fixture.port, TOKENS[0], id, "map"))
            .collect::<Vec<_>>();
        let reserved_deadline = Instant::now() + Duration::from_millis(100);
        while lease_upload_state(&fixture, id)
            != (
                0,
                WORKER_LEASE_MAX_IN_FLIGHT_UPLOADS,
                WORKER_LEASE_MAX_IN_FLIGHT_UPLOADS as u64,
            )
        {
            assert!(
                Instant::now() < reserved_deadline,
                "the first N stalled PUTs did not reserve their slots"
            );
            std::thread::sleep(Duration::from_millis(2));
        }

        let mut extra = stalled_artifact_put(fixture.port, TOKENS[0], id, "map");
        assert_eq!(
            response_status(&mut extra).expect("read the N+1 refusal"),
            429,
            "the extra stalled PUT must be refused before it reserves a writer"
        );

        let idle_deadline = Instant::now() + Duration::from_millis(500);
        while lease_upload_state(&fixture, id) != (0, 0, 0) {
            assert!(
                Instant::now() < idle_deadline,
                "the accepted stalled PUTs did not release their reservations and temp files"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(stalled);
    }

    #[test]
    fn an_idle_artifact_put_releases_its_reservation_and_temp_file() {
        let fixture = Fixture::with_upload_idle_timeout(
            Duration::from_secs(60),
            test_build(),
            Duration::from_millis(150),
        );
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);

        let mut stalled = stalled_artifact_put(fixture.port, TOKENS[0], id, "map");
        let reserved_deadline = Instant::now() + Duration::from_millis(100);
        while lease_upload_state(&fixture, id) != (0, 1, 1) {
            assert!(
                Instant::now() < reserved_deadline,
                "the stalled PUT did not create its reservation and temp file"
            );
            std::thread::sleep(Duration::from_millis(2));
        }

        assert_eq!(
            response_status(&mut stalled).expect("read the idle-body refusal"),
            400,
            "an idle body follows the existing incomplete-body failure response"
        );
        assert_eq!(lease_upload_state(&fixture, id), (0, 0, 0));
    }

    #[test]
    fn bytes_after_declared_content_length_fail_without_leaking_upload_state() {
        const DECLARED_BYTES: usize = 64;
        const EXTRA_BYTES: usize = 1024 * 1024;

        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);

        // Hyper frames the PUT at Content-Length, so only the declared 64
        // bytes reach its handler. The full-wire digest then makes that
        // request fail with 400. On this keep-alive connection, the trailing
        // 1 MiB is parsed as another request and Hyper rejects it with 431.
        // Neither response may leave an artifact behind.
        let body = vec![b'x'; DECLARED_BYTES + EXTRA_BYTES];
        let reserved_before = lease_upload_state(&fixture, id).2;
        let (statuses, response) = put_with_extra_after_content_length(
            fixture.port,
            TOKENS[0],
            id,
            "map",
            &body,
            DECLARED_BYTES,
        );

        // Hyper currently reports 400 for the incomplete PUT, then 431 for
        // the unframed trailing bytes. Keep those observed codes here, but
        // assert the contract without pinning Hyper's parser details.
        assert!(
            statuses.iter().all(|status| (400..500).contains(status)),
            "expected only 4xx responses, got {statuses:?}: {response}"
        );
        assert_eq!(lease_upload_state(&fixture, id), (0, 0, reserved_before));
        assert!(fixture.hub.uploads(id).is_empty());
    }

    #[test]
    fn artifact_put_refuses_declared_length_above_the_kind_cap_before_reading() {
        const SYMBOLS_KIND_LIMIT: u64 = worker_result::FULL_SYMBOLS_MAX_BYTES;

        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);

        let reserved_before = lease_upload_state(&fixture, id).2;
        let (status, response) = put_declared_without_body(
            fixture.port,
            TOKENS[0],
            id,
            "symbols",
            SYMBOLS_KIND_LIMIT + 1,
        );

        assert_eq!(status, 413, "{response}");
        assert_eq!(lease_upload_state(&fixture, id), (0, 0, reserved_before));
        assert!(fixture.hub.uploads(id).is_empty());
    }

    #[test]
    fn a_lease_total_overflow_is_refused_before_reading_the_body() {
        const TEST_LEASE_BYTES: u64 = 4 * worker_result::EARLY_MAP_MAX_BYTES;

        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);
        fixture
            .hub
            .with_lease(0, id, 1, |lease| {
                // Represent already-stored bytes without making a 1 GiB
                // fixture on disk. The reservation check only needs the
                // lease's existing artifact metadata.
                lease.uploads.insert(
                    "map".to_owned(),
                    Upload {
                        sha256: "f".repeat(64),
                        bytes: TEST_LEASE_BYTES - 1,
                    },
                );
                Ok(())
            })
            .unwrap();

        let (status, response) =
            put_declared_without_body(fixture.port, TOKENS[0], id, "symbols", 2);
        assert_eq!(status, 413, "{response}");
        assert_eq!(fixture.hub.uploads(id).len(), 1);
    }

    #[test]
    fn an_artifact_count_at_the_limit_rejects_new_names_but_allows_replacement() {
        const TEST_ARTIFACT_COUNT: usize = 10_000;

        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);
        assert_eq!(put(fixture.port, TOKENS[0], id, "map", b"old map"), 200);
        fixture
            .hub
            .with_lease(0, id, 1, |lease| {
                for district in 0..TEST_ARTIFACT_COUNT - 1 {
                    lease.uploads.insert(
                        format!("symbols_dir/{district}.json"),
                        Upload {
                            sha256: "0".repeat(64),
                            bytes: 0,
                        },
                    );
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(fixture.hub.uploads(id).len(), TEST_ARTIFACT_COUNT);

        assert_eq!(
            put(fixture.port, TOKENS[0], id, "symbols", b"new name"),
            413
        );
        let replacement = b"replacement map";
        assert_eq!(put(fixture.port, TOKENS[0], id, "map", replacement), 200);
        assert_eq!(
            fixture.hub.uploads(id).get("map"),
            Some(&Upload {
                sha256: sha(replacement),
                bytes: replacement.len() as u64,
            })
        );
        assert_eq!(fixture.hub.uploads(id).len(), TEST_ARTIFACT_COUNT);
    }

    #[test]
    fn replacing_an_artifact_different_bytes_frees_its_blob_and_total() {
        const TEST_LEASE_BYTES: u64 = 4 * worker_result::EARLY_MAP_MAX_BYTES;

        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);
        let original = vec![b'o'; 100];
        assert_eq!(put(fixture.port, TOKENS[0], id, "map", &original), 200);
        assert_eq!(put(fixture.port, TOKENS[0], id, "map", b"new"), 200);
        assert_eq!(lease_blob_stats(&fixture, id), (1, 3));
        assert_eq!(
            fixture.hub.uploads(id).get("map"),
            Some(&Upload {
                sha256: sha(b"new"),
                bytes: b"new".len() as u64,
            })
        );

        // The smaller replacement leaves room for one byte exactly at the
        // lease boundary; a stale reservation of the old 100 bytes would
        // make this upload fail.
        fixture
            .hub
            .with_lease(0, id, 1, |lease| {
                lease.uploads.insert(
                    "symbols".to_owned(),
                    Upload {
                        sha256: "f".repeat(64),
                        bytes: TEST_LEASE_BYTES - 4,
                    },
                );
                Ok(())
            })
            .unwrap();
        assert_eq!(put(fixture.port, TOKENS[0], id, "names", b"x"), 200);
    }

    #[test]
    fn an_identical_reupload_keeps_one_blob_and_counts_its_bytes_once() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);
        let bytes = b"same artifact contents";

        assert_eq!(put(fixture.port, TOKENS[0], id, "map", bytes), 200);
        let first = fixture.hub.uploads(id).get("map").cloned().unwrap();
        assert_eq!(put(fixture.port, TOKENS[0], id, "map", bytes), 200);

        assert_eq!(fixture.hub.uploads(id).get("map"), Some(&first));
        assert_eq!(lease_blob_stats(&fixture, id), (1, bytes.len() as u64));
    }

    #[test]
    fn publish_uploaded_map_does_not_publish_an_upload_over_the_cap() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);

        let dir = fixture
            .hub
            .with_lease(0, id, 1, |lease| {
                lease.uploads.insert(
                    "map".to_owned(),
                    Upload {
                        sha256: "a".repeat(64),
                        bytes: worker_result::EARLY_MAP_MAX_BYTES + 1,
                    },
                );
                Ok(lease.dir.clone())
            })
            .unwrap();
        let (tx, _) = tokio::sync::watch::channel(fixture.snapshot(id));

        publish_uploaded_map(&fixture.state.jobs, &fixture.hub, &tx, id, &dir);

        assert!(!fixture.snapshot(id).map_ready);
        assert!(fixture.state.jobs.early_map("test/demo", COMMIT).is_none());
    }

    #[test]
    fn a_short_body_releases_its_lease_byte_reservation() {
        const TEST_LEASE_BYTES: u64 = 4 * worker_result::EARLY_MAP_MAX_BYTES;

        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);
        fixture
            .hub
            .with_lease(0, id, 1, |lease| {
                lease.uploads.insert(
                    "symbols".to_owned(),
                    Upload {
                        sha256: "f".repeat(64),
                        bytes: TEST_LEASE_BYTES - 2,
                    },
                );
                Ok(())
            })
            .unwrap();

        assert_ne!(
            request(
                fixture.port,
                "PUT",
                &format!("/workers/artifacts/{id}/1/names"),
                &[bearer(TOKENS[0]), (SHA256_HEADER, sha(b"x")),],
                b"x",
                Some(2),
            )
            .0,
            200,
            "a body shorter than its declared length must fail"
        );
        assert_eq!(put(fixture.port, TOKENS[0], id, "names", b"xy"), 200);
    }

    #[test]
    fn an_uploaded_map_over_256_mib_is_not_opened_early_or_served() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        let text = agent.recv_text();
        assert!(
            matches!(
                serde_json::from_str::<MasterMessage>(&text).unwrap(),
                MasterMessage::Assign { .. }
            ),
            "expected assign: {text}"
        );
        agent.event(
            id,
            WorkerEvent::StageStarted {
                v: 1,
                stage: StageId::WriteMap,
            },
        );
        fixture.wait_for(id, "indexing", |snapshot| {
            snapshot.status == JobStatus::Indexing
        });

        // Do not send the body: the service must reject this declaration
        // before polling the stream, rather than start a 256 MiB write.
        let uploaded_bytes = worker_result::EARLY_MAP_MAX_BYTES + 1;
        assert_eq!(
            put_declared_without_body(fixture.port, TOKENS[0], id, "map", uploaded_bytes,).0,
            413
        );
        assert!(fixture.hub.uploads(id).is_empty());
        agent.event(
            id,
            WorkerEvent::StageFinished {
                v: 1,
                stage: StageId::WriteMap,
                duration_s: 0.1,
                success: true,
            },
        );
        agent.event(
            id,
            WorkerEvent::Log {
                v: 1,
                message: "after early map attempt".to_owned(),
            },
        );
        fixture.wait_for(id, "early map attempt", |snapshot| {
            snapshot.stage == "after early map attempt"
        });
        assert!(!fixture.snapshot(id).map_ready);
        assert!(fixture.state.jobs.early_map("test/demo", COMMIT).is_none());
        assert_eq!(
            request(
                fixture.port,
                "GET",
                &format!("/api/maps/test/demo?commit={COMMIT}"),
                &[],
                &[],
                None,
            )
            .0,
            404
        );
    }

    /// §4.2, §9: another agent's token, no token, the wrong epoch, a bad
    /// name, a wrong digest, a missing digest, a short body -- each refused
    /// and none recorded; then a result whose listed size disagrees with
    /// the upload is rejected and never reaches the store.
    #[test]
    fn artifact_requests_outside_the_lease_or_with_bad_digests_are_refused() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut holder = fixture.agent(0);
        let _other = fixture.agent(1);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(holder.assigned(), id);
        let path = |epoch: u64, name: &str| format!("/workers/artifacts/{id}/{epoch}/{name}");
        let good = (SHA256_HEADER, sha(MAP));
        let put_raw = |path: &str, headers: &[(&str, String)], length: Option<usize>| {
            request(fixture.port, "PUT", path, headers, MAP, length).0
        };
        assert_eq!(
            put_raw(&path(1, "map"), &[bearer(TOKENS[1]), good.clone()], None),
            403
        );
        assert_eq!(put_raw(&path(1, "map"), &[good.clone()], None), 401);
        assert_eq!(
            put_raw(&path(2, "map"), &[bearer(TOKENS[0]), good.clone()], None),
            403
        );
        for name in [
            "../x",
            "symbols_dir/a.json",
            "symbols_dir/../map",
            "inputs/names",
            "map/",
        ] {
            assert_eq!(
                put_raw(&path(1, name), &[bearer(TOKENS[0]), good.clone()], None),
                400,
                "{name}"
            );
        }
        assert_eq!(
            put_raw(
                &path(1, "map"),
                &[bearer(TOKENS[0]), (SHA256_HEADER, sha(b"other bytes"))],
                None
            ),
            400
        );
        assert_eq!(put_raw(&path(1, "map"), &[bearer(TOKENS[0])], None), 400);
        assert_ne!(
            put_raw(
                &path(1, "map"),
                &[bearer(TOKENS[0]), good.clone()],
                Some(MAP.len() + 10)
            ),
            200
        );
        assert!(
            fixture.hub.uploads(id).is_empty(),
            "a refused upload was recorded"
        );

        assert_eq!(put(fixture.port, TOKENS[0], id, "map", MAP), 200);
        assert_eq!(put(fixture.port, TOKENS[0], id, "symbols", SYMBOLS), 200);
        let mut wrong_size = artifact("map", MAP);
        wrong_size.bytes += 1;
        holder.event(
            id,
            result_event(vec![wrong_size, artifact("symbols", SYMBOLS)]),
        );
        match holder.recv() {
            MasterMessage::ResultRejected { reason, .. } => {
                assert!(reason.contains("SHA-256 and size"), "{reason}")
            }
            other => panic!("expected result_rejected, got {other:?}"),
        }
        let snapshot = fixture.snapshot(id);
        assert_eq!(snapshot.status, JobStatus::Failed);
        assert_eq!(
            snapshot.error_code.as_deref(),
            Some("invalid_worker_result")
        );
        fixture.nothing_stored("test/demo");
    }

    /// §3.4: a remote result that names a filesystem path is refused, even
    /// with its artifacts uploaded, and nothing reaches the store.
    #[test]
    fn a_result_naming_a_path_is_rejected_and_never_stored() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);
        let artifacts = upload_all(fixture.port, id);
        let WorkerEvent::Result {
            v,
            symbols_path,
            symbols_dir,
            names_cache,
            commit,
            branch,
            lang,
            files,
            districts,
            modularity,
            ..
        } = result_event(Vec::new())
        else {
            unreachable!()
        };
        agent.event(
            id,
            WorkerEvent::Result {
                v,
                map_path: fixture.state.config.db_path.to_string_lossy().into_owned(),
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
            },
        );
        match agent.recv() {
            MasterMessage::ResultRejected { reason, .. } => {
                assert!(reason.contains("never paths"), "{reason}")
            }
            other => panic!("expected result_rejected, got {other:?}"),
        }
        let snapshot = fixture.snapshot(id);
        assert_eq!(
            snapshot.error_code.as_deref(),
            Some("invalid_worker_result")
        );
        fixture.nothing_stored("test/demo");
    }

    /// §5.4, #97 phase 2: a result reporting a commit other than the one the
    /// job was admitted against is refused -- checked before artifacts are
    /// even looked at, so no upload is needed to exercise it -- even though
    /// it is a well-formed object id: the master checks against its own
    /// admission, never the agent's say-so (`check_result`'s doc comment).
    #[test]
    fn a_result_with_the_wrong_commit_is_rejected_and_never_stored() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);
        let WorkerEvent::Result {
            v,
            map_path,
            symbols_path,
            symbols_dir,
            names_cache,
            branch,
            lang,
            files,
            districts,
            modularity,
            ..
        } = result_event(Vec::new())
        else {
            unreachable!()
        };
        agent.event(
            id,
            WorkerEvent::Result {
                v,
                map_path,
                symbols_path,
                symbols_dir,
                names_cache,
                commit: "f".repeat(40),
                branch,
                lang,
                files,
                districts,
                modularity,
                artifacts: Vec::new(),
            },
        );
        match agent.recv() {
            MasterMessage::ResultRejected { reason, .. } => {
                assert!(reason.contains("admitted"), "{reason}")
            }
            other => panic!("expected result_rejected, got {other:?}"),
        }
        let snapshot = fixture.snapshot(id);
        assert_eq!(
            snapshot.error_code.as_deref(),
            Some("invalid_worker_result")
        );
        fixture.nothing_stored("test/demo");
    }

    /// §4.3, §3.6: an over-sized frame, an unknown type, a binary frame and
    /// a first frame that is not hello each close the channel with `error`.
    #[test]
    fn oversized_unknown_and_binary_frames_close_the_channel_with_an_error() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut big = fixture.agent(0);
        big.send_text("x".repeat(MAX_CONTROL_FRAME_BYTES + 1));
        big.expect_error("frame_too_large");
        let mut unknown = fixture.agent(1);
        unknown.send_text(r#"{"type":"bogus"}"#.to_owned());
        unknown.expect_error("protocol_error");
        let mut binary = fixture.agent(0);
        binary
            .socket
            .send(tungstenite::Message::binary(vec![1u8, 2, 3]))
            .unwrap();
        binary.expect_error("protocol_error");
        let mut early = FakeAgent::raw(fixture.port, TOKENS[0]);
        early.send(&WorkerMessage::Ready { slots_free: 1 });
        early.expect_error("protocol_error");
        let mut twice = fixture.agent(1);
        twice.send(&hello(test_build(), (PROTO, PROTO)));
        twice.expect_error("protocol_error");
    }

    #[test]
    fn a_proto_mismatch_is_refused_with_unsupported_proto() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = FakeAgent::raw(fixture.port, TOKENS[0]);
        agent.send(&hello(test_build(), (PROTO + 1, PROTO + 2)));
        agent.expect_error("unsupported_proto");
    }

    /// §3.6: an agent of another build stays connected and is never
    /// assigned work; an agent of this build then takes the job.
    #[test]
    fn a_build_mismatch_leaves_the_agent_idle() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut stranger = FakeAgent::join(
            fixture.port,
            TOKENS[0],
            WorkerBuild {
                commit: "another-build".to_owned(),
                ..test_build()
            },
        );
        let id = fixture.spawn(remote_repo("demo"));
        assert!(
            stranger.recv_within(Duration::from_millis(1500)).is_none(),
            "an agent of another build was sent work"
        );
        assert_eq!(fixture.snapshot(id).status, JobStatus::Queued);
        let mut matching = fixture.agent(1);
        assert_eq!(matching.assigned(), id);
        assert!(stranger.recv_within(Duration::from_millis(300)).is_none());
    }

    /// §3.6: `"unknown"` (build.rs found neither `TOLMAP_BUILD_COMMIT` nor a
    /// git checkout) matches nothing, including another `"unknown"` with an
    /// otherwise identical build -- a mismatch fixed by #153 for a build
    /// that could hash its own binary must stay fixed for one that cannot
    /// name its commit at all.
    #[test]
    fn an_unknown_commit_never_matches_even_another_unknown() {
        let left = WorkerBuild {
            commit: "unknown".to_owned(),
            ..test_build()
        };
        let right = WorkerBuild {
            commit: "unknown".to_owned(),
            ..test_build()
        };
        assert!(!same_build(&left, &right));
        assert!(!same_build(&left, &left.clone()));
        assert!(same_build(&test_build(), &test_build()));
    }

    /// §6 "cancel races", before `assign`: the job never leaves the queue.
    #[test]
    fn a_job_cancelled_before_assign_is_never_assigned() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let id = fixture.spawn(remote_repo("demo"));
        std::thread::sleep(Duration::from_millis(300));
        let snapshot = fixture.state.jobs.cancel(id).unwrap();
        assert_eq!(snapshot.error_code.as_deref(), Some("cancelled"));
        let mut agent = fixture.agent(0);
        assert!(agent.recv_within(Duration::from_millis(1200)).is_none());
        assert_eq!(fixture.leases(), 0);
    }

    /// After `assign`, before the first event: the cancel is terminal at
    /// once, `cancel` follows `assign` on the channel, and the agent counts
    /// as busy until it releases the job (§2.4).
    #[test]
    fn a_job_cancelled_after_assign_holds_the_agent_until_released() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);
        let snapshot = fixture.state.jobs.cancel(id).unwrap();
        assert_eq!(snapshot.status, JobStatus::Failed);
        assert_eq!(snapshot.error_code.as_deref(), Some("cancelled"));
        match agent.recv() {
            MasterMessage::Cancel { job_id, reason, .. } => {
                assert_eq!(job_id, id.to_string());
                assert_eq!(reason, CancelReason::Cancelled);
            }
            other => panic!("expected cancel, got {other:?}"),
        }
        let second = fixture.spawn(remote_repo("second"));
        assert!(
            agent.recv_within(Duration::from_millis(700)).is_none(),
            "work was assigned to an agent that had not released its cancelled job"
        );
        agent.send(&WorkerMessage::Released {
            job_id: id.to_string(),
            epoch: 1,
            reason: ReleasedReason::Cancelled,
            peak_rss_bytes: Some(1 << 20),
        });
        agent.send(&WorkerMessage::Ready { slots_free: 1 });
        assert_eq!(agent.assigned(), second);
        assert_eq!(
            fixture.snapshot(id).error_code.as_deref(),
            Some("cancelled")
        );
    }

    /// Mid-run: events after the cancel are dropped, uploads are refused,
    /// and a result that arrives afterwards gets `result_rejected`.
    #[test]
    fn a_result_after_a_mid_run_cancel_is_rejected() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);
        agent.event(
            id,
            WorkerEvent::StageStarted {
                v: 1,
                stage: StageId::Clone,
            },
        );
        agent.event(
            id,
            WorkerEvent::StageFinished {
                v: 1,
                stage: StageId::Clone,
                duration_s: 0.1,
                success: true,
            },
        );
        agent.event(
            id,
            WorkerEvent::StageStarted {
                v: 1,
                stage: StageId::Parse,
            },
        );
        fixture.wait_for(id, "indexing", |s| s.status == JobStatus::Indexing);
        fixture.state.jobs.cancel(id).unwrap();
        assert!(matches!(agent.recv(), MasterMessage::Cancel { .. }));
        assert_eq!(put(fixture.port, TOKENS[0], id, "map", MAP), 403);
        agent.event(
            id,
            WorkerEvent::Log {
                v: 1,
                message: "after the cancel".to_owned(),
            },
        );
        agent.event(id, result_event(vec![artifact("map", MAP)]));
        match agent.recv() {
            MasterMessage::ResultRejected { job_id, reason, .. } => {
                assert_eq!(job_id, id.to_string());
                assert_eq!(reason, "cancelled");
            }
            other => panic!("expected result_rejected, got {other:?}"),
        }
        agent.send(&WorkerMessage::Released {
            job_id: id.to_string(),
            epoch: 1,
            reason: ReleasedReason::Cancelled,
            peak_rss_bytes: None,
        });
        fixture.wait_for_no_lease();
        let snapshot = fixture.snapshot(id);
        assert_eq!(snapshot.error_code.as_deref(), Some("cancelled"));
        assert_ne!(snapshot.stage, "after the cancel");
        fixture.nothing_stored("test/demo");
    }

    fn clone_started(agent: &mut FakeAgent, id: Uuid) {
        agent.event(
            id,
            WorkerEvent::StageStarted {
                v: 1,
                stage: StageId::Clone,
            },
        );
    }

    /// §6 "worker host dies" (phase 2): a lost agent's channel only
    /// detaches its lease; when the lease runs out the job goes back to the
    /// head of its queue as `queued`, its stages reset and `stage` saying
    /// why, one lost worker counted, and it runs again on the other agent
    /// at a new epoch and ends `done`.
    #[test]
    fn a_lost_agent_lets_its_lease_run_out_and_the_job_reruns_on_the_other() {
        let fixture = Fixture::new(Duration::from_secs(1), test_build());
        let mut first = fixture.agent(0);
        let mut second = fixture.agent(1);
        let id = fixture.spawn(remote_repo("one"));
        assert_eq!(first.assigned(), id);
        assert_eq!(first.epoch, 1);
        clone_started(&mut first, id);
        fixture.wait_for(id, "cloning", |s| s.status == JobStatus::Cloning);
        fixture.wait_for_row(id, "running", |row| row.status == "running");
        drop(first);
        // Not failed at once: the lease still runs.
        std::thread::sleep(Duration::from_millis(300));
        assert_ne!(fixture.snapshot(id).status, JobStatus::Failed);
        assert_eq!(second.assigned(), id);
        assert_eq!(
            second.epoch, 2,
            "a re-queued job is assigned at a new epoch"
        );
        let snapshot = fixture.snapshot(id);
        assert_eq!(snapshot.status, JobStatus::Queued, "{snapshot:?}");
        assert_eq!(snapshot.stage, WORKER_LOST);
        assert!(snapshot
            .stages
            .iter()
            .all(|stage| stage.state == jobs::StageState::Pending));
        let row = fixture.wait_for_row(id, "leased again", |row| row.status == "leased");
        assert_eq!((row.attempt, row.epoch), (2, 2), "{row:?}");
        second.deliver(fixture.port, TOKENS[1], id);
        assert!(matches!(
            second.recv(),
            MasterMessage::ResultAccepted { .. }
        ));
        fixture.wait_for(id, "done", |s| s.status == JobStatus::Done);
        let row = fixture.wait_for_row(id, "done in the store", |row| row.status == "done");
        assert_eq!(row.attempt, 2);
        assert!(fixture
            .state
            .store
            .get("test/one", COMMIT)
            .unwrap()
            .is_some());
    }

    /// An agent that stays connected but goes quiet loses its lease within
    /// the TTL: its channel is closed, and the job is re-queued (not
    /// failed) for the next agent.
    #[test]
    fn an_expired_lease_requeues_the_job_and_closes_the_channel() {
        let fixture = Fixture::new(Duration::from_secs(1), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);
        let assigned_at = Instant::now();
        let row = fixture.wait_for_row(id, "the lease expires", |row| {
            row.status == "queued" && row.attempt == 2
        });
        assert!(assigned_at.elapsed() < Duration::from_secs(5));
        assert!(row.lease_holder.is_none(), "{row:?}");
        let snapshot = fixture.snapshot(id);
        assert_eq!(snapshot.status, JobStatus::Queued);
        assert_eq!(snapshot.stage, WORKER_LOST);
        assert!(agent.closed());
        let mut next = fixture.agent(1);
        assert_eq!(next.assigned(), id);
        assert_eq!(next.epoch, 2);
    }

    /// §10.6: a job whose workers keep dying is retried twice, then fails
    /// `worker_crashed` "lost 3 workers" -- a failure of the machine, never
    /// a refusal at admission.
    #[test]
    fn a_job_whose_workers_keep_dying_fails_past_the_retry_bound() {
        let fixture = Fixture::new(Duration::from_secs(1), test_build());
        let id = fixture.spawn(remote_repo("doomed"));
        for attempt in 1..=3u64 {
            let mut agent = fixture.agent((attempt % 2) as usize);
            assert_eq!(agent.assigned(), id);
            assert_eq!(agent.epoch, attempt);
            clone_started(&mut agent, id);
            drop(agent);
            if attempt < 3 {
                fixture.wait_for_row(id, "re-queued", |row| {
                    row.status == "queued" && row.attempt == attempt as i64 + 1
                });
            }
        }
        let snapshot = fixture.wait_for(id, "the job fails", |s| s.status == JobStatus::Failed);
        assert_eq!(snapshot.error_code.as_deref(), Some("worker_crashed"));
        let message = snapshot.error.clone().unwrap_or_default();
        assert!(message.contains("lost 3 workers"), "{message}");
        let row = fixture.wait_for_row(id, "failed in the store", |row| row.status == "failed");
        assert_eq!(row.attempt, 3);
        fixture.nothing_stored("test/doomed");
    }

    /// §6 "duplicate or stale result": a result for an epoch whose lease
    /// ran out is refused with `result_rejected`, and so is its upload; the
    /// current epoch's result is registered once. A repeat of it with the
    /// same artifacts is acknowledged again with `result_accepted` (#97
    /// phase 2 step 3 reverses #155's refusal, as the spec asks), one with
    /// different artifacts is refused, and neither is registered: only the
    /// current epoch's first bytes reach the store.
    #[test]
    fn stale_epoch_results_are_refused_and_a_repeat_of_the_registered_one_is_acknowledged_again() {
        let fixture = Fixture::new(Duration::from_secs(1), test_build());
        let mut stale = fixture.agent(0);
        let mut current = fixture.agent(1);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(stale.assigned(), id);
        // `stale` goes quiet; its lease runs out and `current` gets epoch 2.
        assert_eq!(current.assigned(), id);
        assert_eq!(current.epoch, 2);
        assert!(
            stale.closed(),
            "the master closes an expired holder's channel"
        );
        // The old holder comes back on a new channel and delivers epoch 1.
        let mut stale = fixture.agent(0);
        assert_eq!(
            put_at(fixture.port, TOKENS[0], id, 1, "map", b"{\"stale\":1}"),
            403
        );
        stale.event_at(id, 1, result_event(vec![artifact("map", b"{\"stale\":1}")]));
        match stale.recv() {
            MasterMessage::ResultRejected { epoch, reason, .. } => {
                assert_eq!(epoch, 1);
                assert!(reason.contains("stale epoch"), "{reason}");
            }
            other => panic!("expected result_rejected, got {other:?}"),
        }
        assert!(fixture
            .state
            .store
            .get("test/demo", COMMIT)
            .unwrap()
            .is_none());
        current.deliver(fixture.port, TOKENS[1], id);
        assert!(matches!(
            current.recv(),
            MasterMessage::ResultAccepted { .. }
        ));
        let again = [
            ("map", MAP),
            ("symbols", SYMBOLS),
            ("symbols_dir/0.json", DISTRICT),
            ("names", NAMES),
        ]
        .iter()
        .map(|(name, body)| artifact(name, body))
        .collect();
        // The same artifacts in another order: the same result.
        let mut again: Vec<Artifact> = again;
        again.reverse();
        current.event(id, result_event(again));
        match current.recv() {
            MasterMessage::ResultAccepted { epoch, .. } => assert_eq!(epoch, 2),
            other => panic!("expected result_accepted again, got {other:?}"),
        }
        current.event(id, result_event(vec![artifact("map", b"{\"other\":1}")]));
        match current.recv() {
            MasterMessage::ResultRejected { reason, .. } => {
                assert!(reason.contains("different result"), "{reason}")
            }
            other => panic!("expected result_rejected, got {other:?}"),
        }
        // The stale epoch stays refused after the job is done.
        stale.event_at(id, 1, result_event(vec![artifact("map", MAP)]));
        assert!(matches!(stale.recv(), MasterMessage::ResultRejected { .. }));
        let row = fixture
            .state
            .store
            .get("test/demo", COMMIT)
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read(&row.map_path).unwrap(), MAP);
        assert_eq!(fixture.snapshot(id).status, JobStatus::Done);
    }

    /// §2.4 in the store: a cancel while leased and one while running are
    /// terminal in the table at once, and a re-queue racing the cancel
    /// never brings the job back.
    #[test]
    fn a_cancel_is_terminal_in_the_store_whatever_state_the_job_is_in() {
        let fixture = Fixture::new(Duration::from_secs(1), test_build());
        let mut agent = fixture.agent(0);
        let leased = fixture.spawn(remote_repo("leased"));
        assert_eq!(agent.assigned(), leased);
        fixture.wait_for_row(leased, "leased", |row| row.status == "leased");
        let snapshot = jobs::cancel_job(&fixture.state, leased).unwrap();
        assert_eq!(snapshot.error_code.as_deref(), Some("cancelled"));
        let row = fixture.row(leased);
        assert_eq!(row.status, "failed", "recorded before the answer");
        assert!(matches!(agent.recv(), MasterMessage::Cancel { .. }));
        // The agent never releases it; the lease runs out and the job
        // stays cancelled rather than going back to the queue.
        std::thread::sleep(Duration::from_millis(1800));
        assert_eq!(fixture.row(leased).status, "failed");
        assert_eq!(
            fixture.snapshot(leased).error_code.as_deref(),
            Some("cancelled")
        );

        let mut agent = fixture.agent(1);
        let running = fixture.spawn(remote_repo("running"));
        assert_eq!(agent.assigned(), running);
        clone_started(&mut agent, running);
        fixture.wait_for_row(running, "running", |row| row.status == "running");
        jobs::cancel_job(&fixture.state, running).unwrap();
        assert_eq!(fixture.row(running).status, "failed");
        assert!(matches!(agent.recv(), MasterMessage::Cancel { .. }));
        agent.event(running, result_event(vec![artifact("map", MAP)]));
        assert!(matches!(agent.recv(), MasterMessage::ResultRejected { .. }));
        fixture.nothing_stored("test/running");
        // A repeated cancel answers the same terminal snapshot.
        assert_eq!(
            jobs::cancel_job(&fixture.state, running)
                .unwrap()
                .error_code
                .as_deref(),
            Some("cancelled")
        );
    }

    fn stored_job(
        id: Uuid,
        name: &str,
        status: &str,
        order: i64,
        epoch: i64,
    ) -> crate::service::store::JobRow {
        let slug = format!("test/{name}");
        let spec = JobSpec {
            slug: slug.clone(),
            owner: "test".to_owned(),
            repo: name.to_owned(),
            source: "https://example.invalid/test.git".to_owned(),
            local: false,
            commit: COMMIT.to_owned(),
            all_sources: false,
            prune_variant: "node-relative".to_owned(),
            namer: "idf".to_owned(),
            namer_model: String::new(),
            refs: Some("hand".to_owned()),
            install: None,
        };
        let public = match status {
            "running" => JobStatus::Indexing,
            "done" => JobStatus::Done,
            _ => JobStatus::Queued,
        };
        let snapshot = JobSnapshot {
            job_id: id,
            slug: slug.clone(),
            commit: Some(COMMIT.to_owned()),
            status: public,
            stage: format!("{status} before the restart"),
            queue_position: None,
            started_at: "2026-09-27T00:00:00Z".to_owned(),
            finished_at: None,
            error: None,
            error_code: None,
            progress: None,
            eta: None,
            eta_start_s: None,
            elapsed_s: 1.0,
            stages: StageId::ALL
                .iter()
                .map(|&stage| jobs::StageSnapshot {
                    id: stage,
                    label: stage.label().to_owned(),
                    state: jobs::StageState::Pending,
                    started_at: None,
                    duration_s: None,
                })
                .collect(),
            map_ready: false,
        };
        crate::service::store::JobRow {
            job_id: id.to_string(),
            slug,
            commit: COMMIT.to_owned(),
            spec_json: serde_json::to_string(&spec).unwrap(),
            class: 0,
            status: status.to_owned(),
            attempt: 1,
            epoch,
            lease_holder: None,
            lease_deadline_ms: None,
            queue_order: order,
            snapshot_json: serde_json::to_string(&snapshot).unwrap(),
        }
    }

    /// §6 "master restarts mid-job", in process: a master started on a
    /// store a previous one left behind reloads its jobs before serving
    /// anyone. The finished job answers from the store; the running one
    /// keeps its snapshot and its slot for one TTL, then goes back to the
    /// head of the queue without counting an attempt; the queued ones keep
    /// their order and positions; and they then run in that order.
    #[test]
    fn a_restarted_master_reloads_its_jobs_and_reruns_a_running_one_uncounted() {
        let dir = tempfile::tempdir().unwrap();
        let (done, running, first, second) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        {
            let store = Store::open(&dir.path().join("store.sqlite3")).unwrap();
            store
                .insert_job(&stored_job(done, "done", "done", 1, 1))
                .unwrap();
            store
                .insert_job(&stored_job(running, "running", "running", 2, 1))
                .unwrap();
            store
                .insert_job(&stored_job(second, "second", "queued", 4, 0))
                .unwrap();
            store
                .insert_job(&stored_job(first, "first", "queued", 3, 0))
                .unwrap();
        }
        let fixture = Fixture::with(dir, Duration::from_secs(1), test_build(), 1);
        let finished = jobs::persisted_snapshot(&fixture.state, done).expect("the finished job");
        assert_eq!(finished.status, JobStatus::Done);
        assert!(fixture.state.jobs.subscribe(done).is_none());
        assert_eq!(fixture.snapshot(running).status, JobStatus::Indexing);
        assert_eq!(fixture.snapshot(first).queue_position, Some(1));
        assert_eq!(fixture.snapshot(second).queue_position, Some(2));
        let (a, b) = (
            fixture.snapshot(first).eta_start_s.unwrap(),
            fixture.snapshot(second).eta_start_s.unwrap(),
        );
        assert!(a <= b, "{a} then {b}");
        // The dedup key is rebuilt: the same repository and commit is the
        // same job.
        assert_eq!(fixture.spawn(remote_repo("first")), first);

        let snapshot = fixture.wait_for(running, "re-queued after the restart", |s| {
            s.status == JobStatus::Queued
        });
        assert_eq!(snapshot.stage, jobs::RESTARTED);
        let row = fixture.row(running);
        assert_eq!(
            (row.attempt, row.status.as_str()),
            (1, "queued"),
            "a restart is not a lost worker"
        );
        let mut agent = fixture.agent(0);
        for (expected, name) in [(running, "running"), (first, "first"), (second, "second")] {
            assert_eq!(agent.assigned(), expected, "{name} next");
            agent.deliver(fixture.port, TOKENS[0], expected);
            assert!(matches!(agent.recv(), MasterMessage::ResultAccepted { .. }));
            agent.send(&WorkerMessage::Ready { slots_free: 1 });
            fixture.wait_for_row(expected, "done", |row| row.status == "done");
        }
        assert_eq!(fixture.row(running).epoch, 2);
    }

    /// Heartbeats renew the lease (§2.2), so a quiet job outlives its TTL.
    #[test]
    fn heartbeats_keep_a_lease_past_its_ttl() {
        let fixture = Fixture::new(Duration::from_secs(1), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(300));
            agent.send(&WorkerMessage::Heartbeat {
                jobs: vec![HeartbeatJob {
                    job_id: id.to_string(),
                    epoch: 1,
                    last_seq: 0,
                }],
                rss_bytes: None,
            });
            match agent.recv() {
                MasterMessage::LeaseRenewed { acked_seq, .. } => assert_eq!(acked_seq, 0),
                other => panic!("expected lease_renewed, got {other:?}"),
            }
        }
        assert_eq!(fixture.snapshot(id).status, JobStatus::Queued);
        assert_eq!(fixture.leases(), 1);
    }

    /// An agent that exits is started again, with backoff, until the
    /// supervisor is stopped.
    #[test]
    fn the_supervisor_restarts_an_agent_that_exits_until_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("starts");
        let command: AgentCommand = {
            let log = log.clone();
            Arc::new(move |agent: usize| {
                let mut command = Command::new("sh");
                command
                    .arg("-c")
                    .arg(format!("echo {agent} >> '{}'; exit 3", log.display()));
                command
            })
        };
        let starts = || {
            std::fs::read_to_string(&log)
                .map(|text| text.lines().count())
                .unwrap_or(0)
        };
        let supervisor = Supervisor::start(1, command);
        let deadline = Instant::now() + Duration::from_secs(8);
        while starts() < 2 {
            assert!(Instant::now() < deadline, "the agent was not restarted");
            std::thread::sleep(Duration::from_millis(50));
        }
        supervisor.reap(Duration::from_secs(2));
        let after = starts();
        std::thread::sleep(Duration::from_millis(2500));
        assert_eq!(starts(), after, "a stopped supervisor restarted an agent");
    }

    /// An in-process TCP relay between an agent and the master (§9 "failure
    /// injection"): it forwards bytes both ways and can pause them (both
    /// directions held, the TCP connections kept open), cut every
    /// connection it carries, or refuse new ones (accept, then close at
    /// once), which with a cut makes a drop last as long as the test wants.
    struct Relay {
        port: u16,
        control: Arc<RelayControl>,
    }

    #[derive(Default)]
    struct RelayControl {
        paused: AtomicBool,
        /// Raised by `cut`: every connection opened before it closes.
        cuts: std::sync::atomic::AtomicU64,
        refusing: AtomicBool,
        stopped: AtomicBool,
    }

    impl Relay {
        fn start(target: u16) -> Relay {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let control = Arc::new(RelayControl::default());
            let shared = control.clone();
            std::thread::spawn(move || {
                for inbound in listener.incoming() {
                    if shared.stopped.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(inbound) = inbound else { continue };
                    if shared.refusing.load(Ordering::SeqCst) {
                        drop(inbound);
                        continue;
                    }
                    let Ok(outbound) = TcpStream::connect(("127.0.0.1", target)) else {
                        continue;
                    };
                    let generation = shared.cuts.load(Ordering::SeqCst);
                    for (from, to) in [
                        (inbound.try_clone().unwrap(), outbound.try_clone().unwrap()),
                        (outbound, inbound),
                    ] {
                        let shared = shared.clone();
                        std::thread::spawn(move || pump(from, to, &shared, generation));
                    }
                }
            });
            Relay { port, control }
        }

        fn url(&self) -> String {
            format!("ws://127.0.0.1:{}/workers/connect", self.port)
        }

        fn pause(&self) {
            self.control.paused.store(true, Ordering::SeqCst);
        }

        fn resume(&self) {
            self.control.paused.store(false, Ordering::SeqCst);
        }

        fn cut(&self) {
            self.control.cuts.fetch_add(1, Ordering::SeqCst);
        }

        fn refuse(&self, refusing: bool) {
            self.control.refusing.store(refusing, Ordering::SeqCst);
        }
    }

    impl Drop for Relay {
        fn drop(&mut self) {
            self.control.stopped.store(true, Ordering::SeqCst);
            self.cut();
            let _ = TcpStream::connect(("127.0.0.1", self.port));
        }
    }

    fn pump(mut from: TcpStream, mut to: TcpStream, control: &RelayControl, generation: u64) {
        let _ = from.set_read_timeout(Some(Duration::from_millis(20)));
        let mut buffer = [0u8; 16 * 1024];
        loop {
            if control.cuts.load(Ordering::SeqCst) != generation {
                let _ = from.shutdown(std::net::Shutdown::Both);
                let _ = to.shutdown(std::net::Shutdown::Both);
                return;
            }
            if control.paused.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            match from.read(&mut buffer) {
                Ok(0) => {
                    let _ = to.shutdown(std::net::Shutdown::Write);
                    return;
                }
                Ok(count) => {
                    if to.write_all(&buffer[..count]).is_err() {
                        return;
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => {
                    let _ = to.shutdown(std::net::Shutdown::Both);
                    return;
                }
            }
        }
    }

    /// A one-commit git repository at `root/demo` and a scripted job child
    /// that reports one stage and then sleeps, writing its spec and pid.
    fn scripted_job(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let repo = root.join("demo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
        for args in [
            &["init", "-q"][..],
            &["add", "-A"][..],
            &["commit", "-qm", "initial"][..],
        ] {
            assert!(std::process::Command::new("git")
                .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success());
        }
        let spec = root.join("spec.json");
        let pid_file = root.join("child.pid");
        let worker = root.join("worker.sh");
        std::fs::write(
            &worker,
            format!(
                "#!/bin/sh\ncat > '{}'\nsleep 30 &\necho $! > '{}'\nprintf '%s\\n' '{{\"type\":\"stage_started\",\"v\":1,\"stage\":\"parse\"}}'\nwait\n",
                spec.display(),
                pid_file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755)).unwrap();
        (repo, worker, pid_file)
    }

    /// `repo`'s HEAD, the commit a job on it is admitted against.
    fn head_of(repo: &Path) -> String {
        let head = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap();
        String::from_utf8(head.stdout).unwrap().trim().to_owned()
    }

    fn local_demo(repo: &Path) -> RepoRef {
        RepoRef {
            slug: "local/demo".to_owned(),
            owner: "local".to_owned(),
            repo: "demo".to_owned(),
            source: RepoSource::Local(repo.to_path_buf()),
        }
    }

    /// The pid a scripted job child wrote, once it has.
    #[cfg(target_os = "linux")]
    fn child_pid(pid_file: &Path) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(pid) = std::fs::read_to_string(pid_file)
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "the job child never started");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Whether `pid` still runs (a zombie does not).
    #[cfg(target_os = "linux")]
    fn running(pid: i32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.split(") ")
                .nth(1)
                .is_some_and(|tail| !tail.starts_with('Z'))
        })
    }

    #[cfg(target_os = "linux")]
    fn wait_until_gone(pid: i32, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while running(pid) {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for_ready_agent(fixture: &Fixture, token: usize) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !fixture
            .hub
            .lock()
            .conns
            .values()
            .any(|conn| conn.agent == token && conn.ready)
        {
            assert!(
                Instant::now() < deadline,
                "agent {token} never became ready"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The real agent (`agent::run`, in a thread, with token 0 and `worker`
    /// as its job child) dialling the fixture's relay, once it is ready.
    fn relayed_agent(
        fixture: &Fixture,
        worker: PathBuf,
    ) -> std::thread::JoinHandle<anyhow::Result<()>> {
        let root = fixture.dir.path();
        let token_file = root.join("agent.token");
        std::fs::write(&token_file, TOKENS[0]).unwrap();
        let config = crate::service::agent::AgentConfig {
            connect: fixture.relay.as_ref().unwrap().url(),
            token_file,
            cache_dir: root.join("agent"),
            worker_exe: worker,
            class_memory: None,
            memory_events: None,
        };
        let agent = std::thread::spawn(move || crate::service::agent::run(config));
        wait_for_ready_agent(fixture, 0);
        agent
    }

    /// A one-commit repository at `root/demo`, and a job child that reports
    /// `parse` progress 1..=`steps`, one every 0.1 s, and then -- once the
    /// file `gate` exists, when one is named -- writes `MAP` and its
    /// siblings into its output directory and reports a `result` for them,
    /// as the real child would. Each start appends the child's pid to
    /// `root/starts`.
    fn resulting_job(root: &Path, steps: u64, gate: Option<&Path>) -> (PathBuf, String, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let (repo, _, _) = scripted_job(root);
        let head = head_of(&repo);
        let wait = gate.map_or(String::new(), |gate| {
            format!("while [ ! -f '{}' ]; do sleep 0.02; done\n", gate.display())
        });
        let script = r##"#!/bin/sh
spec=$(cat)
out=$(printf '%s' "$spec" | sed -n 's/.*"output_dir":"\([^"]*\)".*/\1/p')
echo $$ >> '@STARTS@'
printf '%s\n' '{"type":"stage_started","v":1,"stage":"parse"}'
i=1
while [ $i -le @STEPS@ ]; do
  printf '{"type":"progress","v":1,"value":{"stage":"parse","stage_index":6,"stage_count":22,"label":"Parsing files","unit":"files","done":%d,"total":@STEPS@,"rate_per_s":null}}\n' $i
  sleep 0.1
  i=$((i+1))
done
printf '%s\n' '{"type":"stage_finished","v":1,"stage":"parse","duration_s":1.5,"success":true}'
@WAIT@mkdir -p "$out/demo.symbols"
printf '%s' '@MAP@' > "$out/demo.json"
printf '%s' '@SYMBOLS@' > "$out/demo.symbols.json"
printf '%s' '@DISTRICT@' > "$out/demo.symbols/0.json"
printf '%s' '@NAMES@' > "$out/demo.names.json"
printf '{"type":"result","v":1,"map_path":"%s/demo.json","symbols_path":"%s/demo.symbols.json","symbols_dir":"%s/demo.symbols","names_cache":"%s/demo.names.json","commit":"@HEAD@","branch":"main","lang":"py","files":1,"districts":1,"modularity":0.5}\n' "$out" "$out" "$out" "$out"
"##
        .replace("@STARTS@", &root.join("starts").display().to_string())
        .replace("@STEPS@", &steps.to_string())
        .replace("@WAIT@", &wait)
        .replace("@MAP@", std::str::from_utf8(MAP).unwrap())
        .replace("@SYMBOLS@", std::str::from_utf8(SYMBOLS).unwrap())
        .replace("@DISTRICT@", std::str::from_utf8(DISTRICT).unwrap())
        .replace("@NAMES@", std::str::from_utf8(NAMES).unwrap())
        .replace("@HEAD@", &head);
        let worker = root.join("resulting-worker.sh");
        std::fs::write(&worker, script).unwrap();
        std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755)).unwrap();
        (repo, head, worker)
    }

    /// How many times a `resulting_job` child started.
    fn starts(root: &Path) -> usize {
        std::fs::read_to_string(root.join("starts"))
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    fn stored_map(fixture: &Fixture, slug: &str, commit: &str) -> Vec<u8> {
        let row = fixture
            .state
            .store
            .get(slug, commit)
            .unwrap()
            .expect("a map row");
        std::fs::read(&row.map_path).unwrap()
    }

    /// §6 "channel lost, worker alive", §9 "drop the channel for less than
    /// the TTL", with the real agent behind the relay: the channel is cut
    /// mid-stage and the agent resumes. The job is not re-run (one child
    /// start, epoch 1, attempt 1, no other agent asked), its progress over
    /// SSE never decreases and runs on to the child's last value across the
    /// cut, the stage the child finished while the channel was down is
    /// applied once, and the stored map is the child's bytes exactly.
    #[test]
    fn a_channel_cut_for_less_than_the_lease_resumes_without_a_rerun() {
        use axum::http::Request;
        use tower::ServiceExt;
        const STEPS: u64 = 30;
        let fixture = Fixture::relayed(Duration::from_secs(5), own_build());
        let root = fixture.dir.path().to_path_buf();
        let (repo, head, worker) = resulting_job(&root, STEPS, None);
        let agent = relayed_agent(&fixture, worker);
        let mut other = FakeAgent::join(fixture.port, TOKENS[1], own_build());
        let id = fixture.spawn_at(local_demo(&repo), &head);
        let runtime = fixture.runtime.as_ref().unwrap();
        let router = crate::service::http::router(fixture.state.clone());
        let events = runtime.spawn(async move {
            let request = Request::builder()
                .uri(format!("/api/jobs/{id}/events"))
                .body(Body::empty())
                .unwrap();
            let response = router.oneshot(request).await.unwrap();
            axum::body::to_bytes(response.into_body(), 16 << 20)
                .await
                .unwrap()
        });
        let before = fixture.wait_for(id, "parse progress under way", |s| {
            s.progress
                .as_ref()
                .is_some_and(|progress| progress.done >= 5)
        });
        let before = before.progress.unwrap().done;
        fixture.relay.as_ref().unwrap().cut();
        let done = fixture.wait_for(id, "the job ends", jobs::is_terminal);
        assert_eq!(done.status, JobStatus::Done, "{done:?}");
        let row = fixture.wait_for_row(id, "done in the store", |row| row.status == "done");
        assert_eq!((row.epoch, row.attempt), (1, 1), "{row:?}");
        assert_eq!(starts(&root), 1, "the job child ran again");
        let parse = &done.stages[StageId::Parse.index() - 1];
        assert_eq!(parse.state, jobs::StageState::Done);
        assert_eq!(parse.duration_s, Some(1.5), "the stage was applied once");
        assert_eq!(stored_map(&fixture, "local/demo", &head), MAP);
        let body = runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(10), events).await })
            .expect("the SSE stream ends with the job")
            .unwrap();
        let frames = String::from_utf8(body.to_vec()).unwrap();
        let seen: Vec<u64> = frames
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|value| value["progress"]["stage"] == "parse")
            .filter_map(|value| value["progress"]["done"].as_u64())
            .collect();
        assert!(seen.windows(2).all(|pair| pair[0] <= pair[1]), "{seen:?}");
        assert!(seen.iter().any(|done| *done <= before), "{seen:?}");
        assert_eq!(seen.last().copied(), Some(STEPS), "{seen:?}");
        assert!(
            other.recv_within(Duration::from_millis(100)).is_none(),
            "the job went to another agent"
        );
        assert!(!agent.is_finished(), "a lost channel ended the agent");
        fixture.hub.shutdown_now();
        let outcome = agent.join().unwrap();
        assert!(outcome.is_ok(), "{outcome:?}");
    }

    /// §6 "channel lost past the lease TTL", §2.5's second branch, with the
    /// real agent behind the relay. First a stall shorter than the lease
    /// changes nothing: the agent's heartbeats arrive late, not never. Then
    /// its channel is cut and the relay turns it away for longer than the
    /// lease; its job child keeps running meanwhile. The master re-queues
    /// the job to the other agent at epoch 2, which finishes it; the old
    /// agent comes back naming epoch 1, is answered `cancel` (`lease_lost`),
    /// kills its child and is ready for work again, and nothing of epoch 1
    /// is registered.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_channel_cut_for_longer_than_the_lease_is_cancelled_on_resume() {
        let fixture = Fixture::relayed(Duration::from_secs(3), own_build());
        let root = fixture.dir.path().to_path_buf();
        let (repo, worker, pid_file) = scripted_job(&root);
        let head = head_of(&repo);
        let agent = relayed_agent(&fixture, worker);
        let mut other = FakeAgent::join(fixture.port, TOKENS[1], own_build());
        let id = fixture.spawn_at(local_demo(&repo), &head);
        fixture.wait_for(id, "the child starts parsing", |s| {
            s.status == JobStatus::Indexing
        });
        let child = child_pid(&pid_file);
        let relay = fixture.relay.as_ref().unwrap();
        relay.pause();
        std::thread::sleep(Duration::from_millis(1200));
        relay.resume();
        std::thread::sleep(Duration::from_millis(1500));
        let row = fixture.row(id);
        assert_eq!(
            (row.status.as_str(), row.epoch, row.attempt),
            ("running", 1, 1),
            "{row:?}"
        );
        assert!(other.recv_within(Duration::from_millis(100)).is_none());
        relay.refuse(true);
        relay.cut();
        assert_eq!(other.assigned(), id);
        assert_eq!(other.epoch, 2);
        assert!(running(child), "a lost channel killed the job");
        let artifacts = upload_all_at(fixture.port, TOKENS[1], id, 2);
        other.event(id, result_at(artifacts, &head));
        match other.recv() {
            MasterMessage::ResultAccepted { epoch, .. } => assert_eq!(epoch, 2),
            unexpected => panic!("expected result_accepted, got {unexpected:?}"),
        }
        let row = fixture.wait_for_row(id, "done in the store", |row| row.status == "done");
        assert_eq!((row.epoch, row.attempt), (2, 2), "{row:?}");
        relay.refuse(false);
        wait_until_gone(
            child,
            "the old agent's child outlived the `cancel` of its resume",
        );
        wait_for_ready_agent(&fixture, 0);
        assert_eq!(stored_map(&fixture, "local/demo", &head), MAP);
        assert!(!agent.is_finished(), "a lost channel ended the agent");
        fixture.hub.shutdown_now();
        let outcome = agent.join().unwrap();
        assert!(outcome.is_ok(), "{outcome:?}");
    }

    /// §9 "pause the connection": the relay holds both directions for
    /// longer than the lease, with every connection open. It is the same as
    /// a long drop: the master re-queues the job and closes the old channel,
    /// the agent, hearing nothing for a whole TTL, redials, and once the
    /// relay lets it through is answered `cancel` and kills its child.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_channel_paused_for_longer_than_the_lease_is_the_same_as_a_long_drop() {
        let fixture = Fixture::relayed(Duration::from_secs(3), own_build());
        let root = fixture.dir.path().to_path_buf();
        let (repo, worker, pid_file) = scripted_job(&root);
        let head = head_of(&repo);
        let agent = relayed_agent(&fixture, worker);
        let mut other = FakeAgent::join(fixture.port, TOKENS[1], own_build());
        let id = fixture.spawn_at(local_demo(&repo), &head);
        fixture.wait_for(id, "the child starts parsing", |s| {
            s.status == JobStatus::Indexing
        });
        let child = child_pid(&pid_file);
        let relay = fixture.relay.as_ref().unwrap();
        relay.pause();
        assert_eq!(other.assigned(), id);
        assert_eq!(other.epoch, 2);
        let artifacts = upload_all_at(fixture.port, TOKENS[1], id, 2);
        other.event(id, result_at(artifacts, &head));
        match other.recv() {
            MasterMessage::ResultAccepted { epoch, .. } => assert_eq!(epoch, 2),
            unexpected => panic!("expected result_accepted, got {unexpected:?}"),
        }
        assert!(running(child), "a stalled channel killed the job");
        relay.resume();
        wait_until_gone(
            child,
            "the old agent's child outlived the `cancel` of its resume",
        );
        wait_for_ready_agent(&fixture, 0);
        let row = fixture.row(id);
        assert_eq!(
            (row.status.as_str(), row.epoch, row.attempt),
            ("done", 2, 2),
            "{row:?}"
        );
        assert_eq!(stored_map(&fixture, "local/demo", &head), MAP);
        assert!(!agent.is_finished(), "a stalled channel ended the agent");
        fixture.hub.shutdown_now();
        let outcome = agent.join().unwrap();
        assert!(outcome.is_ok(), "{outcome:?}");
    }

    /// §6 "duplicate or stale result" across a drop, with the real agent:
    /// the channel is cut the moment the agent's uploads are in, so its
    /// `result` is lost in flight or lands just before the cut, and a
    /// verdict, if any, is lost with the channel. Either way the agent keeps
    /// the result, resumes, and sends it again only if the master had not
    /// applied it; the master registers it once, the agent hears a verdict
    /// (it is ready again), and the job is done at epoch 1, attempt 1.
    #[test]
    fn a_result_cut_off_as_it_is_sent_is_registered_once_after_a_resume() {
        let fixture = Fixture::relayed(Duration::from_secs(5), own_build());
        let root = fixture.dir.path().to_path_buf();
        let gate = root.join("go");
        let (repo, head, worker) = resulting_job(&root, 3, Some(&gate));
        let agent = relayed_agent(&fixture, worker);
        let id = fixture.spawn_at(local_demo(&repo), &head);
        fixture.wait_for(id, "parse finished", |s| {
            s.stages[StageId::Parse.index() - 1].state == jobs::StageState::Done
        });
        std::fs::write(&gate, b"").unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while fixture.hub.uploads(id).len() < 4 {
            assert!(
                Instant::now() < deadline,
                "the agent never uploaded its result"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        fixture.relay.as_ref().unwrap().cut();
        let done = fixture.wait_for(id, "the job ends", jobs::is_terminal);
        assert_eq!(done.status, JobStatus::Done, "{done:?}");
        let row = fixture.wait_for_row(id, "done in the store", |row| row.status == "done");
        assert_eq!((row.epoch, row.attempt), (1, 1), "{row:?}");
        assert_eq!(starts(&root), 1, "the job child ran again");
        assert_eq!(stored_map(&fixture, "local/demo", &head), MAP);
        wait_for_ready_agent(&fixture, 0);
        assert!(!agent.is_finished(), "a lost channel ended the agent");
        fixture.hub.shutdown_now();
        let outcome = agent.join().unwrap();
        assert!(outcome.is_ok(), "{outcome:?}");
    }

    /// `result_event` for a job admitted against `commit`.
    fn result_at(artifacts: Vec<Artifact>, commit: &str) -> WorkerEvent {
        let mut event = result_event(artifacts);
        if let WorkerEvent::Result {
            commit: reported, ..
        } = &mut event
        {
            *reported = commit.to_owned();
        }
        event
    }

    fn stage_finished(stage: StageId, duration_s: f64) -> WorkerEvent {
        WorkerEvent::StageFinished {
            v: 1,
            stage,
            duration_s,
            success: true,
        }
    }

    /// §3.5 on the master's side, with scripted agents: a lease whose
    /// channel dropped is resumed within the TTL only by its holder at its
    /// epoch -- another token, a stale epoch and a job nobody holds are
    /// answered `cancel` with `lease_lost` -- and `continue` carries the
    /// master's own `acked_seq`. The lease moves to the new channel, a
    /// replay of what it already applied is ignored (the stage it finished
    /// is not counted twice), and the job ends `done` at epoch 1, attempt 1.
    #[test]
    fn a_dropped_lease_is_resumed_from_the_masters_acked_seq_by_its_holder_only() {
        let fixture = Fixture::new(Duration::from_secs(3), test_build());
        let mut agent = fixture.agent(0);
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);
        clone_started(&mut agent, id);
        agent.event(id, stage_finished(StageId::Clone, 0.5));
        agent.event(
            id,
            WorkerEvent::StageStarted {
                v: 1,
                stage: StageId::Parse,
            },
        );
        agent.event(id, stage_finished(StageId::Parse, 1.0));
        fixture.wait_for(id, "parse finished", |s| {
            s.stages[StageId::Parse.index() - 1].state == jobs::StageState::Done
        });
        drop(agent);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !fixture.hub.detached(id) {
            assert!(
                Instant::now() < deadline,
                "the master never saw the channel close"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let lease_lost = |answers: &[WelcomeResume]| {
            answers.iter().all(|answer| {
                answer.action == ResumeAction::Cancel
                    && answer.reason == Some(CancelReason::LeaseLost)
            })
        };
        let (_, answers) = FakeAgent::rejoin(fixture.port, TOKENS[1], vec![resume_entry(id, 1, 4)]);
        assert!(answers.len() == 1 && lease_lost(&answers), "{answers:?}");
        let (_, answers) = FakeAgent::rejoin(
            fixture.port,
            TOKENS[0],
            vec![resume_entry(id, 2, 4), resume_entry(Uuid::new_v4(), 1, 0)],
        );
        assert!(answers.len() == 2 && lease_lost(&answers), "{answers:?}");
        let (mut back, answers) =
            FakeAgent::rejoin(fixture.port, TOKENS[0], vec![resume_entry(id, 1, 6)]);
        assert_eq!(answers.len(), 1, "{answers:?}");
        assert_eq!(answers[0].action, ResumeAction::Continue);
        assert_eq!(answers[0].acked_seq, 4);
        // The agent replays from what it last knew was acknowledged (2):
        // seqs 3 and 4 are repeats and ignored, the result is seq 5.
        back.epoch = 1;
        back.seq = 2;
        back.event(
            id,
            WorkerEvent::StageStarted {
                v: 1,
                stage: StageId::Parse,
            },
        );
        back.event(id, stage_finished(StageId::Parse, 1.0));
        back.deliver(fixture.port, TOKENS[0], id);
        assert!(matches!(back.recv(), MasterMessage::ResultAccepted { .. }));
        let row = fixture.wait_for_row(id, "done in the store", |row| row.status == "done");
        assert_eq!((row.epoch, row.attempt), (1, 1), "{row:?}");
        let parse = &fixture.snapshot(id).stages[StageId::Parse.index() - 1];
        assert_eq!(
            parse.duration_s,
            Some(1.0),
            "a replayed event was applied twice"
        );
    }

    /// §6 "duplicate or stale result", after a drop, with a scripted agent:
    /// the result was registered but the channel died before the verdict
    /// reached the agent. It resumes naming the job, is answered `continue`,
    /// and hears the verdict again; a repeat of its result is acknowledged
    /// again, once. The job is registered once.
    #[test]
    fn a_verdict_lost_with_the_channel_is_sent_again_on_resume() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let (mut agent, answers) = FakeAgent::rejoin(fixture.port, TOKENS[0], Vec::new());
        assert!(answers.is_empty());
        agent.send(&WorkerMessage::Ready { slots_free: 1 });
        let id = fixture.spawn(remote_repo("demo"));
        assert_eq!(agent.assigned(), id);
        clone_started(&mut agent, id);
        agent.deliver(fixture.port, TOKENS[0], id);
        fixture.wait_for_row(id, "registered", |row| row.status == "done");
        // The verdict is on its way; the channel dies before it is read.
        drop(agent);
        let (mut back, answers) =
            FakeAgent::rejoin(fixture.port, TOKENS[0], vec![resume_entry(id, 1, 2)]);
        assert_eq!(answers.len(), 1, "{answers:?}");
        assert_eq!(answers[0].action, ResumeAction::Continue);
        assert_eq!(answers[0].acked_seq, 2);
        match back.recv() {
            MasterMessage::ResultAccepted { job_id, epoch, .. } => {
                assert_eq!((job_id, epoch), (id.to_string(), 1))
            }
            other => panic!("expected the verdict again, got {other:?}"),
        }
        back.epoch = 1;
        back.seq = 2;
        let again = [
            ("map", MAP),
            ("symbols", SYMBOLS),
            ("symbols_dir/0.json", DISTRICT),
            ("names", NAMES),
        ]
        .iter()
        .map(|(name, body)| artifact(name, body))
        .collect();
        back.event(id, result_event(again));
        assert!(matches!(back.recv(), MasterMessage::ResultAccepted { .. }));
        assert!(back.recv_within(Duration::from_millis(300)).is_none());
        assert_eq!(fixture.snapshot(id).status, JobStatus::Done);
        let row = fixture
            .state
            .store
            .get("test/demo", COMMIT)
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read(&row.map_path).unwrap(), MAP);
    }

    /// §3.6: `hello.resume` is gated on the `resume` feature. An agent that
    /// did not advertise it and names jobs anyway gets a protocol error,
    /// and no channel.
    #[test]
    fn resume_entries_from_an_agent_that_did_not_advertise_resume_are_a_protocol_error() {
        let fixture = Fixture::new(Duration::from_secs(60), test_build());
        let mut agent = FakeAgent::raw(fixture.port, TOKENS[0]);
        agent.send(&hello_with(
            test_build(),
            (PROTO, PROTO),
            &[FEATURE_LOCAL_PATHS],
            vec![resume_entry(Uuid::new_v4(), 1, 0)],
        ));
        agent.expect_error("protocol_error");
        assert_eq!(fixture.hub.lock().conns.len(), 0);
    }

    /// The real agent (`agent::run`, in a thread, with a scripted job child)
    /// on the real channel: it runs the executor, forwards events, and on
    /// `cancel` kills the child's process group and releases the job. The
    /// `WorkerSpec` its child receives names nothing of the master's.
    #[test]
    fn the_agent_kills_its_child_and_releases_the_job_on_cancel() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new(Duration::from_secs(60), own_build());
        let root = fixture.dir.path();
        let repo = root.join("demo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
        for args in [
            &["init", "-q"][..],
            &["add", "-A"][..],
            &["commit", "-qm", "initial"][..],
        ] {
            assert!(std::process::Command::new("git")
                .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success());
        }
        let spec = root.join("spec.json");
        let pid_file = root.join("child.pid");
        let worker = root.join("worker.sh");
        std::fs::write(
            &worker,
            format!(
                "#!/bin/sh\ncat > '{}'\nsleep 30 &\necho $! > '{}'\nprintf '%s\\n' '{{\"type\":\"stage_started\",\"v\":1,\"stage\":\"parse\"}}'\nwait\n",
                spec.display(),
                pid_file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755)).unwrap();
        let token_file = root.join("agent.token");
        std::fs::write(&token_file, TOKENS[0]).unwrap();
        let config = crate::service::agent::AgentConfig {
            connect: format!("ws://127.0.0.1:{}/workers/connect", fixture.port),
            token_file,
            cache_dir: root.join("agent"),
            worker_exe: worker,
            class_memory: None,
            memory_events: None,
        };
        let agent = std::thread::spawn(move || crate::service::agent::run(config));

        let head = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let head = String::from_utf8(head.stdout).unwrap().trim().to_owned();
        let id = fixture.spawn_at(
            RepoRef {
                slug: "local/demo".to_owned(),
                owner: "local".to_owned(),
                repo: "demo".to_owned(),
                source: RepoSource::Local(repo.clone()),
            },
            &head,
        );
        let snapshot = fixture.wait_for(id, "the child starts parsing", |s| {
            s.status == JobStatus::Indexing
        });
        assert_eq!(
            snapshot.stages[StageId::Clone.index() - 1].state,
            jobs::StageState::Done
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let child: i32 = loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "the job child never started");
            std::thread::sleep(Duration::from_millis(20));
        };
        let spec = std::fs::read_to_string(&spec).unwrap();
        for secret in [
            fixture.state.config.db_path.to_string_lossy().into_owned(),
            fixture
                .state
                .config
                .cache_dir
                .to_string_lossy()
                .into_owned(),
        ] {
            assert!(
                !spec.contains(&secret),
                "the child's spec names {secret}: {spec}"
            );
        }

        fixture.state.jobs.cancel(id).unwrap();
        fixture.wait_for_no_lease();
        assert_eq!(
            fixture.snapshot(id).error_code.as_deref(),
            Some("cancelled")
        );
        #[cfg(target_os = "linux")]
        {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let stat = std::fs::read_to_string(format!("/proc/{child}/stat"));
                let gone = stat.as_ref().map_or(true, |stat| {
                    stat.split(") ")
                        .nth(1)
                        .is_some_and(|tail| tail.starts_with('Z'))
                });
                if gone {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "the job child survived the cancel"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = child;

        // `shutdown now` ends the agent cleanly.
        fixture.hub.shutdown_now();
        let outcome = agent.join().unwrap();
        assert!(outcome.is_ok(), "{outcome:?}");
    }

    // ---- classes (#97 phase 2, step 4) ------------------------------------

    #[test]
    fn loopback_classes_are_sizes_and_agent_counts_and_refuse_anything_else() {
        let class = |usable_memory: Option<u64>, slots: usize| Class {
            usable_memory,
            slots,
        };
        assert_eq!(loopback_classes(None, 3), Ok(vec![class(None, 3)]));
        assert_eq!(loopback_classes(Some("  "), 2), Ok(vec![class(None, 2)]));
        // Smallest first, whatever the order written.
        assert_eq!(
            loopback_classes(Some("16GiB:1, 2GiB:2"), 3),
            Ok(vec![class(Some(2 * GIB), 2), class(Some(16 * GIB), 1)])
        );
        assert_eq!(
            loopback_classes(Some("512MiB:1,1073741824:1,1TiB:1,4KiB:1"), 4),
            Ok(vec![
                class(Some(4 << 10), 1),
                class(Some(512 << 20), 1),
                class(Some(GIB), 1),
                class(Some(1 << 40), 1),
            ])
        );
        for (bad, agents) in [
            ("2GiB", 1),
            ("2GiB:0", 1),
            ("2GB:1", 1),
            ("0:1", 1),
            ("two:1", 1),
            ("2GiB:x", 1),
            ("2GiB:1,", 1),
            ("2GiB:1,2GiB:1", 2),
            ("2GiB:1,16GiB:1", 3),
            ("99999999999TiB:1", 1),
        ] {
            assert!(
                loopback_classes(Some(bad), agents).is_err(),
                "{bad} for {agents} agent(s)"
            );
        }
    }

    /// Makes `slug` predicted large at admission (§2.1 step 1): its last
    /// job was killed at 8 GiB, a floor the prediction stays above.
    fn predicted_large(fixture: &Fixture, slug: &str) {
        let row = crate::service::eta::TimingRow {
            features: crate::worker::RepoFeatures::default(),
            elapsed_s: 1.0,
            stage_s: vec![None; StageId::ALL.len()],
            peak_rss_bytes: Some(8 * GIB),
            slug: Some(slug.to_owned()),
        };
        fixture
            .state
            .store
            .save_timing(&format!("seed {slug}"), &row)
            .unwrap();
        fixture
            .state
            .jobs
            .load_timings(&fixture.state.store)
            .unwrap();
    }

    /// Delivers the job `agent` holds and makes it ready again.
    fn finish(fixture: &Fixture, agent: &mut FakeAgent, token: usize, id: Uuid) {
        agent.deliver(fixture.port, TOKENS[token], id);
        match agent.recv() {
            MasterMessage::ResultAccepted { job_id, .. } => assert_eq!(job_id, id.to_string()),
            other => panic!("expected result_accepted, got {other:?}"),
        }
        agent.send(&WorkerMessage::Ready { slots_free: 1 });
    }

    /// §7.1, §9 "class selection": a job predicted large waits for the
    /// large agent, which is busy, while small jobs flow through the small
    /// one; the small agent never takes it, and the large agent takes it
    /// when it frees.
    #[test]
    fn a_large_job_waits_for_the_large_agent_while_small_jobs_flow() {
        let fixture = Fixture::classed(Duration::from_secs(60), test_build(), two_classes(), 16);
        predicted_large(&fixture, "test/big");
        predicted_large(&fixture, "test/huge");
        let mut small = fixture.class_agent(0, 2 * GIB);
        let mut large = fixture.class_agent(1, 16 * GIB);
        let big = fixture.spawn(remote_repo("big"));
        assert_eq!(large.assigned(), big);
        let huge = fixture.spawn(remote_repo("huge"));
        let first = fixture.spawn(remote_repo("first"));
        let second = fixture.spawn(remote_repo("second"));
        assert_eq!(small.assigned(), first);
        let waiting = fixture.snapshot(huge);
        assert_eq!(waiting.status, JobStatus::Queued);
        assert_eq!(waiting.queue_position, Some(1), "{waiting:?}");
        assert_eq!(fixture.snapshot(second).queue_position, Some(1));
        assert_eq!(fixture.row(huge).class, 1);
        assert_eq!(fixture.row(second).class, 0);
        finish(&fixture, &mut small, 0, first);
        assert_eq!(small.assigned(), second);
        finish(&fixture, &mut small, 0, second);
        assert!(
            small.recv_within(Duration::from_millis(700)).is_none(),
            "the small agent was sent a large job"
        );
        assert_eq!(fixture.snapshot(huge).status, JobStatus::Queued);
        finish(&fixture, &mut large, 1, big);
        assert_eq!(large.assigned(), huge);
    }

    /// §7.1, §9 "spill-down": the large agent, with nothing of its own to
    /// run, takes a small job; a large job admitted meanwhile waits for that
    /// one small job and no more -- the small agent, free first, never takes
    /// it.
    #[test]
    fn a_large_agent_spills_down_and_a_large_job_waits_for_that_one_small_job() {
        let fixture = Fixture::classed(Duration::from_secs(60), test_build(), two_classes(), 16);
        predicted_large(&fixture, "test/big");
        let mut small = fixture.class_agent(0, 2 * GIB);
        let mut large = fixture.class_agent(1, 16 * GIB);
        let first = fixture.spawn(remote_repo("first"));
        assert_eq!(small.assigned(), first);
        let spilled = fixture.spawn(remote_repo("spilled"));
        assert_eq!(large.assigned(), spilled, "an idle large agent spills down");
        assert_eq!(
            fixture.row(spilled).class,
            0,
            "a spilled job keeps its class"
        );
        let big = fixture.spawn(remote_repo("big"));
        assert_eq!(fixture.snapshot(big).queue_position, Some(1));
        finish(&fixture, &mut small, 0, first);
        assert!(
            small.recv_within(Duration::from_millis(700)).is_none(),
            "the small agent was sent a large job"
        );
        finish(&fixture, &mut large, 1, spilled);
        assert_eq!(large.assigned(), big);
    }

    /// §7.2 with two classes, against a schedule worked out by hand. Every
    /// job has the same predicted midpoint `m` (nothing has reported
    /// features, and failed seed rows move no stage estimate).
    ///
    /// With only the small agent connected, the idle large slot has no
    /// agent behind it: it takes no small job and counts for nothing in the
    /// quote, so no queued job is ever quoted a start of 0 on it, and the
    /// small jobs queue behind the small agent. A large job then has no
    /// agent that can take it, so it has no start at all. Once the large
    /// agent connects it runs the large job, and a large job admitted later
    /// goes ahead of every small one on it (classes are simulated largest
    /// first) while the small queue spills onto it exactly as dispatch
    /// would.
    #[test]
    fn queued_etas_with_two_classes_match_a_hand_computed_schedule() {
        use crate::service::eta::EtaModel;
        let fixture = Fixture::classed(Duration::from_secs(60), test_build(), two_classes(), 16);
        predicted_large(&fixture, "test/big");
        predicted_large(&fixture, "test/huge");
        let m = EtaModel::default()
            .predict(
                &crate::worker::RepoFeatures::default(),
                &[false; StageId::ALL.len()],
                None,
            )
            .midpoint();
        let quote = |id: Uuid| {
            let snapshot = fixture.snapshot(id);
            (snapshot.queue_position, snapshot.eta_start_s)
        };
        let mut small = fixture.class_agent(0, 2 * GIB);
        let s1 = fixture.spawn(remote_repo("s1"));
        assert_eq!(small.assigned(), s1);
        let s2 = fixture.spawn(remote_repo("s2"));
        let s3 = fixture.spawn(remote_repo("s3"));
        // The small agent frees at m, then m + m; the large slot, with no
        // agent, neither takes s2 nor quotes it 0.
        assert_eq!(quote(s2), (Some(1), Some(m)));
        assert_eq!(quote(s3), (Some(2), Some(m + m)));
        // No agent can take a large job: it waits with no start.
        let huge = fixture.spawn(remote_repo("huge"));
        assert_eq!(quote(huge).1, None, "{:?}", fixture.snapshot(huge));
        assert_eq!(fixture.snapshot(huge).status, JobStatus::Queued);

        let mut large = fixture.class_agent(1, 16 * GIB);
        assert_eq!(large.assigned(), huge);
        let s4 = fixture.spawn(remote_repo("s4"));
        let big = fixture.spawn(remote_repo("big"));
        // Workers free at m (small, s1) and m (large, huge). Largest class
        // first: big -> large at m, which is then free at m + m. Then the
        // small queue in order: s2 -> small at m (small then at m + m);
        // s3 -> both at m + m, the tie goes to the lower worker id, small
        // (then at 3m); s4 -> large, spilling, at m + m.
        assert_eq!(quote(big), (Some(1), Some(m)));
        assert_eq!(quote(s2), (Some(1), Some(m)));
        assert_eq!(quote(s3), (Some(2), Some(m + m)));
        assert_eq!(quote(s4), (Some(3), Some(m + m)));
    }

    /// §7.3 with several classes: the queue bound applies per class, so a
    /// full small queue turns small jobs away with `busy` while large jobs
    /// still queue for the large class, up to that class's own bound.
    #[test]
    fn the_queue_bound_holds_per_class() {
        let fixture = Fixture::classed(Duration::from_secs(60), test_build(), two_classes(), 1);
        for slug in ["test/big", "test/big2", "test/big3"] {
            predicted_large(&fixture, slug);
        }
        let try_spawn = |name: &str| {
            let _entered = fixture.runtime.as_ref().unwrap().enter();
            jobs::spawn_job(fixture.state.clone(), remote_repo(name), COMMIT.to_owned())
        };
        let mut small = fixture.class_agent(0, 2 * GIB);
        let first = fixture.spawn(remote_repo("first"));
        assert_eq!(small.assigned(), first);
        let second = fixture.spawn(remote_repo("second"));
        assert_eq!(fixture.snapshot(second).queue_position, Some(1));
        let refused = try_spawn("third").expect_err("a small job past its class's bound");
        assert_eq!(refused.body.error, "busy");
        // The large class's slot takes the first large job (it waits there
        // for a large agent); the next one queues although the small queue
        // is full; the one after that finds the large queue full.
        let big = fixture.spawn(remote_repo("big"));
        let big2 = try_spawn("big2").expect("the large queue has room");
        assert_eq!(fixture.row(big).class, 1);
        assert_eq!(fixture.row(big2).class, 1);
        let refused = try_spawn("big3").expect_err("a large job past its class's bound");
        assert_eq!(refused.body.error, "busy");
    }

    fn py_features(files: u64) -> WorkerEvent {
        WorkerEvent::Features {
            v: 1,
            features: crate::worker::RepoFeatures {
                languages: [(
                    "py".to_owned(),
                    crate::worker::LanguageFeatures {
                        files,
                        bytes: files * 8_000,
                    },
                )]
                .into(),
                ..crate::worker::RepoFeatures::default()
            },
        }
    }

    /// §2.1 step 3 with scripted agents: a job admitted small whose
    /// `features` predict more than 2 GiB is told `cancel` `reroute`; on
    /// `released` it moves to the large class at the head of its queue,
    /// epoch 2, attempt not counted, and completes there. Features that
    /// predict less on the large agent never move it back down.
    #[test]
    fn a_job_whose_features_outgrow_its_class_is_rerouted_uncounted() {
        let fixture = Fixture::classed(Duration::from_secs(60), test_build(), two_classes(), 16);
        let mut small = fixture.class_agent(0, 2 * GIB);
        let mut large = fixture.class_agent(1, 16 * GIB);
        let id = fixture.spawn(remote_repo("grows"));
        assert_eq!(small.assigned(), id);
        assert_eq!(fixture.row(id).class, 0);
        clone_started(&mut small, id);
        // A clone's `features` name no files yet: nothing to act on.
        small.event(
            id,
            WorkerEvent::Features {
                v: 1,
                features: crate::worker::RepoFeatures::default(),
            },
        );
        assert!(small.recv_within(Duration::from_millis(300)).is_none());
        // Detection's: 40,000 Python files, several GiB on the hand curve.
        small.event(id, py_features(40_000));
        match small.recv() {
            MasterMessage::Cancel {
                job_id,
                epoch,
                reason,
            } => {
                assert_eq!((job_id, epoch), (id.to_string(), 1));
                assert_eq!(reason, CancelReason::Reroute);
            }
            other => panic!("expected cancel reroute, got {other:?}"),
        }
        // A second `features` event asks nothing more.
        small.event(id, py_features(50_000));
        assert!(small.recv_within(Duration::from_millis(300)).is_none());
        small.send(&WorkerMessage::Released {
            job_id: id.to_string(),
            epoch: 1,
            reason: ReleasedReason::Reroute,
            peak_rss_bytes: Some(GIB),
        });
        assert_eq!(large.assigned(), id);
        assert_eq!(large.epoch, 2);
        let row = fixture.wait_for_row(id, "leased on the large class", |row| {
            row.status == "leased"
        });
        assert_eq!((row.class, row.attempt, row.epoch), (1, 1, 2), "{row:?}");
        clone_started(&mut large, id);
        large.event(id, py_features(10));
        assert!(
            large.recv_within(Duration::from_millis(300)).is_none(),
            "a job was rerouted down"
        );
        large.deliver(fixture.port, TOKENS[1], id);
        assert!(matches!(large.recv(), MasterMessage::ResultAccepted { .. }));
        let row = fixture.wait_for_row(id, "done", |row| row.status == "done");
        assert_eq!((row.class, row.attempt), (1, 1), "{row:?}");
        assert!(small.recv_within(Duration::from_millis(300)).is_none());
    }

    /// §6 "worker host dies" near its memory limit: the lost agent's last
    /// heartbeat had its job at 95% of its class, so the lease's expiry
    /// moves the job to the next class instead of retrying it on this one.
    /// It is a lost worker (attempt 2), and its new class's retry count
    /// starts at 0.
    #[test]
    fn a_worker_lost_near_its_memory_limit_moves_the_job_to_the_next_class() {
        let fixture = Fixture::classed(Duration::from_secs(1), test_build(), two_classes(), 16);
        let mut small = fixture.class_agent(0, 2 * GIB);
        let mut large = fixture.class_agent(1, 16 * GIB);
        let id = fixture.spawn(remote_repo("heavy"));
        assert_eq!(small.assigned(), id);
        clone_started(&mut small, id);
        small.send(&WorkerMessage::Heartbeat {
            jobs: vec![HeartbeatJob {
                job_id: id.to_string(),
                epoch: 1,
                last_seq: 1,
            }],
            rss_bytes: Some(2 * GIB / 100 * 95),
        });
        assert!(matches!(small.recv(), MasterMessage::LeaseRenewed { .. }));
        drop(small);
        assert_eq!(large.assigned(), id);
        assert_eq!(large.epoch, 2);
        let row = fixture.wait_for_row(id, "leased again", |row| row.status == "leased");
        assert_eq!((row.class, row.attempt), (1, 2), "{row:?}");
        assert_eq!(fixture.snapshot(id).stage, LOST_TO_MEMORY);
    }

    /// A job child for the out-of-memory tests: it reads its spec, records
    /// its start, reports a stage, then -- as the kernel's OOM killer would
    /// leave things -- raises the `oom_kill` count in its agent's memory
    /// events file (`<agent cache>/memory.events`, four levels above its
    /// job directory, which is its `HOME`) when `raise` is set, and SIGKILLs
    /// itself.
    fn oom_job(root: &Path, raise: bool) -> (PathBuf, String, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let (repo, _, _) = scripted_job(root);
        let head = head_of(&repo);
        let marker = if raise {
            "events=\"$HOME/../../../../memory.events\"\n\
             n=$(sed -n 's/^oom_kill //p' \"$events\")\n\
             echo \"oom_kill $((n+1))\" > \"$events\"\n"
        } else {
            ""
        };
        let script = format!(
            "#!/bin/sh\ncat > /dev/null\necho $$ >> '{}'\n\
             printf '%s\\n' '{{\"type\":\"stage_started\",\"v\":1,\"stage\":\"parse\"}}'\n\
             {marker}kill -9 $$\n",
            root.join("starts").display()
        );
        let worker = root.join("oom-worker.sh");
        std::fs::write(&worker, script).unwrap();
        std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755)).unwrap();
        (repo, head, worker)
    }

    /// The real agent (`agent::run`, in a thread) with token `token`,
    /// advertising `memory_bytes`, its own cache directory and a memory
    /// events file there reading `oom_kill 0`, dialling the fixture.
    fn class_agent_real(
        fixture: &Fixture,
        token: usize,
        memory_bytes: u64,
        worker: PathBuf,
    ) -> std::thread::JoinHandle<anyhow::Result<()>> {
        let root = fixture.dir.path();
        let cache_dir = root.join(format!("agent-{token}"));
        std::fs::create_dir_all(&cache_dir).unwrap();
        let events = cache_dir.join("memory.events");
        std::fs::write(&events, "oom 0\noom_kill 0\n").unwrap();
        let token_file = root.join(format!("agent-{token}.token"));
        std::fs::write(&token_file, TOKENS[token]).unwrap();
        let config = crate::service::agent::AgentConfig {
            connect: format!("ws://127.0.0.1:{}/workers/connect", fixture.port),
            token_file,
            cache_dir,
            worker_exe: worker,
            class_memory: Some(memory_bytes),
            memory_events: Some(events),
        };
        let agent = std::thread::spawn(move || crate::service::agent::run(config));
        wait_for_ready_agent(fixture, token);
        agent
    }

    /// §6 "job child OOM-killed", §9's fake OOM, with real agents: the job
    /// child raises the marker and SIGKILLs itself. On the small agent the
    /// job is released `oom` and escalates to the large one, uncounted; on
    /// the large agent, the largest class, it fails `worker_crashed` "out
    /// of memory on the largest worker class". Each attempt's peak is
    /// recorded as a timing row of its own, for the memory model.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_job_killed_for_memory_escalates_and_fails_on_the_largest_class() {
        let fixture = Fixture::classed(Duration::from_secs(60), own_build(), two_classes(), 16);
        let root = fixture.dir.path().to_path_buf();
        let (repo, head, worker) = oom_job(&root, true);
        let small = class_agent_real(&fixture, 0, 2 * GIB, worker.clone());
        let large = class_agent_real(&fixture, 1, 16 * GIB, worker);
        let id = fixture.spawn_at(local_demo(&repo), &head);
        let failed = fixture.wait_for(id, "the job fails", jobs::is_terminal);
        assert_eq!(failed.error_code.as_deref(), Some("worker_crashed"));
        let message = failed.error.clone().unwrap_or_default();
        assert!(message.contains(OOM_ON_LARGEST), "{message}");
        let row = fixture.wait_for_row(id, "failed in the store", |row| row.status == "failed");
        assert_eq!((row.class, row.epoch, row.attempt), (1, 2, 1), "{row:?}");
        assert_eq!(starts(&root), 2, "one attempt on each class");
        let oom_rows = fixture
            .state
            .store
            .recent_timings()
            .unwrap()
            .into_iter()
            .filter(|row| {
                row.slug.as_deref() == Some("local/demo")
                    && row.peak_rss_bytes.is_some_and(|peak| peak > 0)
                    && row.stage_s.iter().all(Option::is_none)
            })
            .count();
        assert!(oom_rows >= 2, "{oom_rows} out-of-memory timing row(s)");
        fixture.hub.shutdown_now();
        for agent in [small, large] {
            let outcome = agent.join().unwrap();
            assert!(outcome.is_ok(), "{outcome:?}");
        }
    }

    /// A SIGKILL the memory cgroup did not count is not an out-of-memory
    /// kill: the job fails `worker_crashed` on the class it ran on, as any
    /// crash does, and nothing escalates.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_sigkill_with_no_oom_kill_counted_is_a_crash_not_an_escalation() {
        let fixture = Fixture::classed(Duration::from_secs(60), own_build(), two_classes(), 16);
        let root = fixture.dir.path().to_path_buf();
        let (repo, head, worker) = oom_job(&root, false);
        let small = class_agent_real(&fixture, 0, 2 * GIB, worker.clone());
        let large = class_agent_real(&fixture, 1, 16 * GIB, worker);
        let id = fixture.spawn_at(local_demo(&repo), &head);
        let failed = fixture.wait_for(id, "the job fails", jobs::is_terminal);
        assert_eq!(failed.error_code.as_deref(), Some("worker_crashed"));
        let message = failed.error.clone().unwrap_or_default();
        assert!(!message.contains(OOM_ON_LARGEST), "{message}");
        let row = fixture.wait_for_row(id, "failed in the store", |row| row.status == "failed");
        assert_eq!((row.class, row.epoch, row.attempt), (0, 1, 1), "{row:?}");
        assert_eq!(starts(&root), 1);
        fixture.hub.shutdown_now();
        for agent in [small, large] {
            let outcome = agent.join().unwrap();
            assert!(outcome.is_ok(), "{outcome:?}");
        }
    }

    // ---- remote mode (#97 phase 3) -----------------------------------------

    /// The worker ids `TOKENS` are bound to in a remote fixture's token file.
    const WORKER_IDS: [&str; 2] = ["w0", "w1"];

    /// Writes a token file binding each token's SHA-256 to its worker id,
    /// the way the docs ask for an edit: a new file renamed over the old.
    /// `0600` whatever the umask, since a group-writable file is refused.
    fn write_token_file(path: &Path, tokens: &[(&str, &str)]) {
        use std::os::unix::fs::PermissionsExt;
        let lines: String = tokens
            .iter()
            .map(|(token, id)| format!("{} {id}\n", hex(&token_digest(token.as_bytes()))))
            .collect();
        let temp = path.with_extension("new");
        std::fs::write(&temp, format!("# test workers\n\n{lines}")).unwrap();
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::rename(&temp, path).unwrap();
    }

    impl Fixture {
        /// A remote-mode master on loopback without TLS, as CI runs one: one
        /// slot, and a token file binding `TOKENS[i]` to `WORKER_IDS[i]` at
        /// `<dir>/worker-tokens`. `frames`: a frame limit other than the
        /// default.
        fn remote(dir: tempfile::TempDir, lease_ttl: Duration, frames: Option<(u32, u32)>) -> Self {
            let tokens = dir.path().join("worker-tokens");
            write_token_file(
                &tokens,
                &[(TOKENS[0], WORKER_IDS[0]), (TOKENS[1], WORKER_IDS[1])],
            );
            Self::build_with(
                dir,
                lease_ttl,
                test_build(),
                one_class(1),
                Limits::default(),
                false,
                &|hub| {
                    let hub = hub.with_token_file(TokenFile::open(&tokens).unwrap(), false);
                    match frames {
                        Some((rate, burst)) => hub.with_frame_limit(rate, burst),
                        None => hub,
                    }
                },
            )
        }
    }

    /// A remote worker's `hello`: its worker id, `resume`, no `local_paths`.
    fn remote_hello(worker_id: &str, resume: Vec<ResumeEntry>) -> WorkerMessage {
        let mut hello = hello_with(test_build(), (PROTO, PROTO), &[FEATURE_RESUME], resume);
        if let WorkerMessage::Hello { worker_id: id, .. } = &mut hello {
            *id = worker_id.to_owned();
        }
        hello
    }

    impl FakeAgent {
        /// A new channel with `token` that sends `hello`; the master's
        /// first answer.
        fn hello_as(port: u16, token: &str, hello: &WorkerMessage) -> (Self, MasterMessage) {
            let mut agent = FakeAgent::raw(port, token);
            agent.send(hello);
            let first = agent.recv();
            (agent, first)
        }

        /// A remote worker `worker_id` joining with `resume`; the welcome's
        /// answers.
        fn rejoin_remote(
            port: u16,
            token: &str,
            worker_id: &str,
            resume: Vec<ResumeEntry>,
        ) -> (Self, Vec<WelcomeResume>) {
            match Self::hello_as(port, token, &remote_hello(worker_id, resume)) {
                (agent, MasterMessage::Welcome { resume, .. }) => (agent, resume),
                (_, other) => panic!("expected welcome, got {other:?}"),
            }
        }
    }

    fn empty_heartbeat() -> WorkerMessage {
        WorkerMessage::Heartbeat {
            jobs: Vec::new(),
            rss_bytes: None,
        }
    }

    #[test]
    fn workers_mode_remote_needs_tls_off_loopback_and_both_files_or_neither() {
        let loopback: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let private: SocketAddr = "10.1.2.3:7000".parse().unwrap();
        let every: SocketAddr = "0.0.0.0:7000".parse().unwrap();
        let (cert, key) = (Some("cert.pem".to_owned()), Some("key.pem".to_owned()));
        assert_eq!(remote_tls_files(loopback, None, None), Ok(None));
        for listen in [private, every] {
            let error = remote_tls_files(listen, None, None).unwrap_err();
            assert!(error.contains("needs TLS"), "{error}");
            assert_eq!(
                remote_tls_files(listen, cert.clone(), key.clone()),
                Ok(Some((PathBuf::from("cert.pem"), PathBuf::from("key.pem"))))
            );
        }
        for listen in [loopback, private] {
            assert!(remote_tls_files(listen, cert.clone(), None).is_err());
            assert!(remote_tls_files(listen, None, key.clone()).is_err());
        }
    }

    /// §5.1: the listener's key must be private; the committed test pair
    /// makes a usable TLS 1.3 configuration once it is.
    #[test]
    fn the_listener_refuses_a_key_its_group_or_others_can_read() {
        use std::os::unix::fs::PermissionsExt;
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tls");
        let cert = fixtures.join("test-only-server.pem");
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("server.key");
        std::fs::copy(fixtures.join("test-only-server.key"), &key).unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        let error = format!("{:#}", server_tls(&cert, &key).unwrap_err());
        assert!(error.contains("mode 644"), "{error}");
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(server_tls(&cert, &key).is_err());
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        server_tls(&cert, &key).unwrap();
        // A certificate file with no certificate in it is refused, not
        // served as an empty chain.
        let empty = dir.path().join("empty.pem");
        std::fs::write(&empty, "no certificate here\n").unwrap();
        assert!(server_tls(&empty, &key).is_err());
    }

    #[test]
    fn a_token_file_binds_hashes_to_worker_ids_and_refuses_anything_malformed() {
        let (a, b) = (hex(&token_digest(b"a")), hex(&token_digest(b"b")));
        // Comments, blank lines, spacing, upper-case hex, and one worker id
        // on two lines (a rotation in progress).
        let text = format!("# workers\n\n{a} w0\n  {}   w0  \n", b.to_uppercase());
        assert_eq!(
            parse_token_file(&text).unwrap(),
            vec![
                (token_digest(b"a"), "w0".to_owned()),
                (token_digest(b"b"), "w0".to_owned())
            ]
        );
        assert_eq!(parse_token_file("# none yet\n").unwrap(), vec![]);
        for bad in [
            a.clone(),
            format!("{a} w0 extra"),
            "abc w0".to_owned(),
            format!("{a}0 w0"),
            format!("{a} bad/id"),
            format!("{a} {}", "x".repeat(65)),
            format!("{a} w0\n{a} w1"),
        ] {
            assert!(parse_token_file(&bad).is_err(), "{bad:?}");
        }
        // A token pasted where its hash belongs is never echoed back.
        let pasted = new_token();
        let error = parse_token_file(&format!("{pasted}ff w0")).unwrap_err();
        assert!(!error.contains(&pasted), "{error}");
        let (longest, too_long) = ("x".repeat(64), "x".repeat(65));
        for good in ["w0", "worker-1", "gpu_16.a", longest.as_str()] {
            assert!(is_valid_worker_id(good), "{good}");
        }
        for bad in ["", "a b", "a/b", "a:b", "é", too_long.as_str()] {
            assert!(!is_valid_worker_id(bad), "{bad}");
        }
    }

    /// `tolmap worker-token new`: a 256-bit token and the line binding its
    /// SHA-256 to the id, which the token file parser reads back.
    #[test]
    fn a_new_worker_token_comes_with_the_line_that_binds_its_hash() {
        let (token, line) = issue_worker_token("worker-1").unwrap();
        assert_eq!(token.len(), 64);
        assert!(is_sha256_hex(&token));
        let (hash, id) = line.split_once(' ').unwrap();
        assert_eq!((hash, id), (sha(token.as_bytes()).as_str(), "worker-1"));
        assert_eq!(
            parse_token_file(&line).unwrap(),
            vec![(token_digest(token.as_bytes()), "worker-1".to_owned())]
        );
        assert_ne!(issue_worker_token("worker-1").unwrap().0, token);
        assert!(issue_worker_token("bad id").is_err());
        assert!(issue_worker_token("").is_err());
    }

    /// The token file fails closed: unreadable, group-writable or malformed
    /// refuses every token until it is fixed, and a worker keeps its number
    /// across it.
    #[test]
    fn an_unusable_token_file_refuses_every_token_until_it_is_fixed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens");
        write_token_file(&path, &[(TOKENS[0], "w0"), (TOKENS[1], "w1")]);
        let file = TokenFile::open(&path).unwrap();
        let (zero, one) = (
            token_digest(TOKENS[0].as_bytes()),
            token_digest(TOKENS[1].as_bytes()),
        );
        let (agent, id) = file.lookup(&zero).unwrap();
        assert_eq!(id, "w0");
        assert_eq!(file.lookup(&one).unwrap().1, "w1");
        assert!(file.lookup(&token_digest(b"stranger")).is_none());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o620)).unwrap();
        assert!(file.lookup(&zero).is_none(), "a group-writable file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(file.lookup(&zero), Some((agent, "w0".to_owned())));
        std::fs::write(&path, "not a token file\n").unwrap();
        assert!(file.lookup(&zero).is_none(), "a malformed file");
        std::fs::remove_file(&path).unwrap();
        assert!(file.lookup(&zero).is_none(), "a missing file");
        write_token_file(&path, &[(TOKENS[0], "w0")]);
        assert_eq!(file.lookup(&zero), Some((agent, "w0".to_owned())));
        assert!(file.lookup(&one).is_none(), "revoked by the rewrite");
        // At startup a bad file stops the service instead.
        std::fs::write(&path, "not a token file\n").unwrap();
        assert!(TokenFile::open(&path).is_err());
    }

    /// §5.1: in remote mode a token is one worker's, and its `hello` must
    /// name that worker.
    #[test]
    fn a_remote_hello_must_name_the_worker_its_token_was_issued_for() {
        let fixture = Fixture::remote(tempfile::tempdir().unwrap(), Duration::from_secs(5), None);
        let (mut agent, first) = FakeAgent::hello_as(
            fixture.port,
            TOKENS[0],
            &remote_hello(WORKER_IDS[1], vec![]),
        );
        match first {
            MasterMessage::Error { code, .. } => assert_eq!(code, "worker_id_mismatch"),
            other => panic!("expected error, got {other:?}"),
        }
        assert!(agent.closed(), "the master must close after an error");
        let (_agent, first) = FakeAgent::hello_as(
            fixture.port,
            TOKENS[0],
            &remote_hello(WORKER_IDS[0], vec![]),
        );
        assert!(matches!(first, MasterMessage::Welcome { .. }), "{first:?}");
        // A token the file does not list gets no channel.
        let refused = dial(fixture.port, "not-a-listed-token").unwrap_err();
        assert!(refused.contains("401"), "{refused}");
    }

    /// §5.1 "revoking a worker means deleting its line": the next dial gets
    /// a 401, so does an artifact request, and the channel that is up is
    /// closed at its next heartbeat. The other worker is untouched.
    #[test]
    fn a_revoked_token_is_refused_at_its_next_dial_and_closed_at_its_next_heartbeat() {
        let fixture = Fixture::remote(tempfile::tempdir().unwrap(), Duration::from_secs(5), None);
        let (mut agent, _) =
            FakeAgent::rejoin_remote(fixture.port, TOKENS[0], WORKER_IDS[0], vec![]);
        agent.send(&empty_heartbeat());
        assert!(
            agent.recv_within(Duration::from_millis(500)).is_none(),
            "a listed worker's heartbeat is not refused"
        );
        write_token_file(
            &fixture.dir.path().join("worker-tokens"),
            &[(TOKENS[1], WORKER_IDS[1])],
        );
        let refused = dial(fixture.port, TOKENS[0]).unwrap_err();
        assert!(refused.contains("401"), "{refused}");
        let (status, _) = request(
            fixture.port,
            "GET",
            &format!("/workers/artifacts/{}/1/inputs/names", Uuid::new_v4()),
            &[bearer(TOKENS[0])],
            b"",
            None,
        );
        assert_eq!(status, 401);
        agent.send(&empty_heartbeat());
        agent.expect_error("token_revoked");
        let (_other, _) = FakeAgent::rejoin_remote(fixture.port, TOKENS[1], WORKER_IDS[1], vec![]);
    }

    /// §4.3: a channel that floods the master past its frame budget is
    /// closed with `rate_limited`; a burst within it is not.
    #[test]
    fn a_flood_of_frames_is_closed_with_rate_limited() {
        let fixture = Fixture::remote(
            tempfile::tempdir().unwrap(),
            Duration::from_secs(5),
            Some((5, 10)),
        );
        let (mut agent, _) =
            FakeAgent::rejoin_remote(fixture.port, TOKENS[0], WORKER_IDS[0], vec![]);
        for _ in 0..10 {
            agent.send(&empty_heartbeat());
        }
        assert!(
            agent.recv_within(Duration::from_millis(200)).is_none(),
            "a burst within the budget is accepted"
        );
        // One frame at a time from here, each given a moment for an answer,
        // so the master has read everything by the time it closes.
        let mut sent = 0;
        let refused = loop {
            agent.send(&empty_heartbeat());
            sent += 1;
            if let Some(message) = agent.recv_within(Duration::from_millis(30)) {
                break message;
            }
            assert!(
                sent < 10,
                "no rate_limited after {sent} frames past the burst"
            );
        };
        match refused {
            MasterMessage::Error { code, .. } => assert_eq!(code, "rate_limited"),
            other => panic!("expected error, got {other:?}"),
        }
        assert!(agent.closed(), "the master must close after an error");
    }

    /// #156's first departure, lifted (§6 "master restarts mid-job"): a
    /// restarted remote master adopts the lease its store names with the
    /// worker id that held it, and hands it back to that worker -- to no
    /// other -- so the job finishes at the same epoch and attempt, with no
    /// re-run.
    #[test]
    fn a_restarted_remote_master_hands_an_adopted_lease_back_to_its_worker() {
        let dir = tempfile::tempdir().unwrap();
        let running = Uuid::new_v4();
        {
            let store = Store::open(&dir.path().join("store.sqlite3")).unwrap();
            let mut row = stored_job(running, "running", "running", 1, 1);
            row.lease_holder = Some(WORKER_IDS[0].to_owned());
            store.insert_job(&row).unwrap();
        }
        let fixture = Fixture::remote(dir, Duration::from_secs(5), None);
        assert_eq!(fixture.snapshot(running).status, JobStatus::Indexing);
        // Another worker naming the job is told to let it go.
        let (_other, answers) = FakeAgent::rejoin_remote(
            fixture.port,
            TOKENS[1],
            WORKER_IDS[1],
            vec![resume_entry(running, 1, 3)],
        );
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].action, ResumeAction::Cancel);
        // Its holder resumes it from what this master applied: nothing.
        let (mut agent, answers) = FakeAgent::rejoin_remote(
            fixture.port,
            TOKENS[0],
            WORKER_IDS[0],
            vec![resume_entry(running, 1, 3)],
        );
        assert_eq!(answers.len(), 1);
        assert_eq!(
            (answers[0].action, answers[0].acked_seq),
            (ResumeAction::Continue, 0)
        );
        agent.event(
            running,
            WorkerEvent::StageStarted {
                v: 1,
                stage: StageId::Parse,
            },
        );
        agent.deliver(fixture.port, TOKENS[0], running);
        match agent.recv() {
            MasterMessage::ResultAccepted { epoch, .. } => assert_eq!(epoch, 1),
            other => panic!("expected result_accepted, got {other:?}"),
        }
        let row = fixture.wait_for_row(running, "done", |row| row.status == "done");
        assert_eq!((row.attempt, row.epoch), (1, 1), "{row:?}");
        assert!(fixture
            .state
            .store
            .get("test/running", COMMIT)
            .unwrap()
            .is_some());
    }

    /// The loopback side of the same restart is unchanged: an adopted lease
    /// has no holder there, so its old agent's resume is answered `cancel`.
    #[test]
    fn a_restarted_loopback_master_still_never_hands_an_adopted_lease_back() {
        let dir = tempfile::tempdir().unwrap();
        let running = Uuid::new_v4();
        {
            let store = Store::open(&dir.path().join("store.sqlite3")).unwrap();
            let mut row = stored_job(running, "running", "running", 1, 1);
            row.lease_holder = Some("agent 0 (fake)".to_owned());
            store.insert_job(&row).unwrap();
        }
        let fixture = Fixture::with(dir, Duration::from_secs(5), test_build(), 1);
        let (_agent, answers) =
            FakeAgent::rejoin(fixture.port, TOKENS[0], vec![resume_entry(running, 1, 3)]);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].action, ResumeAction::Cancel);
    }
}
