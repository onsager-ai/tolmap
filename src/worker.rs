//! The versioned one-job process protocol. The child never opens the master
//! database; it receives inputs and returns paths to files in shared storage.

use std::io::{BufRead, Write};
use std::path::PathBuf;

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
    pub cache_dir: String,
    pub output_dir: String,
    pub max_clone_bytes: u64,
    pub max_history_commits: usize,
    pub max_files: usize,
    pub prune_variant: String,
    pub namer: String,
    pub namer_model: String,
    pub previous_map: Option<String>,
    pub names_cache: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerEvent {
    StageStarted { v: u8, stage: StageId },
    Progress { v: u8, value: ProgressValue },
    StageFinished { v: u8, stage: StageId, duration_s: f64, success: bool },
    Log { v: u8, message: String },
    Result { v: u8, map_path: String, symbols_path: String, symbols_dir: String,
        names_cache: String, commit: String, branch: Option<String>,
        lang: String, files: usize, districts: usize, modularity: f64 },
    Error { v: u8, code: String, message: String },
}

impl WorkerEvent {
    pub fn version(&self) -> u8 {
        match self {
            Self::StageStarted { v, .. } | Self::Progress { v, .. } |
            Self::StageFinished { v, .. } | Self::Log { v, .. } |
            Self::Result { v, .. } | Self::Error { v, .. } => *v,
        }
    }
}

pub fn run_stdio() -> Result<()> {
    let stdin = std::io::stdin();
    let line = stdin.lock().lines().next().context("missing worker spec")??;
    let spec: WorkerSpec = serde_json::from_str(&line).context("parse worker spec")?;
    anyhow::ensure!(spec.v == 1, "unsupported worker protocol version {}", spec.v);
    let output = std::sync::Mutex::new(std::io::BufWriter::new(std::io::stdout()));
    let emit = move |event: WorkerEvent| {
        let mut output = output.lock().expect("worker stdout mutex");
        serde_json::to_writer(&mut *output, &event).expect("serialize worker event");
        output.write_all(b"\n").expect("write worker event");
        output.flush().expect("flush worker event");
    };
    let progress = Progress::new(emit);
    match run(spec, &progress) {
        Ok(event) => (progress_event(&progress, event)),
        Err(error) => progress_event(&progress, WorkerEvent::Error {
            v: 1, code: error.code, message: error.message,
        }),
    }
    Ok(())
}

fn progress_event(progress: &Progress, event: WorkerEvent) {
    // A one-off event uses the same sink as stage counters.
    progress.emit_event(event);
}

pub struct WorkerError { pub code: String, pub message: String }

fn run(spec: WorkerSpec, progress: &Progress) -> std::result::Result<WorkerEvent, WorkerError> {
    use crate::service::{clone, config::Limits, store};
    use crate::{detect, extract, geometry, naming, symbols};
    let fail = |code: &str, message: String| WorkerError { code: code.to_owned(), message };
    let repo_ref = clone::RepoRef {
        slug: spec.slug, owner: spec.owner, repo: spec.repo,
        source: if spec.local { clone::RepoSource::Local(PathBuf::from(spec.source)) }
            else { clone::RepoSource::Remote(spec.source) },
    };
    let limits = Limits { max_clone_bytes: spec.max_clone_bytes,
        max_history_commits: spec.max_history_commits, max_files: spec.max_files,
        ..Limits::default() };
    let clone_stage = progress.stage(StageId::Clone, None);
    let materialized = clone::materialize_with_progress(
        &PathBuf::from(spec.cache_dir), &repo_ref, &limits, progress,
    ).map_err(|error| WorkerError { code: error.body.error, message: error.body.message })?;
    clone_stage.finish();
    let detect_stage = progress.stage(StageId::Detect, Some(1));
    let detection = detect::detect(&materialized.path)
        .map_err(|error| fail("detection_failed", error.to_string()))?;
    let chosen = detection.chosen;
    if chosen.confidence == detect::Confidence::Low {
        return Err(fail("detection_uncertain", chosen.describe()));
    }
    if chosen.file_count > limits.max_files {
        return Err(fail("repo_too_large", format!("file count {} exceeds the configured limit of {}", chosen.file_count, limits.max_files)));
    }
    detect_stage.set(1);
    detect_stage.finish();

    let graph = extract::build_with_progress(&materialized.path, &chosen.pkg, chosen.language, progress)
        .map_err(|error| fail("index_failed", format!("{error:#}")))?;
    let nodes = graph.nodes.clone();
    let previous = spec.previous_map.as_ref()
        .and_then(|path| store::read_map_document(&PathBuf::from(path)).ok());
    let output_dir = PathBuf::from(spec.output_dir);
    std::fs::create_dir_all(&output_dir)
        .map_err(|error| fail("internal_error", error.to_string()))?;
    let names_path = output_dir.join(format!("{}.names.json", repo_ref.repo));
    if let Some(cache) = spec.names_cache {
        std::fs::copy(cache, &names_path)
            .map_err(|error| fail("internal_error", error.to_string()))?;
    }
    let map_path = geometry::build_from_graph_warm_with_progress(
        graph, repo_ref.repo.clone(), &output_dir, 1.1,
        geometry::BuildFeatures { parcels: true,
            prune_variant: spec.prune_variant.parse().map_err(|error: String| fail("internal_error", error))?,
            namer: spec.namer.parse().map_err(|error: &str| fail("internal_error", error.to_owned()))?,
            namer_model: spec.namer_model,
        }, previous.as_ref(), progress,
    ).map_err(|error| fail("index_failed", format!("{error:#}")))?;
    symbols::write_sibling_with_progress(&materialized.path, &nodes, &map_path, progress)
        .map_err(|error| fail("index_failed", format!("{error:#}")))?;
    let document = store::read_map_document(&map_path)
        .map_err(|error| fail("internal_error", error.to_string()))?;
    Ok(WorkerEvent::Result { v: 1,
        symbols_path: map_path.with_extension("symbols.json").to_string_lossy().into_owned(),
        symbols_dir: map_path.with_extension("symbols").to_string_lossy().into_owned(),
        map_path: map_path.to_string_lossy().into_owned(),
        names_cache: names_path.to_string_lossy().into_owned(),
        commit: materialized.commit, branch: materialized.branch,
        lang: document.lang, files: document.files.len(),
        districts: document.districts.len(), modularity: document.q,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_round_trip() {
        let event = WorkerEvent::Progress { v: 1, value: ProgressValue {
            stage: StageId::Parse, stage_index: 7, stage_count: 17,
            label: "Parsing files".to_owned(), unit: "files".to_owned(),
            done: 3, total: Some(7), rate_per_s: Some(2.5),
        }};
        let decoded: WorkerEvent = serde_json::from_slice(&serde_json::to_vec(&event).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(event).unwrap(), serde_json::to_value(decoded).unwrap());
    }
}
