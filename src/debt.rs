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

pub const DEBT_REPORT_VERSION: u32 = 1;

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

// Unreached file exclusions. Entry points have no importer by design, so a
// file is an entry when the map marks it an `entry` landmark, or by name:
// - its base name, up to the first `.`, is one of these. `tests` is a Rust
//   `#[cfg(test)] mod tests;` child: nothing `use`s it, and its parent's `mod`
//   gives no inbound edge on the map, so on semon 04aeb54 9 of the 26 files
//   with no inbound edge found were `tests.rs` (docs/FINDINGS.md, finding 63);
pub const ENTRY_STEMS: &[&str] = &[
    "main", "__main__", "__init__", "index", "lib", "mod", "build", "app", "cli", "server",
    "setup", "conftest", "manage", "wsgi", "asgi", "tests",
];
// - a directory on its path is one of these (tests, tools and examples are
//   run, not imported);
pub const ENTRY_DIRS: &[&str] = &[
    "test",
    "tests",
    "__tests__",
    "spec",
    "e2e",
    "bin",
    "scripts",
    "examples",
    "benches",
];
// - its base name contains one of these (test, story, tool config and
//   ambient declaration files);
pub const ENTRY_MARKERS: &[&str] = &[".test.", ".spec.", ".stories.", ".config.", ".d."];
// - or its stem starts `test_` or ends `_test` / `_spec` (Python, Go, Ruby).
pub const ENTRY_STEM_PREFIXES: &[&str] = &["test_"];
pub const ENTRY_STEM_SUFFIXES: &[&str] = &["_test", "_spec"];

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
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct DistrictEdges {
    pub district: usize,
    pub name: String,
    pub edges: usize,
}

// `by_district` counts the file's incident edges by the other end's
// district (its own included), most first. `share` is `to_district`'s
// share of `edges`, rounded to 4 places.
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
    pub unreached: Vec<UnreachedFile>,
    pub hidden_coupling_measured: bool,
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
    // For each ordered district pair, its file edges' representative: the
    // smallest by path, so the choice survives files being added elsewhere.
    let mut pairs: BTreeMap<(usize, usize), (&str, &str)> = BTreeMap::new();
    let mut graph: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for &[a, b] in &map.edges {
        let (da, db) = (district(a), district(b));
        if da == db {
            continue;
        }
        graph.entry(da).or_default().insert(db);
        let edge = (map.files[a].as_str(), map.files[b].as_str());
        pairs
            .entry((da, db))
            .and_modify(|e| *e = (*e).min(edge))
            .or_insert(edge);
    }
    components(&graph)
        .into_iter()
        .map(|scc| {
            let set: BTreeSet<usize> = scc.iter().copied().collect();
            let inside = |(da, db): &(usize, usize)| set.contains(da) && set.contains(db);
            // Start at the district of the smallest closing edge by path, and
            // visit successors by their representative edge's paths: the cycle
            // then depends on file paths, not on district ids.
            let start = pairs
                .iter()
                .filter(|(k, _)| inside(*k))
                .min_by_key(|(_, e)| **e)
                .unwrap()
                .0
                 .0;
            let successors = |u: usize| {
                let mut next: Vec<(usize, (&str, &str))> = graph[&u]
                    .iter()
                    .filter(|v| set.contains(*v))
                    .map(|&v| (v, pairs[&(u, v)]))
                    .collect();
                next.sort_by_key(|(_, e)| *e);
                next
            };
            let mut prev = BTreeMap::from([(start, start)]);
            let mut queue = VecDeque::from([start]);
            let mut route = vec![];
            'bfs: while let Some(u) = queue.pop_front() {
                for (v, _) in successors(u) {
                    if v == start {
                        let mut c = u;
                        route.push(u);
                        while c != start {
                            c = prev[&c];
                            route.push(c);
                        }
                        route.reverse();
                        break 'bfs;
                    }
                    if let std::collections::btree_map::Entry::Vacant(e) = prev.entry(v) {
                        e.insert(u);
                        queue.push_back(v);
                    }
                }
            }
            let closing_edges = (0..route.len())
                .map(|i| {
                    let (da, db) = (route[i], route[(i + 1) % route.len()]);
                    let (from, to) = pairs[&(da, db)];
                    DebtEdge {
                        from: from.to_owned(),
                        to: to.to_owned(),
                        from_district: da,
                        to_district: db,
                    }
                })
                .collect();
            let cross_edges = map
                .edges
                .iter()
                .filter(|&&[a, b]| {
                    let (da, db) = (district(a), district(b));
                    da != db && set.contains(&da) && set.contains(&db)
                })
                .count();
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
            }
        })
        .collect()
}

