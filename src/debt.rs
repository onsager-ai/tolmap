//! Structural debt findings (issues #250, #251): district cycles, misplaced
//! files, god files, hotspots, unreached files and hidden coupling, computed
//! from a map document
//! alone. `findings` is a pure function of `&MapDocument`: it never changes
//! the map, and the same map always gives a byte-identical report and lens.
//!
//! Every count is a lower bound on what the map's edges show, and the map's
//! edges are themselves a lower bound (`coverage`, echoed in the report).
//! Dynamic imports, reflection and declared connections are invisible here,
//! so an unreached file is reported as "no inbound edge found", never as
//! dead.
//!
//! Plain `//` comments on exported types, not doc comments: ts-rs copies doc
//! comments into the bindings.
use crate::cycle_cut::{greedy_cut, mutual_pairs, Bundles};
use crate::extract::COCHANGE_MIN_COUNT;
use crate::lens::{
    self, FileRename, HopMode, Lens, LensArea, LensColor, LensNote, LensPath, LensReport, LensStop,
    LENS_VERSION,
};
use crate::lens_comparison::{self, ComparisonMode, LensDelta};
use crate::pipeline::SHARE_COCHANGE;
use crate::schema::{CoverageReport, MapDocument};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use ts_rs::TS;

pub const DEBT_REPORT_VERSION: u32 = 4;

// ---- thresholds ------------------------------------------------------------

// Misplaced file: a file needs at least this many incident edges (in plus
// out, self-edges excluded) before its district is questioned. Below it, one
// or two imports decide the share, which says more about the file's size
// than its placement.
pub const MISPLACED_MIN_EDGES: usize = 5;
// ...and strictly more than this share of those edges must go to one other
// district: "most of its edges", so the other district outweighs its own and
// every third district combined.
pub const MISPLACED_MIN_SHARE: f64 = 0.5;
// God file: in the top 5% of files (the 95th percentile, nearest rank) by
// fan-in AND by complexity. Either alone is common and often healthy (a
// shared utility, a parser); both together is the file everything depends on
// and nobody can change safely.
pub const GOD_TOP_FRACTION: f64 = 0.05;
// Hotspot: churn × complexity in the top 5% of files (nearest rank)...
pub const HOTSPOT_TOP_FRACTION: f64 = 0.05;
// ...but never fewer than two, so the list always covers the map's two
// `hazard` landmarks, which it generalises.
pub const HOTSPOT_MIN_COUNT: usize = 2;
// Hidden coupling uses the evidence retained in the optional map sidecar.
// The count and cutoff are the extraction/pipeline constants that wrote it.
pub const HIDDEN_COUPLING_LENS_PATHS: usize = 20;
// Keep actionable-cycle paths bounded just as hidden-coupling paths are: a
// large cycle can have hundreds of cut bundles, while its area remains whole.
pub const CYCLE_LENS_PATHS: usize = 20;

// Unreached file exclusions. Entry points have no importer by design, so a
// file is an entry when the map marks it an `entry` landmark, or by name:
// - its base name, up to the first `.`, is one of these. `tests` is a Rust
//   `#[cfg(test)] mod tests;` child: nothing `use`s it, and its parent's `mod`
//   gives no inbound edge on the map, so on semon 04aeb54 9 of the 26 files
//   with no inbound edge found were `tests.rs` (docs/FINDINGS.md, finding 63);
const TEST_STEMS: &[&str] = &["tests"];
const TEST_DIRS: &[&str] = &["test", "tests", "__tests__", "spec", "e2e"];
const TEST_MARKERS: &[&str] = &[".test.", ".spec."];
const TEST_STEM_PREFIXES: &[&str] = &["test_"];
const TEST_STEM_SUFFIXES: &[&str] = &["_test", "_tests", "_spec"];
const NON_PRODUCTION_DIRS: &[&str] = &["storybook"];
const NON_PRODUCTION_MARKERS: &[&str] = &[".stories."];

pub const ENTRY_STEMS: &[&str] = &[
    "main",
    "__main__",
    "__init__",
    "index",
    "lib",
    "mod",
    "build",
    "app",
    "cli",
    "server",
    "setup",
    "conftest",
    "manage",
    "wsgi",
    "asgi",
    TEST_STEMS[0],
];
// - a directory on its path is one of these (tests, tools and examples are
//   run, not imported);
pub const ENTRY_DIRS: &[&str] = &[
    TEST_DIRS[0],
    TEST_DIRS[1],
    TEST_DIRS[2],
    TEST_DIRS[3],
    TEST_DIRS[4],
    "bin",
    "scripts",
    "examples",
    "benches",
];
// - its base name contains one of these (test, story, tool config and
//   ambient declaration files);
pub const ENTRY_MARKERS: &[&str] = &[TEST_MARKERS[0], TEST_MARKERS[1], ".config.", ".d."];
// - or its stem starts `test_` or ends `_test` / `_spec` (Python, Go, Ruby).
pub const ENTRY_STEM_PREFIXES: &[&str] = &[TEST_STEM_PREFIXES[0]];
pub const ENTRY_STEM_SUFFIXES: &[&str] = &[TEST_STEM_SUFFIXES[0], TEST_STEM_SUFFIXES[1]];

// ---- report ----------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
pub struct DebtThresholds {
    pub misplaced_min_edges: usize,
    pub misplaced_min_share: f64,
    pub god_top_fraction: f64,
    // The nearest-rank cuts the fraction gives on this map; null when no file
    // has a positive value.
    pub god_min_fanin: Option<f64>,
    pub god_min_complexity: Option<usize>,
    pub hotspot_top_fraction: f64,
    pub hotspot_min_count: usize,
    pub hotspot_min_score: Option<usize>,
    pub test_hotspot_files_excluded: usize,
    pub non_production_hotspot_files_excluded: usize,
    pub misplaced_test_files_excluded: usize,
    pub misplaced_generated_files_excluded: usize,
    pub unreached_generated_files_excluded: usize,
    pub hidden_coupling_test_pairs_excluded: usize,
    pub hidden_coupling_generated_pairs_suppressed: usize,
    pub hidden_coupling_min_count: usize,
    pub hidden_coupling_min_weight: f64,
    pub entry_stems: Vec<String>,
    pub entry_dirs: Vec<String>,
    pub entry_markers: Vec<String>,
    pub entry_stem_prefixes: Vec<String>,
    pub entry_stem_suffixes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct DebtDistrict {
    pub id: usize,
    pub name: String,
}

// One file edge of `E`, by path, with both districts.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct DebtEdge {
    pub from: String,
    pub to: String,
    pub from_district: usize,
    pub to_district: usize,
}

// A strongly connected set of districts in the district graph built from
// `E`. `route` is one shortest cycle through it (district ids, the first not
// repeated at the end), and `closing_edges` one file edge per hop of the
// route, the last closing back to the first district. `cross_edges` counts
// every file edge between two different districts of the set.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct DistrictCycle {
    pub districts: Vec<DebtDistrict>,
    pub route: Vec<usize>,
    pub closing_edges: Vec<DebtEdge>,
    pub cross_edges: usize,
    /// The lighter direction of every mutually connected district pair.
    pub mutual_pairs: Vec<DistrictBundle>,
    /// A deterministic greedy feedback-arc cut. It is a small cut, not a
    /// minimum cut: returning every bundle makes the district graph acyclic.
    pub cut: Vec<DistrictBundle>,
}

/// All file edges in one directed district-to-district bundle.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct DistrictBundle {
    pub from_district: usize,
    pub to_district: usize,
    pub edges: Vec<DebtEdge>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct DistrictEdges {
    pub district: usize,
    pub name: String,
    pub edges: usize,
}

// `by_district` counts the file's incident `E` edges, plus a declared Rust
// `mod` parent-child pair only when `E` has no edge between those files, by
// the other end's district (its own included), most first. `share` is
// `to_district`'s share of `edges`, rounded to 4 places.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
pub struct MisplacedFile {
    pub path: String,
    pub district: usize,
    pub edges: usize,
    pub to_district: usize,
    pub share: f64,
    pub by_district: Vec<DistrictEdges>,
}

// Ranks are 1 + the number of files with a strictly greater value.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
pub struct GodFile {
    pub path: String,
    pub district: usize,
    pub fanin: f64,
    pub fanin_rank: usize,
    pub complexity: usize,
    pub complexity_rank: usize,
}

// `score` is churn × complexity; `rank` orders by score, then map order,
// exactly as the `hazard` landmark does.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct Hotspot {
    pub path: String,
    pub district: usize,
    pub rank: usize,
    pub churn: usize,
    pub complexity: usize,
    pub score: usize,
}

// A non-entry file with at least one outbound edge and no inbound edge found.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct UnreachedFile {
    pub path: String,
    pub district: usize,
    pub out_edges: usize,
}

/// A Rust source file in a crate source tree that no resolved crate-root
/// declaration tree reaches. This is a lower bound: `#[path]` is included,
/// but macro-generated `mod`s are intentionally not guessed.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct UndeclaredRustFile {
    pub path: String,
    pub district: usize,
}

