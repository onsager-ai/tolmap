use std::collections::BTreeMap;

use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use ts_rs::TS;

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct District {
    pub size: usize,
    pub c: [f64; 2],
    pub blob: Vec<Vec<[f64; 2]>>,
    // Issue #34 ("islands"): a district's legibility class, over the same
    // partition Leiden already produced -- a rendering decision, not a
    // repartition (see `geometry::classify_districts`). `#[serde(default)]`
    // for the same reason `GraphData::sources` carries it: the nine
    // committed `data/*.json` fixtures were recorded before this field
    // existed, and every one of them predates the concept -- they still
    // deserialise, defaulting to `Mainland`, which is the closest thing to
    // "unclassified" this type has (and is also correct for eight of the
    // nine: only prometheus has any sub-1% district at all, and parity.rs
    // never reads this field, so the default never feeds a comparison).
    #[serde(default)]
    pub class: DistrictClass,
}

/// A district's legibility class -- see `geometry::classify_districts` for
/// how it's computed and `docs/FINDINGS.md`/issue #34 for the measurement
/// that motivated it. Lowercase on the wire (`#[serde(rename_all =
/// "lowercase")]`) to match every other string the map JSON hands the
/// viewer (`lang`, `names`'s values, landmark `why`/`detail`).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum DistrictClass {
    // See `District::class`'s doc comment: `#[default]` is only reached
    // deserialising a pre-#34 fixture, where it is also numerically correct
    // on all but one of the nine.
    #[default]
    Mainland,
    Island,
    Unconnected,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(
    type = "[number, number, number, number, number, number, number, number, number, number, number]"
)]
pub struct NodeRow(
    pub  (
        usize,
        f64,
        f64,
        usize,
        usize,
        usize,
        f64,
        f64,
        f64,
        f64,
        f64,
    ),
);

impl NodeRow {
    pub fn district(&self) -> usize {
        self.0 .0
    }

    pub fn point(&self) -> [f64; 2] {
        [self.0 .1, self.0 .2]
    }

    pub fn loc(&self) -> usize {
        self.0 .3
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(type = "[number, string, string, number]")]
pub struct LandmarkRow(pub (usize, String, String, usize));

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(type = "[string, number, number, number]")]
pub struct SymbolRow(pub (String, usize, usize, usize));

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
#[ts(type = "[number, number, number]")]
pub struct RoadRow(pub (usize, usize, f64));

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CoverageLanguage {
    pub zero_edge_files: usize,
    pub total_files: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CoverageReport {
    pub zero_edge_files: usize,
    pub total_files: usize,
    pub by_language: BTreeMap<String, CoverageLanguage>,
    // Issue #110 P1a: which reference graph each language's static signal
    // and symbol references came from, and why. Written only by
    // `tolmap build --refs scip`; absent means every language used the
    // hand-written resolver, so a `--refs hand` map stays byte-identical to
    // one built before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<BTreeMap<String, ReferenceCoverage>>,
}

// One language's reference path under `--refs scip`. Plain `//` comments
// here, not doc comments: ts-rs copies doc comments into the hand-checked
// bindings.
//
// `path` is "scip" when the language's SCIP index passed the fallback gate
// (`scip_ingest::gate`), "hand" otherwise. `reason` is a stable code, never
// indexer output: "indexed" (admitted), "below_min_recall",
// "indexer_not_found", "indexer_failed", "indexer_spawn_failed",
// "no_index_written", "no_tsconfig", "no_documents" or "ingest_failed".
// Nothing here is a timing, so the map stays byte-identical across runs.
//
// `install` (issue #110 P1c) is present only on the TypeScript row of a
// build that allowed dependency installs (`tolmap build --install sandbox`,
// or a job service with `TOLMAP_SCIP_INSTALL=sandbox`, its default). It is
// absent otherwise, so a map built without installs stays byte-identical
// to one built before the field existed.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct ReferenceCoverage {
    pub path: String,
    pub reason: String,
    // `tool_info` name and version from the index metadata.
    pub indexer: Option<String>,
    pub exit_code: Option<i32>,
    // Mapped files of this language, and how many the index documented.
    pub files: usize,
    pub files_indexed: Option<usize>,
    // Distinct pairs at `granularity` in the hand-written graph, directed
    // file pairs in the SCIP graph, and the share of the former the latter
    // keeps (rounded to 4 places; the gate compares the unrounded value).
    pub hand_pairs: usize,
    pub scip_pairs: Option<usize>,
    pub recall: Option<f64>,
    pub min_recall: f64,
    pub granularity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<InstallCoverage>,
}

