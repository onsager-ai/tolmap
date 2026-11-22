//! `tolmap check` (issue #170, HANDOFF.md item 2, docs/CHECK.md): what a
//! diff does to the map, as two numbers a CI job can gate on.
//!
//! - **Districts crossed**: the distinct base-map districts holding a changed
//!   file. Renames are followed to their base path. A new file is placed in
//!   the district its resolved imports point to most (by blended weight); a
//!   new file with no resolved import into a base district is *unplaced*,
//!   never guessed.
//! - **Δq**: `Q(P, G_head) - Q(P, G_base)`, with `P` the base map's
//!   partition (extended with the placed new files) held fixed and only the
//!   graph changing. Nothing is re-partitioned: a Leiden run would move with
//!   its own seed and cost minutes, and it would measure the new partition
//!   rather than what the diff did to the old one.
//!
//! Both graphs come from the same stages a map build runs up to the
//! partitioner -- detection (on the base checkout), extraction, then blend
//! and prune with the build's defaults -- so the graph `Q` is measured on is
//! the graph the partition was made from. `Q` is the partition stage's own
//! objective: weighted modularity with the configuration null model at the
//! build's resolution (`RBConfigurationVertexPartition`, γ = 1.1). That is
//! not the map's `q`, which `native/leiden_bridge.cpp` reports topology-only
//! (unweighted, γ = 1) on the pre-merge membership; the two are not
//! comparable and docs/CHECK.md says so.
//!
//! **Co-change is the base's on both sides** (issue #176, finding 61). The
//! head graph takes its imports, symbol uses, names and paths from head, but
//! its co-change from the base commit's history. Head's own `git log` is one
//! commit longer: its 4000-commit window slides by one, and the diff's own
//! commit adds co-change. That moved Δq on commits that change no mapped
//! file, down to -0.0042 on django (finding 60), deeper than the narrow
//! coupling changes the check exists to catch.
//!
//! Every number is computed on tolmap's own reference graph, which misses
//! what it cannot resolve (CLAUDE.md: numbers are a lower bound), and the
//! report says so in `lower_bound`.
//!
//! The user's checkout is never touched: base (and `--head`, when given) are
//! checked out into temporary `git worktree`s, with hooks off, and removed
//! afterwards. A worktree rather than `git archive` because extraction reads
//! the base's `git log` for co-change, and the head worktree reads it too,
//! through the object store they share.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::detect::{self, Confidence};
use crate::extract::{self, round_to, CochangeHistory, InstallMode, LanguageKind, RefsMode};
use crate::geometry::{self, BuildFeatures};
use crate::lens::{self, Lens, LensTargetArchitecture};
use crate::naming::{self, NamerKind};
use crate::pipeline::{self, PruneVariant};
use crate::schema::{
    CheckCochangeExpected, CheckDefaultThresholds, CheckDistrict, CheckEdge, CheckEdgeCause,
    CheckFile, CheckFileStatus, CheckLandmark, CheckOutsideGraph, CheckReport, CheckSpread,
    CheckThresholdSource, CheckThresholds, CheckVerdict, GraphData, MapDocument, NodeRow,
};

/// `CheckReport::version`. Bumped on any change to a field's meaning or
/// shape; docs/CHECK.md has the policy.
pub const REPORT_VERSION: u32 = 1;

/// The resolution every map build partitions at (`tolmap build`'s default
/// and the service's fixed value), so `Q` is the partitioner's objective.
pub const RESOLUTION: f64 = 1.1;

/// Decimal places every float in the report is rounded to. Six keeps a
/// single edge's effect on a large repository visible (Δq of order 1e-5)
/// while making the JSON byte-identical across platforms.
const ROUND_PLACES: i32 = 6;

/// Edges listed in the text format; JSON lists all of them.
pub const TEXT_EDGE_LIMIT: usize = 20;

/// Pruning-caused edges listed in the text format, after their count.
pub const TEXT_PRUNE_LIMIT: usize = 3;

/// Expected co-change pairs shown in text; JSON lists all of them.
const TEXT_COCHANGE_LIMIT: usize = 5;

/// Landmark kinds reported as context (issue #170: hazard and bridge).
const TOUCHED_LANDMARKS: [&str; 2] = ["bridge", "hazard"];

/// The default `--max-districts`, applied when no threshold flag is given.
/// Calibrated on 600 real commits (docs/FINDINGS.md finding 60): the last
/// 200 first-parent commits of django, prometheus and vuejs/core. Their p99
/// is 3 or 4 districts, and the tail above it is a clear knee (one commit
/// at 5, then only 7s), so 4 fires on 2, 2 and 1 of the 200 (1%, 1%,
/// 0.5%). It is absolute rather than a share of the map's districts: the
/// three maps have 6-8 and 11-15 districts, yet their tails are the same in
/// absolute terms, and a share would fire most on the map with the fewest
/// districts (3 of vue's 200 at 40%, against 2 and 2).
pub const DEFAULT_MAX_DISTRICTS: usize = 4;

/// The default `--max-dq`, applied when no threshold flag is given.
/// Recalibrated on the same 600 commits once co-change was held at the
/// base (issue #176, finding 61). A commit that changes no mapped file now
/// gives Δq exactly 0, and one that adds no cross-district edge moves it by
/// at most 0.000064. Commits adding 1-3 cross-district imports drop it by
/// 0.000083-0.000572. 0.0001 sits between the two: it fires on 8 of django's
/// 9 such commits and all 4 of vuejs/core's, and on 2 commits that add a
/// cross-district edge made of naming similarity alone. (The first
/// calibration, finding 60, set 0.01 above a history-drift floor of 0.0042
/// that no longer exists.)
pub const DEFAULT_MAX_DQ: f64 = 0.0001;

pub const LOWER_BOUND_NOTE: &str = "numbers are a lower bound: tolmap's graph holds only the references it can resolve (calls through variables, dynamic imports and reflection are missed), so the change couples at least this much";

#[derive(Clone, Debug)]
pub struct CheckOptions {
    pub repo: PathBuf,
    pub base: String,
    pub head: Option<String>,
    pub base_map: Option<PathBuf>,
    pub target: Option<PathBuf>,
    pub pkg: Vec<String>,
    pub lang: Vec<String>,
    pub all_sources: bool,
    pub max_districts: Option<usize>,
    pub max_dq: Option<f64>,
    /// No threshold at all: the check reports and exits 0 (PR 1's
    /// behaviour). Cannot be combined with a threshold flag.
    pub report_only: bool,
}

/// The thresholds a run applies. Any threshold flag replaces the defaults
/// as a set, so a caller that names only `--max-dq` gets only `--max-dq`:
/// the defaults never mix with an explicit choice.
pub fn resolve_thresholds(options: &CheckOptions) -> Result<CheckThresholds, CheckError> {
    let given = options.max_districts.is_some() || options.max_dq.is_some();
    if options.report_only && given {
        return Err(input(
            "--report-only applies no threshold; it cannot be combined with --max-districts or --max-dq",
        ));
    }
    if let Some(max_dq) = options.max_dq {
        if !(max_dq.is_finite() && max_dq >= 0.0) {
            return Err(input(format!(
                "--max-dq {max_dq} is not a modularity drop: pass a number >= 0 (0.005 fails a change that lowers q by more than 0.005)"
            )));
        }
    }
    Ok(if options.report_only {
        CheckThresholds {
            max_districts: None,
            max_dq: None,
            source: CheckThresholdSource::ReportOnly,
        }
    } else if given {
        CheckThresholds {
            max_districts: options.max_districts,
            max_dq: options.max_dq.map(round),
            source: CheckThresholdSource::Flags,
        }
    } else {
        CheckThresholds {
            max_districts: Some(DEFAULT_MAX_DISTRICTS),
            max_dq: Some(DEFAULT_MAX_DQ),
            source: CheckThresholdSource::Default,
        }
    })
}

/// Why a check produced no report. The variant is the exit code's class:
/// consumers depend on the code, not on the message.
#[derive(Debug)]
pub enum CheckError {
    /// Exit 2: a bad ref, a missing or unverifiable `--base-map`, detection
    /// refused, a bad flag value.
    Input(String),
    /// Exit 3: anything else -- the check itself failed.
    Internal(anyhow::Error),
}

impl CheckError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Input(_) => 2,
            Self::Internal(_) => 3,
        }
    }
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input(message) => write!(out, "{message}"),
            Self::Internal(error) => write!(out, "internal error: {error:#}"),
        }
    }
}

fn input(message: impl Into<String>) -> CheckError {
    CheckError::Input(message.into())
}

fn internal(error: anyhow::Error) -> CheckError {
    CheckError::Internal(error)
}

/// Exit code for a finished report: 1 when a threshold was crossed, else 0.
pub fn exit_code(report: &CheckReport) -> i32 {
    match report.verdict {
        CheckVerdict::Pass => 0,
        CheckVerdict::Fail => 1,
    }
}