// A co-change pair without any file edge in `E` or reference edge in `U`.
// The sidecar itself is optional, so `hidden_coupling_measured` distinguishes
// an old map from one where the measured lower bound is zero.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
pub struct HiddenCoupling {
    pub left: String,
    pub right: String,
    pub left_district: usize,
    pub right_district: usize,
    pub cochanges: usize,
    pub weight: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DebtReport {
    pub version: u32,
    pub repo: String,
    // Always true: see the module comment.
    pub lower_bound: bool,
    pub files: usize,
    pub edges: usize,
    pub thresholds: DebtThresholds,
    // The map's own edge coverage, unchanged; null for maps built before it.
    pub coverage: Option<CoverageReport>,
    pub cycles: Vec<DistrictCycle>,
    pub misplaced: Vec<MisplacedFile>,
    pub god_files: Vec<GodFile>,
    pub hotspots: Vec<Hotspot>,
    /// Test files use the same path rule as no-inbound entry files and are
    /// excluded from production hotspot ranking.
    pub test_hotspot_files_excluded: usize,
    /// Storybook files and Storybook stories are non-production, but not
    /// tests, so only production-hotspot ranking excludes them.
    pub non_production_hotspot_files_excluded: usize,
    /// Test files are excluded from misplaced-file findings.
    pub misplaced_test_files_excluded: usize,
    /// False for maps built before generated-file evidence was retained.
    pub generated_file_measurement: bool,
    /// Generated files excluded from misplaced findings.
    pub misplaced_generated_files_excluded: usize,
    /// Generated files excluded from no-inbound findings.
    pub unreached_generated_files_excluded: usize,
    pub unreached: Vec<UnreachedFile>,
    /// False for maps built before the Rust declaration sidecar existed.
    pub rust_mod_reachability_measured: bool,
    pub undeclared_rust: Vec<UndeclaredRustFile>,
    /// False for maps built before statically readable bundler entries were
    /// retained in the debt sidecar.
    pub build_entry_measurement: bool,
    pub build_entry_files: usize,
    pub hidden_coupling_measured: bool,
    /// Test paths use the same rule as production hotspot exclusions and are
    /// excluded from hidden-coupling ranking.
    pub hidden_coupling_test_pairs_excluded: usize,
    /// Pairs suppressed because a generated ts-rs binding is paired with its
    /// generator or a file already statically linked to that generator.
    pub hidden_coupling_generated_pairs_suppressed: usize,
    pub hidden_coupling: Vec<HiddenCoupling>,
    // Files the entry rule excludes from `unreached`, and files with no edge
    // at all. The latter are not reported as unreached: a file the extractor
    // saw no import in or out of is a coverage gap before it is a finding.
    pub entry_files: usize,
    pub isolated_files: usize,
}

// ---- findings --------------------------------------------------------------

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn district_name(map: &MapDocument, id: usize) -> String {
    map.names.get(&id.to_string()).cloned().unwrap_or_default()
}

pub fn is_entry_path(path: &str) -> bool {
    let (dirs, name) = path.rsplit_once('/').unwrap_or(("", path));
    if dirs.split('/').any(|d| ENTRY_DIRS.contains(&d)) {
        return true;
    }
    let stem = name.split('.').next().unwrap_or(name);
    ENTRY_STEMS.contains(&stem)
        || ENTRY_MARKERS.iter().any(|m| name.contains(m))
        || ENTRY_STEM_PREFIXES.iter().any(|p| stem.starts_with(p))
        || ENTRY_STEM_SUFFIXES.iter().any(|s| stem.ends_with(s))
}

fn is_test_path(path: &str) -> bool {
    let (dirs, name) = path.rsplit_once('/').unwrap_or(("", path));
    if dirs.split('/').any(|dir| TEST_DIRS.contains(&dir)) {
        return true;
    }
    let stem = name.split('.').next().unwrap_or(name);
    TEST_STEMS.contains(&stem)
        || TEST_MARKERS.iter().any(|marker| name.contains(marker))
        || TEST_STEM_PREFIXES
            .iter()
            .any(|prefix| stem.starts_with(prefix))
        || TEST_STEM_SUFFIXES
            .iter()
            .any(|suffix| stem.ends_with(suffix))
}

fn is_non_production_path(path: &str) -> bool {
    let (dirs, name) = path.rsplit_once('/').unwrap_or(("", path));
    dirs.split('/')
        .any(|dir| NON_PRODUCTION_DIRS.contains(&dir))
        || NON_PRODUCTION_MARKERS
            .iter()
            .any(|marker| name.contains(marker))
}

// There is no per-file language in the compact map. Keep related extensions
// together so TypeScript's `.ts` and `.tsx`, for example, do not look like a
// cross-language contract merely because their suffixes differ.
fn language_from_path(path: &str) -> &str {
    match path.rsplit_once('.').map_or("", |(_, extension)| extension) {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" | "pyi" => "python",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "rb" => "ruby",
        "php" => "php",
        "cs" => "csharp",
        extension => extension,
    }
}

fn same_parent_directory(left: &str, right: &str) -> bool {
    left.rsplit_once('/').map_or("", |(parent, _)| parent)
        == right.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn is_rust_target_path(path: &str) -> bool {
    path == "build.rs"
        || path.ends_with("/build.rs")
        || path.starts_with("src/bin/")
        || path.contains("/examples/")
        || path.contains("/tests/")
        || path.contains("/benches/")
}

fn is_below_rust_root(path: &str, root: &str) -> bool {
    if root == "build.rs" || root.ends_with("/build.rs") {
        return false;
    }
    let source = root
        .find("src/")
        .map(|index| &root[..index + 3])
        .or_else(|| root.rsplit_once('/').map(|(parent, _)| parent));
    source.is_some_and(|source| path.starts_with(source))
}

// The nearest-rank cut for the top `fraction` of `values`: the k-th largest,
// k = ceil(fraction × n), at least `min_count`. None when it is not positive.
fn top_cut<T: Copy + PartialOrd + Default>(
    values: &[T],
    fraction: f64,
    min_count: usize,
) -> Option<T> {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let k = ((fraction * values.len() as f64).ceil() as usize)
        .max(min_count)
        .max(1)
        .min(values.len());
    let cut = *sorted.get(k.checked_sub(1)?)?;
    (cut > T::default()).then_some(cut)
}

fn rank<T: PartialOrd>(values: &[T], value: &T) -> usize {
    1 + values.iter().filter(|v| *v > value).count()
}

// Tarjan's strongly connected components over district ids, visited in id
// order so the output never depends on hash order.
fn components(graph: &BTreeMap<usize, BTreeSet<usize>>) -> Vec<Vec<usize>> {
    struct T<'a> {
        graph: &'a BTreeMap<usize, BTreeSet<usize>>,
        index: BTreeMap<usize, usize>,
        low: BTreeMap<usize, usize>,
        stack: Vec<usize>,
        on: BTreeSet<usize>,
        out: Vec<Vec<usize>>,
    }
    fn visit(t: &mut T, v: usize) {
        let i = t.index.len();
        t.index.insert(v, i);
        t.low.insert(v, i);
        t.stack.push(v);
        t.on.insert(v);
        let graph = t.graph;
        for &w in graph.get(&v).into_iter().flatten() {
            if !t.index.contains_key(&w) {
                visit(t, w);
                let lw = t.low[&w];
                let lv = t.low.get_mut(&v).unwrap();
                *lv = (*lv).min(lw);
            } else if t.on.contains(&w) {
                let iw = t.index[&w];
                let lv = t.low.get_mut(&v).unwrap();
                *lv = (*lv).min(iw);
            }
        }
        if t.low[&v] == t.index[&v] {
            let mut scc = vec![];
            while let Some(w) = t.stack.pop() {
                t.on.remove(&w);
                scc.push(w);
                if w == v {
                    break;
                }
            }
            scc.sort_unstable();
            t.out.push(scc);
        }
    }
    let mut t = T {
        graph,
        index: BTreeMap::new(),
        low: BTreeMap::new(),
        stack: vec![],
        on: BTreeSet::new(),
        out: vec![],
    };
    for &v in graph.keys() {
        if !t.index.contains_key(&v) {
            visit(&mut t, v);
        }
    }
    let mut out: Vec<_> = t.out.into_iter().filter(|c| c.len() > 1).collect();
    out.sort();
    out
}

fn cycles(map: &MapDocument) -> Vec<DistrictCycle> {
    let district = |f: usize| map.nodes[f].district();
    let mut bundles = BTreeMap::<(usize, usize), Vec<DebtEdge>>::new();
    let mut graph: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for &[a, b] in &map.edges {
        let (from_district, to_district) = (district(a), district(b));
        if from_district == to_district {
            continue;
        }
        graph.entry(from_district).or_default().insert(to_district);
        bundles
            .entry((from_district, to_district))
            .or_default()
            .push(DebtEdge {
                from: map.files[a].clone(),
                to: map.files[b].clone(),
                from_district,
                to_district,
            });
    }
    for edges in bundles.values_mut() {
        edges.sort_by(|a, b| a.from.cmp(&b.from).then(a.to.cmp(&b.to)));
    }
    components(&graph)
        .into_iter()
        .map(|scc| {
            let set: BTreeSet<usize> = scc.iter().copied().collect();
            let inside = |(from, to): &(usize, usize)| set.contains(from) && set.contains(to);
            let weights: Bundles = bundles
                .iter()
                .filter(|(key, _)| inside(key))
                .map(|(&key, edges)| (key, edges.len()))
                .collect();
            let bundle = |(from_district, to_district)| DistrictBundle {
                from_district,
                to_district,
                edges: bundles[&(from_district, to_district)].clone(),
            };
            // Start and traverse by file paths so the contextual route does
            // not change merely because districts were renumbered.
            let start = bundles
                .iter()
                .filter(|(key, _)| inside(key))
                .min_by_key(|(_, edges)| (&edges[0].from, &edges[0].to))
                .expect("cycle has a bundle")
                .0
                 .0;
            let successors = |from: usize| {
                let mut next = graph[&from]
                    .iter()
                    .filter(|to| set.contains(to))
                    .map(|&to| (to, &bundles[&(from, to)][0]))
                    .collect::<Vec<_>>();
                next.sort_by_key(|(_, edge)| (&edge.from, &edge.to));
                next
            };
            let mut prev = BTreeMap::from([(start, start)]);
            let mut queue = VecDeque::from([start]);
            let mut route = vec![];
            'bfs: while let Some(from) = queue.pop_front() {
                for (to, _) in successors(from) {
                    if to == start {
                        let mut current = from;
                        route.push(from);
                        while current != start {
                            current = prev[&current];
                            route.push(current);
                        }
                        route.reverse();
                        break 'bfs;
                    }
                    if let std::collections::btree_map::Entry::Vacant(entry) = prev.entry(to) {
                        entry.insert(from);
                        queue.push_back(to);
                    }
                }
            }
            let closing_edges = (0..route.len())
                .map(|index| bundles[&(route[index], route[(index + 1) % route.len()])][0].clone())
                .collect();
            let cross_edges = weights.values().sum();
            DistrictCycle {
                districts: scc
                    .iter()
                    .map(|&id| DebtDistrict {
                        id,
                        name: district_name(map, id),
                    })
                    .collect(),
                route,
                closing_edges,
                cross_edges,
                mutual_pairs: mutual_pairs(&weights).into_iter().map(bundle).collect(),
                cut: greedy_cut(&weights).into_iter().map(bundle).collect(),
            }
        })
        .collect()
}

