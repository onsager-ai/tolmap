//! The versioned one-job process protocol. The child never opens the master
//! database; it receives inputs and returns paths to files in shared storage.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::progress::{Progress, ProgressValue, StageId};

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct WorkerSpec {
    pub v: u8,
    pub slug: String,
    pub owner: String,
    pub repo: String,
    pub source: String,
    pub local: bool,
    pub all_sources: bool,
    pub cache_dir: String,
    pub output_dir: String,
    pub clone_cache_bytes: u64,
    pub prune_variant: String,
    pub namer: String,
    pub namer_model: String,
    pub previous_maps: Vec<PreviousMap>,
    pub names_cache: Option<String>,
    // Issue #110: "hand" (the default, also when absent) or "scip", as
    // `tolmap build --refs`. The service always sends its configured
    // `TOLMAP_REFS`; only a service from before the option omits it, and
    // that service meant the hand-written resolver. Optional on the wire so
    // the two still agree.
    #[serde(default)]
    pub refs: Option<String>,
    // Issue #110 P1c: "sandbox" when the service will run a TypeScript
    // dependency install for this job on request (`WorkerEvent::
    // InstallRequest`), absent otherwise. The worker itself never installs:
    // it runs unprivileged, and the sandbox needs root to start (see
    // `indexers::install`). Absent from the wire when unset, so a spec
    // without installs serializes exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct PreviousMap {
    pub branch: Option<String>,
    pub path: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, TS)]
pub struct RepoFeatures {
    pub clone_bytes: Option<u64>,
    pub commits: Option<u64>,
    pub languages: BTreeMap<String, LanguageFeatures>,
    // "scip" when the job indexes with SCIP: the ETA model only expects the
    // indexing stages then. Absent for hand-written jobs, so their feature
    // rows serialize exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs: Option<String>,
    // Issue #110 P1c: the package manager ("pnpm" or "npm") of the
    // dependency install this job expects to run before scip-typescript.
    // Absent when no install is planned, so the ETA model only expects the
    // install stage when there will be one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, TS)]
pub struct LanguageFeatures {
    pub files: u64,
    pub bytes: u64,
}

// A named file a worker agent uploaded on the artifact channel (docs/
// WORKER_TIER.md §3.4, §4.2), reported in a `result` so the master can
// register it without trusting a path the worker chose. Issue #97 phase 1,
// owner's change: `symbols_dir` is no longer one tar; each district file
// under it becomes its own artifact, named `symbols_dir/<file name>`, so
// one failed upload costs one district, not the whole result. A plain `//`
// comment, not `///`: exported to TypeScript (it is now reachable from
// `WorkerEvent::Result.artifacts`), and none of this file's other wire
// structs carry a doc comment onto the wire either.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct Artifact {
    pub name: String,
    pub sha256: String,
    pub bytes: u64,
}