// Whether a language's dependencies were installed, in the sandbox, before
// its indexer ran (issue #110 P1c, `indexers::install`). Plain `//`
// comments for the same ts-rs reason as above.
//
// `status` is "installed"; "skipped", when the install policy does not
// install for this repository; or "fell_back", when it wanted to but the
// sandbox could not be set up or the install did not finish, so the indexer
// ran without installs exactly as it would have with installs off. `reason`
// is a stable code, never package-manager output: "installed"; for
// "skipped", "no_package_json", "unsafe_manifest", "node_modules_present",
// "no_lockfile", "unsupported_lockfile" or "not_a_workspace"; for "fell_back",
// "sandbox_unavailable", "install_failed", "install_timeout",
// "install_disk_budget" or "cancelled". `manager` is "pnpm" or "npm" once a
// lockfile has chosen one.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct InstallCoverage {
    pub status: String,
    pub reason: String,
    pub manager: Option<String>,
}

/// Indices in this document are global and stable across district responses.
#[derive(Clone, Debug, Serialize, TS, PartialEq, Eq)]
#[ts(type = "[number, string, number, number, number, number, number, boolean]")]
pub struct HierSymbolRow(pub (usize, String, usize, usize, usize, isize, usize, bool));

impl<'de> Deserialize<'de> for HierSymbolRow {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Current((usize, String, usize, usize, usize, isize, usize, bool)),
            Legacy((usize, String, usize, usize, usize, isize, usize)),
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Current(row) => Self(row),
            Wire::Legacy((a, b, c, d, e, f, g)) => Self((a, b, c, d, e, f, g, false)),
        })
    }
}

/// Four columns: source, target, occurrences, kind index. Legacy triples
/// receive the "unknown" index so an old document remains readable.
fn deserialize_symbol_edges<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<[usize; 4]>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire {
        Current([usize; 4]),
        Legacy([usize; 3]),
    }
    let rows = Vec::<Wire>::deserialize(deserializer)?;
    Ok(rows
        .into_iter()
        .map(|row| match row {
            Wire::Current(row) => row,
            Wire::Legacy([a, b, count]) => [a, b, count, 0],
        })
        .collect())
}

