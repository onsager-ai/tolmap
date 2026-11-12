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

/// Optional browser composition. Every endpoint and provider action is host-owned.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct HostProfile {
    #[ts(type = "1")]
    pub version: u8,
    pub private: bool,
    pub workspace: String,
    pub repositories: Vec<HostRepository>,
    pub actions: Vec<HostAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub onboarding: Option<HostOnboarding>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct HostRepository {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct HostAction {
    pub label: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "\"post\"")]
    pub method: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct HostOnboarding {
    pub offered_repositories_path: String,
    pub batch_index_path: String,
    /// Same-origin template containing {batch_id} exactly once.
    pub batch_progress_path: String,
    /// Query parameter whose value "1" reopens the sheet after provider return.
    pub open_sheet_parameter: String,
    pub install: Option<HostAction>,
    pub manage_access: Option<HostAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sign_in: Option<HostAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub authorize: Option<HostAction>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct OfferedRepository {
    pub provider_id: String,
    pub full_name: String,
    pub visibility: RepositoryVisibility,
    pub connected_repository_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryVisibility {
    Public,
    Private,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct BatchIndexRequest {
    pub provider_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct BatchIndexJob {
    pub repository_id: String,
    pub job_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct BatchIndexResponse {
    pub batch_id: String,
    pub jobs: Vec<BatchIndexJob>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct BatchProgress {
    pub batch_id: String,
    pub jobs: Vec<crate::service::jobs::JobSnapshot>,
}
