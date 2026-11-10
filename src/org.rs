//! Organization maps (issue #255): several repositories read as one system.
//!
//! An organization manifest (`<name>.org.toml`) names the organization and
//! pins every member repository to a full commit. `tolmap org check`
//! validates it; `tolmap org build` reads each member at its commit (from a
//! detached temporary worktree of a local clone, never a working tree),
//! takes its stored map or builds one, and composes an
//! [`OrganizationDocument`]: members, repository-level dependency edges with
//! their evidence, pin skew, external dependencies and unresolved items.
//!
//! Member maps are read or built, never changed or re-clustered, and no
//! modularity is computed or combined across repositories (findings 5 and
//! 10): a member's own `q` stays in its own map. Edges come only from
//! declared dependencies in manifests, so every count is a lower bound: an
//! HTTP call, a queue or copied code links repositories invisibly.
//!
//! Plain `//` comments on exported types, not doc comments: ts-rs copies doc
//! comments into the bindings.
pub(crate) mod deps;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::check::{self, CheckError, Scratch};
use crate::extract::{InstallMode, RefsMode};
use crate::geometry::{self, BuildFeatures};
use crate::naming::{self, NamerKind};
use crate::pipeline::PruneVariant;
use crate::schema::MapDocument;

pub const MANIFEST_VERSION: u32 = 1;
pub const DOCUMENT_VERSION: u32 = 2;

// ---------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------

// One member repository. `commit` is a full object id and always required,
// so the manifest alone pins the organization. `path`, relative to the
// manifest, is a local clone to read the commit from instead of
// `<clones>/<owner>/<repo>`; only committed content at `commit` is read.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct OrganizationMember {
    pub repo: String,
    pub commit: String,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct OrganizationManifest {
    pub version: u32,
    pub name: String,
    pub members: Vec<OrganizationMember>,
}