/// The whole command: runs the check, prints the report in `format`
/// ("text" or "json") on stdout and anything else on stderr, and returns
/// the process exit code. A panic is an internal error (3), not Rust's 101,
/// so a consumer only ever sees the four documented codes.
pub fn run_cli(options: &CheckOptions, format: &str) -> i32 {
    write_report(options, format, std::io::stdout(), std::io::stderr())
}

pub fn write_report(
    options: &CheckOptions,
    format: &str,
    mut stdout: impl Write,
    mut stderr: impl Write,
) -> i32 {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(options)));
    match outcome {
        Ok(Ok(report)) => {
            let rendered = if format == "json" {
                render_json(&report)
            } else {
                render_text(&report)
            };
            if stdout
                .write_all(rendered.as_bytes())
                .and_then(|()| stdout.flush())
                .is_err()
            {
                return 3;
            }
            exit_code(&report)
        }
        Ok(Err(error)) => {
            let _ = writeln!(stderr, "tolmap check: {error}");
            error.exit_code()
        }
        Err(_) => {
            let _ = writeln!(stderr, "tolmap check: internal error: panicked");
            3
        }
    }
}

pub fn render_json(report: &CheckReport) -> String {
    let mut text = serde_json::to_string_pretty(report).expect("a CheckReport always serialises");
    text.push('\n');
    text
}

pub fn run(options: &CheckOptions) -> Result<CheckReport, CheckError> {
    let started = Instant::now();
    let thresholds = resolve_thresholds(options)?;
    let target = options
        .target
        .as_deref()
        .map(|path| {
            let text = std::fs::read_to_string(path)
                .map_err(|error| input(format!("{}: {error}", path.display())))?;
            Lens::parse(&text)
                .map_err(|error| input(format!("{}: {error}", path.display())))?
                .target
                .ok_or_else(|| {
                    input(format!(
                        "{}: target lens needs a [target] section",
                        path.display()
                    ))
                })
        })
        .transpose()?;
    let top = repository_root(&options.repo, options.head.is_some())?;
    let base = resolve_commit(&top, &options.base, "--base")?;
    let head = options
        .head
        .as_deref()
        .map(|reference| resolve_commit(&top, reference, "--head"))
        .transpose()?;
    // Before any expensive work: a stored map that is missing, unreadable or
    // from another commit is an input error the caller can fix.
    let supplied_map = options
        .base_map
        .as_deref()
        .map(|path| load_base_map(path, &base))
        .transpose()?;
    let changes = diff(&top, &base, head.as_deref())?;

    let mut scratch = Scratch::new(&top)?;
    let base_dir = scratch.worktree(&base, "base")?;
    let recorded_scope = supplied_map
        .as_ref()
        .filter(|_| !options.all_sources && options.pkg.is_empty() && options.lang.is_empty());
    let sources = if let Some(map) = recorded_scope {
        extract::snapshot_sources(&base_dir, &map.files).map_err(internal)?
    } else {
        resolve_sources(&base_dir, options)?
    };
    eprintln!(
        "tolmap check: base {base}, head {}, {} source(s)",
        head.as_deref().unwrap_or("working tree"),
        sources.len()
    );
    let stage = Instant::now();
    let (base_graph, base_history) = extract::build_check_base_graph(
        &base_dir,
        &sources,
        recorded_scope.map(|map| map.files.as_slice()),
    )
    .map_err(|error| internal(error.context(format!("extract {}", base_dir.display()))))?;
    let recorded_files = recorded_scope.map(|map| map.files.clone());
    let base_graph_s = stage.elapsed().as_secs_f64();
    let stage = Instant::now();
    let base_map = match supplied_map {
        Some(document) => document,
        None => build_base_map(base_graph.clone(), &scratch.root.join("out"))?,
    };
    let base_map_s = stage.elapsed().as_secs_f64();
    let head_dir = match &head {
        Some(sha) => scratch.worktree(sha, "head")?,
        None => top.clone(),
    };
    // The head graph reads the base's co-change history (issue #176), with
    // renamed files credited under their head paths: the two graphs then
    // differ only in the diff's own static signals, so a commit that changes
    // no mapped file gives Δq exactly 0.
    let history = CochangeHistory {
        revision: base.clone(),
        renames: changes
            .iter()
            .filter_map(|change| match change {
                Change::Renamed { from, to } => Some((from.clone(), to.clone())),
                _ => None,
            })
            .collect(),
    };
    let stage = Instant::now();
    let head_graph = if let Some(files) = recorded_files {
        let mut head_files = files
            .into_iter()
            .map(|file| history.renames.get(&file).cloned().unwrap_or(file))
            .filter(|file| source_language(file).is_some() && head_dir.join(file).is_file())
            .collect::<BTreeSet<_>>();
        // New files can enter the recorded languages/roots, but unchanged
        // files omitted by the stored snapshot cannot silently enter it.
        let eligible = sources
            .iter()
            .filter(|(pkg, _)| head_dir.join(pkg).is_dir())
            .map(|(pkg, lang)| extract::source_files(&head_dir, pkg, *lang))
            .collect::<anyhow::Result<Vec<_>>>()
            .map_err(internal)?
            .into_iter()
            .flatten()
            .collect::<BTreeSet<_>>();
        for change in &changes {
            let path = match change {
                Change::Added(path) => path,
                Change::Renamed { to, .. } => to,
                _ => continue,
            };
            if eligible.contains(path) {
                head_files.insert(path.clone());
            }
        }
        extract::build_scoped_snapshot_graph(
            &head_dir,
            &head_files.into_iter().collect::<Vec<_>>(),
            &sources,
            &history,
        )
        .map_err(internal)?
    } else {
        for (pkg, _) in &sources {
            if !head_dir.join(pkg).is_dir() {
                return Err(input(format!(
                    "source root {pkg} selected at base is not a directory at head"
                )));
            }
        }
        extract_head_graph(&head_dir, &sources, &history)?
    };
    let head_graph_s = stage.elapsed().as_secs_f64();
    drop(scratch);

    let report = compare(
        (base, head),
        &changes,
        &base_map,
        (base_graph, Some(&base_history)),
        head_graph,
        thresholds,
        target.as_ref(),
    )?;
    // Timings go to stderr only: the report is byte-identical run to run.
    eprintln!(
        "tolmap check: done in {:.2}s (base graph {base_graph_s:.2}s, base map {base_map_s:.2}s{}, head graph {head_graph_s:.2}s)",
        started.elapsed().as_secs_f64(),
        if options.base_map.is_some() {
            " from --base-map"
        } else {
            ""
        },
    );
    Ok(report)
}

// ---------------------------------------------------------------------
// The metrics
// ---------------------------------------------------------------------

/// A changed path from `git diff --name-status -M`, plus untracked files as
/// additions when head is the working tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Added(String),
    Modified(String),
    Deleted(String),
    Renamed { from: String, to: String },
}

/// One side's blended, pruned graph: exactly the graph the partition stage
/// is handed, as index pairs into `files`.
struct SideGraph {
    files: Vec<String>,
    /// `(a, b, blended weight, carries an import)`, in extraction order.
    edges: Vec<(usize, usize, f64, bool)>,
    /// Directed `importer -> target` pairs, for orienting a listed edge.
    imports: BTreeSet<(usize, usize)>,
    /// Every candidate pair (`a < b`) before pruning, to tell an edge that
    /// the code removed from one that pruning dropped.
    candidates: BTreeSet<(usize, usize)>,
}

fn side_graph(mut data: GraphData) -> Result<SideGraph, CheckError> {
    let index = file_index(&data);
    let mut candidates = BTreeSet::new();
    for edge in &data.edges {
        if let (Some(&a), Some(&b)) = (index.get(edge.a.as_str()), index.get(edge.b.as_str())) {
            candidates.insert((a.min(b), a.max(b)));
        }
    }
    // A side with no candidate edges (a base before its first import, say)
    // has nothing to blend or prune; blending it is an error.
    if !data.edges.is_empty() {
        pipeline::apply_prune_variant(&mut data, PruneVariant::default()).map_err(internal)?;
    }
    let mut edges = Vec::with_capacity(data.edges.len());
    for edge in &data.edges {
        let (Some(&a), Some(&b)) = (index.get(edge.a.as_str()), index.get(edge.b.as_str())) else {
            return Err(internal(anyhow::anyhow!(
                "edge {} -- {} names a file the graph does not hold",
                edge.a,
                edge.b
            )));
        };
        edges.push((a, b, edge.weight, edge.static_signal > 0.0));
    }
    let imports = data
        .imports
        .iter()
        .map(|&(a, b, _)| (a as usize, b as usize))
        .collect();
    Ok(SideGraph {
        files: data.nodes.into_iter().map(|node| node.file).collect(),
        edges,
        imports,
        candidates,
    })
}

fn file_index(data: &GraphData) -> BTreeMap<String, usize> {
    data.nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.file.clone(), index))
        .collect()
}

