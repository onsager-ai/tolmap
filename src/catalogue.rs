//! Shared catalogue and repository history contract for services and hosts.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct CatalogueEntry {
    pub slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub label: Option<String>,
    pub owner: String,
    pub repo: String,
    #[serde(default)]
    pub file: String,
    #[ts(type = "number")]
    pub files: i64,
    #[ts(type = "number")]
    pub districts: i64,
    pub modularity: f64,
    pub lang: String,
    pub source: CatalogueSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub repository_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub latest_snapshot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub indexed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub snapshot_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub status: Option<RepositoryStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CatalogueSource {
    Static,
    Service,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepositoryStatus {
    Indexing {
        phase: String,
        job_id: Option<String>,
    },
    Failed {
        message: String,
    },
    ReconnectNeeded,
}

/// Paths are same-origin and owned by the host. Omitted actions confer no UI.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct RepositoryAction {
    pub path: String,
    pub method: ActionMethod,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "UPPERCASE")]
pub enum ActionMethod {
    Get,
    Post,
    Delete,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, TS)]
pub struct RepositoryActions {
    pub reopen: Option<RepositoryAction>,
    pub reindex: Option<RepositoryAction>,
    pub disconnect: Option<RepositoryAction>,
    pub delete_repository: Option<RepositoryAction>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct RepositorySnapshot {
    pub id: String,
    pub commit: String,
    pub indexed_at: String,
    #[ts(type = "number")]
    pub files: i64,
    #[ts(type = "number")]
    pub districts: i64,
    /// Unknown provenance is null, never inferred from the running binary.
    pub core_revision: Option<String>,
    pub reopen: Option<RepositoryAction>,
    pub delete_snapshot: Option<RepositoryAction>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct RepositoryHistory {
    pub repository_id: String,
    pub name: String,
    pub status: Option<RepositoryStatus>,
    pub current_core_revision: Option<String>,
    pub actions: RepositoryActions,
    /// Newest first, including all retained snapshots.
    pub snapshots: Vec<RepositorySnapshot>,
}
