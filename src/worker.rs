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
    // `tolmap build --refs`. Optional on the wire so a service and a worker
    // from before the option still agree.
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
    let previous =
        previous_path.and_then(|path| store::read_map_document(&PathBuf::from(path)).ok());
    let output_dir = PathBuf::from(spec.output_dir);
    std::fs::create_dir_all(&output_dir)
        .map_err(|error| fail("internal_error", error.to_string()))?;
    let names_path = output_dir.join(format!("{}.names.json", repo_ref.repo));
    if let Some(cache) = spec.names_cache {
        std::fs::copy(cache, &names_path)
            .map_err(|error| fail("internal_error", error.to_string()))?;
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
}