pub fn symbol_edge_kinds() -> Vec<String> {
    [
        "unknown",
        "call",
        "extends",
        "implements",
        "overrides",
        "annotation",
        "decorator",
        "value",
        "possible_implementation",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct SymbolCoverage {
    pub calls_total: usize,
    pub calls_resolved: usize,
    #[serde(default)]
    pub inherited_calls_resolved: usize,
    #[serde(default)]
    pub possible_implementations: usize,
    pub unresolved: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct SymbolsDocument {
    pub files: Vec<usize>,
    pub symbols: Vec<HierSymbolRow>,
    /// [source symbol, target symbol, occurrence count, kind index].
    #[serde(deserialize_with = "deserialize_symbol_edges")]
    pub edges: Vec<[usize; 4]>,
    #[serde(default = "symbol_edge_kinds")]
    pub kinds: Vec<String>,
    /// Per-file code lines outside every top-level symbol.
    pub module_code_lines: BTreeMap<usize, usize>,
    pub coverage: SymbolCoverage,
    /// Index aligned with symbols. Each ring is a flat x,y pair stream:
    /// absolute first point, then deltas, all in 1e-11 world units. Exterior
    /// and hole rings use even-odd fill when a card surrounds a sibling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_rings: Option<Vec<Option<Vec<Vec<i64>>>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_rings: Option<BTreeMap<usize, Vec<Vec<i64>>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_rings: Option<BTreeMap<usize, Vec<Vec<i64>>>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct DistrictSymbols {
    pub district: usize,
    pub files: Vec<usize>,
    /// Global symbol indices, including remote endpoints of touching edges.
    pub symbol_indices: Vec<usize>,
    pub symbols: Vec<HierSymbolRow>,
    #[serde(deserialize_with = "deserialize_symbol_edges")]
    pub edges: Vec<[usize; 4]>,
    #[serde(default = "symbol_edge_kinds")]
    pub kinds: Vec<String>,
    pub module_code_lines: BTreeMap<usize, usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_rings: Option<Vec<Option<Vec<Vec<i64>>>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_rings: Option<BTreeMap<usize, Vec<Vec<i64>>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_rings: Option<BTreeMap<usize, Vec<Vec<i64>>>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Neighbourhood {
    pub d: usize,
    pub size: usize,
    pub label: String,
    pub blob: Vec<Vec<[f64; 2]>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MapDocument {
    pub repo: String,
    pub q: f64,
    pub names: BTreeMap<String, String>,
    pub districts: BTreeMap<String, District>,
    #[serde(rename = "F")]
    pub files: Vec<String>,
    #[serde(rename = "N")]
    pub nodes: Vec<NodeRow>,
    #[serde(rename = "C", default, skip_serializing_if = "Option::is_none")]
    pub code_lines: Option<Vec<usize>>,
    #[serde(rename = "E")]
    pub edges: Vec<[usize; 2]>,
    #[serde(rename = "L")]
    pub landmarks: Vec<LandmarkRow>,
    #[serde(rename = "S")]
    pub symbols: BTreeMap<String, Vec<SymbolRow>>,
    #[serde(rename = "U")]
    pub uses: BTreeMap<String, Vec<usize>>,
    pub roads: Vec<RoadRow>,
    pub lang: String,
    // Kept-edge coverage; absent in maps built before issue #40.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<CoverageReport>,
    #[serde(rename = "P", skip_serializing_if = "Option::is_none")]
    pub parcels: Option<BTreeMap<String, Vec<[f64; 2]>>>,
    /// Index aligned with F; N remains the original layout position so a
    /// later warm start cannot accidentally feed displayed geometry back in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footprint_centroids: Option<Vec<[f64; 2]>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_neighbourhoods: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neighbourhoods: Option<BTreeMap<String, Neighbourhood>>,
}

// GraphData and its parts derive Serialize/Deserialize so a graph can be
// pre-extracted and checked in: CI cannot clone nine large repositories on
// every push (see docs/ARCHITECTURE.md's CI section), but it can run
// blend -> prune -> partition -> parity offline against a graph that was
// dumped once, out-of-band, against a fixture that is already small (flask,
// httpx). `tolmap dump-graph` produces this file; `tolmap build --graph`
// consumes it in place of a repository + extraction pass.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceNode {
    pub file: String,
    pub loc: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_lines: Option<usize>,
    pub complexity: usize,
    pub churn: usize,
    pub fanin: f64,
    pub module: String,
    // Polyglot union extraction: which language this file was parsed as.
    // `#[serde(default)]` so `data/ci/*.graph.json`, dumped before this
    // field existed, still deserialise -- an old graph is necessarily
    // single-language, and every reader of this field already has
    // `GraphData.lang` (or `GraphData.sources`) available as the
    // single-source fallback.
    #[serde(default)]
    pub lang: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignalEdge {
    pub a: String,
    pub b: String,
    pub weight: f64,
    pub static_signal: f64,
    pub cochange: f64,
    pub proximity: f64,
    pub semantic: f64,
}

pub type FileId = u32;

#[derive(Clone, Debug)]
pub struct GraphData {
    pub repo: String,
    pub pkg: String,
    pub lang: String,
    // Polyglot union extraction: every `(pkg, lang)` source that was
    // unioned to build this graph, sorted by `(lang, pkg)` -- the same
    // order `build_multi_source` merges in. `pkg`/`lang` above stay the
    // dominant source (by file count) for backward compatibility; this is
    // the field that tells the whole story for a merged graph. A
    // single-source graph carries exactly one entry, matching `pkg`/`lang`.
    // `#[serde(default)]` so old checked-in `data/ci/*.graph.json` (recorded
    // before this field existed) still deserialise via `tolmap build
    // --graph` -- they are necessarily single-source, and an empty vec here
    // is never read as "more than one source" by anything downstream.
    pub sources: Vec<(String, String)>,
    /// Resolved imports indexed into `nodes`. Keeping the two paths only in
    /// `SourceNode::file` avoids repeating them for every package fan-out
    /// edge; the serde implementation below retains the established
    /// path-based graph JSON format.
    pub imports: Vec<(FileId, FileId, f64)>,
    pub symbols: BTreeMap<String, Vec<SymbolRow>>,
    /// Resolved symbol uses indexed into `nodes`; names remain strings.
    pub uses: Vec<(FileId, FileId, String)>,
    pub commits_scanned: usize,
    pub nodes: Vec<SourceNode>,
    pub edges: Vec<SignalEdge>,
    /// `--refs scip` only: the per-language reference paths, carried to the
    /// map's `coverage.references`. Serialized only when present, so a
    /// hand-written graph dumps byte-identically.
    pub references: Option<BTreeMap<String, ReferenceCoverage>>,
}

#[derive(Deserialize)]
struct GraphDataWire {
    repo: String,
    pkg: String,
    lang: String,
    #[serde(default)]
    sources: Vec<(String, String)>,
    imports: Vec<(String, String, f64)>,
    symbols: BTreeMap<String, Vec<SymbolRow>>,
    uses: Vec<(String, String, String)>,
    commits_scanned: usize,
    nodes: Vec<SourceNode>,
    edges: Vec<SignalEdge>,
    #[serde(default)]
    references: Option<BTreeMap<String, ReferenceCoverage>>,
}

struct GraphImports<'a>(&'a GraphData);

impl Serialize for GraphImports<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.imports.len()))?;
        for &(a, b, value) in &self.0.imports {
            let a = &self.0.nodes[a as usize].file;
            let b = &self.0.nodes[b as usize].file;
            sequence.serialize_element(&(a, b, value))?;
        }
        sequence.end()
    }
}