pub fn findings(map: &MapDocument) -> DebtReport {
    let n = map.files.len();
    let district = |f: usize| map.nodes[f].district();
    let linked = map
        .edges
        .iter()
        .map(|&[left, right]| (left.min(right), left.max(right)))
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

    let mut misplaced: Vec<MisplacedFile> = (0..n)
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
    let hotspot_min_score = top_cut(&score, HOTSPOT_TOP_FRACTION, HOTSPOT_MIN_COUNT);
    let mut hot: Vec<usize> = match hotspot_min_score {
        Some(cut) => (0..n).filter(|&f| score[f] >= cut).collect(),
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
    let entry = |f: usize| landmark_entries.contains(&f) || is_entry_path(&map.files[f]);
    let entry_files = (0..n).filter(|&f| entry(f)).count();
    let isolated_files = (0..n)
        .filter(|&f| inbound[f] == 0 && outbound[f] == 0)
        .count();
    let unreached = (0..n)
        .filter(|&f| inbound[f] == 0 && outbound[f] > 0 && !entry(f))
        .map(|f| UnreachedFile {
            path: map.files[f].clone(),
            district: district(f),
            out_edges: outbound[f],
        })
        .collect();

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
                    if cochanges < COCHANGE_MIN_COUNT
                        || weight < SHARE_COCHANGE
                        || linked.contains(&(left.min(right), left.max(right)))
                    {
                        return None;
                    }
                    Some(HiddenCoupling {
                        left: map.files.get(left)?.clone(),
                        right: map.files.get(right)?.clone(),
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
    // Cross-district pairs rank first, then the strongest evidence, then
    // file order. The sidecar itself is already index/path deterministic.
    hidden_coupling.sort_by(|a, b| {
        (a.left_district == a.right_district)
            .cmp(&(b.left_district == b.right_district))
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
        unreached,
        hidden_coupling_measured,
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
                "misplaced file: {} of {} edges ({}) go to district {}, {own} stay in {} (threshold: at least {} edges, more than {} to one other district)",
                to.edges,
                m.edges,
                pct(m.share),
                to.name,
                district_name(map, m.district),
                t.misplaced_min_edges,
                pct(t.misplaced_min_share)
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
                "hotspot #{}: churn {} × complexity {} = {} (top {}, at least {}; score ≥ {})",
                h.rank,
                h.churn,
                h.complexity,
                h.score,
                pct(t.hotspot_top_fraction),
                t.hotspot_min_count,
                t.hotspot_min_score.unwrap_or_default()
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
                "{shown} of {} hidden-coupling pairs shown as paths; all are in `tolmap debt`",
                report.hidden_coupling.len()
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
        "{} district cycles; {} misplaced files (>= {} edges, > {} to one other district); {} god files (top {} by fan-in and complexity); {} hotspots (top {} by churn x complexity, at least {}); {} files with no inbound edge found ({} entry files and {} files with no edge excluded)",
        report.cycles.len(),
        report.misplaced.len(),
        t.misplaced_min_edges,
        pct(t.misplaced_min_share),
        report.god_files.len(),
        pct(t.god_top_fraction),
        report.hotspots.len(),
        pct(t.hotspot_top_fraction),
        t.hotspot_min_count,
        report.unreached.len(),
        report.entry_files,
        report.isolated_files,
    );
    if report.hidden_coupling_measured {
        let _ = writeln!(
            out,
            "{} hidden-coupling pairs (at least {} co-changes, weight >= {}, no static or reference edge found)",
            report.hidden_coupling.len(),
            t.hidden_coupling_min_count,
            t.hidden_coupling_min_weight,
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

const KINDS: [&str; 6] = [
    KIND_CYCLE,
    KIND_MISPLACED,
    KIND_GOD,
    KIND_HOTSPOT,
    KIND_UNREACHED,
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
                    (kind == KIND_CYCLE && path.name.starts_with("cycle-"))
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
    fn old_maps_say_hidden_coupling_was_not_measured() {
        let report = findings(&map(None));
        assert!(!report.hidden_coupling_measured);
        assert!(report.hidden_coupling.is_empty());
        assert!(render_text(&report).contains("hidden coupling not measured"));
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
