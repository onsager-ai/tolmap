//! Shared, versioned envelope for compact queries over stored snapshots.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const QUERY_SCHEMA: &str = "tolmap.query/v1";

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct QueryEnvelope<T> {
    pub schema: String,
    pub query: Query,
    pub map: MapIdentity,
    pub coverage: QueryCoverage,
    pub limit: usize,
    pub total: usize,
    pub truncated: bool,
    pub results: Vec<T>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct MapIdentity {
    pub repo: String,
    pub commit: Option<String>,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct QueryCoverage {
    pub refs: ReferenceMode,
    pub zero_edge_files: Option<usize>,
    pub calls_total: Option<usize>,
    pub calls_resolved: Option<usize>,
    pub unresolved: Option<BTreeMap<String, usize>>,
    pub lower_bound: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReferenceMode {
    Hand,
    Scip,
    Mixed,
}

#[derive(
    Clone, Copy, Debug, Default, Serialize, Deserialize, TS, clap::ValueEnum, PartialEq, Eq,
)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    In,
    Out,
    #[default]
    Both,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Query {
    Where { target: String, ambiguous: bool },
    Deps { path: String, direction: Direction },
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum QueryResult {
    File {
        path: String,
        district: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        symbol: Option<QuerySymbol>,
    },
    Dependency {
        path: String,
        district: String,
        direction: Direction,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct QuerySymbol {
    pub name: String,
    pub qualified_name: String,
    pub start: usize,
    pub end: usize,
}

pub type QueryResponse = QueryEnvelope<QueryResult>;
