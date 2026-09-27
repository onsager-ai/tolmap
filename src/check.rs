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
//! Every number is computed on tolmap's own reference graph, which misses
//! what it cannot resolve (CLAUDE.md: numbers are a lower bound), and the
//! report says so in `lower_bound`.
//!
//! The user's checkout is never touched: base (and `--head`, when given) are
//! checked out into temporary `git worktree`s, with hooks off, and removed
//! afterwards. A worktree rather than `git archive` because extraction's
//! co-change signal reads the checkout's own `git log`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::detect::{self, Confidence};
use crate::extract::{self, round_to, InstallMode, LanguageKind, RefsMode};
use crate::geometry::{self, BuildFeatures};
use crate::naming::{self, NamerKind};
use crate::pipeline::{self, PruneVariant};
use crate::schema::{
    CheckDistrict, CheckEdge, CheckFile, CheckFileStatus, CheckLandmark, CheckReport,
    CheckThresholds, CheckVerdict, GraphData, MapDocument,
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

/// Landmark kinds reported as context (issue #170: hazard and bridge).
const TOUCHED_LANDMARKS: [&str; 2] = ["bridge", "hazard"];

pub const LOWER_BOUND_NOTE: &str = "numbers are a lower bound: tolmap's graph holds only the references it can resolve (calls through variables, dynamic imports and reflection are missed), so the change couples at least this much";

#[derive(Clone, Debug)]
pub struct CheckOptions {
    pub repo: PathBuf,
    pub base: String,
    pub head: Option<String>,
    pub base_map: Option<PathBuf>,
    pub max_districts: Option<usize>,
    pub max_dq: Option<f64>,
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
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(options)));
    match outcome {
        Ok(Ok(report)) => {
            let rendered = if format == "json" {
                render_json(&report)
            } else {
                render_text(&report)
            };
            let mut stdout = std::io::stdout().lock();
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
            eprintln!("tolmap check: {error}");
            error.exit_code()
        }
        Err(_) => {
            eprintln!("tolmap check: internal error: panicked");
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
    if let Some(max_dq) = options.max_dq {
        if !(max_dq.is_finite() && max_dq >= 0.0) {
            return Err(input(format!(
                "--max-dq {max_dq} is not a modularity drop: pass a number >= 0 (0.005 fails a change that lowers q by more than 0.005)"
            )));
        }
    }
    let top = toplevel(&options.repo)?;
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
    let source = detect_source(&base_dir)?;
    eprintln!(
        "tolmap check: base {base}, head {}, source {} at {}",
        head.as_deref().unwrap_or("working tree"),
        source.1.as_str(),
        source.0
    );
    let stage = Instant::now();
    let base_graph = extract_graph(&base_dir, &source)?;
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
    if !head_dir.join(&source.0).is_dir() {
        return Err(input(format!(
            "the source root {} detected at base is not a directory at head; the check compares one source on both sides",
            source.0
        )));
    }
    let stage = Instant::now();
    let head_graph = extract_graph(&head_dir, &source)?;
    let head_graph_s = stage.elapsed().as_secs_f64();
    drop(scratch);

    let thresholds = CheckThresholds {
        max_districts: options.max_districts,
        max_dq: options.max_dq.map(round),
    };
    let report = compare(
        base, head, &changes, &base_map, base_graph, head_graph, thresholds,
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
}

fn side_graph(mut data: GraphData) -> Result<SideGraph, CheckError> {
    pipeline::apply_prune_variant(&mut data, PruneVariant::default()).map_err(internal)?;
    let index = file_index(&data);
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
    base: String,
    head: Option<String>,
    changes: &[Change],
    base_map: &MapDocument,
    base_graph: GraphData,
    head_graph: GraphData,
    thresholds: CheckThresholds,
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
    let renamed_from = changes
        .iter()
        .filter_map(|change| match change {
            Change::Renamed { from, to } => Some((to.as_str(), from.as_str())),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();

    // Head nodes: a file the base map holds (under its base path, for a
    // rename) keeps its base district; anything else is new and is placed.
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

    let base_side = side_graph(base_graph)?;
    let head_side = side_graph(head_graph)?;
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
    let edges_added = sorted_edges(
        head_cross
            .iter()
            .filter(|(key, _)| !base_cross.contains_key(*key))
            .map(|(_, edge)| edge.clone())
            .collect(),
    );
    let edges_removed = sorted_edges(
        base_cross
            .iter()
            .filter(|(key, _)| !head_cross.contains_key(*key))
            .map(|(_, edge)| edge.clone())
            .collect(),
    );

    // The changed files that are on the map, and the districts they count
    // toward.
    let mut files = Vec::new();
    for change in changes {
        let (path, base_path, status, reported_base) = match change {
            Change::Added(path) => (path, None, CheckFileStatus::Added, None),
            Change::Modified(path) => (path, Some(path), CheckFileStatus::Modified, None),
            Change::Deleted(path) => (path, Some(path), CheckFileStatus::Deleted, None),
            Change::Renamed { from, to } => {
                (to, Some(from), CheckFileStatus::Renamed, Some(from.clone()))
            }
        };
        let in_base = base_path.and_then(|file| base_membership.get(file.as_str()).copied());
        let in_head = if status == CheckFileStatus::Deleted {
            None
        } else {
            head_index.get(path).copied()
        };
        let (district, placed) = match (in_base, in_head) {
            (Some(district), _) => (Some(district), false),
            (None, Some(index)) => (head_district[index], head_district[index].is_some()),
            (None, None) => continue,
        };
        files.push(CheckFile {
            path: path.clone(),
            base_path: reported_base,
            status,
            district,
            placed,
        });
    }
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
    let failed = thresholds
        .max_districts
        .is_some_and(|max| districts_crossed > max)
        || thresholds.max_dq.is_some_and(|max| delta_q < -max);
    Ok(CheckReport {
        version: REPORT_VERSION,
        base,
        head,
        districts_crossed,
        districts,
        files,
        unplaced_files,
        modularity_base: round(q_base),
        modularity_head: round(q_head),
        delta_q,
        edges_added,
        edges_removed,
        landmark_touches,
        thresholds,
        verdict: if failed {
            CheckVerdict::Fail
        } else {
            CheckVerdict::Pass
        },
        lower_bound: true,
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
        });
    }
    result
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

// ---------------------------------------------------------------------
// Text format
// ---------------------------------------------------------------------

/// Three lines (districts crossed, Δq, verdict), then the top
/// [`TEXT_EDGE_LIMIT`] cross-district edges by weight, context lines, and
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

    let mut listed = report
        .edges_added
        .iter()
        .map(|edge| ('+', edge))
        .chain(report.edges_removed.iter().map(|edge| ('-', edge)))
        .collect::<Vec<_>>();
    listed.sort_by(|left, right| {
        right
            .1
            .weight
            .total_cmp(&left.1.weight)
            .then_with(|| left.0.cmp(&right.0))
            .then_with(|| left.1.source.cmp(&right.1.source))
            .then_with(|| left.1.target.cmp(&right.1.target))
    });
    let _ = writeln!(
        out,
        "cross-district edges: {} added, {} removed",
        report.edges_added.len(),
        report.edges_removed.len()
    );
    for (sign, edge) in listed.iter().take(TEXT_EDGE_LIMIT) {
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
    }
    if listed.len() > TEXT_EDGE_LIMIT {
        let _ = writeln!(
            out,
            "  ... {} more in --format json",
            listed.len() - TEXT_EDGE_LIMIT
        );
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
    if checks.is_empty() {
        format!("{verdict} (report only: no threshold given)")
    } else {
        format!("{verdict} ({})", checks.join(", "))
    }
}

// ---------------------------------------------------------------------
// Inputs: git, detection, extraction, the base map
// ---------------------------------------------------------------------

fn git_output(dir: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
}

/// Runs git and returns stdout, or an internal error with git's stderr.
fn git_stdout(dir: &Path, args: &[&str]) -> Result<Vec<u8>, CheckError> {
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

fn resolve_commit(top: &Path, reference: &str, flag: &str) -> Result<String, CheckError> {
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
    let recorded = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    if !is_object_id(recorded) {
        return Err(input(format!(
            "--base-map {}: cannot verify which commit it was built from; name it <full commit id>.json, as the service store does",
            path.display()
        )));
    }
    if recorded != base {
        return Err(input(format!(
            "--base-map {} was built from {recorded}, not --base {base}",
            path.display()
        )));
    }
    Ok(document)
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
    let mut fields = bytes
        .split(|&byte| byte == 0)
        .map(|field| String::from_utf8_lossy(field).into_owned());
    let mut changes = Vec::new();
    let truncated = || {
        internal(anyhow::anyhow!(
            "git diff --name-status output is truncated"
        ))
    };
    while let Some(status) = fields.next() {
        if status.is_empty() {
            continue;
        }
        let path = fields.next().ok_or_else(truncated)?;
        let change = match status.as_bytes()[0] {
            b'R' => Change::Renamed {
                from: path,
                to: fields.next().ok_or_else(truncated)?,
            },
            b'C' => Change::Added(fields.next().ok_or_else(truncated)?),
            b'A' => Change::Added(path),
            b'D' => Change::Deleted(path),
            _ => Change::Modified(path),
        };
        changes.push(change);
    }
    Ok(changes)
}

/// Temporary worktrees, and the directory that holds them and the built base
/// map. Removed on drop, including on an error or a panic, so the user's
/// repository is left with no stray worktree.
struct Scratch {
    top: PathBuf,
    root: PathBuf,
    worktrees: Vec<PathBuf>,
}

static SCRATCH_COUNTER: AtomicUsize = AtomicUsize::new(0);

impl Scratch {
    fn new(top: &Path) -> Result<Self, CheckError> {
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
    fn worktree(&mut self, commit: &str, label: &str) -> Result<PathBuf, CheckError> {
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
fn detect_source(dir: &Path) -> Result<(String, LanguageKind), CheckError> {
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

fn extract_graph(dir: &Path, source: &(String, LanguageKind)) -> Result<GraphData, CheckError> {
    extract::build_multi_source_with_refs(dir, std::slice::from_ref(source), RefsMode::default())
        .map_err(|error| internal(error.context(format!("extract {}", dir.display()))))
}

/// Builds the base map the way `tolmap build` would, cold, into scratch.
/// Parcels are skipped: they are drawn after the partition and never change
/// membership, names or landmarks, which is all the check reads.
fn build_base_map(graph: GraphData, out: &Path) -> Result<MapDocument, CheckError> {
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
    serde_json::from_slice(&raw)
        .map_err(|error| internal(anyhow::anyhow!("parse {}: {error}", path.display())))
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
}
