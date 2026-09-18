use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct District {
    pub size: usize,
    pub c: [f64; 2],
    pub blob: Vec<Vec<[f64; 2]>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(type = "[number, number, number, number, number, number, number, number, number, number, number]")]
pub struct NodeRow(pub (usize, f64, f64, usize, usize, usize, f64, f64, f64, f64, f64));

impl NodeRow {
    pub fn district(&self) -> usize {
        self.0.0
    }

    pub fn point(&self) -> [f64; 2] {
        [self.0.1, self.0.2]
    }

    pub fn loc(&self) -> usize {
        self.0.3
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
}

#[derive(Clone, Debug)]
pub struct SourceNode {
    pub file: String,
    pub loc: usize,
    pub complexity: usize,
    pub churn: usize,
    pub fanin: f64,
    pub module: String,
}

#[derive(Clone, Debug)]
pub struct SignalEdge {
    pub a: String,
    pub b: String,
    pub weight: f64,
    pub static_signal: f64,
    pub cochange: f64,
    pub proximity: f64,
    pub semantic: f64,
}

#[derive(Clone, Debug)]
pub struct GraphData {
    pub repo: String,
    pub pkg: String,
    pub lang: String,
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