/// Modularity of a fixed partition on a weighted graph, in the form the
/// partition stage optimises (`RBConfigurationVertexPartition`, normalised
/// by total weight):
///
/// `Q = Σ_c [ w_c / m - γ (K_c / 2m)² ]`
///
/// where `m` is the total edge weight, `w_c` the weight of edges inside
/// community `c` and `K_c` the summed weighted degree of its nodes. A node
/// whose membership is `None` is not in the partition: it and every edge
/// touching it are left out, on the side where it is `None`. Communities
/// are summed in id order so the float result is reproducible. An edgeless
/// graph scores 0, as `partition::LeidenFfi` scores one.
pub fn fixed_partition_modularity(
    edges: impl IntoIterator<Item = (usize, usize, f64)>,
    membership: &[Option<usize>],
    resolution: f64,
) -> f64 {
    let mut total = 0.0;
    let mut inside = BTreeMap::<usize, f64>::new();
    let mut degree = BTreeMap::<usize, f64>::new();
    for (a, b, weight) in edges {
        let (Some(Some(left)), Some(Some(right))) = (membership.get(a), membership.get(b)) else {
            continue;
        };
        total += weight;
        *degree.entry(*left).or_default() += weight;
        *degree.entry(*right).or_default() += weight;
        if left == right {
            *inside.entry(*left).or_default() += weight;
        }
    }
    if total <= 0.0 {
        return 0.0;
    }
    let covered = inside.values().sum::<f64>() / total;
    let expected = degree
        .values()
        .map(|sum| (sum / (2.0 * total)).powi(2))
        .sum::<f64>();
    covered - resolution * expected
}

/// New-file placement: the district holding the plurality of a new file's
/// resolved-import weight. Ties go to the lowest district id, so the answer
/// never depends on iteration order. `None` when no district carries any
/// weight -- the file is unplaced, never guessed.
pub fn place(weight_by_district: &BTreeMap<usize, f64>) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (&district, &weight) in weight_by_district {
        if weight.is_nan() || weight <= 0.0 {
            continue;
        }
        // Strictly greater: an equal weight keeps the earlier, lower id.
        if best.is_none_or(|(_, top)| weight > top) {
            best = Some((district, weight));
        }
    }
    best.map(|(district, _)| district)
}

fn round(value: f64) -> f64 {
    let rounded = round_to(value, ROUND_PLACES);
    // `-0.0` would print as `-0.0`; the report says `0.0`.
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

/// Everything after the graphs exist. Pure: no git, no filesystem.
fn compare(
    (base, head): (String, Option<String>),
    changes: &[Change],
    base_map: &MapDocument,
    base_graph_and_history: (GraphData, Option<&str>),
    head_graph: GraphData,
    thresholds: CheckThresholds,
    target: Option<&LensTargetArchitecture>,
) -> Result<CheckReport, CheckError> {
    if base_map.files.len() != base_map.nodes.len() {
        return Err(input(
            "the base map's F and N arrays differ in length; it is not a tolmap map",
        ));
    }
    let base_membership = base_map
        .files
        .iter()
        .zip(&base_map.nodes)
        .map(|(file, node)| (file.as_str(), node.district()))
        .collect::<BTreeMap<_, _>>();
    let renamed_from = renamed_from(changes);
    let (base_graph, base_history) = base_graph_and_history;

    // Head nodes: a file the base map holds (under its base path, for a
    // rename) keeps its base district; anything else is new and is placed.
    let base_index = file_index(&base_graph);
    let head_index = file_index(&head_graph);
    let head_files = head_graph
        .nodes
        .iter()
        .map(|node| node.file.clone())
        .collect::<Vec<_>>();
    let identity = head_files
        .iter()
        .map(|file| {
            renamed_from
                .get(file.as_str())
                .map_or_else(|| file.clone(), |from| (*from).to_owned())
        })
        .collect::<Vec<_>>();
    let mut head_district = identity
        .iter()
        .map(|file| base_membership.get(file.as_str()).copied())
        .collect::<Vec<_>>();
    let inherited = head_district
        .iter()
        .map(Option::is_some)
        .collect::<Vec<_>>();

    // Placement weights are the blended weights before pruning (finding 1's
    // mass normalisation, before the global rescale, which only scales them):
    // every resolved import has a candidate edge there, while pruning may
    // have dropped some. Pruning is not what placement is asking about.
    let blended = if head_graph.edges.is_empty() {
        Vec::new()
    } else {
        pipeline::mass_normalized_weights(&head_graph).map_err(internal)?
    };
    let mut pair_weight = BTreeMap::<(usize, usize), f64>::new();
    for (edge, weight) in head_graph.edges.iter().zip(blended) {
        if let (Some(&a), Some(&b)) = (head_index.get(&edge.a), head_index.get(&edge.b)) {
            pair_weight.insert((a.min(b), a.max(b)), weight);
        }
    }
    let mut import_weight = BTreeMap::<usize, BTreeMap<usize, f64>>::new();
    for &(importer, target, _) in &head_graph.imports {
        let (importer, target) = (importer as usize, target as usize);
        if importer == target || inherited[importer] || !inherited[target] {
            continue;
        }
        let Some(district) = head_district[target] else {
            continue;
        };
        let weight = pair_weight
            .get(&(importer.min(target), importer.max(target)))
            .copied()
            .unwrap_or(0.0);
        *import_weight
            .entry(importer)
            .or_default()
            .entry(district)
            .or_default() += weight;
    }
    for (file, district) in head_district.iter_mut().enumerate() {
        if !inherited[file] {
            *district = import_weight.get(&file).and_then(place);
        }
    }

    // Deleted files drop out of both sides: a base file counts only if it
    // is still on the head side (under its new path, for a rename).
    let surviving = identity
        .iter()
        .zip(&inherited)
        .filter(|(_, kept)| **kept)
        .map(|(file, _)| file.clone())
        .collect::<BTreeSet<_>>();

    let mut spread = if base_history.is_some() {
        let pairs = base_graph
            .cochange
            .iter()
            .flat_map(|pairs| pairs.keys())
            .map(|&(a, b)| {
                (
                    base_graph.nodes[a as usize].file.as_str(),
                    base_graph.nodes[b as usize].file.as_str(),
                )
            });
        cochange_spread(changes, pairs)
    } else {
        CheckSpread::default()
    };

    let side = |mut graph, membership: &BTreeMap<&str, usize>| {
        // Score the same generated-file projection the map pipeline blends.
        // Evaluate before side_graph prunes edges for the modularity check.
        crate::generated::collapse(&mut graph);
        let report = target
            .map(|target| lens::resolve_graph_target(target, &graph, membership, &base_map.names))
            .transpose()
            .map_err(internal)?;
        Ok::<_, CheckError>((side_graph(graph)?, report))
    };
    let target_head_membership = if target.is_some() {
        head_files
            .iter()
            .zip(&head_district)
            .filter_map(|(file, district)| district.map(|id| (file.as_str(), id)))
            .collect()
    } else {
        BTreeMap::new()
    };
    // Target scores keep every mapped base file so deleting a file can
    // remove its violations. The modularity metric below uses survivors.
    let (base_side, base_target) = side(base_graph, &base_membership)?;
    let (head_side, head_target) = side(head_graph, &target_head_membership)?;
    let target = base_target
        .zip(head_target)
        .map(|(base, head)| lens::compare_target(base, head, &renamed_from));
    let base_district = base_side
        .files
        .iter()
        .map(|file| {
            if surviving.contains(file) {
                base_membership.get(file.as_str()).copied()
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let base_identity = base_side.files.clone();
    let head_identity = identity;

    let q_base = fixed_partition_modularity(
        base_side
            .edges
            .iter()
            .map(|&(a, b, weight, _)| (a, b, weight)),
        &base_district,
        RESOLUTION,
    );
    let q_head = fixed_partition_modularity(
        head_side
            .edges
            .iter()
            .map(|&(a, b, weight, _)| (a, b, weight)),
        &head_district,
        RESOLUTION,
    );
    let delta_q = round(q_head - q_base);

    let base_cross = cross_edges(&base_side, &base_district, &base_identity);
    let head_cross = cross_edges(&head_side, &head_district, &head_identity);
    let base_candidates = candidate_keys(&base_side, &base_identity);
    let head_candidates = candidate_keys(&head_side, &head_identity);
    let edges_added = sorted_edges(
        head_cross
            .iter()
            .filter(|(key, _)| !base_cross.contains_key(*key))
            .map(|(key, edge)| caused_by(edge, &base_candidates, key))
            .collect(),
    );
    let edges_removed = sorted_edges(
        base_cross
            .iter()
            .filter(|(key, _)| !head_cross.contains_key(*key))
            .map(|(key, edge)| caused_by(edge, &head_candidates, key))
            .collect(),
    );

    // The changed files that are on the map, and the districts they count
    // toward.
    let mut files = Vec::new();
    let mut outside_graph = Vec::new();
    for change in changes {
        let (path, base_path, status, reported_base) = match change {
            Change::Added(path) => (path, None, CheckFileStatus::Added, None),
            Change::Modified(path) => (path, Some(path), CheckFileStatus::Modified, None),
            Change::Deleted(path) => (path, Some(path), CheckFileStatus::Deleted, None),
            Change::Renamed { from, to } => {
                (to, Some(from), CheckFileStatus::Renamed, Some(from.clone()))
            }
        };
        let in_base = base_path
            .filter(|file| base_index.contains_key(*file))
            .and_then(|file| base_membership.get(file.as_str()).copied());
        let in_head = if status == CheckFileStatus::Deleted {
            None
        } else {
            head_index.get(path).copied()
        };
        let (district, placed) = match (in_base, in_head) {
            (Some(district), _) => (Some(district), false),
            (None, Some(index)) => (head_district[index], head_district[index].is_some()),
            (None, None) => {
                outside_graph.push(CheckOutsideGraph {
                    path: path.clone(),
                    base_path: reported_base,
                    status,
                    reason: if source_language(path).is_some() {
                        "not_in_scope"
                    } else {
                        "unsupported_language"
                    }
                    .into(),
                });
                continue;
            }
        };
        files.push(CheckFile {
            path: path.clone(),
            base_path: reported_base,
            status,
            district,
            placed,
        });
    }
    outside_graph.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.base_path.cmp(&b.base_path))
    });
    outside_graph.dedup();
    files.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.base_path.cmp(&right.base_path))
    });
    files.dedup();

    let mut counts = BTreeMap::<usize, usize>::new();
    for file in &files {
        if let Some(district) = file.district {
            *counts.entry(district).or_default() += 1;
        }
    }
    let districts = counts
        .into_iter()
        .map(|(id, changed_files)| CheckDistrict {
            id,
            name: base_map
                .names
                .get(&id.to_string())
                .cloned()
                .unwrap_or_default(),
            changed_files,
        })
        .collect::<Vec<_>>();

    let mut unplaced_files = head_files
        .iter()
        .zip(&head_district)
        .filter(|(_, district)| district.is_none())
        .map(|(file, _)| file.clone())
        .collect::<Vec<_>>();
    unplaced_files.sort();

    let mut landmarks = BTreeMap::<&str, Vec<(&str, &str)>>::new();
    for row in &base_map.landmarks {
        let (node, why, detail, _) = &row.0;
        if !TOUCHED_LANDMARKS.contains(&why.as_str()) {
            continue;
        }
        if let Some(file) = base_map.files.get(*node) {
            landmarks
                .entry(file.as_str())
                .or_default()
                .push((why.as_str(), detail.as_str()));
        }
    }
    let mut landmark_touches = Vec::new();
    for file in &files {
        let base_path = match (&file.base_path, file.status) {
            (Some(from), _) => from.as_str(),
            (None, CheckFileStatus::Added) => continue,
            (None, _) => file.path.as_str(),
        };
        for (kind, detail) in landmarks.get(base_path).into_iter().flatten() {
            landmark_touches.push(CheckLandmark {
                file: file.path.clone(),
                kind: (*kind).to_owned(),
                detail: (*detail).to_owned(),
            });
        }
    }
    landmark_touches.sort_by(|left, right| {
        (&left.file, &left.kind, &left.detail).cmp(&(&right.file, &right.kind, &right.detail))
    });
    landmark_touches.dedup();

    let districts_crossed = districts.len();
    spread.districts_touched = districts_crossed;
    spread.percentile_districts =
        base_history.and_then(|log| district_percentile(log, &base_membership, districts_crossed));
    let base_districts = base_map
        .nodes
        .iter()
        .map(|node| node.district())
        .collect::<BTreeSet<_>>()
        .len();
    let failed = thresholds
        .max_districts
        .is_some_and(|max| districts_crossed > max)
        || thresholds.max_dq.is_some_and(|max| delta_q < -max);
    Ok(CheckReport {
        version: REPORT_VERSION,
        base,
        head,
        base_files: base_map.files.len(),
        base_districts,
        districts_crossed,
        districts,
        files,
        unplaced_files,
        outside_graph,
        modularity_base: round(q_base),
        modularity_head: round(q_head),
        delta_q,
        edges_added,
        edges_removed,
        landmark_touches,
        default_thresholds: (thresholds.source == CheckThresholdSource::ReportOnly)
            .then(|| default_thresholds(districts_crossed, delta_q)),
        thresholds,
        verdict: if failed {
            CheckVerdict::Fail
        } else {
            CheckVerdict::Pass
        },
        lower_bound: true,
        spread,
        target,
    })
}