struct GraphUses<'a>(&'a GraphData);

impl Serialize for GraphUses<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.uses.len()))?;
        for (a, b, name) in &self.0.uses {
            let a = &self.0.nodes[*a as usize].file;
            let b = &self.0.nodes[*b as usize].file;
            sequence.serialize_element(&(a, b, name))?;
        }
        sequence.end()
    }
}

impl Serialize for GraphData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Keep the derived serializer's established field order so
        // `tolmap dump-graph` remains byte-identical.
        let fields = if self.references.is_some() { 11 } else { 10 };
        let mut state = serializer.serialize_struct("GraphData", fields)?;
        state.serialize_field("repo", &self.repo)?;
        state.serialize_field("pkg", &self.pkg)?;
        state.serialize_field("lang", &self.lang)?;
        state.serialize_field("sources", &self.sources)?;
        state.serialize_field("imports", &GraphImports(self))?;
        state.serialize_field("symbols", &self.symbols)?;
        state.serialize_field("uses", &GraphUses(self))?;
        state.serialize_field("commits_scanned", &self.commits_scanned)?;
        state.serialize_field("nodes", &self.nodes)?;
        state.serialize_field("edges", &self.edges)?;
        if let Some(references) = &self.references {
            state.serialize_field("references", references)?;
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for GraphData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = GraphDataWire::deserialize(deserializer)?;
        if wire.nodes.len() > FileId::MAX as usize {
            return Err(D::Error::custom("graph has more files than u32 can index"));
        }
        let file_ids = wire
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.file.as_str(), index as FileId))
            .collect::<BTreeMap<_, _>>();
        let imports =
            wire.imports
                .into_iter()
                .map(|(a, b, value)| {
                    let a = file_ids.get(a.as_str()).copied().ok_or_else(|| {
                        D::Error::custom(format!("import names unknown file {a:?}"))
                    })?;
                    let b = file_ids.get(b.as_str()).copied().ok_or_else(|| {
                        D::Error::custom(format!("import names unknown file {b:?}"))
                    })?;
                    Ok((a, b, value))
                })
                .collect::<Result<Vec<_>, D::Error>>()?;
        let uses = wire
            .uses
            .into_iter()
            .map(|(a, b, name)| {
                let a = file_ids
                    .get(a.as_str())
                    .copied()
                    .ok_or_else(|| D::Error::custom(format!("use names unknown file {a:?}")))?;
                let b = file_ids
                    .get(b.as_str())
                    .copied()
                    .ok_or_else(|| D::Error::custom(format!("use names unknown file {b:?}")))?;
                Ok((a, b, name))
            })
            .collect::<Result<Vec<_>, D::Error>>()?;
        drop(file_ids);
        Ok(Self {
            repo: wire.repo,
            pkg: wire.pkg,
            lang: wire.lang,
            sources: wire.sources,
            imports,
            symbols: wire.symbols,
            uses,
            commits_scanned: wire.commits_scanned,
            nodes: wire.nodes,
            edges: wire.edges,
            references: wire.references,
        })
    }
}