/// Whether `name` is one a `result` event's `artifacts` list may use: the
/// three fixed names, or one district file inside `symbols_dir`. Mirrors
/// `service::worker_result::is_district_file`'s digits-only rule -- both
/// sides must agree on what an entry there can be named -- and, like it,
/// rejects anything with an extra `/` or a `..` component by construction:
/// `strip_prefix`/`strip_suffix` leave those out of the all-digits check.
pub fn is_valid_artifact_name(name: &str) -> bool {
    match name {
        "map" | "symbols" | "names" => true,
        _ => name
            .strip_prefix("symbols_dir/")
            .and_then(|entry| entry.strip_suffix(".json"))
            .is_some_and(|digits| {
                !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
            }),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerEvent {
    StageStarted {
        v: u8,
        stage: StageId,
    },
    Progress {
        v: u8,
        value: ProgressValue,
    },
    StageFinished {
        v: u8,
        stage: StageId,
        duration_s: f64,
        success: bool,
    },
    Features {
        v: u8,
        features: RepoFeatures,
    },
    Log {
        v: u8,
        message: String,
    },
    Result {
        v: u8,
        map_path: String,
        symbols_path: String,
        symbols_dir: String,
        names_cache: String,
        commit: String,
        branch: Option<String>,
        lang: String,
        files: usize,
        districts: usize,
        modularity: f64,
        // Issue #97 phase 1: the remote agent's uploaded artifacts (docs/
        // WORKER_TIER.md §3.4), absent when this event comes straight from
        // a job child's own stdout (today's only path) or from local mode.
        // `skip_serializing_if` keeps a v1-only result byte-identical to
        // before this field existed, which
        // `a_result_without_artifacts_serializes_exactly_as_before` checks
        // against a literal.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        artifacts: Vec<Artifact>,
    },
    Error {
        v: u8,
        code: String,
        message: String,
    },
    // Issue #110 P1c: sent only when the spec's `install` is "sandbox",
    // just before scip-typescript. The service runs the sandboxed install
    // on the job's checkout and answers with one `InstallCoverage` JSON
    // line on the worker's stdin; the worker blocks until it does.
    InstallRequest {
        v: u8,
    },
}

impl WorkerEvent {
    pub fn version(&self) -> u8 {
        match self {
            Self::StageStarted { v, .. }
            | Self::Progress { v, .. }
            | Self::StageFinished { v, .. }
            | Self::Features { v, .. }
            | Self::Log { v, .. }
            | Self::Result { v, .. }
            | Self::Error { v, .. }
            | Self::InstallRequest { v } => *v,
        }
    }
}

/// The portable half of `WorkerSpec` (docs/WORKER_TIER.md §3.3): what the
/// job is, with no filesystem path in it, so `assign` can carry it over the
/// network. A remote agent turns it into an ordinary v1 `WorkerSpec` after
/// cloning and checking out `commit` itself; the job child then cannot tell
/// whether it runs under `tolmap serve` or under an agent.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct JobSpec {
    pub slug: String,
    pub owner: String,
    pub repo: String,
    pub source: String,
    pub local: bool,
    // The commit the master resolved and admitted the job against (§3.3):
    // the agent checks out this commit, not whatever the branch points to
    // by the time it clones, so `(slug, commit)` stays the cache key even
    // if the branch moved on in between. `WorkerSpec` has no field for this
    // -- by the time `to_worker_spec` runs, whatever needed the pin has
    // already happened -- so it never crosses into the v1 spec.
    pub commit: String,
    pub all_sources: bool,
    pub prune_variant: String,
    pub namer: String,
    pub namer_model: String,
    #[serde(default)]
    pub refs: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<String>,
}

/// The filesystem half of a v1 `WorkerSpec` (docs/WORKER_TIER.md §3.3):
/// local mode already has all of this on hand, and a remote agent derives
/// it after fetching the job's inputs (§4.2). Never serialized itself --
/// only `JobSpec` and the artifact/input URLs cross the network -- so it
/// carries no serde or ts-rs derive.
#[derive(Clone, Debug)]
pub struct LocalInputs {
    pub cache_dir: String,
    pub output_dir: String,
    pub clone_cache_bytes: u64,
    pub previous_maps: Vec<PreviousMap>,
    pub names_cache: Option<String>,
}

impl JobSpec {
    /// Builds the v1 `WorkerSpec` a job child reads from stdin: the same
    /// shape `tolmap serve` builds today for a local job (`src/service/
    /// jobs.rs`), assembled in this one place so a remote agent and local
    /// mode cannot drift apart on it.
    pub fn to_worker_spec(&self, local: LocalInputs) -> WorkerSpec {
        WorkerSpec {
            v: 1,
            slug: self.slug.clone(),
            owner: self.owner.clone(),
            repo: self.repo.clone(),
            source: self.source.clone(),
            local: self.local,
            all_sources: self.all_sources,
            cache_dir: local.cache_dir,
            output_dir: local.output_dir,
            clone_cache_bytes: local.clone_cache_bytes,
            prune_variant: self.prune_variant.clone(),
            namer: self.namer.clone(),
            namer_model: self.namer_model.clone(),
            previous_maps: local.previous_maps,
            names_cache: local.names_cache,
            refs: self.refs.clone(),
            install: self.install.clone(),
        }
    }
}

// --- Issue #97 phase 1: the channel protocol between a remote worker agent
// and the master (docs/WORKER_TIER.md §3). Distinct from `v` above, which
// stays the job child's own protocol version and is carried unchanged
// inside `WorkerMessage::JobEvent`.

/// The channel protocol version (§3.1): negotiated once in `hello`/
/// `welcome`, separate from the job child's `WorkerEvent::version`.
pub const PROTO: u32 = 1;

/// The largest control frame the master accepts (§4.3). Bounds the
/// protocol, not a job -- the largest legitimate frame today is a
/// `features` or `log` event -- so it never limits a repository's size.
pub const MAX_CONTROL_FRAME_BYTES: usize = 1 << 20;

/// `hello.features` values a worker may advertise (§3.6). A peer sends a
/// message type gated on one of these only if the other side listed it
/// first, so a new feature never needs a `proto` bump.
pub const FEATURE_INSTALL_SANDBOX: &str = "install_sandbox";
pub const FEATURE_RESUME: &str = "resume";
pub const FEATURE_LOCAL_PATHS: &str = "local_paths";

/// Why `negotiate` could not agree on a `proto`: the wire error code the
/// master then sends in `MasterMessage::Error` before closing (§3.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnsupportedProto;

impl std::fmt::Display for UnsupportedProto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("unsupported_proto")
    }
}

impl std::error::Error for UnsupportedProto {}