/// The side's edges whose two files sit in different districts, keyed by
/// the pair of base-side identities (base path for a file the base map
/// holds, head path for a new one) so the two sides line up across renames.
fn cross_edges(
    side: &SideGraph,
    membership: &[Option<usize>],
    identity: &[String],
) -> BTreeMap<(String, String), CheckEdge> {
    let mut result = BTreeMap::new();
    for &(a, b, weight, static_import) in &side.edges {
        let (Some(left), Some(right)) = (membership[a], membership[b]) else {
            continue;
        };
        if left == right {
            continue;
        }
        let key = if identity[a] <= identity[b] {
            (identity[a].clone(), identity[b].clone())
        } else {
            (identity[b].clone(), identity[a].clone())
        };
        let forward = side.imports.contains(&(a, b));
        let backward = side.imports.contains(&(b, a));
        let a_first = match (forward, backward) {
            (true, false) => true,
            (false, true) => false,
            _ => side.files[a] <= side.files[b],
        };
        let (source, target) = if a_first { (a, b) } else { (b, a) };
        result.entry(key).or_insert_with(|| CheckEdge {
            source: side.files[source].clone(),
            target: side.files[target].clone(),
            source_district: membership[source].expect("checked above"),
            target_district: membership[target].expect("checked above"),
            weight: round(weight),
            static_import,
            cause: CheckEdgeCause::Code,
        });
    }
    result
}

/// The side's pre-prune candidate pairs, keyed like [`cross_edges`].
fn candidate_keys(side: &SideGraph, identity: &[String]) -> BTreeSet<(String, String)> {
    side.candidates
        .iter()
        .map(|&(a, b)| {
            if identity[a] <= identity[b] {
                (identity[a].clone(), identity[b].clone())
            } else {
                (identity[b].clone(), identity[a].clone())
            }
        })
        .collect()
}

/// An edge only on one side is `prune` when the other side still holds
/// the pair as a candidate, and `code` when it does not.
fn caused_by(
    edge: &CheckEdge,
    other_candidates: &BTreeSet<(String, String)>,
    key: &(String, String),
) -> CheckEdge {
    CheckEdge {
        cause: if other_candidates.contains(key) {
            CheckEdgeCause::Prune
        } else {
            CheckEdgeCause::Code
        },
        ..edge.clone()
    }
}

fn sorted_edges(mut edges: Vec<CheckEdge>) -> Vec<CheckEdge> {
    edges.sort_by(|left, right| {
        right
            .weight
            .total_cmp(&left.weight)
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.target.cmp(&right.target))
    });
    edges
}

fn cochange_spread<'a>(
    changes: &[Change],
    cochange: impl Iterator<Item = (&'a str, &'a str)>,
) -> CheckSpread {
    let mut touched = BTreeMap::<&str, Vec<String>>::new();
    let mut by_base_path = BTreeMap::<&str, &str>::new();
    for change in changes {
        let (path, base_path) = match change {
            Change::Added(path) => (path, None),
            Change::Modified(path) | Change::Deleted(path) => (path, Some(path)),
            Change::Renamed { from, to } => (to, Some(from)),
        };
        touched.entry(path).or_default();
        if let Some(base_path) = base_path {
            by_base_path.insert(base_path, path);
        }
    }
    let mut usual = BTreeSet::new();
    for (a, b) in cochange {
        match (by_base_path.get(a), by_base_path.get(b)) {
            (Some(&left), Some(&right)) if left != right => usual.extend([left, right]),
            (Some(&path), None) => touched.get_mut(path).unwrap().push(b.to_owned()),
            (None, Some(&path)) => touched.get_mut(path).unwrap().push(a.to_owned()),
            _ => {}
        }
    }
    let files_touched = touched.len();
    let outside_usual = if files_touched > 1 {
        touched
            .keys()
            .filter(|path| !usual.contains(*path))
            .map(|path| (*path).to_owned())
            .collect()
    } else {
        Vec::new()
    };
    let cochange_expected = touched
        .into_iter()
        .map(|(path, mut neighbors)| {
            neighbors.sort_unstable();
            neighbors.dedup();
            CheckCochangeExpected {
                path: path.to_owned(),
                neighbors,
            }
        })
        .collect();

    CheckSpread {
        files_touched,
        outside_usual,
        cochange_expected,
        ..CheckSpread::default()
    }
}