#[derive(Clone, Debug)]
pub struct WeightedEdge {
    pub a: usize,
    pub b: usize,
    pub weight: f64,
}

#[derive(Clone, Debug)]
pub struct WeightedGraph {
    pub node_count: usize,
    pub edges: Vec<WeightedEdge>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_map_with_terrain_deserializes_and_drops_removed_field() {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../data/flask.json")).unwrap();
        value["terrain"] = serde_json::json!({
            "0": {
                "arterials": [{"file": 0, "stranded": 2, "links": [1]}],
                "subdistricts": [{"suffix": 1, "members": [0], "c": [0.0, 0.0], "blob": []}],
                "parcels": [{"address": "src", "members": [1], "rect": [0.0, 0.0, 1.0, 1.0]}],
                "max_suffix": 1
            }
        });
        // Serde ignores unknown fields unless deny_unknown_fields is set.
        // Stored maps may still carry this removed optional block.
        let document: MapDocument = serde_json::from_value(value).unwrap();
        assert!(document.code_lines.is_none());
        assert!(serde_json::to_value(document)
            .unwrap()
            .get("terrain")
            .is_none());
    }

    #[test]
    fn graph_data_keeps_path_based_json_with_indexed_edges() {
        let data = GraphData {
            repo: "fixture".to_owned(),
            pkg: ".".to_owned(),
            lang: "go".to_owned(),
            sources: vec![(".".to_owned(), "go".to_owned())],
            imports: vec![(0, 1, 0.5)],
            symbols: BTreeMap::new(),
            uses: vec![(0, 1, "Target".to_owned())],
            commits_scanned: 0,
            nodes: ["a.go", "pkg/b.go"]
                .into_iter()
                .map(|file| SourceNode {
                    file: file.to_owned(),
                    loc: 1,
                    code_lines: None,
                    complexity: 0,
                    churn: 0,
                    fanin: 0.0,
                    module: file.to_owned(),
                    lang: "go".to_owned(),
                })
                .collect(),
            edges: Vec::new(),
            references: None,
        };

        let bytes = serde_json::to_vec(&data).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value["imports"],
            serde_json::json!([["a.go", "pkg/b.go", 0.5]])
        );
        assert_eq!(
            value["uses"],
            serde_json::json!([["a.go", "pkg/b.go", "Target"]])
        );

        let round_trip: GraphData = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(round_trip.imports, data.imports);
        assert_eq!(round_trip.uses, data.uses);
        assert_eq!(serde_json::to_vec(&round_trip).unwrap(), bytes);
    }
}