/// The channel protocol version both sides can speak: the highest version
/// at most both `proto_max`s allow and at least both `proto_min`s require
/// (§3.6). `ours` and `theirs` are each `(proto_min, proto_max)`.
pub fn negotiate(ours: (u32, u32), theirs: (u32, u32)) -> Result<u32, UnsupportedProto> {
    let (our_min, our_max) = ours;
    let (their_min, their_max) = theirs;
    let highest = our_max.min(their_max);
    if highest >= our_min.max(their_min) {
        Ok(highest)
    } else {
        Err(UnsupportedProto)
    }
}

/// `hello.build` (§3.2): identifies the exact build a worker runs, since a
/// different build or indexer version can produce a different map for the
/// same `(slug, commit)` (§3.6, "build identity is a scheduling
/// constraint"). The master only assigns jobs to agents whose `build`
/// matches its own.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct WorkerBuild {
    pub version: String,
    pub commit: String,
    pub indexers: BTreeMap<String, String>,
}

/// `hello.class` (§3.2): the worker host's capacity, used for scheduling.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct WorkerClass {
    pub memory_bytes: u64,
    pub cpus: u32,
}

/// One entry of `hello.resume[]` (§3.2, §3.5): a job this worker still holds
/// across a reconnect, so the master can answer `continue` or `cancel`.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct ResumeEntry {
    pub job_id: String,
    pub epoch: u64,
    pub last_seq: u64,
}

/// One entry of `heartbeat.jobs[]` (§3.2): distinct from `ResumeEntry`
/// because the two tables describe different frames, even though today
/// their fields coincide.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct HeartbeatJob {
    pub job_id: String,
    pub epoch: u64,
    pub last_seq: u64,
}

/// `released.reason` (§3.2): why the agent stopped a job and freed its
/// memory. A worker-originated set, distinct from `CancelReason` -- `oom`
/// and `worker_stopping` never arrive as a reason to cancel, since the
/// master does not originate them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ReleasedReason {
    Cancelled,
    LeaseLost,
    ServerStopping,
    Reroute,
    WorkerStopping,
    Oom,
}

/// `cancel.reason` (§3.2): why the master is stopping a job now. A
/// master-originated set, distinct from `ReleasedReason`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CancelReason {
    Cancelled,
    LeaseLost,
    ServerStopping,
    Reroute,
}

/// `shutdown.mode` (§3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownMode {
    Drain,
    Now,
}

/// `welcome.resume[].action` (§3.2, §3.5): whether the master still honours
/// a job the worker reported holding in its `hello`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ResumeAction {
    Continue,
    Cancel,
}

/// One entry of `welcome.resume[]` (§3.2, §3.5).
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct WelcomeResume {
    pub job_id: String,
    pub action: ResumeAction,
    pub acked_seq: u64,
}

/// One entry of `assign.inputs.previous_maps[]` (§3.3): the same shape as
/// `PreviousMap`, but a fetchable URL rather than a local path, since only
/// `JobSpec` and its input URLs cross the network.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct PreviousMapUrl {
    pub branch: Option<String>,
    // Issue #97 phase 1, the loopback PR: the commit the stored map was
    // built from. The agent needs it for the same reason local mode does:
    // `executor::stage_previous_map` names the job child's copy
    // `<commit>.json`, and the child logs `warm start from <commit>` from
    // that name, which the image e2e asserts on. A URL is opaque to the
    // agent, so the commit travels beside it. Absent on the wire from a
    // master that predates it, which the agent treats as a cold start (the
    // empty string is not an object id).
    #[serde(default)]
    pub commit: String,
    pub url: String,
}

/// `assign.inputs` (§3.3): the input URLs a remote agent downloads before it
/// can write the job child's own `WorkerSpec`.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct AssignInputs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub names_cache: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub previous_maps: Vec<PreviousMapUrl>,
}

/// A session message a worker agent sends the master (§3.1, §3.2). Not
/// exported to TypeScript: the web client never opens this channel, only
/// `tolmap-agent` does, so there is nothing here for `generate-types` to
/// give it.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerMessage {
    Hello {
        proto_min: u32,
        proto_max: u32,
        worker_id: String,
        build: WorkerBuild,
        class: WorkerClass,
        slots: u32,
        features: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        resume: Vec<ResumeEntry>,
    },
    Ready {
        slots_free: u32,
    },
    JobEvent {
        job_id: String,
        epoch: u64,
        seq: u64,
        event: WorkerEvent,
        // Issue #97 phase 1, the loopback PR: the job child's own peak RSS
        // (`wait4`, docs/WORKER_TIER.md §2.1), set on the terminal event --
        // the forwarded `result` or `error` -- once the agent has reaped the
        // child. In local mode the master reaps the child itself and records
        // the peak next to the job's timings (#149); an agent reaps it on
        // another host, so the figure has to cross the channel, and the
        // envelope is where it can without touching the v1 event inside.
        // `released` already carries the same field for a stopped job.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        peak_rss_bytes: Option<u64>,
    },
    Heartbeat {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        jobs: Vec<HeartbeatJob>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rss_bytes: Option<u64>,
    },
    Released {
        job_id: String,
        epoch: u64,
        reason: ReleasedReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        peak_rss_bytes: Option<u64>,
    },
    Draining,
}

