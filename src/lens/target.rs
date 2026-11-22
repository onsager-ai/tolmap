use super::{check_spec, Cx, LensError, Unresolved, UnresolvedKind};
use crate::extract::round_to;
use crate::schema::{CheckTarget, GraphData};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use ts_rs::TS;

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LensTargetArchitecture {
    pub owners: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub allow_only: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub forbid: Vec<TargetForbid>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetForbid {
    pub from: String,
    pub to: String,
    pub reason: String,
}

impl LensTargetArchitecture {
    pub(super) fn validate(&self) -> Result<(), LensError> {
        let bad = |detail| LensError::Invalid(format!("target: {detail}"));
        if self.owners.is_empty() {
            return Err(bad("at least one owner is required".into()));
        }
        for (name, members) in &self.owners {
            if name.trim().is_empty() || members.is_empty() {
                return Err(bad(format!("owner {name:?} needs a name and members")));
            }
            for spec in members {
                check_spec(spec, true).map_err(|e| bad(format!("owner {name:?}: {e}")))?;
            }
        }
        let check_owner = |name: &str| {
            if self.owners.contains_key(name) {
                Ok(())
            } else {
                Err(bad(format!("unknown owner {name:?}")))
            }
        };
        let mut forbidden = BTreeSet::new();
        for rule in &self.forbid {
            check_owner(&rule.from)?;
            check_owner(&rule.to)?;
            if rule.from == rule.to || !forbidden.insert((&rule.from, &rule.to)) {
                return Err(bad(format!(
                    "forbid {:?} -> {:?} must name distinct owners and appear once",
                    rule.from, rule.to
                )));
            }
            if rule.reason.trim().is_empty() {
                return Err(bad("each forbid rule requires a reason".into()));
            }
        }
        for (from, allowed) in &self.allow_only {
            check_owner(from)?;
            for to in allowed {
                check_owner(to)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
pub struct TargetDistance {
    pub violating_mass_share: Option<f64>,
    pub misplaced_share: f64,
    pub distance: Option<f64>,
    pub lower_bound: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TargetEdgeKind {
    Import,
    SymbolUse,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TargetViolation {
    Edge {
        from: String,
        to: String,
        from_owner: String,
        to_owner: String,
        edge_kinds: Vec<TargetEdgeKind>,
        forbid_reason: Option<String>,
        outside_allow_only: bool,
    },
    Misplaced {
        file: String,
        owner: String,
        district: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
pub struct TargetReport {
    pub distance: TargetDistance,
    pub violations: Vec<TargetViolation>,
}

pub(super) fn evaluate(
    target: &LensTargetArchitecture,
    cx: &mut Cx<'_>,
    names: &BTreeMap<String, String>,
    imports: &[[usize; 2]],
    uses: &BTreeMap<String, Vec<usize>>,
    mass: Option<&[(usize, usize, f64)]>,
) -> TargetReport {
    let paths = cx.paths;
    let mut claims = BTreeMap::<&str, (usize, BTreeSet<&str>)>::new();
    for (owner, members) in &target.owners {
        for (index, spec) in members.iter().enumerate() {
            let location = format!("target.owners.{owner}[{index}]");
            for member in cx.resolve_spec(spec, &location, true) {
                claims
                    .entry(paths[member.file].as_str())
                    .or_insert_with(|| (member.file, BTreeSet::new()))
                    .1
                    .insert(owner);
            }
        }
    }
    let mut owners = vec![None; cx.paths.len()];
    let mut violations = Vec::new();
    let mut misplaced = 0;
    let mut owned = 0;
    for (path, (file, claims)) in claims {
        if claims.len() > 1 {
            cx.fail(
                UnresolvedKind::Ambiguous,
                "target.owners",
                path,
                format!(
                    "multiple owners: {}",
                    claims.iter().copied().collect::<Vec<_>>().join(", ")
                ),
            );
            continue;
        }
        let owner = *claims.first().expect("every claimed file has an owner");
        owners[file] = Some(owner);
        owned += 1;
        let district = (cx.district)(file);
        if let Some(district) = names
            .get(&district.to_string())
            .filter(|name| name.as_str() != owner)
        {
            misplaced += 1;
            violations.push(TargetViolation::Misplaced {
                file: path.to_owned(),
                owner: owner.to_owned(),
                district: district.clone(),
            });
        }
    }
    let cross_area = |a: usize, b: usize| {
        let (Some(&Some(from)), Some(&Some(to))) = (owners.get(a), owners.get(b)) else {
            return None;
        };
        (from != to).then_some((from, to))
    };
    let forbidden = target
        .forbid
        .iter()
        .map(|rule| ((rule.from.as_str(), rule.to.as_str()), rule.reason.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut directed = BTreeMap::<(usize, usize), BTreeSet<TargetEdgeKind>>::new();
    for &[a, b] in imports {
        if cross_area(a, b).is_some() {
            directed
                .entry((a, b))
                .or_default()
                .insert(TargetEdgeKind::Import);
        }
    }
    for (destination, users) in uses {
        let Some(b) = destination
            .split_once(':')
            .and_then(|(file, _)| file.parse().ok())
        else {
            continue;
        };
        for &a in users {
            if cross_area(a, b).is_some() {
                directed
                    .entry((a, b))
                    .or_default()
                    .insert(TargetEdgeKind::SymbolUse);
            }
        }
    }
    let mut violating_pairs = BTreeSet::new();
    for ((a, b), kinds) in directed {
        let (from_owner, to_owner) =
            cross_area(a, b).expect("only cross-area dependencies were collected");
        let reason = forbidden.get(&(from_owner, to_owner));
        let outside_allow_only = target
            .allow_only
            .get(from_owner)
            .is_some_and(|allowed| !allowed.iter().any(|owner| owner == to_owner));
        if reason.is_some() || outside_allow_only {
            violating_pairs.insert((a.min(b), a.max(b)));
            violations.push(TargetViolation::Edge {
                from: cx.paths[a].clone(),
                to: cx.paths[b].clone(),
                from_owner: from_owner.to_owned(),
                to_owner: to_owner.to_owned(),
                edge_kinds: kinds.into_iter().collect(),
                forbid_reason: reason.map(|reason| (*reason).to_owned()),
                outside_allow_only,
            });
        }
    }
    violations.sort();
    let ratio = |num, den| if den > 0.0 { num / den } else { 0.0 };
    let violating_mass_share = mass.map(|rows| {
        let mut cross = 0.0;
        let mut violating = 0.0;
        for &(a, b, weight) in rows {
            if cross_area(a, b).is_some() {
                cross += weight;
                if violating_pairs.contains(&(a.min(b), a.max(b))) {
                    violating += weight;
                }
            }
        }
        ratio(violating, cross)
    });
    let misplaced_share = ratio(misplaced as f64, owned as f64);
    TargetReport {
        distance: TargetDistance {
            violating_mass_share: violating_mass_share.map(|share| round_to(share, 6)),
            misplaced_share: round_to(misplaced_share, 6),
            distance: violating_mass_share.map(|share| round_to(share + misplaced_share, 6)),
            lower_bound: true,
        },
        violations,
    }
}

pub(crate) fn resolve_graph_target(
    target: &LensTargetArchitecture,
    graph: &GraphData,
    membership: &BTreeMap<&str, usize>,
    names: &BTreeMap<String, String>,
) -> anyhow::Result<(TargetReport, Vec<Unresolved>)> {
    let paths = graph
        .nodes
        .iter()
        .map(|node| node.file.clone())
        .collect::<Vec<_>>();
    let symbols = paths
        .iter()
        .enumerate()
        .filter_map(|(file, path)| {
            graph
                .symbols
                .get(path)
                .map(|rows| (file.to_string(), rows.clone()))
        })
        .collect();
    let mut cx = Cx {
        paths: &paths,
        symbols: &symbols,
        district: &|file| {
            membership
                .get(paths[file].as_str())
                .copied()
                .unwrap_or(usize::MAX)
        },
        files: paths
            .iter()
            .enumerate()
            .map(|(file, path)| (path.as_str(), file))
            .collect(),
        unresolved: Vec::new(),
    };
    let imports = graph
        .imports
        .iter()
        .map(|&(a, b, _)| [a as usize, b as usize])
        .collect::<Vec<_>>();
    let uses = crate::geometry::compact_uses(graph, &cx.files);
    let mass = crate::pipeline::blended_edge_mass(graph)?;
    let report = evaluate(target, &mut cx, names, &imports, &uses, Some(&mass));
    Ok((report, cx.unresolved))
}

pub(crate) fn compare_target(
    (base, unresolved_base): (TargetReport, Vec<Unresolved>),
    (head, unresolved_head): (TargetReport, Vec<Unresolved>),
    renamed_from: &BTreeMap<&str, &str>,
) -> CheckTarget {
    // A pair is still the same violation if the evidence changes from an
    // import to a symbol use. A recognized rename keeps its base identity.
    fn key(
        violation: &TargetViolation,
        identity: impl Fn(&str) -> String,
    ) -> (String, String, String, String) {
        match violation {
            TargetViolation::Edge {
                from,
                to,
                from_owner,
                to_owner,
                ..
            } => (
                identity(from),
                identity(to),
                from_owner.clone(),
                to_owner.clone(),
            ),
            TargetViolation::Misplaced {
                file,
                owner,
                district,
            } => (
                identity(file),
                String::new(),
                owner.clone(),
                district.clone(),
            ),
        }
    }
    let base_keys = base
        .violations
        .iter()
        .map(|violation| key(violation, str::to_owned))
        .collect::<BTreeSet<_>>();
    let head_key = |violation: &TargetViolation| {
        key(violation, |path| {
            renamed_from.get(path).copied().unwrap_or(path).to_owned()
        })
    };
    let head_keys = head
        .violations
        .iter()
        .map(head_key)
        .collect::<BTreeSet<_>>();
    CheckTarget {
        base: base.distance,
        head: head.distance,
        violations_added: head
            .violations
            .into_iter()
            .filter(|v| !base_keys.contains(&head_key(v)))
            .collect(),
        violations_removed: base
            .violations
            .into_iter()
            .filter(|v| !head_keys.contains(&key(v, str::to_owned)))
            .collect(),
        unresolved_base,
        unresolved_head,
    }
}

pub(crate) fn render_target_distance(out: &mut String, label: &str, distance: &TargetDistance) {
    let value =
        |share: Option<f64>| share.map_or_else(|| "unavailable".to_owned(), |n| format!("{n:.6}"));
    let _ = writeln!(
        out,
        "{label}: {} (violating mass {}, misplaced {:.6}; lower bounds)",
        value(distance.distance),
        value(distance.violating_mass_share),
        distance.misplaced_share
    );
}

pub(crate) fn render_target_violation(out: &mut String, prefix: &str, violation: &TargetViolation) {
    match violation {
        TargetViolation::Edge {
            from,
            to,
            from_owner,
            to_owner,
            edge_kinds,
            forbid_reason,
            outside_allow_only,
        } => {
            let kinds = edge_kinds
                .iter()
                .map(|kind| match kind {
                    TargetEdgeKind::Import => "import",
                    TargetEdgeKind::SymbolUse => "symbol use",
                })
                .collect::<Vec<_>>()
                .join(", ");
            let _ = write!(
                out,
                "  {prefix} {from} -> {to} ({kinds}; {from_owner} -> {to_owner})"
            );
            if let Some(reason) = forbid_reason {
                let _ = write!(out, ": {reason}");
            }
            if *outside_allow_only {
                out.push_str(" [outside allow_only]");
            }
            out.push('\n');
        }
        TargetViolation::Misplaced {
            file,
            owner,
            district,
        } => {
            let _ = writeln!(out, "  {prefix} {file} in {district:?}, owned by {owner:?}");
        }
    }
}