pub fn findings(map: &MapDocument) -> DebtReport {
    let n = map.files.len();
    let district = |f: usize| map.nodes[f].district();
    let file_edges = map
        .edges
        .iter()
        .map(|&[left, right]| (left.min(right), left.max(right)))
        .collect::<BTreeSet<_>>();
    let linked = file_edges
        .iter()
        .copied()
        .chain(map.uses.iter().flat_map(|(key, users)| {
            let Some((target, _)) = key.split_once(':') else {
                return Vec::new();
            };
            let Ok(target) = target.parse::<usize>() else {
                return Vec::new();
            };
            users
                .iter()
                .map(|&user| (target.min(user), target.max(user)))
                .collect()
        }))
        .collect::<BTreeSet<_>>();
    let generated = map.debt.as_ref().and_then(|debt| debt.generated.as_ref());
    let generated_file_measurement = generated.is_some();
    let generated_files = generated
        .map(|files| {
            files
                .iter()
                .map(|file| file.file as usize)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let generated_paths = generated_files
        .iter()
        .filter_map(|&file| map.files.get(file).map(String::as_str))
        .collect::<BTreeSet<_>>();
    let generated_by_file = generated
        .map(|files| {
            files
                .iter()
                .map(|file| (file.file as usize, file.generator.map(|id| id as usize)))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut inbound = vec![0usize; n];
    let mut outbound = vec![0usize; n];
    let mut by_district: Vec<BTreeMap<usize, usize>> = vec![BTreeMap::new(); n];
    for &[a, b] in &map.edges {
        if a == b {
            continue;
        }
        outbound[a] += 1;
        inbound[b] += 1;
        *by_district[a].entry(district(b)).or_default() += 1;
        *by_district[b].entry(district(a)).or_default() += 1;
    }
    // A Rust `mod` declaration is not a graph edge: it names no item and must
    // remain outside extraction, blending, clustering and layout. For this
    // report only, however, it is one incident relationship between parent and
    // child when `E` did not retain that relationship already.
    if let Some(rust_mod) = map.debt.as_ref().and_then(|debt| debt.rust_mod.as_ref()) {
        for pair in &rust_mod.pairs {
            let (a, b) = pair.0;
            if a == b || file_edges.contains(&(a.min(b), a.max(b))) {
                continue;
            }
            *by_district[a].entry(district(b)).or_default() += 1;
            *by_district[b].entry(district(a)).or_default() += 1;
        }
    }

    let misplaced_test_files_excluded = (0..n).filter(|&f| is_test_path(&map.files[f])).count();
    let mut misplaced: Vec<MisplacedFile> = (0..n)
        .filter(|&f| !is_test_path(&map.files[f]))
        .filter_map(|f| {
            let counts = &by_district[f];
            let edges: usize = counts.values().sum();
            if edges < MISPLACED_MIN_EDGES {
                return None;
            }
            let own = district(f);
            // Most edges first, then the smaller district id.
            let (&to, &count) = counts
                .iter()
                .filter(|(d, _)| **d != own)
                .max_by_key(|(d, c)| (**c, std::cmp::Reverse(**d)))?;
            let share = count as f64 / edges as f64;
            if share <= MISPLACED_MIN_SHARE {
                return None;
            }
            let mut rows: Vec<DistrictEdges> = counts
                .iter()
                .map(|(&d, &c)| DistrictEdges {
                    district: d,
                    name: district_name(map, d),
                    edges: c,
                })
                .collect();
            rows.sort_by_key(|r| (std::cmp::Reverse(r.edges), r.district));
            Some(MisplacedFile {
                path: map.files[f].clone(),
                district: own,
                edges,
                to_district: to,
                share: round4(share),
                by_district: rows,
            })
        })
        .collect();
    misplaced.sort_by(|a, b| {
        b.share
            .total_cmp(&a.share)
            .then(b.edges.cmp(&a.edges))
            .then(a.path.cmp(&b.path))
    });
    let misplaced_generated_files_excluded = misplaced
        .iter()
        .filter(|file| generated_paths.contains(file.path.as_str()))
        .count();
    misplaced.retain(|file| !generated_paths.contains(file.path.as_str()));

    let fanin: Vec<f64> = map.nodes.iter().map(|r| r.fanin()).collect();
    let complexity: Vec<usize> = map.nodes.iter().map(|r| r.complexity()).collect();
    let god_min_fanin = top_cut(&fanin, GOD_TOP_FRACTION, 1);
    let god_min_complexity = top_cut(&complexity, GOD_TOP_FRACTION, 1);
    let mut god_files: Vec<GodFile> = match (god_min_fanin, god_min_complexity) {
        (Some(cf), Some(cc)) => (0..n)
            .filter(|&f| fanin[f] >= cf && complexity[f] >= cc)
            .map(|f| GodFile {
                path: map.files[f].clone(),
                district: district(f),
                fanin: fanin[f],
                fanin_rank: rank(&fanin, &fanin[f]),
                complexity: complexity[f],
                complexity_rank: rank(&complexity, &complexity[f]),
            })
            .collect(),
        _ => vec![],
    };
    god_files.sort_by(|a, b| {
        b.fanin
            .total_cmp(&a.fanin)
            .then(b.complexity.cmp(&a.complexity))
            .then(a.path.cmp(&b.path))
    });

    let score: Vec<usize> = map
        .nodes
        .iter()
        .map(|r| r.churn() * r.complexity())
        .collect();
    // Tests churn with production work, but they are not production change
    // hotspots. Storybook is likewise non-production, without becoming a
    // test or an entry for any other finding.
    let test_hotspot_files_excluded = (0..n).filter(|&f| is_test_path(&map.files[f])).count();
    let non_production_hotspot_files_excluded = (0..n)
        .filter(|&f| is_non_production_path(&map.files[f]))
        .count();
    let production =
        |f: usize| !is_test_path(&map.files[f]) && !is_non_production_path(&map.files[f]);
    let production_scores = (0..n)
        .filter(|&f| production(f))
        .map(|f| score[f])
        .collect::<Vec<_>>();
    let hotspot_min_score = top_cut(&production_scores, HOTSPOT_TOP_FRACTION, HOTSPOT_MIN_COUNT);
    let mut hot: Vec<usize> = match hotspot_min_score {
        Some(cut) => (0..n)
            .filter(|&f| production(f) && score[f] >= cut)
            .collect(),
        None => vec![],
    };
    // Stable: ties keep map order, as `pipeline::landmarks` orders hazards.
    hot.sort_by_key(|&f| std::cmp::Reverse(score[f]));
    let hotspots = hot
        .into_iter()
        .enumerate()
        .map(|(i, f)| Hotspot {
            path: map.files[f].clone(),
            district: district(f),
            rank: i + 1,
            churn: map.nodes[f].churn(),
            complexity: complexity[f],
            score: score[f],
        })
        .collect();

    let landmark_entries: BTreeSet<usize> = map
        .landmarks
        .iter()
        .filter(|l| l.0 .1 == "entry")
        .map(|l| l.0 .0)
        .collect();
    let build_entries = map
        .debt
        .as_ref()
        .and_then(|debt| debt.build_entries.as_ref());
    let build_entry_measurement = build_entries.is_some();
    let build_entry_files = build_entries.map_or(0, |entries| entries.files.len());
    let configured_entries = build_entries
        .map(|entries| entries.files.iter().copied().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    let entry = |f: usize| {
        landmark_entries.contains(&f)
            || configured_entries.contains(&f)
            || is_entry_path(&map.files[f])
    };
    let entry_files = (0..n).filter(|&f| entry(f)).count();
    let isolated_files = (0..n)
        .filter(|&f| inbound[f] == 0 && outbound[f] == 0)
        .count();
    let rust_mod = map.debt.as_ref().and_then(|debt| debt.rust_mod.as_ref());
    let rust_mod_reachability_measured = rust_mod.is_some();
    let declared_rust = rust_mod
        .map(|tree| {
            tree.pairs
                .iter()
                .map(|pair| pair.0 .1)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let rust_roots = rust_mod
        .map(|tree| tree.roots.iter().copied().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    let mut unreached = (0..n)
        .filter(|&f| {
            inbound[f] == 0
                && outbound[f] > 0
                && !entry(f)
                && !(map.files[f].ends_with(".rs") && declared_rust.contains(&f))
        })
        .map(|f| UnreachedFile {
            path: map.files[f].clone(),
            district: district(f),
            out_edges: outbound[f],
        })
        .collect::<Vec<_>>();
    let unreached_generated_files_excluded = unreached
        .iter()
        .filter(|file| generated_paths.contains(file.path.as_str()))
        .count();
    unreached.retain(|file| !generated_paths.contains(file.path.as_str()));
    let undeclared_rust = rust_mod
        .map(|_| {
            (0..n)
                .filter(|&f| {
                    map.files[f].ends_with(".rs")
                        && !rust_roots.contains(&f)
                        && !declared_rust.contains(&f)
                        && !is_rust_target_path(&map.files[f])
                        && rust_roots
                            .iter()
                            .any(|&root| is_below_rust_root(&map.files[f], &map.files[root]))
                })
                .map(|f| UndeclaredRustFile {
                    path: map.files[f].clone(),
                    district: district(f),
                })
                .collect()
        })
        .unwrap_or_default();

    let declared_rust_pairs = rust_mod
        .map(|tree| {
            tree.pairs
                .iter()
                .map(|pair| {
                    let (left, right) = pair.0;
                    (left.min(right), left.max(right))
                })
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let mut hidden_coupling_test_pairs_excluded = 0;
    let mut hidden_coupling_generated_pairs_suppressed = 0;
    let hidden_coupling = map
        .debt
        .as_ref()
        .and_then(|debt| debt.cochange.as_ref())
        .map(|cochange| {
            cochange
                .pairs
                .iter()
                .filter_map(|pair| {
                    let (left, right, cochanges, weight) = pair.0;
                    if cochanges < COCHANGE_MIN_COUNT || weight < SHARE_COCHANGE {
                        return None;
                    }
                    let left_path = map.files.get(left)?;
                    let right_path = map.files.get(right)?;
                    if is_test_path(left_path) || is_test_path(right_path) {
                        hidden_coupling_test_pairs_excluded += 1;
                        return None;
                    }
                    let suppresses = |generated: usize, other: usize| {
                        let Some(Some(generator)) = generated_by_file.get(&generated) else {
                            return false;
                        };
                        other == *generator
                            || linked.contains(&((*generator).min(other), (*generator).max(other)))
                            || declared_rust_pairs
                                .contains(&((*generator).min(other), (*generator).max(other)))
                    };
                    if suppresses(left, right) || suppresses(right, left) {
                        hidden_coupling_generated_pairs_suppressed += 1;
                        return None;
                    }
                    if linked.contains(&(left.min(right), left.max(right)))
                        || declared_rust_pairs.contains(&(left.min(right), left.max(right)))
                    {
                        return None;
                    }
                    Some(HiddenCoupling {
                        left: left_path.clone(),
                        right: right_path.clone(),
                        left_district: map.nodes.get(left)?.district(),
                        right_district: map.nodes.get(right)?.district(),
                        cochanges,
                        weight,
                    })
                })
                .collect::<Vec<_>>()
        });
    let hidden_coupling_measured = hidden_coupling.is_some();
    let mut hidden_coupling = hidden_coupling.unwrap_or_default();
    // Contracts across languages come first. Within that group, cross-folder
    // and cross-district evidence is usually more actionable than a local
    // implementation detail; the remaining keys make every tie deterministic.
    hidden_coupling.sort_by(|a, b| {
        (language_from_path(&a.left) == language_from_path(&a.right))
            .cmp(&(language_from_path(&b.left) == language_from_path(&b.right)))
            .then(
                same_parent_directory(&a.left, &a.right)
                    .cmp(&same_parent_directory(&b.left, &b.right)),
            )
            .then((a.left_district == a.right_district).cmp(&(b.left_district == b.right_district)))
            .then(b.weight.total_cmp(&a.weight))
            .then(b.cochanges.cmp(&a.cochanges))
            .then(a.left.cmp(&b.left))
            .then(a.right.cmp(&b.right))
    });

    let strings = |s: &[&str]| -> Vec<String> { s.iter().map(|x| (*x).to_owned()).collect() };
    DebtReport {
        version: DEBT_REPORT_VERSION,
        repo: map.repo.clone(),
        lower_bound: true,
        files: n,
        edges: map.edges.len(),
        thresholds: DebtThresholds {
            misplaced_min_edges: MISPLACED_MIN_EDGES,
            misplaced_min_share: MISPLACED_MIN_SHARE,
            god_top_fraction: GOD_TOP_FRACTION,
            god_min_fanin,
            god_min_complexity,
            hotspot_top_fraction: HOTSPOT_TOP_FRACTION,
            hotspot_min_count: HOTSPOT_MIN_COUNT,
            hotspot_min_score,
            test_hotspot_files_excluded,
            non_production_hotspot_files_excluded,
            misplaced_test_files_excluded,
            misplaced_generated_files_excluded,
            unreached_generated_files_excluded,
            hidden_coupling_test_pairs_excluded,
            hidden_coupling_generated_pairs_suppressed,
            hidden_coupling_min_count: COCHANGE_MIN_COUNT,
            hidden_coupling_min_weight: SHARE_COCHANGE,
            entry_stems: strings(ENTRY_STEMS),
            entry_dirs: strings(ENTRY_DIRS),
            entry_markers: strings(ENTRY_MARKERS),
            entry_stem_prefixes: strings(ENTRY_STEM_PREFIXES),
            entry_stem_suffixes: strings(ENTRY_STEM_SUFFIXES),
        },
        coverage: map.coverage.clone(),
        cycles: cycles(map),
        misplaced,
        god_files,
        hotspots,
        test_hotspot_files_excluded,
        non_production_hotspot_files_excluded,
        misplaced_test_files_excluded,
        generated_file_measurement,
        misplaced_generated_files_excluded,
        unreached_generated_files_excluded,
        unreached,
        rust_mod_reachability_measured,
        undeclared_rust,
        build_entry_measurement,
        build_entry_files,
        hidden_coupling_measured,
        hidden_coupling_test_pairs_excluded,
        hidden_coupling_generated_pairs_suppressed,
        hidden_coupling,
        entry_files,
        isolated_files,
    }
}

// ---- lens ------------------------------------------------------------------

// Kind names, used as area labels. The lens has one area per finding kind
// (owner decision, 2026-11-09): an area tints every district of its members,
// so one area per finding stacked tints wherever findings shared a district.
// Labels carry nothing measured and no district name; values, district names
// and per-district edge counts go in notes anchored to each file.
pub const KIND_CYCLE: &str = "district cycle";
pub const KIND_MISPLACED: &str = "misplaced file";
pub const KIND_GOD: &str = "god file";
pub const KIND_HOTSPOT: &str = "hotspot";
pub const KIND_UNREACHED: &str = "no inbound edge found";
pub const KIND_UNDECLARED_RUST: &str = "undeclared Rust file";
pub const KIND_HIDDEN_COUPLING: &str = "hidden coupling";

fn cycle_files(cycle: &DistrictCycle) -> BTreeSet<&str> {
    cycle
        .closing_edges
        .iter()
        .flat_map(|e| [e.from.as_str(), e.to.as_str()])
        .collect()
}

// A path name ([a-z0-9_-]) derived from the cycle's files, not its position
// in the list, so the same cycle keeps its name across commits. FNV-1a, 64
// bits.
fn path_hash<'a>(files: impl IntoIterator<Item = &'a str>) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in files.into_iter().flat_map(|file| file.bytes().chain([0])) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn cycle_path_name(files: &BTreeSet<&str>) -> String {
    format!("cycle-{:016x}", path_hash(files.iter().copied()))
}

fn hidden_coupling_path_name(coupling: &HiddenCoupling) -> String {
    format!(
        "hidden-coupling-{:016x}",
        path_hash([coupling.left.as_str(), coupling.right.as_str()])
    )
}

fn cut_path_name(bundle: &DistrictBundle) -> String {
    format!(
        "cycle-cut-{:016x}",
        path_hash(
            bundle
                .edges
                .iter()
                .flat_map(|edge| [edge.from.as_str(), edge.to.as_str()]),
        )
    )
}

fn pct(share: f64) -> String {
    format!("{}%", round4(share * 100.0))
}

// The lens equivalent of a report: at most one area per finding kind (in
// `KINDS` order, members sorted), one path per cycle and notes carrying the
// measured values. A cycle path's closing
// edges are computed hops. Consecutive closing edges inside one district
// are joined by a declared hop only because a lens path needs one: no route
// between those two files is guaranteed, and the reason says that no edge
// is claimed, since the viewer draws a declared hop as a link.
pub fn to_lens(report: &DebtReport, map: &MapDocument) -> Lens {
    let t = &report.thresholds;
    let mut kinds = [
        (KIND_CYCLE, LensColor::Red, BTreeSet::new()),
        (KIND_MISPLACED, LensColor::Amber, BTreeSet::new()),
        (KIND_GOD, LensColor::Purple, BTreeSet::new()),
        (KIND_HOTSPOT, LensColor::Orange, BTreeSet::new()),
        (KIND_UNREACHED, LensColor::Grey, BTreeSet::new()),
        (KIND_UNDECLARED_RUST, LensColor::Grey, BTreeSet::new()),
        (KIND_HIDDEN_COUPLING, LensColor::Teal, BTreeSet::new()),
    ];
    fn add<'a>(
        kinds: &mut [(&str, LensColor, BTreeSet<&'a str>)],
        kind: &str,
        files: impl IntoIterator<Item = &'a str>,
    ) {
        let entry = kinds.iter_mut().find(|k| k.0 == kind).unwrap();
        entry.2.extend(files);
    }
    let mut paths = vec![];
    let mut notes = vec![];
    let mut note = |anchor: &str, text: String| {
        notes.push(LensNote {
            anchor: anchor.to_owned(),
            text,
        })
    };

    for cycle in &report.cycles {
        let files = cycle_files(cycle);
        let names: Vec<String> = cycle
            .route
            .iter()
            .chain(cycle.route.first())
            .map(|&d| district_name(map, d))
            .collect();
        let first = &cycle.closing_edges[0].from;
        note(
            first,
            format!(
                "district cycle: {}; {} cross-district edges among {} districts (lower bound)",
                names.join(" → "),
                cycle.cross_edges,
                cycle.districts.len()
            ),
        );
        let declared = |district: usize| LensStop {
            at: String::new(),
            hop: HopMode::Declared,
            reason: Some(format!(
                "no edge claimed: both files are in district {}, where the cycle continues",
                district_name(map, district)
            )),
        };
        let mut stops = vec![LensStop {
            at: first.clone(),
            hop: HopMode::Computed,
            reason: None,
        }];
        for edge in &cycle.closing_edges {
            if stops.last().unwrap().at != edge.from {
                stops.push(LensStop {
                    at: edge.from.clone(),
                    ..declared(edge.from_district)
                });
            }
            stops.push(LensStop {
                at: edge.to.clone(),
                hop: HopMode::Computed,
                reason: None,
            });
        }
        if stops.last().unwrap().at != *first {
            stops.push(LensStop {
                at: first.clone(),
                ..declared(cycle.closing_edges[0].from_district)
            });
        }
        paths.push(LensPath {
            name: cycle_path_name(&files),
            stops,
        });
        let shown = cycle.cut.len().min(CYCLE_LENS_PATHS);
        note(
            first,
            format!(
                "{shown} of {} heuristic cut bundles shown as paths; removing all makes this district graph acyclic",
                cycle.cut.len()
            ),
        );
        for bundle in cycle.cut.iter().take(CYCLE_LENS_PATHS) {
            let edges = &bundle.edges;
            let mut stops = vec![LensStop {
                at: edges[0].from.clone(),
                hop: HopMode::Computed,
                reason: None,
            }];
            for edge in edges {
                if stops.last().unwrap().at != edge.from {
                    stops.push(LensStop {
                        at: edge.from.clone(),
                        hop: HopMode::Declared,
                        reason: Some("no edge claimed between cut-bundle edges".to_owned()),
                    });
                }
                stops.push(LensStop {
                    at: edge.to.clone(),
                    hop: HopMode::Computed,
                    reason: None,
                });
            }
            paths.push(LensPath {
                name: cut_path_name(bundle),
                stops,
            });
        }
        add(&mut kinds, KIND_CYCLE, files);
    }
    for m in &report.misplaced {
        let own = m
            .by_district
            .iter()
            .find(|r| r.district == m.district)
            .map_or(0, |r| r.edges);
        let to = m
            .by_district
            .iter()
            .find(|r| r.district == m.to_district)
            .unwrap();
        note(
            &m.path,
            format!(
                "misplaced file: {} of {} edges ({}) go to district {}, {own} stay in {} (threshold: at least {} edges, more than {} to one other district; {} test files excluded)",
                to.edges,
                m.edges,
                pct(m.share),
                to.name,
                district_name(map, m.district),
                t.misplaced_min_edges,
                pct(t.misplaced_min_share),
                report.misplaced_test_files_excluded
            ),
        );
        add(&mut kinds, KIND_MISPLACED, [m.path.as_str()]);
    }
    for g in &report.god_files {
        note(
            &g.path,
            format!(
                "god file: fan-in {} (rank {}), complexity {} (rank {}); both in the top {} (fan-in ≥ {}, complexity ≥ {})",
                g.fanin,
                g.fanin_rank,
                g.complexity,
                g.complexity_rank,
                pct(t.god_top_fraction),
                t.god_min_fanin.unwrap_or_default(),
                t.god_min_complexity.unwrap_or_default()
            ),
        );
        add(&mut kinds, KIND_GOD, [g.path.as_str()]);
    }
    for h in &report.hotspots {
        note(
            &h.path,
            format!(
                "hotspot #{}: churn {} × complexity {} = {} (top {}, at least {}; score ≥ {}; {} test files and {} non-production files excluded)",
                h.rank,
                h.churn,
                h.complexity,
                h.score,
                pct(t.hotspot_top_fraction),
                t.hotspot_min_count,
                t.hotspot_min_score.unwrap_or_default(),
                report.test_hotspot_files_excluded,
                report.non_production_hotspot_files_excluded
            ),
        );
        add(&mut kinds, KIND_HOTSPOT, [h.path.as_str()]);
    }
    for u in &report.unreached {
        note(
            &u.path,
            format!(
                "no inbound edge found ({} outbound); dynamic imports, reflection and declared connections are invisible to the map",
                u.out_edges
            ),
        );
        add(&mut kinds, KIND_UNREACHED, [u.path.as_str()]);
    }
    for file in &report.undeclared_rust {
        note(
            &file.path,
            "undeclared Rust file: no `mod` declares it; likely not compiled (macro-generated declarations may be missed)".to_owned(),
        );
        add(&mut kinds, KIND_UNDECLARED_RUST, [file.path.as_str()]);
    }
    if !report.hidden_coupling.is_empty() {
        add(
            &mut kinds,
            KIND_HIDDEN_COUPLING,
            report
                .hidden_coupling
                .iter()
                .flat_map(|coupling| [coupling.left.as_str(), coupling.right.as_str()]),
        );
        let shown = report.hidden_coupling.len().min(HIDDEN_COUPLING_LENS_PATHS);
        note(
            &report.hidden_coupling[0].left,
            format!(
                "{shown} of {} hidden-coupling pairs shown as paths; {} test pairs excluded; all remaining pairs are in `tolmap debt`",
                report.hidden_coupling.len(), report.hidden_coupling_test_pairs_excluded
            ),
        );
        for coupling in report
            .hidden_coupling
            .iter()
            .take(HIDDEN_COUPLING_LENS_PATHS)
        {
            note(
                &coupling.left,
                format!(
                    "hidden coupling: co-changed {} times (weight {:.2}); no static or reference edge found (lower bound)",
                    coupling.cochanges, coupling.weight
                ),
            );
            paths.push(LensPath {
                name: hidden_coupling_path_name(coupling),
                stops: vec![
                    LensStop {
                        at: coupling.left.clone(),
                        hop: HopMode::Computed,
                        reason: None,
                    },
                    LensStop {
                        at: coupling.right.clone(),
                        hop: HopMode::Declared,
                        reason: Some(format!(
                            "co-changed {} times; no static edge found",
                            coupling.cochanges
                        )),
                    },
                ],
            });
        }
    }
    let areas = kinds
        .into_iter()
        .filter(|(_, _, files)| !files.is_empty())
        .map(|(kind, color, files)| LensArea {
            label: kind.to_owned(),
            color,
            members: files.into_iter().map(str::to_owned).collect(),
        })
        .collect();
    Lens {
        version: LENS_VERSION,
        title: Some("Debt findings (lower bounds)".into()),
        areas,
        paths,
        notes,
    }
}

// ---- output ----------------------------------------------------------------

pub fn render_json(report: &DebtReport) -> String {
    let mut text = serde_json::to_string_pretty(report).expect("a DebtReport always serialises");
    text.push('\n');
    text
}

pub fn render_text(report: &DebtReport) -> String {
    use std::fmt::Write;
    let t = &report.thresholds;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "debt findings for {} ({} files, {} edges); counts are lower bounds",
        report.repo, report.files, report.edges
    );
    match &report.coverage {
        Some(c) => {
            let _ = writeln!(
                out,
                "coverage: {} of {} files have no kept edge",
                c.zero_edge_files, c.total_files
            );
        }
        None => out.push_str("coverage: not recorded in this map\n"),
    }
    let _ = writeln!(
        out,
        "{} district cycles; {} misplaced files (>= {} edges, > {} to one other district; {} test files and {} generated files excluded); {} god files (top {} by fan-in and complexity); {} production hotspots (top {} by churn x complexity, at least {}; {} test files and {} non-production files excluded); {} files with no inbound edge found ({} entry files, {} generated files and {} files with no edge excluded)",
        report.cycles.len(),
        report.misplaced.len(),
        t.misplaced_min_edges,
        pct(t.misplaced_min_share),
        report.misplaced_test_files_excluded,
        report.misplaced_generated_files_excluded,
        report.god_files.len(),
        pct(t.god_top_fraction),
        report.hotspots.len(),
        pct(t.hotspot_top_fraction),
        t.hotspot_min_count,
        report.test_hotspot_files_excluded,
        report.non_production_hotspot_files_excluded,
        report.unreached.len(),
        report.entry_files,
        report.unreached_generated_files_excluded,
        report.isolated_files,
    );
    if !report.generated_file_measurement {
        out.push_str("generated-file handling not measured (map has no generated-file sidecar)\n");
    }
    if report.rust_mod_reachability_measured {
        let _ = writeln!(
            out,
            "{} undeclared Rust files (no `mod` declares them; likely not compiled)",
            report.undeclared_rust.len(),
        );
    } else {
        out.push_str("Rust mod reachability not measured (map has no Rust mod sidecar)\n");
    }
    if report.build_entry_measurement {
        let _ = writeln!(
            out,
            "{} entry files found by statically readable build config",
            report.build_entry_files,
        );
    } else {
        out.push_str("build-config entries not measured (map has no build-entry sidecar)\n");
    }
    if report.hidden_coupling_measured {
        let _ = writeln!(
            out,
            "{} hidden-coupling pairs (at least {} co-changes, weight >= {}, no static or reference edge found; {} test pairs excluded, {} generated-binding pairs suppressed)",
            report.hidden_coupling.len(),
            t.hidden_coupling_min_count,
            t.hidden_coupling_min_weight,
            report.hidden_coupling_test_pairs_excluded,
            report.hidden_coupling_generated_pairs_suppressed,
        );
    } else {
        out.push_str("hidden coupling not measured (map has no co-change sidecar)\n");
    }
    for c in &report.cycles {
        let ids: Vec<String> = c.route.iter().map(usize::to_string).collect();
        let _ = writeln!(
            out,
            "cycle districts {} ({} cross-district edges)",
            ids.join(" -> "),
            c.cross_edges
        );
        for e in &c.closing_edges {
            let _ = writeln!(out, "  {} -> {}", e.from, e.to);
        }
        for pair in &c.mutual_pairs {
            let _ = writeln!(
                out,
                "  mutual lighter bundle {} -> {} ({} file edges)",
                pair.from_district,
                pair.to_district,
                pair.edges.len(),
            );
        }
        let _ = writeln!(out, "  heuristic cut ({} bundles):", c.cut.len());
        for bundle in &c.cut {
            let _ = writeln!(
                out,
                "    {} -> {} ({} file edges)",
                bundle.from_district,
                bundle.to_district,
                bundle.edges.len(),
            );
            for edge in &bundle.edges {
                let _ = writeln!(out, "      {} -> {}", edge.from, edge.to);
            }
        }
    }
    for m in &report.misplaced {
        let _ = writeln!(
            out,
            "misplaced {}: {} of {} edges to district {}",
            m.path,
            pct(m.share),
            m.edges,
            m.to_district
        );
    }
    for g in &report.god_files {
        let _ = writeln!(
            out,
            "god {}: fan-in {}, complexity {}",
            g.path, g.fanin, g.complexity
        );
    }
    for h in &report.hotspots {
        let _ = writeln!(
            out,
            "hotspot #{} {}: churn {} x complexity {} = {}",
            h.rank, h.path, h.churn, h.complexity, h.score
        );
    }
    for u in &report.unreached {
        let _ = writeln!(
            out,
            "no inbound edge found {} ({} outbound)",
            u.path, u.out_edges
        );
    }
    for coupling in &report.hidden_coupling {
        let _ = writeln!(
            out,
            "hidden coupling {} <-> {}: {} co-changes, weight {:.2}",
            coupling.left, coupling.right, coupling.cochanges, coupling.weight
        );
    }
    out
}

// `tolmap debt`. Findings are informational: 0 on success, 2 on an
// unreadable map.
pub fn run(map: &std::path::Path, format: &str) -> i32 {
    let bytes = match std::fs::read(map) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("tolmap debt: {}: {e}", map.display());
            return 2;
        }
    };
    let doc: MapDocument = match serde_json::from_slice(&bytes) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("tolmap debt: {}: not a map document: {e}", map.display());
            return 2;
        }
    };
    let report = findings(&doc);
    let rendered = match format {
        "json" => render_json(&report),
        "lens" => to_lens(&report, &doc).to_toml(),
        _ => render_text(&report),
    };
    print!("{rendered}");
    0
}

// ---- built-in lens and comparison (issue #250 items 3-4) -------------------

// The built-in debt lens's name in `LensResponse`/`LensComparison` and the
// viewer's `?lens=`. Repository lens names start with [a-z0-9], so this can
// never collide with `.tolmap/lenses/<name>.toml`.
pub const LENS_NAME: &str = "@debt";

const KINDS: [&str; 7] = [
    KIND_CYCLE,
    KIND_MISPLACED,
    KIND_GOD,
    KIND_HOTSPOT,
    KIND_UNREACHED,
    KIND_UNDECLARED_RUST,
    KIND_HIDDEN_COUPLING,
];

// The debt lens resolved against the map it was generated from.
pub fn lens_report(map: &MapDocument) -> LensReport {
    lens::resolve(&to_lens(&findings(map), map), map)
}

// Debt at base and at head, compared on the base map. Head findings come
// from the head snapshot's own map (its own partition); the head lens is
// resolved on `head_frame`, the head projected onto the base partition
// (`check::lens_head_map`), so only base geometry is ever drawn.
//
// `lens_comparison::changes` keys an area item by its member file alone, so
// areas match across labels. One file is often in several kinds at once (a
// god file is usually a hotspot too), and one comparison over the whole lens
// would then read a new god file as `shared`. Comparing each kind on its own
// keeps every file to at most one area item per comparison, since each kind
// has one area. The statuses are `changes`' own; none is added.
pub fn compare(
    base: &MapDocument,
    head: &MapDocument,
    head_frame: &MapDocument,
    renames: &[FileRename],
) -> (LensReport, LensReport, Vec<LensDelta>) {
    let before = lens_report(base);
    let mut after = lens::resolve(&to_lens(&findings(head), head), head_frame);
    lens::report_renames(&mut after, renames);
    let only = |report: &LensReport, kind: &str| LensReport {
        areas: report
            .areas
            .iter()
            .filter(|a| a.label == kind)
            .cloned()
            .collect(),
        paths: if kind == KIND_CYCLE || kind == KIND_HIDDEN_COUPLING {
            report
                .paths
                .iter()
                .filter(|path| {
                    (kind == KIND_CYCLE
                        && (path.name.starts_with("cycle-") || path.name.starts_with("cycle-cut-")))
                        || (kind == KIND_HIDDEN_COUPLING
                            && path.name.starts_with("hidden-coupling-"))
                })
                .cloned()
                .collect()
        } else {
            vec![]
        },
        ..report.clone()
    };
    let changes = KINDS
        .iter()
        .flat_map(|kind| {
            lens_comparison::changes(
                &only(&before, kind),
                &only(&after, kind),
                base,
                renames,
                ComparisonMode::Commits,
            )
        })
        .collect();
    (before, after, changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{COCHANGE_HISTORY_WINDOW, COCHANGE_MIN_COUNT};
    use crate::pipeline::SHARE_COCHANGE;
    use crate::schema::MapDocument;
    use serde_json::json;

    fn map(debt: Option<serde_json::Value>) -> MapDocument {
        let mut value = json!({
            "repo": "fixture",
            "q": 0.0,
            "names": {"0": "left", "1": "right"},
            "districts": {},
            "F": ["a.rs", "b.rs", "c.rs"],
            "N": [[0, 0.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0], [1, 1.0, 0.0, 1, 0, 0, 0.0, 1.0, 0.0, 1.0, 1.0], [1, 2.0, 0.0, 1, 0, 0, 0.0, 2.0, 0.0, 1.0, 1.0]],
            "E": [[0, 2]],
            "L": [],
            "S": {},
            "U": {"2:0": [1]},
            "roads": [],
            "lang": "rs"
        });
        if let Some(debt) = debt {
            value["D"] = debt;
        }
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn build_config_entries_are_entries_and_old_maps_are_unmeasured() {
        let configured = findings(&map(Some(json!({"build_entries": {"files": [0]}}))));
        assert!(configured.build_entry_measurement);
        assert_eq!(configured.build_entry_files, 1);
        assert!(
            configured.unreached.is_empty(),
            "configured a.rs is an entry"
        );
        assert!(render_text(&configured)
            .contains("1 entry files found by statically readable build config"));

        let old = findings(&map(None));
        assert!(!old.build_entry_measurement);
        assert!(render_text(&old).contains("build-config entries not measured"));
    }

    #[test]
    fn generated_files_are_excluded_and_old_maps_are_unmeasured() {
        let generated: MapDocument = serde_json::from_value(json!({
            "repo": "generated", "q": 0.0, "names": {"0": "left", "1": "right"}, "districts": {},
            "F": ["bindings/Thing.ts", "src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs", "src/e.rs"],
            "N": [
                [0, 0.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [1, 1.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [1, 2.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [1, 3.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [1, 4.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [1, 5.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0]
            ],
            "E": [[0, 1], [0, 2], [0, 3], [0, 4], [0, 5]], "L": [], "S": {}, "U": {}, "roads": [], "lang": "ts",
            "D": {"generated": [{"file": 0, "generator": null}]}
        })).unwrap();
        let report = findings(&generated);
        assert!(report.generated_file_measurement);
        assert_eq!(report.misplaced_generated_files_excluded, 1);
        assert_eq!(report.unreached_generated_files_excluded, 1);
        assert!(
            report.misplaced.is_empty(),
            "the generated binding would otherwise be misplaced"
        );
        assert!(
            report.unreached.is_empty(),
            "the generated binding would otherwise lack inbound edges"
        );

        let old = findings(&map(None));
        assert!(!old.generated_file_measurement);
        assert!(render_text(&old).contains("generated-file handling not measured"));
    }

    #[test]
    fn generated_binding_hidden_coupling_is_suppressed_only_with_static_generator_link() {
        let document: MapDocument = serde_json::from_value(json!({
            "repo": "generated", "q": 0.0, "names": {"0": "only"}, "districts": {},
            "F": ["bindings/Thing.ts", "src/model.rs", "src/linked.rs", "src/unlinked.rs"],
            "N": [
                [0, 0.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 1.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 2.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 3.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0]
            ],
            "E": [[1, 2]], "L": [], "S": {}, "U": {}, "roads": [], "lang": "ts",
            "D": {
                "generated": [{"file": 0, "generator": 1}],
                "cochange": {"commits": 12, "window": COCHANGE_HISTORY_WINDOW, "min_count": COCHANGE_MIN_COUNT, "min_weight": SHARE_COCHANGE,
                    "pairs": [[0, 1, 4, 0.5], [0, 2, 4, 0.5], [0, 3, 4, 0.5]]}
            }
        })).unwrap();
        let report = findings(&document);
        assert_eq!(report.hidden_coupling_generated_pairs_suppressed, 2);
        assert_eq!(report.hidden_coupling.len(), 1);
        assert_eq!(report.hidden_coupling[0].right, "src/unlinked.rs");
    }

    #[test]
    fn test_files_are_excluded_from_misplaced_findings() {
        let mut value = serde_json::to_value(map(None)).unwrap();
        value["F"] = json!([
            "tests/case.rs",
            "other/a.rs",
            "other/b.rs",
            "other/c.rs",
            "other/d.rs",
            "other/e.rs"
        ]);
        value["N"] = json!([
            [0, 0.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
            [1, 1.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
            [1, 2.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
            [1, 3.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
            [1, 4.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
            [1, 5.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0]
        ]);
        value["E"] = json!([[0, 1], [0, 2], [0, 3], [0, 4], [0, 5]]);
        let report = findings(&serde_json::from_value(value).unwrap());
        assert_eq!(report.misplaced_test_files_excluded, 1);
        assert!(report.misplaced.is_empty());
        assert!(is_test_path("src/assertion_evidence_tests.rs"));
        assert!(render_text(&report).contains("1 test files excluded"));
    }

    #[test]
    fn test_files_do_not_rank_as_production_hotspots() {
        let map: MapDocument = serde_json::from_value(json!({
            "repo": "hotspots", "q": 0.0, "names": {"0": "only"}, "districts": {},
            "F": ["src/model/tests.rs", "src/model.rs", "src/query.rs"],
            "N": [
                [0, 0.0, 0.0, 1, 100, 100, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 1.0, 0.0, 1, 20, 20, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 2.0, 0.0, 1, 10, 10, 0.0, 0.0, 0.0, 1.0, 1.0]
            ],
            "E": [], "L": [], "S": {}, "U": {}, "roads": [], "lang": "rs"
        }))
        .unwrap();
        let report = findings(&map);
        assert_eq!(report.test_hotspot_files_excluded, 1);
        assert_eq!(
            report
                .hotspots
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["src/model.rs", "src/query.rs"]
        );
    }

    #[test]
    fn storybook_is_non_production_without_becoming_a_test_or_entry() {
        let map: MapDocument = serde_json::from_value(json!({
            "repo": "storybook", "q": 0.0, "names": {"0": "only"}, "districts": {},
            "F": ["web/storybook/fixtures.ts", "web/button.stories.tsx", "src/model.rs", "src/query.rs"],
            "N": [
                [0, 0.0, 0.0, 1, 100, 100, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 1.0, 0.0, 1, 90, 90, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 2.0, 0.0, 1, 20, 20, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 3.0, 0.0, 1, 10, 10, 0.0, 0.0, 0.0, 1.0, 1.0]
            ],
            "E": [[0, 2], [1, 2]], "L": [], "S": {}, "U": {}, "roads": [], "lang": "ts"
        }))
        .unwrap();
        let report = findings(&map);
        assert_eq!(report.non_production_hotspot_files_excluded, 2);
        assert_eq!(report.test_hotspot_files_excluded, 0);
        assert_eq!(report.entry_files, 0);
        assert_eq!(
            report
                .hotspots
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["src/model.rs", "src/query.rs"]
        );
        assert!(report
            .unreached
            .iter()
            .any(|file| file.path == "web/storybook/fixtures.ts"));
        assert!(!is_test_path("web/storybook/fixtures.ts"));
        assert!(!is_test_path("web/button.stories.tsx"));
        assert!(!is_entry_path("web/storybook/fixtures.ts"));
    }

    #[test]
    fn old_maps_say_hidden_coupling_was_not_measured() {
        let report = findings(&map(None));
        assert!(!report.hidden_coupling_measured);
        assert!(report.hidden_coupling.is_empty());
        assert!(render_text(&report).contains("hidden coupling not measured"));
    }

    #[test]
    fn mod_sidecar_excludes_declared_modules_and_reports_orphans() {
        let map: MapDocument = serde_json::from_value(json!({
            "repo": "rust", "q": 0.0, "names": {"0": "only"}, "districts": {},
            "F": ["src/lib.rs", "src/child.rs", "src/orphan.rs", "src/path.rs", "src/bin/tool.rs", "tests/integration.rs", "build.rs"],
            "N": [
                [0, 0.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 1.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 2.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 3.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 4.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 5.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 6.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0]
            ],
            "E": [[1, 0]], "L": [], "S": {}, "U": {}, "roads": [], "lang": "rs",
            "D": {"rust_mod": {"roots": [0, 6], "pairs": [[0, 1], [0, 3]]}}
        })).unwrap();
        let report = findings(&map);
        assert!(report.rust_mod_reachability_measured);
        assert!(
            report.unreached.is_empty(),
            "a mod child is compiled even with no inbound E edge"
        );
        assert_eq!(
            report
                .undeclared_rust
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["src/orphan.rs"]
        );
        let lens = to_lens(&report, &map);
        assert!(lens
            .areas
            .iter()
            .any(|area| area.label == KIND_UNDECLARED_RUST));
    }

    #[test]
    fn old_maps_say_rust_mod_reachability_was_not_measured() {
        let report = findings(&map(None));
        assert!(!report.rust_mod_reachability_measured);
        assert!(render_text(&report).contains("Rust mod reachability not measured"));
    }

    #[test]
    fn cycle_report_has_light_cut_and_mutual_lighter_bundle() {
        let map: MapDocument = serde_json::from_value(json!({
            "repo": "cycle", "q": 0.0, "names": {"0": "a", "1": "b", "2": "c"}, "districts": {},
            "F": ["a.rs", "b.rs", "c.rs", "c2.rs"],
            "N": [
                [0, 0.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [1, 1.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [2, 2.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [2, 3.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0]
            ],
            "E": [[0, 1], [1, 2], [2, 0], [1, 0]],
            "L": [], "S": {}, "U": {}, "roads": [], "lang": "rs"
        }))
        .unwrap();
        let first = findings(&map);
        let second = findings(&map);
        assert_eq!(render_json(&first), render_json(&second));
        let cycle = &first.cycles[0];
        assert_eq!(cycle.cut.len(), 1);
        // 0 -> 1 lies on both cycles (0 -> 1 -> 0 and 0 -> 1 -> 2 -> 0), so
        // removing that one bundle makes the district graph acyclic; 2 -> 0
        // alone would leave 0 <-> 1.
        assert_eq!(
            (cycle.cut[0].from_district, cycle.cut[0].to_district),
            (0, 1)
        );
        assert_eq!(cycle.cut[0].edges.len(), 1);
        // 0 -> 1 and 1 -> 0 carry one file edge each, so the tie breaks on
        // the smaller `from` district (src/cycle_cut.rs).
        assert_eq!(
            (
                cycle.mutual_pairs[0].from_district,
                cycle.mutual_pairs[0].to_district
            ),
            (0, 1)
        );
        assert!(to_lens(&first, &map)
            .paths
            .iter()
            .any(|path| path.name.starts_with("cycle-cut-")));
    }

    #[test]
    fn rust_mod_pairs_count_once_for_misplaced_files() {
        fn misplaced_map(rust_mod: Option<serde_json::Value>, parent_edge: bool) -> MapDocument {
            let mut edges = vec![[1, 2], [1, 4], [1, 5], [1, 6]];
            if parent_edge {
                edges.push([0, 1]);
            } else {
                edges.push([1, 3]);
            }
            let mut value = json!({
                "repo": "misplaced", "q": 0.0, "names": {"0": "own", "1": "other"}, "districts": {},
                "F": ["src/parent.rs", "src/child.rs", "src/own.rs", "src/own_two.rs", "other/a.rs", "other/b.rs", "other/c.rs"],
                "N": [
                    [0, 0.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                    [0, 1.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                    [0, 2.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                    [0, 3.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                    [1, 4.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                    [1, 5.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                    [1, 6.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0]
                ],
                "E": edges, "L": [], "S": {}, "U": {}, "roads": [], "lang": "rs"
            });
            if let Some(rust_mod) = rust_mod {
                value["D"] = json!({"rust_mod": rust_mod});
            }
            serde_json::from_value(value).unwrap()
        }

        let without_pair = findings(&misplaced_map(None, false));
        assert_eq!(without_pair.misplaced[0].path, "src/child.rs");
        assert_eq!(without_pair.misplaced[0].edges, 5);

        let with_pair = findings(&misplaced_map(
            Some(json!({"roots": [0], "pairs": [[0, 1]]})),
            false,
        ));
        assert!(
            with_pair.misplaced.is_empty(),
            "the parent link makes the foreign district exactly half of incident evidence"
        );

        let with_edge_and_pair = findings(&misplaced_map(
            Some(json!({"roots": [0], "pairs": [[0, 1]]})),
            true,
        ));
        assert_eq!(with_edge_and_pair.misplaced[0].path, "src/child.rs");
        assert_eq!(
            with_edge_and_pair.misplaced[0].edges, 5,
            "an existing E edge prevents a duplicate mod count"
        );

        let old_map = findings(&misplaced_map(None, false));
        assert_eq!(
            old_map.misplaced, without_pair.misplaced,
            "maps without rust_mod retain E-only misplaced evidence"
        );
    }

    #[test]
    fn hidden_coupling_excludes_tests_and_sorts_actionably() {
        let map: MapDocument = serde_json::from_value(json!({
            "repo": "coupling", "q": 0.0, "names": {"0": "left", "1": "right"}, "districts": {},
            "F": [
                "tests/case.rs", "src/lib.rs", "contracts/alpha.rs", "ui/alpha.ts",
                "contracts/beta.rs", "ui/beta.ts", "src/local.rs", "src/other.rs",
                "services/remote.rs", "web/remote.rs", "services/same.rs", "services/same.ts",
                "lib/tail.rs", "lib/tail_two.rs", "services/weight.rs", "web/weight.rs",
                "services/changes.rs", "web/changes.rs", "services/alpha.rs", "web/alpha.rs"
            ],
            "N": [
                [0, 0.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 1.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 2.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [1, 3.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 4.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 5.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 6.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [1, 7.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 8.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [1, 9.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 10.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 11.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 12.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 13.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 14.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [1, 15.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 16.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [1, 17.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [0, 18.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0],
                [1, 19.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 1.0, 1.0]
            ],
            "E": [], "L": [], "S": {}, "U": {}, "roads": [], "lang": "mixed",
            "D": {"cochange": {
                "commits": 12, "window": COCHANGE_HISTORY_WINDOW,
                "min_count": COCHANGE_MIN_COUNT, "min_weight": SHARE_COCHANGE,
                "pairs": [
                    [0, 1, 4, 0.5], [2, 3, 3, 0.35], [4, 5, 9, 0.99],
                    [6, 7, 9, 0.99], [8, 9, 5, 0.35], [10, 11, 9, 0.99],
                    [12, 13, 9, 0.99], [14, 15, 3, 0.9], [16, 17, 5, 0.9],
                    [18, 19, 5, 0.9]
                ]
            }}
        }))
        .unwrap();
        let report = findings(&map);
        assert_eq!(report.hidden_coupling_test_pairs_excluded, 1);
        assert!(render_text(&report).contains("1 test pairs excluded"));
        assert_eq!(
            report
                .hidden_coupling
                .iter()
                .map(|pair| (pair.left.as_str(), pair.right.as_str()))
                .collect::<Vec<_>>(),
            [
                ("contracts/alpha.rs", "ui/alpha.ts"),
                ("contracts/beta.rs", "ui/beta.ts"),
                ("services/same.rs", "services/same.ts"),
                ("services/alpha.rs", "web/alpha.rs"),
                ("services/changes.rs", "web/changes.rs"),
                ("services/weight.rs", "web/weight.rs"),
                ("services/remote.rs", "web/remote.rs"),
                ("src/local.rs", "src/other.rs"),
                ("lib/tail.rs", "lib/tail_two.rs"),
            ]
        );
    }

    #[test]
    fn hidden_coupling_lens_caps_paths_but_keeps_all_area_members() {
        let mut report = findings(&map(Some(json!({
            "cochange": {
                "commits": 12,
                "window": COCHANGE_HISTORY_WINDOW,
                "min_count": COCHANGE_MIN_COUNT,
                "min_weight": SHARE_COCHANGE,
                "pairs": [[0, 1, 4, 0.5]]
            }
        }))));
        report.hidden_coupling = (0..=HIDDEN_COUPLING_LENS_PATHS)
            .map(|index| HiddenCoupling {
                left: format!("left-{index}.rs"),
                right: format!("right-{index}.rs"),
                left_district: 0,
                right_district: 1,
                cochanges: index + COCHANGE_MIN_COUNT,
                weight: 0.5,
            })
            .collect();
        let lens = to_lens(&report, &map(None));
        assert_eq!(
            lens.paths
                .iter()
                .filter(|path| path.name.starts_with("hidden-coupling-"))
                .count(),
            HIDDEN_COUPLING_LENS_PATHS
        );
        assert!(lens.notes.iter().any(|note| {
            note.text.contains(&format!(
                "{HIDDEN_COUPLING_LENS_PATHS} of {} hidden-coupling pairs shown as paths",
                HIDDEN_COUPLING_LENS_PATHS + 1
            ))
        }));
        assert!(lens.areas.iter().any(|area| {
            area.label == KIND_HIDDEN_COUPLING
                && area.members.len() == 2 * (HIDDEN_COUPLING_LENS_PATHS + 1)
        }));
    }

    #[test]
    fn hidden_coupling_excludes_declared_rust_module_pairs() {
        let report = findings(&map(Some(json!({
            "cochange": {
                "commits": 12,
                "window": COCHANGE_HISTORY_WINDOW,
                "min_count": COCHANGE_MIN_COUNT,
                "min_weight": SHARE_COCHANGE,
                "pairs": [[0, 1, 4, 0.5]]
            },
            "rust_mod": {"roots": [0], "pairs": [[1, 0]]}
        }))));
        assert!(report.hidden_coupling_measured);
        assert!(
            report.hidden_coupling.is_empty(),
            "a declared mod pair is not hidden"
        );
    }

    #[test]
    fn hidden_coupling_uses_sidecar_and_emits_declared_path() {
        let report = findings(&map(Some(json!({
            "cochange": {
                "commits": 12,
                "window": COCHANGE_HISTORY_WINDOW,
                "min_count": COCHANGE_MIN_COUNT,
                "min_weight": SHARE_COCHANGE,
                "pairs": [[0, 1, 4, 0.5], [1, 2, 3, 0.35]]
            }
        }))));
        assert!(report.hidden_coupling_measured);
        assert_eq!(report.hidden_coupling.len(), 1);
        assert_eq!(report.hidden_coupling[0].left, "a.rs");
        assert_eq!(report.hidden_coupling[0].right, "b.rs");
        let lens = to_lens(
            &report,
            &map(Some(json!({
                "cochange": {
                    "commits": 12,
                    "window": COCHANGE_HISTORY_WINDOW,
                    "min_count": COCHANGE_MIN_COUNT,
                    "min_weight": SHARE_COCHANGE,
                    "pairs": [[0, 1, 4, 0.5], [1, 2, 3, 0.35]]
                }
            }))),
        );
        assert!(lens
            .areas
            .iter()
            .any(|area| area.label == KIND_HIDDEN_COUPLING));
        assert!(lens.paths.iter().all(|path| {
            path.name.starts_with("hidden-coupling-")
                && path.stops[1].hop == HopMode::Declared
                && path.stops[1]
                    .reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("co-changed"))
        }));
    }
}