fn district_percentile(
    base_history: &str,
    base_membership: &BTreeMap<&str, usize>,
    districts_touched: usize,
) -> Option<f64> {
    let mut historical_districts = BTreeSet::new();
    let (mut total, mut fewer) = (0, 0);
    for paths in extract::history_commit_paths(base_history) {
        historical_districts.clear();
        historical_districts.extend(paths.filter_map(|path| base_membership.get(path).copied()));
        total += 1;
        fewer += usize::from(historical_districts.len() < districts_touched);
    }
    (total > 0).then(|| round(100.0 * fewer as f64 / total as f64))
}

// ---------------------------------------------------------------------
// Text format
// ---------------------------------------------------------------------

/// Districts crossed, Δq, verdict and spread, then the top
/// [`TEXT_EDGE_LIMIT`] code-caused cross-district edges by weight, then the
/// count and top [`TEXT_PRUNE_LIMIT`] pruning-caused ones, context lines, and
/// the lower-bound note.
pub fn render_text(report: &CheckReport) -> String {
    let mut out = String::new();
    let named = report
        .districts
        .iter()
        .map(|district| {
            if district.name.is_empty() {
                district.id.to_string()
            } else {
                format!("{} {}", district.id, district.name)
            }
        })
        .collect::<Vec<_>>();
    let _ = write!(out, "districts crossed: {}", report.districts_crossed);
    if !named.is_empty() {
        let _ = write!(out, " ({})", named.join(", "));
    }
    if !report.unplaced_files.is_empty() {
        let _ = write!(
            out,
            "; {} new file(s) unplaced",
            report.unplaced_files.len()
        );
    }
    out.push('\n');
    let _ = writeln!(
        out,
        "delta q: {:+.6} (base {:.6} -> head {:.6}, base partition held fixed)",
        report.delta_q, report.modularity_base, report.modularity_head
    );
    let _ = writeln!(out, "verdict: {}", verdict_line(report));
    if let Some(defaults) = &report.default_thresholds {
        let _ = writeln!(out, "{}", default_thresholds_line(defaults));
    }

    let spread = &report.spread;
    let rank = spread.percentile_districts.map_or_else(
        || "no base history".to_owned(),
        |rank| format!("p{rank:.0} of history"),
    );
    let _ = write!(
        out,
        "spread: {} files, {} districts ({rank}), {} outside usual co-change",
        spread.files_touched,
        spread.districts_touched,
        spread.outside_usual.len(),
    );
    let mut expected = spread
        .cochange_expected
        .iter()
        .flat_map(|file| file.neighbors.iter().map(|neighbor| (&file.path, neighbor)));
    for (index, (path, neighbor)) in expected.by_ref().take(TEXT_COCHANGE_LIMIT).enumerate() {
        out.push_str(if index == 0 {
            ". Expected co-change: "
        } else {
            ", "
        });
        let _ = write!(out, "{path:?} -> {neighbor:?}");
    }
    let remaining = expected.count();
    if remaining > 0 {
        let _ = write!(out, ", ... {remaining} more in --format json");
    }
    out.push('\n');
    if let Some(target) = &report.target {
        lens::render_target_distance(&mut out, "target base distance", &target.base);
        lens::render_target_distance(&mut out, "target head distance", &target.head);
        let _ = writeln!(
            out,
            "target violations: {} added, {} removed",
            target.violations_added.len(),
            target.violations_removed.len()
        );
        for violation in &target.violations_added {
            lens::render_target_violation(&mut out, "+", violation);
        }
        for violation in &target.violations_removed {
            lens::render_target_violation(&mut out, "-", violation);
        }
        for (side, items) in [
            ("base", &target.unresolved_base),
            ("head", &target.unresolved_head),
        ] {
            for item in items {
                let _ = writeln!(
                    out,
                    "  {side} unresolved {} {:?}: {}",
                    item.location, item.spec, item.detail
                );
            }
        }
    }

    let _ = writeln!(
        out,
        "cross-district edges: {} added, {} removed",
        report.edges_added.len(),
        report.edges_removed.len()
    );
    let (code, prune): (Vec<_>, Vec<_>) = report
        .edges_added
        .iter()
        .map(|edge| ('+', edge))
        .chain(report.edges_removed.iter().map(|edge| ('-', edge)))
        .partition(|(_, edge)| edge.cause == CheckEdgeCause::Code);
    let heaviest_first = |list: &mut Vec<(char, &CheckEdge)>| {
        list.sort_by(|left, right| {
            right
                .1
                .weight
                .total_cmp(&left.1.weight)
                .then_with(|| left.0.cmp(&right.0))
                .then_with(|| left.1.source.cmp(&right.1.source))
                .then_with(|| left.1.target.cmp(&right.1.target))
        });
    };
    let edge_line = |out: &mut String, sign: char, edge: &CheckEdge| {
        let _ = writeln!(
            out,
            "  {sign} {} -> {} (district {} -> {}) weight {:.6}{}",
            edge.source,
            edge.target,
            edge.source_district,
            edge.target_district,
            edge.weight,
            if edge.static_import { ", import" } else { "" }
        );
    };
    let (mut code, mut prune) = (code, prune);
    heaviest_first(&mut code);
    heaviest_first(&mut prune);
    let _ = writeln!(out, "from code or history: {}", code.len());
    for (sign, edge) in code.iter().take(TEXT_EDGE_LIMIT) {
        edge_line(&mut out, *sign, edge);
    }
    if code.len() > TEXT_EDGE_LIMIT {
        let _ = writeln!(
            out,
            "  ... {} more in --format json",
            code.len() - TEXT_EDGE_LIMIT
        );
    }
    if !prune.is_empty() {
        let _ = writeln!(
            out,
            "from blending or pruning (the pair is a candidate on both sides): {}",
            prune.len()
        );
        for (sign, edge) in prune.iter().take(TEXT_PRUNE_LIMIT) {
            edge_line(&mut out, *sign, edge);
        }
        if prune.len() > TEXT_PRUNE_LIMIT {
            let _ = writeln!(
                out,
                "  ... {} more in --format json",
                prune.len() - TEXT_PRUNE_LIMIT
            );
        }
    }
    for file in &report.unplaced_files {
        let _ = writeln!(out, "unplaced: {file} (no resolved import into a district)");
    }
    for touch in &report.landmark_touches {
        let _ = writeln!(
            out,
            "landmark touched: {} is a {} ({})",
            touch.file, touch.kind, touch.detail
        );
    }
    let _ = writeln!(out, "{LOWER_BOUND_NOTE}");
    out
}

fn default_thresholds(districts: usize, delta_q: f64) -> CheckDefaultThresholds {
    let districts_over = districts as i64 - DEFAULT_MAX_DISTRICTS as i64;
    let dq_over = round(-delta_q - DEFAULT_MAX_DQ);
    CheckDefaultThresholds {
        verdict: if districts_over > 0 || dq_over > 0.0 {
            CheckVerdict::Fail
        } else {
            CheckVerdict::Pass
        },
        max_districts: DEFAULT_MAX_DISTRICTS,
        max_dq: DEFAULT_MAX_DQ,
        districts_over,
        dq_over,
    }
}

fn default_thresholds_line(defaults: &CheckDefaultThresholds) -> String {
    let verdict = match defaults.verdict {
        CheckVerdict::Pass => "pass",
        CheckVerdict::Fail => "fail",
    };
    format!(
        "default thresholds would {verdict}: districts {:+} relative to max {}; modularity drop {:+.6} relative to max {:.6} (positive = over, negative = remaining margin)",
        defaults.districts_over, defaults.max_districts, defaults.dq_over, defaults.max_dq
    )
}

fn verdict_line(report: &CheckReport) -> String {
    let verdict = match report.verdict {
        CheckVerdict::Pass => "pass",
        CheckVerdict::Fail => "fail",
    };
    let mut checks = Vec::new();
    if let Some(max) = report.thresholds.max_districts {
        let over = report.districts_crossed > max;
        checks.push(format!(
            "districts {} {} {max}",
            report.districts_crossed,
            if over { ">" } else { "<=" }
        ));
    }
    if let Some(max) = report.thresholds.max_dq {
        let over = report.delta_q < -max;
        checks.push(format!(
            "delta q {:+.6} {} -{max:.6}",
            report.delta_q,
            if over { "<" } else { ">=" }
        ));
    }
    match report.thresholds.source {
        CheckThresholdSource::ReportOnly => format!("{verdict} (report only: --report-only)"),
        CheckThresholdSource::Default => {
            format!("{verdict} ({}; default thresholds)", checks.join(", "))
        }
        CheckThresholdSource::Flags => format!("{verdict} ({})", checks.join(", ")),
    }
}

// ---------------------------------------------------------------------
// Inputs: git, detection, extraction, the base map
// ---------------------------------------------------------------------

pub(crate) fn git_output(dir: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
}