/// Why a manifest was rejected. Each rule has its own variant so a caller
/// (and a test) can tell them apart.
#[derive(Debug, PartialEq, Eq)]
pub enum ManifestError {
    /// Not TOML of the manifest's shape (a missing field, a wrong type).
    Parse(String),
    /// A key the schema does not define.
    UnknownKey(String),
    UnsupportedVersion(u32),
    InvalidName(String),
    NoMembers,
    /// `repo` is not `owner/repo`.
    InvalidRepo(String),
    /// The same repository listed twice (compared case-insensitively).
    DuplicateMember(String),
    /// A hex prefix shorter than a full object id.
    AbbreviatedCommit {
        repo: String,
        commit: String,
    },
    /// Not a hex object id at all.
    InvalidCommit {
        repo: String,
        commit: String,
    },
    /// Empty or absolute: the path is recorded in the document as written,
    /// and an absolute local path would leak.
    InvalidPath {
        repo: String,
        path: String,
    },
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(m) => write!(f, "manifest parse error: {m}"),
            Self::UnknownKey(m) => write!(f, "manifest has an unknown key: {m}"),
            Self::UnsupportedVersion(v) => write!(
                f,
                "unsupported manifest version {v}, expected {MANIFEST_VERSION}"
            ),
            Self::InvalidName(n) => write!(
                f,
                "organization name {n:?} is not [a-z0-9][a-z0-9._-]{{0,63}}"
            ),
            Self::NoMembers => write!(f, "the organization lists no members"),
            Self::InvalidRepo(r) => write!(f, "member repo {r:?} is not owner/repo"),
            Self::DuplicateMember(r) => write!(f, "member {r} is listed more than once"),
            Self::AbbreviatedCommit { repo, commit } => write!(
                f,
                "member {repo}: commit {commit} is abbreviated; pin the full 40-character object id"
            ),
            Self::InvalidCommit { repo, commit } => {
                write!(f, "member {repo}: commit {commit:?} is not a git object id")
            }
            Self::InvalidPath { repo, path } => write!(
                f,
                "member {repo}: path {path:?} must be a non-empty path relative to the manifest"
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

fn valid_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

impl OrganizationMember {
    /// `(owner, repo)`; valid after [`OrganizationManifest::parse`].
    pub fn owner_repo(&self) -> (&str, &str) {
        self.repo.split_once('/').unwrap_or((&self.repo, ""))
    }
}

impl OrganizationManifest {
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let manifest: Self = toml::from_str(text).map_err(|error| {
            let message = error.message().to_owned();
            if message.starts_with("unknown field") {
                ManifestError::UnknownKey(message)
            } else {
                ManifestError::Parse(error.to_string())
            }
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), ManifestError> {
        if self.version != MANIFEST_VERSION {
            return Err(ManifestError::UnsupportedVersion(self.version));
        }
        let name_ok = self.name.len() <= 64
            && self
                .name
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && self.name.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
            });
        if !name_ok {
            return Err(ManifestError::InvalidName(self.name.clone()));
        }
        if self.members.is_empty() {
            return Err(ManifestError::NoMembers);
        }
        let mut seen = BTreeSet::new();
        for member in &self.members {
            let valid_repo = member
                .repo
                .split_once('/')
                .is_some_and(|(owner, repo)| valid_segment(owner) && valid_segment(repo));
            if !valid_repo {
                return Err(ManifestError::InvalidRepo(member.repo.clone()));
            }
            if !seen.insert(member.repo.to_ascii_lowercase()) {
                return Err(ManifestError::DuplicateMember(member.repo.clone()));
            }
            let commit = &member.commit;
            let lower_hex = commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            if commit.is_empty() || !lower_hex {
                return Err(ManifestError::InvalidCommit {
                    repo: member.repo.clone(),
                    commit: commit.clone(),
                });
            }
            if commit.len() < 40 {
                return Err(ManifestError::AbbreviatedCommit {
                    repo: member.repo.clone(),
                    commit: commit.clone(),
                });
            }
            if commit.len() != 40 {
                return Err(ManifestError::InvalidCommit {
                    repo: member.repo.clone(),
                    commit: commit.clone(),
                });
            }
            if let Some(path) = &member.path {
                if path.trim().is_empty() || Path::new(path).is_absolute() {
                    return Err(ManifestError::InvalidPath {
                        repo: member.repo.clone(),
                        path: path.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Ecosystem {
    Cargo,
    Npm,
    Go,
    Python,
}

// What a consumer pins. `commit` is a hex object id (possibly abbreviated);
// `tag`, `branch` and `ref` are git references (`ref` when the manifest
// does not say which); `version` a version or requirement as written;
// `path` and `workspace` build whatever is beside the consumer; `none`
// pins nothing.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum PinKind {
    Commit,
    Tag,
    Branch,
    Ref,
    Version,
    Path,
    Workspace,
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pin {
    pub kind: PinKind,
    pub value: String,
}

// `match`: the pin names the member commit, or a version requirement the
// provider's declared version at that commit satisfies. `skew`: it names a
// different commit or excludes that version. `unjudged`: it cannot be
// compared (a branch, a path, a range this reader does not evaluate); the
// evidence's `reason` says why.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum PinStatus {
    Match,
    Skew,
    Unjudged,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum MapSource {
    Stored,
    Built,
}

// A package a member provides at its commit, from its own manifest.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProvidedPackage {
    pub ecosystem: Ecosystem,
    pub name: String,
    pub file: String,
    pub version: Option<String>,
}

// `files` and `districts` are read from the member's own map; its
// modularity stays there and is never combined across members.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
pub struct OrganizationMemberMap {
    pub repo: String,
    pub commit: String,
    pub path: Option<String>,
    pub map: MapSource,
    pub map_digest: String,
    pub files: usize,
    pub districts: usize,
    pub packages: Vec<ProvidedPackage>,
}

// A repository-sized region in the organization overview. The viewer fits the
// member map's bounds into a unit square, multiplies that square by `scale`,
// then centres it on `translation`. The transform never alters the member
// map's bytes or its own layout.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
pub struct OrganizationRegion {
    pub repo: String,
    pub translation: [f64; 2],
    pub scale: f64,
}

// One inter-repository road. Its endpoints are the placed centres of the
// consumer and provider regions, respectively.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
pub struct OrganizationRoad {
    pub consumer: String,
    pub provider: String,
    pub from: [f64; 2],
    pub to: [f64; 2],
}

// The deterministic, organization-level placement. It composes immutable
// member maps with translations and uniform scales, and never has modularity.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
pub struct OrganizationPlacement {
    pub regions: Vec<OrganizationRegion>,
    pub roads: Vec<OrganizationRoad>,
}

// One declared dependency behind a repository edge.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord)]
pub struct DependencyEvidence {
    pub ecosystem: Ecosystem,
    pub consumer_package: String,
    pub file: String,
    pub section: String,
    pub provider_package: String,
    pub pin: Pin,
    pub status: PinStatus,
    pub reason: Option<String>,
}

// Consumer depends on provider. A lower bound: only declared dependencies.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord)]
pub struct RepositoryEdge {
    pub consumer: String,
    pub provider: String,
    pub evidence: Vec<DependencyEvidence>,
}

// A consumer pinning a different commit or version than the organization's
// member commit.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord)]
pub struct PinSkew {
    pub consumer: String,
    pub consumer_package: String,
    pub file: String,
    pub provider: String,
    pub provider_package: String,
    pub pin: Pin,
    pub member_commit: String,
    pub member_version: Option<String>,
}

// A declared dependency no member provides.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExternalDependency {
    pub consumer: String,
    pub consumer_package: String,
    pub ecosystem: Ecosystem,
    pub file: String,
    pub section: String,
    pub name: String,
    pub pin: Pin,
}

// A declaration that could not be read, or could not be attributed to one
// member.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnresolvedDependency {
    pub consumer: String,
    pub consumer_package: String,
    pub ecosystem: Ecosystem,
    pub file: String,
    pub name: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct OrganizationDocument {
    pub version: u32,
    pub name: String,
    pub members: Vec<OrganizationMemberMap>,
    pub placement: OrganizationPlacement,
    pub edges: Vec<RepositoryEdge>,
    pub pin_skew: Vec<PinSkew>,
    pub external: Vec<ExternalDependency>,
    pub unresolved: Vec<UnresolvedDependency>,
}

const PLACEMENT_ITERATIONS: usize = 64;
const SEPARATION_ITERATIONS: usize = 32;
const REPOSITORY_REPULSION: f64 = 0.08;
const REPOSITORY_ATTRACTION: f64 = 0.03;
const MIN_DISTANCE_SQUARED: f64 = 0.0001;
const MAX_STEP: f64 = 0.05;
const REGION_MARGIN: f64 = 0.1;
const REGION_SEPARATION: f64 = 0.01;

/// Places lexically-sorted repositories with fixed spring and separation
/// passes. The initial circle has a `SEED`-derived phase. Each spring step
/// processes pairs in repository order, so equal graph weights use lexical
/// order as the tie-break; evidence attracts its pair and every pair repels.
/// A per-step cap prevents a high evidence count from overshooting.
///
/// The final separation pass treats the unit-square member regions as discs
/// with radius `scale * sqrt(1/2) + REGION_MARGIN`. It processes every pair in
/// lexical order for a fixed number of iterations, so transformed member maps
/// cannot overlap. This is composition only: member geometry is never read or
/// rewritten here.
fn placement(members: &[OrganizationMemberMap], edges: &[RepositoryEdge]) -> OrganizationPlacement {
    let count = members.len();
    if count == 0 {
        return OrganizationPlacement {
            regions: Vec::new(),
            roads: Vec::new(),
        };
    }

    let index = members
        .iter()
        .enumerate()
        .map(|(position, member)| (member.repo.as_str(), position))
        .collect::<BTreeMap<_, _>>();
    let mut weights = vec![vec![0.0; count]; count];
    for edge in edges {
        let (Some(&consumer), Some(&provider)) = (
            index.get(edge.consumer.as_str()),
            index.get(edge.provider.as_str()),
        ) else {
            continue;
        };
        let weight = (edge.evidence.len() as f64).ln_1p();
        weights[consumer][provider] += weight;
        weights[provider][consumer] += weight;
    }

    let phase = std::f64::consts::TAU * (crate::SEED % count as u64) as f64 / count as f64;
    let mut positions = (0..count)
        .map(|position| {
            let angle = phase + std::f64::consts::TAU * position as f64 / count as f64;
            [angle.cos(), angle.sin()]
        })
        .collect::<Vec<_>>();
    if count == 1 {
        positions[0] = [0.0, 0.0];
    }
    for _ in 0..PLACEMENT_ITERATIONS {
        let mut forces = vec![[0.0, 0.0]; count];
        for left in 0..count {
            for right in (left + 1)..count {
                let dx = positions[right][0] - positions[left][0];
                let dy = positions[right][1] - positions[left][1];
                let distance_squared = (dx * dx + dy * dy).max(MIN_DISTANCE_SQUARED);
                let distance = distance_squared.sqrt();
                let unit = [dx / distance, dy / distance];
                let force = weights[left][right] * REPOSITORY_ATTRACTION * distance
                    - REPOSITORY_REPULSION / distance_squared;
                for axis in 0..2 {
                    let adjustment = unit[axis] * force;
                    forces[left][axis] += adjustment;
                    forces[right][axis] -= adjustment;
                }
            }
        }
        for (position, force) in positions.iter_mut().zip(forces) {
            let magnitude = (force[0] * force[0] + force[1] * force[1]).sqrt();
            let ratio = (MAX_STEP / magnitude).min(1.0);
            position[0] += force[0] * ratio;
            position[1] += force[1] * ratio;
        }
    }

    let largest = members
        .iter()
        .map(|member| member.files)
        .max()
        .unwrap_or(1)
        .max(1);
    let scales = members
        .iter()
        .map(|member| (member.files.max(1) as f64 / largest as f64).sqrt())
        .collect::<Vec<_>>();
    separate_regions(&mut positions, &scales);
    let centre = positions.iter().fold([0.0, 0.0], |mut total, position| {
        total[0] += position[0] / count as f64;
        total[1] += position[1] / count as f64;
        total
    });
    for position in &mut positions {
        position[0] -= centre[0];
        position[1] -= centre[1];
    }

    let regions = members
        .iter()
        .zip(positions.iter().copied().zip(scales))
        .map(|(member, (translation, scale))| OrganizationRegion {
            repo: member.repo.clone(),
            translation,
            scale,
        })
        .collect();
    let roads = edges
        .iter()
        .filter_map(|edge| {
            let (&consumer, &provider) = (
                index.get(edge.consumer.as_str())?,
                index.get(edge.provider.as_str())?,
            );
            Some(OrganizationRoad {
                consumer: edge.consumer.clone(),
                provider: edge.provider.clone(),
                from: positions[consumer],
                to: positions[provider],
            })
        })
        .collect();
    OrganizationPlacement { regions, roads }
}

fn separate_regions(positions: &mut [[f64; 2]], scales: &[f64]) {
    for _ in 0..SEPARATION_ITERATIONS {
        for left in 0..positions.len() {
            for right in (left + 1)..positions.len() {
                let dx = positions[right][0] - positions[left][0];
                let dy = positions[right][1] - positions[left][1];
                let actual_distance_squared = dx * dx + dy * dy;
                let distance = actual_distance_squared.sqrt();
                let required = region_radius(scales[left])
                    + region_radius(scales[right])
                    + REGION_SEPARATION;
                if distance < required {
                    let adjustment = (required - distance) / 2.0;
                    // A coincident pair has no geometric direction. Give it
                    // the lexically ordered pair's fixed horizontal axis so
                    // the separation pass remains deterministic and moves it.
                    let unit = if actual_distance_squared <= MIN_DISTANCE_SQUARED {
                        [1.0, 0.0]
                    } else {
                        [dx / distance, dy / distance]
                    };
                    for axis in 0..2 {
                        positions[left][axis] -= unit[axis] * adjustment;
                        positions[right][axis] += unit[axis] * adjustment;
                    }
                }
            }
        }
    }
}

fn region_radius(scale: f64) -> f64 {
    scale * std::f64::consts::FRAC_1_SQRT_2 + REGION_MARGIN
}

// ---------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------

/// One member as resolution sees it.
pub(crate) struct MemberTree<'a> {
    pub repo: &'a str,
    pub commit: &'a str,
    pub tree: &'a deps::Tree,
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Resolution {
    pub edges: Vec<RepositoryEdge>,
    pub pin_skew: Vec<PinSkew>,
    pub external: Vec<ExternalDependency>,
    pub unresolved: Vec<UnresolvedDependency>,
}

fn package_key(ecosystem: Ecosystem, name: &str) -> (Ecosystem, String) {
    let name = match ecosystem {
        // crates.io treats `-` and `_` as the same name.
        Ecosystem::Cargo => name.replace('_', "-"),
        Ecosystem::Python => deps::normalize_python(name),
        Ecosystem::Npm | Ecosystem::Go => name.to_owned(),
    };
    (ecosystem, name)
}

/// Resolves every member's declared dependencies against every member.
/// `tag_commit(member, tag)` is the commit a tag names in that member's
/// clone, if it exists there. Pure apart from that lookup; every output
/// list is sorted.
pub(crate) fn resolve(
    members: &[MemberTree<'_>],
    tag_commit: &dyn Fn(usize, &str) -> Option<String>,
) -> Resolution {
    let mut providers = BTreeMap::<(Ecosystem, String), Vec<(usize, usize)>>::new();
    for (m, member) in members.iter().enumerate() {
        for (p, package) in member.tree.provided.iter().enumerate() {
            let entry = providers
                .entry(package_key(package.ecosystem, &package.name))
                .or_default();
            // One entry per member: a repository may declare a name twice.
            if !entry.iter().any(|(other, _)| *other == m) {
                entry.push((m, p));
            }
        }
    }
    let by_repo = members
        .iter()
        .enumerate()
        .map(|(m, member)| (member.repo.to_ascii_lowercase(), m))
        .collect::<BTreeMap<_, _>>();
    let mut edges = BTreeMap::<(String, String), Vec<DependencyEvidence>>::new();
    let mut result = Resolution::default();
    for (c, consumer) in members.iter().enumerate() {
        for item in &consumer.tree.unreadable {
            result.unresolved.push(UnresolvedDependency {
                consumer: consumer.repo.to_owned(),
                consumer_package: item.consumer_package.clone(),
                ecosystem: item.ecosystem,
                file: item.file.clone(),
                name: item.name.clone(),
                reason: item.reason.clone(),
            });
        }
        for declared in &consumer.tree.declared {
            let key = package_key(declared.ecosystem, &declared.name);
            let unresolved = |reason: String| UnresolvedDependency {
                consumer: consumer.repo.to_owned(),
                consumer_package: declared.consumer_package.clone(),
                ecosystem: declared.ecosystem,
                file: declared.file.clone(),
                name: declared.name.clone(),
                reason,
            };
            let candidates = providers.get(&key).map(Vec::as_slice).unwrap_or(&[]);
            let provider = match &declared.repo {
                Some(repo) => match by_repo.get(repo) {
                    Some(&p) if p == c => continue,
                    Some(&p) => match candidates.iter().find(|(m, _)| *m == p) {
                        Some(&found) => found,
                        None => {
                            result.unresolved.push(unresolved(format!(
                                "a git dependency on member {}, which provides no {} package of this name at {}",
                                members[p].repo,
                                ecosystem_name(declared.ecosystem),
                                members[p].commit
                            )));
                            continue;
                        }
                    },
                    None => {
                        result.external.push(external(consumer.repo, declared));
                        continue;
                    }
                },
                None => {
                    if candidates.iter().any(|(m, _)| *m == c) {
                        continue; // provided inside the consumer's own repository
                    }
                    match candidates {
                        [] => {
                            result.external.push(external(consumer.repo, declared));
                            continue;
                        }
                        [only] => *only,
                        many => {
                            let names = many
                                .iter()
                                .map(|(m, _)| members[*m].repo)
                                .collect::<Vec<_>>()
                                .join(", ");
                            result.unresolved.push(unresolved(format!(
                                "provided by more than one member: {names}"
                            )));
                            continue;
                        }
                    }
                }
            };
            let (p, package) = provider;
            let provider_member = &members[p];
            let package = &provider_member.tree.provided[package];
            let (status, reason) = judge(
                declared.ecosystem,
                &declared.pin,
                provider_member.commit,
                package,
                &|tag| tag_commit(p, tag),
            );
            if status == PinStatus::Skew {
                result.pin_skew.push(PinSkew {
                    consumer: consumer.repo.to_owned(),
                    consumer_package: declared.consumer_package.clone(),
                    file: declared.file.clone(),
                    provider: provider_member.repo.to_owned(),
                    provider_package: package.name.clone(),
                    pin: declared.pin.clone(),
                    member_commit: provider_member.commit.to_owned(),
                    member_version: package.version.clone(),
                });
            }
            edges
                .entry((consumer.repo.to_owned(), provider_member.repo.to_owned()))
                .or_default()
                .push(DependencyEvidence {
                    ecosystem: declared.ecosystem,
                    consumer_package: declared.consumer_package.clone(),
                    file: declared.file.clone(),
                    section: declared.section.clone(),
                    provider_package: package.name.clone(),
                    pin: declared.pin.clone(),
                    status,
                    reason,
                });
        }
    }
    result.edges = edges
        .into_iter()
        .map(|((consumer, provider), mut evidence)| {
            evidence.sort();
            evidence.dedup();
            RepositoryEdge {
                consumer,
                provider,
                evidence,
            }
        })
        .collect();
    result.pin_skew.sort();
    result.pin_skew.dedup();
    result.external.sort();
    result.external.dedup();
    result.unresolved.sort();
    result.unresolved.dedup();
    result
}

fn ecosystem_name(ecosystem: Ecosystem) -> &'static str {
    match ecosystem {
        Ecosystem::Cargo => "cargo",
        Ecosystem::Npm => "npm",
        Ecosystem::Go => "go",
        Ecosystem::Python => "python",
    }
}

fn external(consumer: &str, declared: &deps::Declared) -> ExternalDependency {
    ExternalDependency {
        consumer: consumer.to_owned(),
        consumer_package: declared.consumer_package.clone(),
        ecosystem: declared.ecosystem,
        file: declared.file.clone(),
        section: declared.section.clone(),
        name: declared.name.clone(),
        pin: declared.pin.clone(),
    }
}

fn judge(
    ecosystem: Ecosystem,
    pin: &Pin,
    member_commit: &str,
    package: &ProvidedPackage,
    tag_commit: &dyn Fn(&str) -> Option<String>,
) -> (PinStatus, Option<String>) {
    let unjudged = |reason: String| (PinStatus::Unjudged, Some(reason));
    let by_commit = |commit: &str| {
        if commit == member_commit {
            (PinStatus::Match, None)
        } else {
            (PinStatus::Skew, None)
        }
    };
    let by_tag = |tag: &str| match tag_commit(tag) {
        Some(commit) => by_commit(&commit),
        None => unjudged(format!(
            "{tag} is not a tag in the provider's clone; a branch or unknown ref moves"
        )),
    };
    match pin.kind {
        PinKind::Commit => {
            if member_commit.starts_with(&pin.value.to_ascii_lowercase()) {
                (PinStatus::Match, None)
            } else {
                (PinStatus::Skew, None)
            }
        }
        PinKind::Tag | PinKind::Ref => by_tag(&pin.value),
        PinKind::Branch => unjudged(format!(
            "branch {} moves; it names no fixed commit",
            pin.value
        )),
        PinKind::Path => unjudged(format!(
            "a path dependency builds whatever is checked out at {}",
            pin.value
        )),
        PinKind::Workspace => {
            unjudged("a workspace dependency resolves inside one workspace".to_owned())
        }
        PinKind::None => unjudged("no version, commit or tag is pinned".to_owned()),
        PinKind::Version if ecosystem == Ecosystem::Go => {
            let version = pin.value.trim_end_matches("+incompatible");
            if let Some(hash) = go_pseudo_version_hash(version) {
                return if member_commit.starts_with(hash) {
                    (PinStatus::Match, None)
                } else {
                    (PinStatus::Skew, None)
                };
            }
            // A module in a subdirectory is tagged `<dir>/<version>`.
            let directory = package.file.rsplit_once('/').map(|(d, _)| d);
            match directory {
                Some(directory) => by_tag(&format!("{directory}/{version}")),
                None => by_tag(version),
            }
        }
        PinKind::Version => {
            let Some(version) = &package.version else {
                return unjudged(format!(
                    "{} declares no version at the member commit",
                    package.name
                ));
            };
            match satisfies(&pin.value, version, ecosystem) {
                Some(true) => (PinStatus::Match, None),
                Some(false) => (PinStatus::Skew, None),
                None => unjudged(format!(
                    "requirement {:?} against version {version} is not evaluated",
                    pin.value
                )),
            }
        }
    }
}

/// The 12-character commit prefix a Go pseudo-version ends in
/// (`v0.0.0-20240101000000-0123456789ab`, `v1.2.4-0.2024...-hash`).
fn go_pseudo_version_hash(version: &str) -> Option<&str> {
    let mut parts = version.rsplitn(2, '-');
    let hash = parts.next()?;
    let rest = parts.next()?;
    let timestamp = rest.rsplit(['-', '.']).next()?;
    (hash.len() == 12
        && deps::is_hex(hash)
        && timestamp.len() == 14
        && timestamp.bytes().all(|b| b.is_ascii_digit()))
    .then_some(hash)
}

/// `1`, `1.2` or `1.2.3`, with an optional leading `v`. A pre-release,
/// build metadata, wildcard or fourth component is not parsed.
fn parse_partial(text: &str) -> Option<Vec<u64>> {
    let text = text.trim();
    let text = text.strip_prefix('v').unwrap_or(text);
    let parts = text
        .split('.')
        .map(|part| {
            if !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) {
                part.parse().ok()
            } else {
                None
            }
        })
        .collect::<Option<Vec<u64>>>()?;
    (1..=3).contains(&parts.len()).then_some(parts)
}

fn pad(parts: &[u64]) -> [u64; 3] {
    let mut full = [0; 3];
    full[..parts.len()].copy_from_slice(parts);
    full
}

/// The exclusive upper bound that bumps component `index` of `parts`.
fn bump(parts: &[u64], index: usize) -> [u64; 3] {
    let mut upper = [0; 3];
    upper[..index].copy_from_slice(&parts[..index]);
    upper[index] = parts[index] + 1;
    upper
}

/// Whether `version` satisfies `requirement`, in the ecosystem's syntax:
/// Cargo (a bare version is a caret requirement, comparators separated by
/// commas), npm (a bare version is exact, comparators separated by
/// spaces) or PEP 440 (`==`, `!=`, `~=`, comparisons). `None` for anything
/// this subset does not evaluate (`||`, hyphen ranges, wildcards,
/// pre-releases): unjudged, never guessed.
pub(crate) fn satisfies(requirement: &str, version: &str, ecosystem: Ecosystem) -> Option<bool> {
    let requirement = requirement.trim();
    let version = pad(&parse_partial(version)?);
    if requirement == "*" {
        return Some(true);
    }
    if requirement.contains("||") || requirement.contains(" - ") {
        return None;
    }
    let comparators: Vec<&str> = match ecosystem {
        Ecosystem::Npm => requirement.split_whitespace().collect(),
        _ => requirement.split(',').map(str::trim).collect(),
    };
    let mut all = true;
    for comparator in comparators {
        let op = ["~=", ">=", "<=", "==", "!=", "^", "~", "=", ">", "<"]
            .into_iter()
            .find(|op| comparator.starts_with(op))
            .unwrap_or("");
        let parts = parse_partial(&comparator[op.len()..])?;
        let lower = pad(&parts);
        let prefix = || version[..parts.len()] == parts[..];
        let caret = || {
            let index = parts
                .iter()
                .position(|&x| x != 0)
                .unwrap_or(parts.len() - 1);
            version >= lower && version < bump(&parts, index)
        };
        let holds = match (op, ecosystem) {
            ("^", _) | ("", Ecosystem::Cargo) => caret(),
            ("", Ecosystem::Npm) | ("=", _) => prefix(),
            ("", _) => return None,
            ("~", _) => version >= lower && version < bump(&parts, usize::from(parts.len() >= 2)),
            ("~=", _) if parts.len() >= 2 => {
                version >= lower && version < bump(&parts, parts.len() - 2)
            }
            ("~=", _) => return None,
            ("==", Ecosystem::Python) => version == lower,
            ("==", _) => prefix(),
            ("!=", _) => version != lower,
            (">=", _) => version >= lower,
            (">", _) => version > lower,
            ("<=", _) => version <= lower,
            ("<", _) => version < lower,
            _ => return None,
        };
        all &= holds;
    }
    Some(all)
}

// ---------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct BuildOptions {
    /// `<clones>/<owner>/<repo>` is a local clone of each member that gives
    /// no `path`.
    pub clones: Option<PathBuf>,
    /// `<maps>/<owner>/<repo>/<commit>.json` is a stored map of a member;
    /// a member with none has its map built from its worktree.
    pub maps: Option<PathBuf>,
}

#[derive(Debug)]
pub enum OrgError {
    /// Exit 2: an invalid manifest.
    Manifest(ManifestError),
    /// Exit 2: a missing clone, commit or flag, or an unreadable map.
    Input(String),
    /// Exit 3: anything else.
    Internal(anyhow::Error),
}

impl OrgError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Manifest(_) | Self::Input(_) => 2,
            Self::Internal(_) => 3,
        }
    }
}

impl std::fmt::Display for OrgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Manifest(error) => write!(f, "{error}"),
            Self::Input(message) => write!(f, "{message}"),
            Self::Internal(error) => write!(f, "internal error: {error:#}"),
        }
    }
}

