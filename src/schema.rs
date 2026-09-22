use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
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
pub struct TerrainArterial {
    pub file: usize,
    pub stranded: usize,
    /// Weighted-pruned in-district neighbours.  Keeping this separate from
    /// `roads` matters: `RoadRow` joins top-level districts, while these
    /// links draw one load-bearing file as a road inside its own district.
    pub links: Vec<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerrainSubdistrict {
    pub suffix: usize,
    pub members: Vec<usize>,
    pub c: [f64; 2],
    pub blob: Vec<Vec<[f64; 2]>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerrainParcel {
    pub address: String,
    pub members: Vec<usize>,
    /// `[x, y, width, height]` in the same region coordinate system as `N`.
    pub rect: [f64; 4],
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerrainDistrict {
    pub arterials: Vec<TerrainArterial>,
    pub subdistricts: Vec<TerrainSubdistrict>,
    pub parcels: Vec<TerrainParcel>,
    /// Monotonic high-water mark; retired suffixes are never reissued.
    pub max_suffix: usize,
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
    #[serde(rename = "P", skip_serializing_if = "Option::is_none")]
    pub parcels: Option<BTreeMap<String, Vec<[f64; 2]>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terrain: Option<BTreeMap<String, TerrainDistrict>>,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    #[serde(default)]
    pub sources: Vec<(String, String)>,
    pub imports: Vec<(String, String, f64)>,
    pub symbols: BTreeMap<String, Vec<SymbolRow>>,
    pub uses: Vec<(String, String, String)>,
    pub commits_scanned: usize,
    pub nodes: Vec<SourceNode>,
    pub edges: Vec<SignalEdge>,
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
