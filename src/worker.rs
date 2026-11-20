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
// the executor contract, §4.2), reported in a `result` so the master can
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
        "map" | "symbols" | "names" | "index" => true,
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
        // the executor contract), absent when this event comes straight from
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

/// Pinned job inputs supplied by the application to the isolated executor.
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

/// Executor-owned paths added to a pinned job before starting its child.
#[derive(Clone, Debug)]
pub struct LocalInputs {
    pub cache_dir: String,
    pub output_dir: String,
    pub clone_cache_bytes: u64,
    pub previous_maps: Vec<PreviousMap>,
    pub names_cache: Option<String>,
}

impl JobSpec {
    /// Build the versioned JSON specification consumed by the child.
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

fn admit_chosen(
    chosen: &crate::detect::SourceCandidate,
    confirmed_scope: Option<&str>,
) -> std::result::Result<(), WorkerError> {
    if chosen.confidence == crate::detect::Confidence::Low
        && confirmed_scope != Some(chosen.describe().as_str())
    {
        return Err(WorkerError {
            code: "detection_uncertain".to_owned(),
            message: chosen.describe(),
        });
    }
    Ok(())
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
        &PathBuf::from(&spec.cache_dir),
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
    let sources = if let Ok(value) = std::env::var("TOLMAP_WORKER_LOCAL_SOURCES") {
        let pairs: Vec<(String, String)> = serde_json::from_str(&value)
            .map_err(|error| fail("detection_failed", error.to_string()))?;
        pairs
            .into_iter()
            .map(|(pkg, lang)| {
                let language = extract::LanguageKind::parse(&lang)
                    .map_err(|error| fail("detection_failed", error.to_string()))?;
                let root = materialized
                    .path
                    .join(&pkg)
                    .canonicalize()
                    .map_err(|error| fail("detection_failed", error.to_string()))?;
                if !root.starts_with(
                    materialized
                        .path
                        .canonicalize()
                        .map_err(|error| fail("detection_failed", error.to_string()))?,
                ) {
                    return Err(fail(
                        "detection_failed",
                        "source root escapes repository".into(),
                    ));
                }
                Ok(detect::SourceCandidate {
                    language,
                    pkg,
                    confidence: detect::Confidence::High,
                    file_count: 0,
                    evidence: "explicit local CLI source".into(),
                })
            })
            .collect::<std::result::Result<Vec<_>, WorkerError>>()?
    } else if spec.all_sources {
        detect::all_sources(&materialized.path)
            .map_err(|error| fail("detection_failed", error.to_string()))?
    } else {
        let detection = detect::detect(&materialized.path)
            .map_err(|error| fail("detection_failed", error.to_string()))?;
        let chosen = detection.chosen;
        let confirmed = std::env::var("TOLMAP_WORKER_CONFIRMED_SCOPE").ok();
        admit_chosen(&chosen, confirmed.as_deref())?;
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
    let cache_identity =
        std::fs::read(PathBuf::from(&spec.cache_dir).join("extraction-identity.json"))
            .map(|bytes| crate::incremental::digest(&bytes))
            .unwrap_or_else(|_| crate::incremental::digest(repo_ref.slug.as_bytes()));
    let progress = &progress
        .clone()
        .with_cache(crate::incremental::Cache::local(
            PathBuf::from(&spec.output_dir).join("extraction"),
            cache_identity,
        ));
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
    let output_dir = PathBuf::from(&spec.output_dir);
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
    let excluded_tests = extract::excluded_tests(&materialized.path, &source_pairs)
        .map_err(|error| fail("index_failed", format!("{error:#}")))?;
    let map_path = geometry::build_from_graph_with_exclusions(
        graph,
        Some(excluded_tests),
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
    let metadata = serde_json::json!({"version":1, "indexed_commit":materialized.commit,
        "sources":source_pairs.iter().map(|(pkg, lang)| (pkg, lang.as_str())).collect::<Vec<_>>(),
        "all_sources":spec.all_sources, "extraction":progress.observations(), "prior_snapshot":previous.as_ref().map(|doc| crate::incremental::digest(&serde_json::to_vec(doc).unwrap())),
        "downstream":"recompute_all", "seed":crate::SEED});
    crate::incremental::atomic_write(
        &PathBuf::from(&spec.output_dir).join("index.json"),
        &serde_json::to_vec(&metadata).unwrap(),
    )
    .map_err(|error| fail("internal_error", error.to_string()))?;
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

    #[test]
    fn low_confidence_requires_the_exact_parent_confirmed_scope() {
        let mut chosen = crate::detect::SourceCandidate {
            language: crate::extract::LanguageKind::Python,
            pkg: ".".into(),
            confidence: crate::detect::Confidence::Low,
            file_count: 13,
            evidence: "fallback fixture".into(),
        };
        for confirmation in [None, Some(""), Some("a different root")] {
            let error = admit_chosen(&chosen, confirmation).err().unwrap();
            assert_eq!(error.code, "detection_uncertain");
            assert_eq!(error.message, chosen.describe());
        }
        assert!(admit_chosen(&chosen, Some(&chosen.describe())).is_ok());
        for confidence in [
            crate::detect::Confidence::Medium,
            crate::detect::Confidence::High,
        ] {
            chosen.confidence = confidence;
            assert!(admit_chosen(&chosen, None).is_ok());
        }
    }

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