/// A session message the master sends a worker agent (§3.1, §3.2). Not
/// exported to TypeScript, for the same reason as `WorkerMessage`.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MasterMessage {
    Welcome {
        proto: u32,
        heartbeat_s: u64,
        lease_ttl_s: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        resume: Vec<WelcomeResume>,
    },
    Assign {
        job_id: String,
        epoch: u64,
        lease_ttl_s: u64,
        job: JobSpec,
        inputs: AssignInputs,
        outputs: String,
    },
    LeaseRenewed {
        job_id: String,
        epoch: u64,
        expires_in_s: u64,
        acked_seq: u64,
    },
    Cancel {
        job_id: String,
        epoch: u64,
        reason: CancelReason,
    },
    ResultAccepted {
        job_id: String,
        epoch: u64,
        reason: String,
    },
    ResultRejected {
        job_id: String,
        epoch: u64,
        reason: String,
    },
    Shutdown {
        mode: ShutdownMode,
        reason: String,
    },
    Error {
        code: String,
        message: String,
    },
}

pub fn run_stdio() -> Result<()> {
    let stdin = std::io::stdin();
    let line = stdin
        .lock()
        .lines()
        .next()
        .context("missing worker spec")??;
    let spec: WorkerSpec = serde_json::from_str(&line).context("parse worker spec")?;
    anyhow::ensure!(
        spec.v == 1,
        "unsupported worker protocol version {}",
        spec.v
    );
    let output = std::sync::Mutex::new(std::io::BufWriter::new(std::io::stdout()));
    let emit = move |event: WorkerEvent| {
        let mut output = output.lock().expect("worker stdout mutex");
        serde_json::to_writer(&mut *output, &event).expect("serialize worker event");
        output.write_all(b"\n").expect("write worker event");
        output.flush().expect("flush worker event");
    };
    let progress = Progress::new(emit);
    match run(spec, &progress) {
        Ok(event) => progress_event(&progress, event),
        Err(error) => progress_event(
            &progress,
            WorkerEvent::Error {
                v: 1,
                code: error.code,
                message: error.message,
            },
        ),
    }
    Ok(())
}

fn progress_event(progress: &Progress, event: WorkerEvent) {
    // A one-off event uses the same sink as stage counters.
    progress.emit_event(event);
}

pub struct WorkerError {
    pub code: String,
    pub message: String,
}