impl From<CheckError> for OrgError {
    fn from(error: CheckError) -> Self {
        match error {
            CheckError::Input(message) => Self::Input(message),
            CheckError::Internal(error) => Self::Internal(error),
        }
    }
}

pub fn read_manifest(path: &Path) -> Result<OrganizationManifest, OrgError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| OrgError::Input(format!("read {}: {error}", path.display())))?;
    OrganizationManifest::parse(&text).map_err(OrgError::Manifest)
}

fn git_succeeds(dir: &Path, args: &[&str]) -> bool {
    check::git_output(dir, args).is_ok_and(|output| output.status.success())
}

fn tag_commit(clone: &Path, tag: &str) -> Option<String> {
    let spec = format!("refs/tags/{tag}^{{commit}}");
    let output = check::git_output(clone, &["rev-parse", "--verify", "--quiet", &spec]).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// A member map built as `tolmap build` builds one with its defaults and
/// the offline IDF namer, from the member's worktree.
fn build_member_map(dir: &Path, name: &str, out: &Path) -> Result<Vec<u8>, OrgError> {
    let (pkg, language) = check::detect_source(dir)?;
    let path = geometry::build(
        dir,
        &pkg,
        language.as_str(),
        Some(name),
        out,
        check::RESOLUTION,
        BuildFeatures {
            parcels: true,
            prune_variant: PruneVariant::default(),
            namer: NamerKind::Idf,
            namer_model: naming::DEFAULT_MODEL.to_owned(),
            refs: RefsMode::default(),
            install: InstallMode::Off,
        },
    )
    .map_err(|error| OrgError::Internal(error.context(format!("build the map of {name}"))))?;
    std::fs::read(&path)
        .map_err(|error| OrgError::Internal(anyhow::anyhow!("read {}: {error}", path.display())))
}

/// Builds the organization document for the manifest at `manifest_path`.
/// The same manifest and member commits give the same document, byte for
/// byte; stored member maps are only read.
pub fn build(
    manifest_path: &Path,
    options: &BuildOptions,
) -> Result<OrganizationDocument, OrgError> {
    let manifest = read_manifest(manifest_path)?;
    let base = manifest_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut members = manifest.members.clone();
    members.sort_by_key(|member| member.repo.to_ascii_lowercase());
    // Scratch worktrees live until the document is built, then are removed.
    let mut scratches = Vec::new();
    let mut clones = Vec::new();
    let mut summaries = Vec::new();
    let mut trees = Vec::new();
    for member in &members {
        let (owner, repo) = member.owner_repo();
        let clone = match (&member.path, &options.clones) {
            (Some(path), _) => base.join(path),
            (None, Some(clones)) => clones.join(owner).join(repo),
            (None, None) => {
                return Err(OrgError::Input(format!(
                    "member {} gives no path; pass --clones",
                    member.repo
                )))
            }
        };
        if !clone.is_dir() {
            return Err(OrgError::Input(format!(
                "member {}: no clone at {}",
                member.repo,
                clone.display()
            )));
        }
        let object = format!("{}^{{commit}}", member.commit);
        if !git_succeeds(&clone, &["cat-file", "-e", &object]) {
            return Err(OrgError::Input(format!(
                "member {}: commit {} is not in the clone at {}",
                member.repo,
                member.commit,
                clone.display()
            )));
        }
        let mut scratch = Scratch::new(&clone)?;
        let tree_dir = scratch.worktree(&member.commit, "tree")?;
        let stored = options.maps.as_ref().map(|maps| {
            maps.join(owner)
                .join(repo)
                .join(format!("{}.json", member.commit))
        });
        let (source, bytes) = match stored.filter(|path| path.is_file()) {
            Some(path) => (
                MapSource::Stored,
                std::fs::read(&path).map_err(|error| {
                    OrgError::Input(format!("read {}: {error}", path.display()))
                })?,
            ),
            None => (
                MapSource::Built,
                build_member_map(&tree_dir, repo, &scratch.root.join("out"))?,
            ),
        };
        let map: MapDocument = serde_json::from_slice(&bytes).map_err(|error| {
            OrgError::Input(format!(
                "the map of {} is not a tolmap map: {error}",
                member.repo
            ))
        })?;
        let tree = deps::scan(&tree_dir);
        summaries.push(OrganizationMemberMap {
            repo: member.repo.clone(),
            commit: member.commit.clone(),
            path: member.path.clone(),
            map: source,
            map_digest: format!("sha256:{:x}", Sha256::digest(&bytes)),
            files: map.files.len(),
            districts: map.districts.len(),
            packages: tree.provided.clone(),
        });
        trees.push(tree);
        clones.push(clone);
        scratches.push(scratch);
    }
    let views = members
        .iter()
        .zip(&trees)
        .map(|(member, tree)| MemberTree {
            repo: &member.repo,
            commit: &member.commit,
            tree,
        })
        .collect::<Vec<_>>();
    let resolution = resolve(&views, &|m, tag| tag_commit(&clones[m], tag));
    drop(scratches);
    let placement = placement(&summaries, &resolution.edges);
    Ok(OrganizationDocument {
        version: DOCUMENT_VERSION,
        name: manifest.name,
        members: summaries,
        placement,
        edges: resolution.edges,
        pin_skew: resolution.pin_skew,
        external: resolution.external,
        unresolved: resolution.unresolved,
    })
}

/// The document as written: pretty JSON with a trailing newline.
pub fn render(document: &OrganizationDocument) -> String {
    let mut text = serde_json::to_string_pretty(document).expect("a document always serialises");
    text.push('\n');
    text
}

/// `tolmap org check`: 0 when the manifest is valid, 2 otherwise.
pub fn run_check(manifest: &Path) -> i32 {
    match read_manifest(manifest) {
        Ok(manifest) => {
            println!(
                "organization {}: {} member{}, manifest valid",
                manifest.name,
                manifest.members.len(),
                if manifest.members.len() == 1 { "" } else { "s" }
            );
            0
        }
        Err(error) => {
            eprintln!("tolmap org check: {error}");
            error.exit_code()
        }
    }
}

/// `tolmap org build`: writes the document to `out`, or stdout.
pub fn run_build(manifest: &Path, options: &BuildOptions, out: Option<&Path>) -> i32 {
    let document = match build(manifest, options) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("tolmap org build: {error}");
            return error.exit_code();
        }
    };
    let text = render(&document);
    match out {
        Some(path) => {
            if let Err(error) = std::fs::write(path, text) {
                eprintln!("tolmap org build: write {}: {error}", path.display());
                return 2;
            }
            eprintln!(
                "tolmap org build: {} members, {} repository edges, {} pin skew, {} external, {} unresolved (lower bounds) -> {}",
                document.members.len(),
                document.edges.len(),
                document.pin_skew.len(),
                document.external.len(),
                document.unresolved.len(),
                path.display()
            );
        }
        None => print!("{text}"),
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    fn manifest(members: &str) -> String {
        format!("version = 1\nname = \"acme\"\n{members}")
    }

    fn member(repo: &str, commit: &str) -> String {
        format!("[[members]]\nrepo = \"{repo}\"\ncommit = \"{commit}\"\n")
    }

    #[test]
    fn a_valid_manifest_parses() {
        let text = manifest(&format!(
            "{}{}path = \"../app\"\n",
            member("acme/core", COMMIT),
            member("acme/app", COMMIT)
        ));
        let parsed = OrganizationManifest::parse(&text).unwrap();
        assert_eq!(parsed.members.len(), 2);
        assert_eq!(parsed.members[1].path.as_deref(), Some("../app"));
    }

    #[test]
    fn unknown_keys_duplicates_and_abbreviated_commits_have_distinct_errors() {
        let unknown = manifest(&format!(
            "{}branch = \"main\"\n",
            member("acme/core", COMMIT)
        ));
        assert!(matches!(
            OrganizationManifest::parse(&unknown),
            Err(ManifestError::UnknownKey(_))
        ));
        let top_unknown = format!("colour = 1\n{}", manifest(&member("acme/core", COMMIT)));
        assert!(matches!(
            OrganizationManifest::parse(&top_unknown),
            Err(ManifestError::UnknownKey(_))
        ));
        let duplicate = manifest(&format!(
            "{}{}",
            member("acme/core", COMMIT),
            member("Acme/Core", COMMIT)
        ));
        assert_eq!(
            OrganizationManifest::parse(&duplicate),
            Err(ManifestError::DuplicateMember("Acme/Core".to_owned()))
        );
        let abbreviated = manifest(&member("acme/core", "0123456"));
        assert_eq!(
            OrganizationManifest::parse(&abbreviated),
            Err(ManifestError::AbbreviatedCommit {
                repo: "acme/core".to_owned(),
                commit: "0123456".to_owned()
            })
        );
        assert!(matches!(
            OrganizationManifest::parse(&manifest(&member("acme/core", "main"))),
            Err(ManifestError::InvalidCommit { .. })
        ));
        assert!(matches!(
            OrganizationManifest::parse(&manifest(&member("acme/core", &"a".repeat(64)))),
            Err(ManifestError::InvalidCommit { .. })
        ));
        assert_eq!(
            OrganizationManifest::parse(&manifest(&member("acme", COMMIT))),
            Err(ManifestError::InvalidRepo("acme".to_owned()))
        );
        let absolute = manifest(&format!(
            "{}path = \"/home/x\"\n",
            member("acme/core", COMMIT)
        ));
        assert!(matches!(
            OrganizationManifest::parse(&absolute),
            Err(ManifestError::InvalidPath { .. })
        ));
        assert_eq!(
            OrganizationManifest::parse("version = 1\nname = \"acme\"\nmembers = []\n"),
            Err(ManifestError::NoMembers)
        );
    }

    #[test]
    fn version_requirements() {
        use Ecosystem::*;
        let cases = [
            ("1.2", "1.4.0", Cargo, Some(true)),
            ("1.2", "2.0.0", Cargo, Some(false)),
            ("0.2.1", "0.3.0", Cargo, Some(false)),
            ("=0.2.0", "0.2.0", Cargo, Some(true)),
            (">=0.1, <0.3", "0.2.9", Cargo, Some(true)),
            ("~1.2", "1.3.0", Cargo, Some(false)),
            ("^1.2.0", "1.4.0", Npm, Some(true)),
            ("1.3.0", "1.4.0", Npm, Some(false)),
            (">=1.0.0 <1.4.0", "1.4.0", Npm, Some(false)),
            ("^1 || ^2", "1.0.0", Npm, None),
            (">=0.3", "0.3.0", Python, Some(true)),
            ("==0.2", "0.2.0", Python, Some(true)),
            ("==0.2", "0.2.1", Python, Some(false)),
            ("~=1.4.5", "1.5.0", Python, Some(false)),
            ("~=2.2", "2.9", Python, Some(true)),
            ("==1.*", "1.0.0", Python, None),
            ("^1.0.0", "1.0.0-beta.1", Npm, None),
        ];
        for (requirement, version, ecosystem, expected) in cases {
            assert_eq!(
                satisfies(requirement, version, ecosystem),
                expected,
                "{requirement} vs {version} ({ecosystem:?})"
            );
        }
    }

    #[test]
    fn go_pseudo_versions() {
        assert_eq!(
            go_pseudo_version_hash("v0.0.0-20240101000000-0123456789ab"),
            Some("0123456789ab")
        );
        assert_eq!(
            go_pseudo_version_hash("v1.2.4-0.20240101000000-0123456789ab"),
            Some("0123456789ab")
        );
        assert_eq!(go_pseudo_version_hash("v1.2.3"), None);
        assert_eq!(go_pseudo_version_hash("v1.2.3-rc.1"), None);
    }

    fn provided(ecosystem: Ecosystem, name: &str, version: Option<&str>) -> ProvidedPackage {
        ProvidedPackage {
            ecosystem,
            name: name.to_owned(),
            file: "Cargo.toml".to_owned(),
            version: version.map(str::to_owned),
        }
    }

    fn declared(name: &str, repo: Option<&str>, kind: PinKind, value: &str) -> deps::Declared {
        deps::Declared {
            ecosystem: Ecosystem::Cargo,
            consumer_package: "app".to_owned(),
            file: "Cargo.toml".to_owned(),
            section: "dependencies".to_owned(),
            name: name.to_owned(),
            repo: repo.map(str::to_owned),
            pin: Pin {
                kind,
                value: value.to_owned(),
            },
        }
    }

    fn member_map(repo: &str, files: usize) -> OrganizationMemberMap {
        OrganizationMemberMap {
            repo: repo.to_owned(),
            commit: COMMIT.to_owned(),
            path: None,
            map: MapSource::Stored,
            map_digest: "sha256:fixture".to_owned(),
            files,
            districts: 1,
            packages: Vec::new(),
        }
    }

    fn edge(consumer: &str, provider: &str, evidence: usize) -> RepositoryEdge {
        RepositoryEdge {
            consumer: consumer.to_owned(),
            provider: provider.to_owned(),
            evidence: (0..evidence)
                .map(|_| DependencyEvidence {
                    ecosystem: Ecosystem::Cargo,
                    consumer_package: "consumer".to_owned(),
                    file: "Cargo.toml".to_owned(),
                    section: "dependencies".to_owned(),
                    provider_package: "provider".to_owned(),
                    pin: Pin {
                        kind: PinKind::Version,
                        value: "1".to_owned(),
                    },
                    status: PinStatus::Match,
                    reason: None,
                })
                .collect(),
        }
    }

    fn distance(left: [f64; 2], right: [f64; 2]) -> f64 {
        let dx = right[0] - left[0];
        let dy = right[1] - left[1];
        (dx * dx + dy * dy).sqrt()
    }

    fn assert_regions_do_not_overlap(placement: &OrganizationPlacement) {
        for (left, region) in placement.regions.iter().enumerate() {
            for other in placement.regions.iter().skip(left + 1) {
                assert!(
                    distance(region.translation, other.translation)
                        >= region_radius(region.scale)
                            + region_radius(other.scale)
                            + REGION_SEPARATION,
                    "{} overlaps {}",
                    region.repo,
                    other.repo
                );
            }
        }
    }

    #[test]
    fn placement_is_byte_identical_keeps_linked_repositories_near_and_separate() {
        let members = vec![
            member_map("acme/app", 1),
            member_map("acme/core", 1),
            member_map("acme/extra", 1),
        ];
        let edges = vec![edge("acme/app", "acme/core", 2)];
        let first = placement(&members, &edges);
        let second = placement(&members, &edges);
        assert_eq!(first, second);
        assert_eq!(
            first
                .regions
                .iter()
                .map(|region| region.repo.as_str())
                .collect::<Vec<_>>(),
            vec!["acme/app", "acme/core", "acme/extra"]
        );
        assert!(first.regions.iter().all(|region| region.scale == 1.0));
        assert_eq!(first.roads.len(), 1);
        assert_eq!(first.roads[0].from, first.regions[0].translation);
        assert_eq!(first.roads[0].to, first.regions[1].translation);
        let app = first.regions[0].translation;
        let core = first.regions[1].translation;
        let extra = first.regions[2].translation;
        assert!(distance(app, core) < distance(app, extra));
        assert!(distance(app, core) < distance(core, extra));
        assert_regions_do_not_overlap(&first);
    }

    #[test]
    fn placement_caps_high_evidence_steps_and_separates_regions() {
        let members = vec![member_map("acme/app", 1), member_map("acme/core", 1)];
        let placement = placement(&members, &[edge("acme/app", "acme/core", 200)]);
        assert!(placement
            .regions
            .iter()
            .flat_map(|region| region.translation)
            .all(f64::is_finite));
        assert_regions_do_not_overlap(&placement);
    }

    #[test]
    fn resolution_lists_edges_skew_external_internal_and_ambiguous() {
        let other = "fedcba9876543210fedcba9876543210fedcba98";
        let core = deps::Tree {
            provided: vec![
                provided(Ecosystem::Cargo, "acme-core", Some("0.2.0")),
                provided(Ecosystem::Cargo, "shared", None),
            ],
            ..Default::default()
        };
        let extra = deps::Tree {
            provided: vec![provided(Ecosystem::Cargo, "shared", None)],
            ..Default::default()
        };
        let app = deps::Tree {
            provided: vec![provided(Ecosystem::Cargo, "app", None)],
            declared: vec![
                declared("acme-core", Some("acme/core"), PinKind::Commit, other),
                declared("acme_core", None, PinKind::Version, "0.2"),
                declared("app", None, PinKind::Path, "."),
                declared("serde", None, PinKind::Version, "1"),
                declared("shared", None, PinKind::Version, "1"),
            ],
            ..Default::default()
        };
        let members = [
            MemberTree {
                repo: "acme/app",
                commit: COMMIT,
                tree: &app,
            },
            MemberTree {
                repo: "acme/core",
                commit: COMMIT,
                tree: &core,
            },
            MemberTree {
                repo: "acme/extra",
                commit: COMMIT,
                tree: &extra,
            },
        ];
        let result = resolve(&members, &|_, _| None);
        assert_eq!(result.edges.len(), 1);
        let edge = &result.edges[0];
        assert_eq!(
            (edge.consumer.as_str(), edge.provider.as_str()),
            ("acme/app", "acme/core")
        );
        let statuses = edge.evidence.iter().map(|e| e.status).collect::<Vec<_>>();
        assert_eq!(statuses, vec![PinStatus::Skew, PinStatus::Match]);
        assert_eq!(result.pin_skew.len(), 1);
        assert_eq!(result.pin_skew[0].pin.value, other);
        assert_eq!(result.pin_skew[0].member_commit, COMMIT);
        let external = result
            .external
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(external, vec!["serde"]);
        assert_eq!(result.unresolved.len(), 1);
        assert!(result.unresolved[0]
            .reason
            .contains("acme/core, acme/extra"));
    }
}