/// Runs git and returns stdout, or an internal error with git's stderr.
pub(crate) fn git_stdout(dir: &Path, args: &[&str]) -> Result<Vec<u8>, CheckError> {
    let output = git_output(dir, args)
        .map_err(|error| internal(anyhow::anyhow!("run git {}: {error}", args.join(" "))))?;
    if !output.status.success() {
        return Err(internal(anyhow::anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

pub(crate) fn repository_root(repo: &Path, allow_bare: bool) -> Result<PathBuf, CheckError> {
    if allow_bare
        && git_stdout(repo, &["rev-parse", "--is-bare-repository"])
            .map(|s| String::from_utf8_lossy(&s).trim() == "true")
            .unwrap_or(false)
    {
        Ok(repo.to_owned())
    } else {
        toplevel(repo)
    }
}

fn toplevel(repo: &Path) -> Result<PathBuf, CheckError> {
    if !repo.is_dir() {
        return Err(input(format!("{} is not a directory", repo.display())));
    }
    let output = git_output(repo, &["rev-parse", "--show-toplevel"])
        .map_err(|error| input(format!("cannot run git: {error}")))?;
    if !output.status.success() {
        return Err(input(format!(
            "{} is not inside a git repository",
            repo.display()
        )));
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok(PathBuf::from(path))
}

pub(crate) fn resolve_commit(
    top: &Path,
    reference: &str,
    flag: &str,
) -> Result<String, CheckError> {
    if reference.is_empty() || reference.starts_with('-') {
        return Err(input(format!("{flag} {reference:?} is not a git ref")));
    }
    let spec = format!("{reference}^{{commit}}");
    let output = git_output(
        top,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ],
    )
    .map_err(|error| input(format!("cannot run git: {error}")))?;
    if !output.status.success() {
        return Err(input(format!(
            "{flag} {reference:?} does not name a commit in {}",
            top.display()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// A full git object id, as the service store names its maps.
#[cfg(test)]
fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Reads `--base-map` and verifies the commit it records equals `base`. The
/// map document itself carries no commit (docs/ARCHITECTURE.md: the
/// compact schema has no commit field); the service store records it in the
/// file name, `<cache_dir>/maps/<owner>/<repo>/<commit>.json`, and so does
/// this check: a map whose file name is not `<full commit id>.json` cannot
/// be verified and is refused rather than trusted.
fn load_base_map(path: &Path, base: &str) -> Result<MapDocument, CheckError> {
    let raw = std::fs::read(path)
        .map_err(|error| input(format!("--base-map {}: {error}", path.display())))?;
    let document: MapDocument = serde_json::from_slice(&raw).map_err(|error| {
        input(format!(
            "--base-map {} is not a tolmap map: {error}",
            path.display()
        ))
    })?;
    let recorded = crate::query::Snapshot::load(path)
        .map_err(|error|input(format!("--base-map {}: {error:#}",path.display())))?
        .identity.commit
        .ok_or_else(||input(format!("--base-map {}: cannot verify its commit; use a retained snapshot or name it <full commit id>.json",path.display())))?;
    if recorded != base {
        return Err(input(format!(
            "--base-map {} was built from {recorded}, not --base {base}",
            path.display()
        )));
    }
    Ok(document)
}

pub(crate) fn renamed_from(changes: &[Change]) -> BTreeMap<&str, &str> {
    changes
        .iter()
        .filter_map(|change| match change {
            Change::Renamed { from, to } => Some((to.as_str(), from.as_str())),
            _ => None,
        })
        .collect()
}

/// `git diff --name-status -M` between base and head, following renames;
/// with head the working tree, untracked (not ignored) files are additions.
pub fn diff(top: &Path, base: &str, head: Option<&str>) -> Result<Vec<Change>, CheckError> {
    let mut args = vec![
        "diff",
        "--name-status",
        "-z",
        "-M",
        "--no-color",
        "--no-ext-diff",
        base,
    ];
    if let Some(head) = head {
        args.push(head);
    }
    let mut changes = parse_name_status(&git_stdout(top, &args)?)?;
    if head.is_none() {
        let untracked = git_stdout(top, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        for path in untracked.split(|&byte| byte == 0) {
            if !path.is_empty() {
                changes.push(Change::Added(String::from_utf8_lossy(path).into_owned()));
            }
        }
    }
    Ok(changes)
}

/// Parses `git diff --name-status -z`: a status field, then one path, or two
/// (source, destination) for a rename or copy. A copy's destination is an
/// addition; type changes and unmerged paths are modifications.
pub fn parse_name_status(bytes: &[u8]) -> Result<Vec<Change>, CheckError> {
    // Every field, the last included, ends in a NUL; a path is never empty,
    // so an empty field where a path belongs means the output was cut short.
    let mut fields = bytes
        .split(|&byte| byte == 0)
        .map(|field| String::from_utf8_lossy(field).into_owned());
    let mut changes = Vec::new();
    let path = |fields: &mut dyn Iterator<Item = String>| {
        fields
            .next()
            .filter(|path| !path.is_empty())
            .ok_or_else(|| {
                internal(anyhow::anyhow!(
                    "git diff --name-status output is truncated"
                ))
            })
    };
    while let Some(status) = fields.next() {
        if status.is_empty() {
            continue;
        }
        let first = path(&mut fields)?;
        let change = match status.as_bytes()[0] {
            b'R' => Change::Renamed {
                from: first,
                to: path(&mut fields)?,
            },
            b'C' => Change::Added(path(&mut fields)?),
            b'A' => Change::Added(first),
            b'D' => Change::Deleted(first),
            _ => Change::Modified(first),
        };
        changes.push(change);
    }
    Ok(changes)
}

/// Temporary worktrees, and the directory that holds them and the built base
/// map. Removed on drop, including on an error or a panic, so the user's
/// repository is left with no stray worktree.
pub(crate) struct Scratch {
    top: PathBuf,
    pub(crate) root: PathBuf,
    worktrees: Vec<PathBuf>,
}

static SCRATCH_COUNTER: AtomicUsize = AtomicUsize::new(0);

impl Scratch {
    pub(crate) fn new(top: &Path) -> Result<Self, CheckError> {
        let root = std::env::temp_dir().join(format!(
            "tolmap-check-{}-{}",
            std::process::id(),
            SCRATCH_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        if root.exists() {
            let _ = std::fs::remove_dir_all(&root);
        }
        std::fs::create_dir_all(&root)
            .map_err(|error| internal(anyhow::anyhow!("create {}: {error}", root.display())))?;
        Ok(Self {
            top: top.to_owned(),
            root,
            worktrees: Vec::new(),
        })
    }

    /// Checks `commit` out into a new detached worktree. Hooks are off: a
    /// repository's post-checkout hook has no business running inside a
    /// read-only check.
    pub(crate) fn worktree(&mut self, commit: &str, label: &str) -> Result<PathBuf, CheckError> {
        let path = self.root.join(label);
        let path_arg = path.to_string_lossy().into_owned();
        git_stdout(
            &self.top,
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "advice.detachedHead=false",
                "worktree",
                "add",
                "--detach",
                "--quiet",
                &path_arg,
                commit,
            ],
        )?;
        self.worktrees.push(path.clone());
        Ok(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for path in &self.worktrees {
            let path_arg = path.to_string_lossy().into_owned();
            let _ = git_output(&self.top, &["worktree", "remove", "--force", &path_arg]);
        }
        let _ = std::fs::remove_dir_all(&self.root);
        if !self.worktrees.is_empty() {
            let _ = git_output(&self.top, &["worktree", "prune"]);
        }
    }
}

/// The source a map build of the base commit would index: detection's
/// choice, refused at low confidence exactly as the service refuses it
/// (`worker::run`'s `detection_uncertain`). The same source is extracted on
/// both sides, so the two graphs describe one codebase.
pub(crate) fn detect_source(dir: &Path) -> Result<(String, LanguageKind), CheckError> {
    let detection = detect::detect(dir)
        .map_err(|error| input(format!("detection failed at base: {error:#}")))?;
    let chosen = detection.chosen;
    if chosen.confidence == Confidence::Low {
        return Err(input(format!(
            "detection refused at base: {} -- too uncertain to check against",
            chosen.describe()
        )));
    }
    Ok((chosen.pkg, chosen.language))
}

fn source_language(path: &str) -> Option<LanguageKind> {
    match path.rsplit('.').next()? {
        "py" => Some(LanguageKind::Python),
        "go" => Some(LanguageKind::Go),
        "rs" => Some(LanguageKind::Rust),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some(LanguageKind::TypeScript),
        _ => None,
    }
}

fn resolve_sources(
    dir: &Path,
    options: &CheckOptions,
) -> Result<Vec<(String, LanguageKind)>, CheckError> {
    if options.all_sources {
        if !options.pkg.is_empty() || !options.lang.is_empty() {
            return Err(input("--all-sources cannot be combined with --pkg/--lang"));
        }
        let sources = detect::all_sources(dir).map_err(internal)?;
        if sources.is_empty() {
            return Err(input(
                "no source cleared the --all-sources floor; pass --pkg/--lang explicitly",
            ));
        }
        return Ok(sources.into_iter().map(|c| (c.pkg, c.language)).collect());
    }
    if options.pkg.len() > 1 || options.lang.len() > 1 {
        if options.pkg.len() != options.lang.len() {
            return Err(input(
                "--pkg and --lang must be given the same number of times; pair them positionally",
            ));
        }
        return options
            .pkg
            .iter()
            .zip(&options.lang)
            .map(|(pkg, lang)| {
                Ok((
                    if dir.join(pkg).is_dir() {
                        pkg.clone()
                    } else {
                        return Err(input(format!("source root {pkg} is not a directory")));
                    },
                    LanguageKind::parse(lang).map_err(|e| input(e.to_string()))?,
                ))
            })
            .collect();
    }
    let source = match (options.pkg.first(), options.lang.first()) {
        (Some(pkg), Some(lang)) => (
            pkg.clone(),
            LanguageKind::parse(lang).map_err(|e| input(e.to_string()))?,
        ),
        (Some(pkg), None) => (pkg.clone(), detect_source(dir)?.1),
        (None, Some(lang)) => {
            let language = LanguageKind::parse(lang).map_err(|e| input(e.to_string()))?;
            let candidate = detect::detect_language(dir, language)
                .map_err(internal)?
                .ok_or_else(|| input(format!("no {lang} source found; pass --pkg explicitly")))?;
            (candidate.pkg, language)
        }
        (None, None) => detect_source(dir)?,
    };
    if !dir.join(&source.0).is_dir() {
        return Err(input(format!(
            "source root {} is not a directory",
            source.0
        )));
    }
    Ok(vec![source])
}

fn extract_graph(dir: &Path, sources: &[(String, LanguageKind)]) -> Result<GraphData, CheckError> {
    extract::build_multi_source_with_refs(dir, sources, RefsMode::default())
        .map_err(|error| internal(error.context(format!("extract {}", dir.display()))))
}

// Lens comparison resolves head content in the existing check frame. This is
// a resolver-only document: no partition/layout is built and no geometry from
// it is delivered to the viewer. Unplaced files keep an explicit sentinel.
pub(crate) fn lens_head_map(
    repo: &Path,
    base: &str,
    head: &str,
    base_map: &MapDocument,
    head_snapshot: &MapDocument,
) -> Result<MapDocument, CheckError> {
    let mut scratch = Scratch::new(repo)?;
    let base_dir = scratch.worktree(base, "base")?;
    let head_dir = scratch.worktree(head, "head")?;
    let changes = diff(repo, base, Some(head))?;
    let history = CochangeHistory {
        revision: base.to_owned(),
        renames: changes
            .iter()
            .filter_map(|change| match change {
                Change::Renamed { from, to } => Some((from.clone(), to.clone())),
                _ => None,
            })
            .collect(),
    };
    let base_graph = extract::build_snapshot_graph(
        &base_dir,
        &base_map.files,
        &CochangeHistory {
            revision: base.into(),
            renames: BTreeMap::new(),
        },
    )
    .map_err(internal)?;
    let graph = extract::build_snapshot_graph(&head_dir, &head_snapshot.files, &history)
        .map_err(internal)?;
    let report = compare(
        (base.into(), Some(head.into())),
        &changes,
        base_map,
        (base_graph, None),
        graph.clone(),
        CheckThresholds {
            max_districts: None,
            max_dq: None,
            source: CheckThresholdSource::ReportOnly,
        },
        None,
    )?;
    let mut result = base_map.clone();
    result.files = graph.nodes.iter().map(|node| node.file.clone()).collect();
    result.nodes = graph
        .nodes
        .iter()
        .map(|node| {
            let district = report
                .files
                .iter()
                .find(|file| file.path == node.file)
                .and_then(|file| file.district)
                .or_else(|| {
                    base_map
                        .files
                        .iter()
                        .position(|file| file == &node.file)
                        .map(|i| base_map.nodes[i].district())
                })
                .unwrap_or(usize::MAX);
            NodeRow((district, 0.0, 0.0, node.loc, 0, 0, 0.0, 0.0, 0.0, 0.0, 0.0))
        })
        .collect();
    result.edges = graph
        .imports
        .iter()
        .map(|&(a, b, _)| [a as usize, b as usize])
        .collect();
    result.symbols = result
        .files
        .iter()
        .enumerate()
        .filter_map(|(i, path)| {
            graph
                .symbols
                .get(path)
                .map(|rows| (i.to_string(), rows.clone()))
        })
        .collect();
    let index = result
        .files
        .iter()
        .enumerate()
        .map(|(i, path)| (path.as_str(), i))
        .collect();
    result.uses = geometry::compact_uses(&graph, &index);
    Ok(result)
}

/// Builds the base map the way `tolmap build` would, cold, into scratch.
/// Parcels are skipped: they are drawn after the partition and never change
/// membership, names or landmarks, which is all the check reads.
fn extract_head_graph(
    dir: &Path,
    sources: &[(String, LanguageKind)],
    history: &CochangeHistory,
) -> Result<GraphData, CheckError> {
    extract::build_multi_source_with_history(dir, sources, RefsMode::default(), history)
        .map_err(|error| internal(error.context(format!("extract {}", dir.display()))))
}

// The same cold, parcel-free build used for check's base map.
pub(crate) fn build_cold_map(repo: &Path, commit: &str) -> Result<Vec<u8>, CheckError> {
    let mut scratch = Scratch::new(repo)?;
    let dir = scratch.worktree(commit, "snapshot")?;
    let source = detect_source(&dir)?;
    let graph = extract_graph(&dir, &[source])?;
    let (_, raw) = build_base_map_bytes(graph, &scratch.root.join("out"))?;
    Ok(raw)
}

fn build_base_map(graph: GraphData, out: &Path) -> Result<MapDocument, CheckError> {
    let (path, raw) = build_base_map_bytes(graph, out)?;
    serde_json::from_slice(&raw)
        .map_err(|error| internal(anyhow::anyhow!("parse {}: {error}", path.display())))
}

fn build_base_map_bytes(graph: GraphData, out: &Path) -> Result<(PathBuf, Vec<u8>), CheckError> {
    let path = geometry::build_from_graph(
        graph,
        "base".to_owned(),
        out,
        RESOLUTION,
        BuildFeatures {
            parcels: false,
            prune_variant: PruneVariant::default(),
            namer: NamerKind::Idf,
            namer_model: naming::DEFAULT_MODEL.to_owned(),
            refs: RefsMode::default(),
            install: InstallMode::Off,
        },
    )
    .map_err(|error| internal(error.context("build the base map")))?;
    let raw = std::fs::read(&path)
        .map_err(|error| internal(anyhow::anyhow!("read {}: {error}", path.display())))?;
    Ok((path, raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(left: f64, right: f64) -> bool {
        (left - right).abs() < 1e-12
    }

    // Two triangles joined by one edge, every weight 1: m = 7, each
    // community holds 3 internal edges and a degree sum of 7, so
    // Q = 6/7 - γ · 2 · (7/14)² = 6/7 - γ/2.
    fn two_triangles() -> Vec<(usize, usize, f64)> {
        vec![
            (0, 1, 1.0),
            (1, 2, 1.0),
            (2, 0, 1.0),
            (3, 4, 1.0),
            (4, 5, 1.0),
            (5, 3, 1.0),
            (2, 3, 1.0),
        ]
    }

    #[test]
    fn spread_percentile_counts_distinct_base_districts_and_excludes_ties() {
        let log = "\x1eempty\n\n\x1eone\n\na.py\nb.py\n\x1etwo\n\na.py\n@mapped.py\n\x1ethree\n\na.py\n@mapped.py\nd.py";
        let base = BTreeMap::from([("a.py", 0), ("b.py", 0), ("@mapped.py", 1), ("d.py", 2)]);
        assert_eq!(district_percentile(log, &base, 2), Some(50.0));
        assert_eq!(district_percentile("", &base, 2), None);
    }

    #[test]
    fn cochange_uses_old_paths_for_renames_and_deleted_files() {
        let changes = [
            Change::Renamed {
                from: "a.py".into(),
                to: "z.py".into(),
            },
            Change::Deleted("c.py".into()),
        ];
        let spread = cochange_spread(
            &changes,
            [("a.py", "c.py"), ("a.py", "d.py"), ("a.py", "b.py")].into_iter(),
        );
        assert!(spread.outside_usual.is_empty());
        assert_eq!(spread.cochange_expected[1].path, "z.py");
        assert_eq!(spread.cochange_expected[1].neighbors, ["b.py", "d.py"]);
    }

    #[test]
    fn fixed_partition_modularity_matches_a_hand_computed_graph() {
        let membership = [Some(0), Some(0), Some(0), Some(1), Some(1), Some(1)];
        let q = fixed_partition_modularity(two_triangles(), &membership, 1.0);
        assert!(close(q, 6.0 / 7.0 - 0.5), "{q}");
        let q = fixed_partition_modularity(two_triangles(), &membership, 1.1);
        assert!(close(q, 6.0 / 7.0 - 0.55), "{q}");
    }

    #[test]
    fn a_node_outside_the_partition_drops_out_with_its_edges() {
        // Node 5 unassigned: its two edges go, leaving m = 5; community 0
        // keeps 3 internal edges and degree 7, community 1 keeps the one
        // edge 3-4 and degree 3 (node 3: 2, node 4: 1).
        // Q = 4/5 - 1.1 · ((7/10)² + (3/10)²) = 0.8 - 1.1 · 0.58.
        let membership = [Some(0), Some(0), Some(0), Some(1), Some(1), None];
        let q = fixed_partition_modularity(two_triangles(), &membership, 1.1);
        assert!(close(q, 0.8 - 1.1 * 0.58), "{q}");
    }

    #[test]
    fn weights_count_and_one_community_is_not_modular() {
        // Weighted: doubling the bridge lowers Q against the unit graph.
        let membership = [Some(0), Some(0), Some(0), Some(1), Some(1), Some(1)];
        let mut heavier = two_triangles();
        heavier[6].2 = 2.0;
        let unit = fixed_partition_modularity(two_triangles(), &membership, 1.1);
        let heavy = fixed_partition_modularity(heavier, &membership, 1.1);
        assert!(heavy < unit, "{heavy} vs {unit}");
        // Everything in one community: Q = 1 - γ.
        let q = fixed_partition_modularity(two_triangles(), &[Some(0); 6], 1.1);
        assert!(close(q, 1.0 - 1.1), "{q}");
        // No edges at all: 0, as the partitioner scores an edgeless graph.
        assert_eq!(fixed_partition_modularity([], &[Some(0)], 1.1), 0.0);
    }

    #[test]
    fn placement_takes_the_heaviest_district_and_breaks_ties_by_lowest_id() {
        let weights = |pairs: &[(usize, f64)]| pairs.iter().copied().collect::<BTreeMap<_, _>>();
        assert_eq!(place(&weights(&[(4, 0.2), (2, 0.7), (9, 0.1)])), Some(2));
        // A tie: the lower id wins whatever order the weights arrived in.
        assert_eq!(place(&weights(&[(7, 0.5), (3, 0.5), (5, 0.2)])), Some(3));
        assert_eq!(place(&weights(&[(3, 0.5), (7, 0.5)])), Some(3));
        // No weight anywhere: unplaced, never guessed.
        assert_eq!(place(&weights(&[])), None);
        assert_eq!(place(&weights(&[(1, 0.0)])), None);
    }

    #[test]
    fn a_side_without_edges_keeps_its_files() {
        let data: GraphData = serde_json::from_value(serde_json::json!({
            "repo": "r", "pkg": "p", "lang": "py", "imports": [], "symbols": {},
            "uses": [], "commits_scanned": 0, "edges": [],
            "nodes": [{"file": "a.py", "loc": 1, "complexity": 0, "churn": 0,
                       "fanin": 0.0, "module": "a"}]
        }))
        .unwrap();
        let side = side_graph(data).unwrap();
        assert_eq!(side.files, ["a.py"]);
        assert!(side.edges.is_empty());
    }

    fn side(edges: &[(usize, usize)], candidates: &[(usize, usize)]) -> SideGraph {
        SideGraph {
            files: ["a.py", "b.py", "c.py", "d.py"].map(String::from).to_vec(),
            edges: edges.iter().map(|&(a, b)| (a, b, 0.5, true)).collect(),
            imports: BTreeSet::new(),
            candidates: candidates.iter().copied().collect(),
        }
    }

    #[test]
    fn a_removed_edge_is_code_unless_the_pair_is_still_a_candidate() {
        // Base: a-b and c-d both survive pruning, across districts 0 | 1.
        // Head: a-b is gone from the code (no candidate); c-d is still a
        // candidate but pruning dropped it.
        let membership = [Some(0), Some(1), Some(0), Some(1)];
        let base = side(&[(0, 1), (2, 3)], &[(0, 1), (2, 3)]);
        let head = side(&[], &[(2, 3)]);
        let identity = base.files.clone();
        let base_cross = cross_edges(&base, &membership, &identity);
        let head_cross = cross_edges(&head, &membership, &identity);
        let head_candidates = candidate_keys(&head, &identity);
        let removed = base_cross
            .iter()
            .filter(|(key, _)| !head_cross.contains_key(*key))
            .map(|(key, edge)| caused_by(edge, &head_candidates, key))
            .map(|edge| (edge.source, edge.cause))
            .collect::<Vec<_>>();
        assert_eq!(
            removed,
            [
                ("a.py".to_owned(), CheckEdgeCause::Code),
                ("c.py".to_owned(), CheckEdgeCause::Prune)
            ]
        );
    }

    #[test]
    fn name_status_parsing_follows_renames_and_copies() {
        let raw = b"M\0a.py\0R087\0old.py\0new.py\0A\0b.py\0D\0c.py\0C100\0d.py\0e.py\0T\0f.py\0";
        assert_eq!(
            parse_name_status(raw).unwrap(),
            vec![
                Change::Modified("a.py".into()),
                Change::Renamed {
                    from: "old.py".into(),
                    to: "new.py".into()
                },
                Change::Added("b.py".into()),
                Change::Deleted("c.py".into()),
                Change::Added("e.py".into()),
                Change::Modified("f.py".into()),
            ]
        );
        assert!(parse_name_status(b"R100\0only-one.py\0").is_err());
        assert!(parse_name_status(b"").unwrap().is_empty());
    }

    #[test]
    fn exit_codes_follow_the_error_class() {
        assert_eq!(input("bad ref").exit_code(), 2);
        assert_eq!(internal(anyhow::anyhow!("boom")).exit_code(), 3);
    }

    #[test]
    fn only_a_full_object_id_is_a_recorded_commit() {
        assert!(is_object_id(&"a".repeat(40)));
        assert!(is_object_id(&"0123456789abcdef".repeat(4)));
        assert!(!is_object_id("abc1234"));
        assert!(!is_object_id(&"A".repeat(40)));
        assert!(!is_object_id("base"));
    }

    #[test]
    fn rounding_never_prints_negative_zero() {
        assert_eq!(round(-0.000_000_1).to_bits(), 0.0_f64.to_bits());
        assert_eq!(round(-0.123_456_7), -0.123_457);
    }

    fn options(
        max_districts: Option<usize>,
        max_dq: Option<f64>,
        report_only: bool,
    ) -> CheckOptions {
        CheckOptions {
            repo: PathBuf::from("."),
            base: "HEAD".to_owned(),
            head: None,
            base_map: None,
            target: None,
            pkg: vec![],
            lang: vec![],
            all_sources: false,
            max_districts,
            max_dq,
            report_only,
        }
    }

    #[test]
    fn advisory_defaults_show_margin_and_strict_boundaries() {
        let pass = default_thresholds(2, -0.000008);
        assert_eq!(pass.verdict, CheckVerdict::Pass);
        assert_eq!(pass.districts_over, -2);
        assert_eq!(pass.dq_over, -0.000092);
        let fail = default_thresholds(5, -0.006704);
        assert_eq!(fail.verdict, CheckVerdict::Fail);
        assert_eq!(fail.districts_over, 1);
        assert_eq!(fail.dq_over, 0.006604);
        assert!(default_thresholds_line(&fail).contains("would fail"));
        assert_eq!(default_thresholds(4, -0.0001).verdict, CheckVerdict::Pass);
        assert_eq!(default_thresholds(5, 0.1).verdict, CheckVerdict::Fail);
        assert_eq!(default_thresholds(0, -0.000101).verdict, CheckVerdict::Fail);
    }

    #[test]
    fn defaults_apply_only_when_no_threshold_flag_is_given() {
        let thresholds = resolve_thresholds(&options(None, None, false)).unwrap();
        assert_eq!(thresholds.max_districts, Some(DEFAULT_MAX_DISTRICTS));
        assert_eq!(thresholds.max_dq, Some(DEFAULT_MAX_DQ));
        assert_eq!(thresholds.source, CheckThresholdSource::Default);

        // One flag replaces both defaults: no default mixes in.
        let thresholds = resolve_thresholds(&options(None, Some(0.002), false)).unwrap();
        assert_eq!(thresholds.max_districts, None);
        assert_eq!(thresholds.max_dq, Some(0.002));
        assert_eq!(thresholds.source, CheckThresholdSource::Flags);
        let thresholds = resolve_thresholds(&options(Some(9), None, false)).unwrap();
        assert_eq!(thresholds.max_districts, Some(9));
        assert_eq!(thresholds.max_dq, None);

        let thresholds = resolve_thresholds(&options(None, None, true)).unwrap();
        assert_eq!(thresholds.max_districts, None);
        assert_eq!(thresholds.max_dq, None);
        assert_eq!(thresholds.source, CheckThresholdSource::ReportOnly);

        for bad in [
            options(Some(1), None, true),
            options(None, Some(0.0), true),
            options(None, Some(f64::NAN), false),
            options(None, Some(-0.1), false),
        ] {
            assert_eq!(resolve_thresholds(&bad).unwrap_err().exit_code(), 2);
        }
    }
}