fn run(spec: WorkerSpec, progress: &Progress) -> std::result::Result<WorkerEvent, WorkerError> {
    use crate::service::{clone, config::Limits, store};
    use crate::{detect, extract, geometry, symbols};
    let fail = |code: &str, message: String| WorkerError {
        code: code.to_owned(),
        message,
    };
    let repo_ref = clone::RepoRef {
        slug: spec.slug,
        owner: spec.owner,
        repo: spec.repo,
        source: if spec.local {
            clone::RepoSource::Local(PathBuf::from(spec.source))
        } else {
            clone::RepoSource::Remote(spec.source)
        },
    };
    let limits = Limits {
        clone_cache_bytes: spec.clone_cache_bytes,
        ..Limits::default()
    };
    let refs = spec
        .refs
        .as_deref()
        .unwrap_or("hand")
        .parse::<extract::RefsMode>()
        .map_err(|error| fail("internal_error", error))?;
    let clone_stage = progress.stage(StageId::Clone, None);
    let materialized = clone::materialize_with_progress(
        &PathBuf::from(spec.cache_dir),
        &repo_ref,
        &limits,
        progress,
    )
    .map_err(|error| WorkerError {
        code: error.body.error,
        message: error.body.message,
    })?;
    clone_stage.finish();
    let mut features = RepoFeatures {
        clone_bytes: clone::directory_size(&materialized.path).ok(),
        commits: Command::new("git")
            .arg("-C")
            .arg(&materialized.path)
            .args(["rev-list", "--count", "HEAD"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|count| count.trim().parse().ok()),
        languages: BTreeMap::new(),
        refs: (refs == extract::RefsMode::Scip).then(|| refs.to_string()),
        install: None,
    };
    progress.emit_event(WorkerEvent::Features {
        v: 1,
        features: features.clone(),
    });
    let detect_stage = progress.stage(StageId::Detect, Some(1));
    let sources = if spec.all_sources {
        detect::all_sources(&materialized.path)
            .map_err(|error| fail("detection_failed", error.to_string()))?
    } else {
        let detection = detect::detect(&materialized.path)
            .map_err(|error| fail("detection_failed", error.to_string()))?;
        let chosen = detection.chosen;
        if chosen.confidence == detect::Confidence::Low {
            return Err(fail("detection_uncertain", chosen.describe()));
        }
        vec![chosen]
    };
    if sources.is_empty() {
        return Err(fail(
            "detection_failed",
            "no source cleared the all-sources floor".to_owned(),
        ));
    }
    for source in &sources {
        let files = extract::source_files(&materialized.path, &source.pkg, source.language)
            .map_err(|error| fail("detection_failed", error.to_string()))?;
        let bytes: u64 = files
            .iter()
            .filter_map(|path| {
                std::fs::metadata(materialized.path.join(path))
                    .ok()
                    .map(|meta| meta.len())
            })
            .sum();
        let row = features
            .languages
            .entry(source.language.as_str().to_owned())
            .or_default();
        row.files += files.len() as u64;
        row.bytes += bytes;
    }
    detect_stage.set(1);
    detect_stage.finish();
    let delegate_installs =
        refs == extract::RefsMode::Scip && spec.install.as_deref() == Some("sandbox");
    if delegate_installs
        && sources
            .iter()
            .any(|source| source.language == extract::LanguageKind::TypeScript)
    {
        // The same file-only policy the service applies before installing;
        // here it only tells the ETA model whether to expect the stage.
        if let crate::indexers::InstallPlan::Install(manager) =
            crate::indexers::install_plan(&materialized.path)
        {
            features.install = Some(manager.as_str().to_owned());
        }
    }
    progress.emit_event(WorkerEvent::Features { v: 1, features });
    let install = if delegate_installs {
        extract::InstallMode::Delegate(std::sync::Arc::new({
            let progress = progress.clone();
            move || request_install(&progress)
        }))
    } else {
        extract::InstallMode::Off
    };

    let source_pairs = sources
        .into_iter()
        .map(|source| (source.pkg, source.language))
        .collect::<Vec<_>>();
    let (graph, symbol_records) = extract::build_multi_source_with_symbols_progress(
        &materialized.path,
        &source_pairs,
        refs,
        &install,
        progress,
    )
    .map_err(|error| fail("index_failed", format!("{error:#}")))?;
    let nodes = graph.nodes.clone();
    let previous_path = spec
        .previous_maps
        .iter()
        .find(|row| row.branch == materialized.branch)
        .or_else(|| spec.previous_maps.first())
        .map(|row| &row.path);
    // A previous map that cannot be read is a cold start, not a failed job,
    // but it must say so: issue #141 was a warm start (finding 4: 88%
    // district retention warm, 46% cold) skipped without a trace because
    // this worker's uid could not read the store the path pointed into.
    // These log lines are what the image-build job's end-to-end test
    // asserts on.
    let previous = match previous_path {
        None => {
            progress.log("cold start: no previous map".to_owned());
            None
        }
        Some(path) => {
            let path = PathBuf::from(path);
            let commit = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            match store::read_map_document(&path) {
                Ok(document) => {
                    progress.log(format!("warm start from {commit}"));
                    Some(document)
                }
                Err(error) => {
                    progress.log(format!(
                        "cold start: previous map {commit} unreadable: {error:#}"
                    ));
                    None
                }
            }
        }
    };
    let output_dir = PathBuf::from(spec.output_dir);
    std::fs::create_dir_all(&output_dir)
        .map_err(|error| fail("internal_error", error.to_string()))?;
    let names_path = output_dir.join(format!("{}.names.json", repo_ref.repo));
    match spec.names_cache {
        Some(cache) => {
            std::fs::copy(cache, &names_path)
                .map_err(|error| fail("internal_error", error.to_string()))?;
            // "Never rename a district without the previous name in hand"
            // (CLAUDE.md): an empty cache on a repository indexed before is
            // the same silent loss as a cold start, so it is logged too.
            let entries = crate::naming::load_cache(&names_path).len();
            progress.log(format!("names cache: {entries} entries in hand"));
        }
        None => progress.log("names cache: none".to_owned()),
    }
    let map_path = geometry::build_from_graph_warm_with_progress(
        graph,
        repo_ref.repo.clone(),
        &output_dir,
        1.1,
        geometry::BuildFeatures {
            parcels: true,
            prune_variant: spec
                .prune_variant
                .parse()
                .map_err(|error: String| fail("internal_error", error))?,
            namer: spec
                .namer
                .parse()
                .map_err(|error: &str| fail("internal_error", error.to_owned()))?,
            namer_model: spec.namer_model,
            refs,
            // Extraction already ran above; the geometry stage never reads it.
            install: extract::InstallMode::Off,
        },
        previous.as_ref(),
        progress,
    )
    .map_err(|error| fail("index_failed", format!("{error:#}")))?;
    symbols::write_sibling_with_progress(
        &materialized.path,
        &nodes,
        &map_path,
        symbol_records,
        progress,
    )
    .map_err(|error| fail("index_failed", format!("{error:#}")))?;
    let document = store::read_map_document(&map_path)
        .map_err(|error| fail("internal_error", error.to_string()))?;
    Ok(WorkerEvent::Result {
        v: 1,
        symbols_path: map_path
            .with_extension("symbols.json")
            .to_string_lossy()
            .into_owned(),
        symbols_dir: map_path
            .with_extension("symbols")
            .to_string_lossy()
            .into_owned(),
        map_path: map_path.to_string_lossy().into_owned(),
        names_cache: names_path.to_string_lossy().into_owned(),
        commit: materialized.commit,
        branch: materialized.branch,
        lang: document.lang,
        files: document.files.len(),
        districts: document.districts.len(),
        modularity: document.q,
        // The job child never uploads; only a remote agent forwarding this
        // event fills `artifacts` in (a sibling PR).
        artifacts: Vec::new(),
    })
}

/// Asks the service for the sandboxed install (`WorkerEvent::InstallRequest`)
/// and blocks for its one-line `InstallCoverage` answer on stdin. The spec
/// line was read through the same process-wide stdin buffer, so nothing the
/// service wrote after it is lost. Any protocol failure is a fallback, never
/// a failed job: the indexer then runs without installs, as it would with
/// installs off.
fn request_install(progress: &Progress) -> crate::schema::InstallCoverage {
    progress.emit_event(WorkerEvent::InstallRequest { v: 1 });
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(count) if count > 0 => {
            match serde_json::from_str::<crate::schema::InstallCoverage>(line.trim_end()) {
                Ok(coverage) => coverage,
                Err(error) => {
                    progress.log(format!("install: unreadable service reply: {error}"));
                    crate::indexers::fell_back("sandbox_unavailable", None)
                }
            }
        }
        Ok(_) => {
            progress.log("install: the service closed the channel without a reply".to_owned());
            crate::indexers::fell_back("sandbox_unavailable", None)
        }
        Err(error) => {
            progress.log(format!(
                "install: reading the service reply failed: {error}"
            ));
            crate::indexers::fell_back("sandbox_unavailable", None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_round_trip() {
        let event = WorkerEvent::Progress {
            v: 1,
            value: ProgressValue {
                stage: StageId::Parse,
                stage_index: 6,
                stage_count: 18,
                label: "Parsing files".to_owned(),
                unit: "files".to_owned(),
                done: 3,
                total: Some(7),
                rate_per_s: Some(2.5),
                transfer_bytes: None,
                transfer_rate_bytes_per_s: None,
            },
        };
        let decoded: WorkerEvent =
            serde_json::from_slice(&serde_json::to_vec(&event).unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::to_value(decoded).unwrap()
        );
    }

    fn round_trips<T: Serialize + for<'de> Deserialize<'de>>(value: &T) -> serde_json::Value {
        let decoded: T = serde_json::from_slice(&serde_json::to_vec(value).unwrap()).unwrap();
        let original = serde_json::to_value(value).unwrap();
        assert_eq!(original, serde_json::to_value(decoded).unwrap());
        original
    }

    fn sample_job_spec() -> JobSpec {
        JobSpec {
            slug: "django/django".to_owned(),
            owner: "django".to_owned(),
            repo: "django".to_owned(),
            source: "https://github.com/django/django.git".to_owned(),
            local: false,
            commit: "a".repeat(40),
            all_sources: false,
            prune_variant: "node-relative".to_owned(),
            namer: "idf".to_owned(),
            namer_model: String::new(),
            refs: Some("hand".to_owned()),
            install: None,
        }
    }

    /// Every `WorkerMessage` variant round-trips through JSON unchanged.
    #[test]
    fn worker_message_variants_round_trip() {
        round_trips(&WorkerMessage::Hello {
            proto_min: 1,
            proto_max: 1,
            worker_id: "w-1".to_owned(),
            build: WorkerBuild {
                version: "0.1.0".to_owned(),
                commit: "b".repeat(40),
                indexers: BTreeMap::from([("scip-python".to_owned(), "0.6.6".to_owned())]),
            },
            class: WorkerClass {
                memory_bytes: 8_000_000_000,
                cpus: 4,
            },
            slots: 2,
            features: vec![
                FEATURE_RESUME.to_owned(),
                FEATURE_INSTALL_SANDBOX.to_owned(),
            ],
            resume: vec![ResumeEntry {
                job_id: "job-1".to_owned(),
                epoch: 1,
                last_seq: 12,
            }],
        });
        round_trips(&WorkerMessage::Ready { slots_free: 2 });
        round_trips(&WorkerMessage::JobEvent {
            job_id: "job-1".to_owned(),
            epoch: 1,
            seq: 1,
            event: WorkerEvent::StageStarted {
                v: 1,
                stage: StageId::Clone,
            },
            peak_rss_bytes: Some(3_000_000),
        });
        round_trips(&WorkerMessage::Heartbeat {
            jobs: vec![HeartbeatJob {
                job_id: "job-1".to_owned(),
                epoch: 1,
                last_seq: 12,
            }],
            rss_bytes: Some(1_000_000),
        });
        round_trips(&WorkerMessage::Released {
            job_id: "job-1".to_owned(),
            epoch: 1,
            reason: ReleasedReason::Oom,
            peak_rss_bytes: Some(2_000_000),
        });
        round_trips(&WorkerMessage::Draining);
    }

    /// Every `MasterMessage` variant round-trips through JSON unchanged.
    #[test]
    fn master_message_variants_round_trip() {
        round_trips(&MasterMessage::Welcome {
            proto: PROTO,
            heartbeat_s: 15,
            lease_ttl_s: 60,
            resume: vec![WelcomeResume {
                job_id: "job-1".to_owned(),
                action: ResumeAction::Continue,
                acked_seq: 3,
            }],
        });
        round_trips(&MasterMessage::Assign {
            job_id: "job-1".to_owned(),
            epoch: 1,
            lease_ttl_s: 60,
            job: sample_job_spec(),
            inputs: AssignInputs {
                names_cache: Some("https://master/artifacts/names".to_owned()),
                previous_maps: vec![PreviousMapUrl {
                    branch: Some("main".to_owned()),
                    commit: "d".repeat(40),
                    url: "https://master/artifacts/prev".to_owned(),
                }],
            },
            outputs: "https://master/workers/artifacts/job-1/1".to_owned(),
        });
        round_trips(&MasterMessage::LeaseRenewed {
            job_id: "job-1".to_owned(),
            epoch: 1,
            expires_in_s: 60,
            acked_seq: 3,
        });
        round_trips(&MasterMessage::Cancel {
            job_id: "job-1".to_owned(),
            epoch: 1,
            reason: CancelReason::Reroute,
        });
        round_trips(&MasterMessage::ResultAccepted {
            job_id: "job-1".to_owned(),
            epoch: 1,
            reason: "ok".to_owned(),
        });
        round_trips(&MasterMessage::ResultRejected {
            job_id: "job-1".to_owned(),
            epoch: 1,
            reason: "digest mismatch".to_owned(),
        });
        round_trips(&MasterMessage::Shutdown {
            mode: ShutdownMode::Drain,
            reason: "rolling upgrade".to_owned(),
        });
        round_trips(&MasterMessage::Error {
            code: "unsupported_proto".to_owned(),
            message: "no proto overlap".to_owned(),
        });
    }

    #[test]
    fn an_unknown_optional_field_is_ignored() {
        let message: WorkerMessage = serde_json::from_str(
            r#"{"type":"ready","slots_free":3,"a_field_from_the_future":true}"#,
        )
        .unwrap();
        assert!(matches!(message, WorkerMessage::Ready { slots_free: 3 }));
    }

    #[test]
    fn an_unknown_type_fails_to_parse() {
        assert!(serde_json::from_str::<WorkerMessage>(r#"{"type":"bogus"}"#).is_err());
        assert!(serde_json::from_str::<MasterMessage>(r#"{"type":"bogus"}"#).is_err());
    }

    /// The v1 `WorkerEvent` stream parses unchanged inside `job_event`: the
    /// envelope adds `job_id`/`epoch`/`seq` around it but does not reshape
    /// the event itself.
    #[test]
    fn a_v1_worker_event_parses_unchanged_inside_job_event() {
        let event = WorkerEvent::StageFinished {
            v: 1,
            stage: StageId::Detect,
            duration_s: 1.5,
            success: true,
        };
        let wrapped = WorkerMessage::JobEvent {
            job_id: "job-1".to_owned(),
            epoch: 2,
            seq: 5,
            event: event.clone(),
            peak_rss_bytes: None,
        };
        let value = serde_json::to_value(&wrapped).unwrap();
        assert_eq!(value["event"], serde_json::to_value(&event).unwrap());
        let WorkerMessage::JobEvent {
            event: decoded_event,
            ..
        } = serde_json::from_value::<WorkerMessage>(value).unwrap()
        else {
            panic!("expected a job_event");
        };
        assert_eq!(
            serde_json::to_value(decoded_event).unwrap(),
            serde_json::to_value(&event).unwrap()
        );
    }

    /// Adding `artifacts` must not move a single byte of a result that does
    /// not use it: the literal is what a v1-only consumer parsed before this
    /// field existed.
    #[test]
    fn a_result_without_artifacts_serializes_exactly_as_before() {
        let event = WorkerEvent::Result {
            v: 1,
            map_path: "output/django.json".to_owned(),
            symbols_path: "output/django.symbols.json".to_owned(),
            symbols_dir: "output/django.symbols".to_owned(),
            names_cache: "output/django.names.json".to_owned(),
            commit: "a".repeat(40),
            branch: Some("main".to_owned()),
            lang: "py".to_owned(),
            files: 851,
            districts: 12,
            modularity: 0.5061,
            artifacts: Vec::new(),
        };
        let literal = concat!(
            r#"{"type":"result","v":1,"map_path":"output/django.json","#,
            r#""symbols_path":"output/django.symbols.json","#,
            r#""symbols_dir":"output/django.symbols","#,
            r#""names_cache":"output/django.names.json","#,
            r#""commit":""#,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            r#"","branch":"main","lang":"py","files":851,"#,
            r#""districts":12,"modularity":0.5061}"#,
        );
        assert_eq!(serde_json::to_string(&event).unwrap(), literal);
        // And the reverse: a pre-artifacts line on the wire still parses,
        // with an empty `artifacts`.
        let decoded: WorkerEvent = serde_json::from_str(literal).unwrap();
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            serde_json::to_value(&event).unwrap()
        );
    }

    /// A `WorkerSpec` built today by the service (`src/service/jobs.rs`'s
    /// local dispatch) round-trips through `JobSpec` + `LocalInputs` to an
    /// equal `WorkerSpec`, split by hand here the way a future caller would.
    #[test]
    fn a_worker_spec_round_trips_through_job_spec_and_local_inputs() {
        let original = WorkerSpec {
            v: 1,
            slug: "django/django".to_owned(),
            owner: "django".to_owned(),
            repo: "django".to_owned(),
            source: "/work/checkout".to_owned(),
            local: true,
            all_sources: false,
            cache_dir: "/work/cache".to_owned(),
            output_dir: "/work/output".to_owned(),
            clone_cache_bytes: 20_000_000_000,
            prune_variant: "node-relative".to_owned(),
            namer: "idf".to_owned(),
            namer_model: "gpt".to_owned(),
            previous_maps: vec![PreviousMap {
                branch: Some("main".to_owned()),
                path: "/work/maps/main.json".to_owned(),
            }],
            names_cache: Some("/work/names.json".to_owned()),
            refs: Some("scip".to_owned()),
            install: Some("sandbox".to_owned()),
        };
        let job = JobSpec {
            slug: original.slug.clone(),
            owner: original.owner.clone(),
            repo: original.repo.clone(),
            source: original.source.clone(),
            local: original.local,
            commit: "c".repeat(40),
            all_sources: original.all_sources,
            prune_variant: original.prune_variant.clone(),
            namer: original.namer.clone(),
            namer_model: original.namer_model.clone(),
            refs: original.refs.clone(),
            install: original.install.clone(),
        };
        let local = LocalInputs {
            cache_dir: original.cache_dir.clone(),
            output_dir: original.output_dir.clone(),
            clone_cache_bytes: original.clone_cache_bytes,
            previous_maps: original.previous_maps.clone(),
            names_cache: original.names_cache.clone(),
        };
        let rebuilt = job.to_worker_spec(local);
        assert_eq!(
            serde_json::to_value(&original).unwrap(),
            serde_json::to_value(&rebuilt).unwrap()
        );
    }

    #[test]
    fn negotiate_picks_the_highest_common_version() {
        assert_eq!(negotiate((1, 3), (2, 5)), Ok(3));
        assert_eq!(negotiate((1, 5), (1, 1)), Ok(1));
        assert_eq!(negotiate((1, 1), (1, 1)), Ok(1));
    }

    #[test]
    fn negotiate_fails_with_no_overlap() {
        assert_eq!(negotiate((1, 2), (3, 4)), Err(UnsupportedProto));
        assert_eq!(negotiate((3, 4), (1, 2)), Err(UnsupportedProto));
        assert_eq!(UnsupportedProto.to_string(), "unsupported_proto");
    }

    #[test]
    fn artifact_names_are_the_fixed_three_or_a_digits_only_district_file() {
        for name in [
            "map",
            "symbols",
            "names",
            "symbols_dir/0.json",
            "symbols_dir/12345.json",
        ] {
            assert!(is_valid_artifact_name(name), "{name} should be valid");
        }
        for name in [
            "symbols_dir",
            "symbols_dir/",
            "symbols_dir/0",
            "symbols_dir/0.json.bak",
            "symbols_dir/0a.json",
            "symbols_dir/../map",
            "symbols_dir/0.json/extra",
            "unknown",
            "",
            "map/",
        ] {
            assert!(!is_valid_artifact_name(name), "{name} should be invalid");
        }
    }
}
