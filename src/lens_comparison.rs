//! Lens deltas on the immutable base partition. Neither side supplies layout.
use crate::{lens::*, schema::MapDocument};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use ts_rs::TS;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LensChange {
    Shared,
    Added,
    Removed,
    Broken,
    Renamed,
    Moved,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
pub struct LensDelta {
    pub kind: String,
    pub label: String,
    pub status: LensChange,
    pub before: Vec<String>,
    pub after: Vec<String>,
    // Only base-map indices/geometry may reach the renderer. New files have
    // district placement only, and never borrow another file's footprint.
    pub targets: Vec<LensTarget>,
    pub districts: Vec<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct LensComparison {
    pub version: u32,
    pub repository: String,
    pub base: String,
    pub head: String,
    pub lens: String,
    pub other: String,
    pub base_map_sha256: String,
    pub lower_bound: bool,
    pub before: LensReport,
    pub after: LensReport,
    pub changes: Vec<LensDelta>,
}

struct Item {
    label: String,
    specs: Vec<String>,
    targets: Vec<LensTarget>,
    resolved: bool,
    order: usize,
    route: Vec<String>,
}

fn identity(spec: &str, renames: &[FileRename]) -> String {
    let (path, symbol) = spec
        .split_once('#')
        .map_or((spec, None), |(p, s)| (p, Some(s)));
    let path = renames
        .iter()
        .find(|r| r.to == path)
        .map_or(path, |r| r.from.as_str());
    // Start lines move as code is edited. The symbol name is the stable
    // comparison identity; repeated names retain their line disambiguator.
    match symbol {
        Some(symbol) => format!("{path}#{symbol}"),
        None => path.to_owned(),
    }
}

fn items(report: &LensReport, renames: &[FileRename]) -> BTreeMap<(String, String), Item> {
    let mut result = BTreeMap::new();
    for area in &report.areas {
        for member in &area.members {
            let target = &member.target;
            let spec = target.symbol.as_ref().map_or_else(
                || target.path.clone(),
                |symbol| format!("{}#{}", target.path, symbol.name),
            );
            result.insert(
                ("area".into(), identity(&spec, renames)),
                Item {
                    label: area.label.clone(),
                    specs: vec![spec],
                    targets: vec![target.clone()],
                    resolved: true,
                    order: 0,
                    route: Vec::new(),
                },
            );
        }
    }
    for path in &report.paths {
        for (i, stop) in path.stops.iter().enumerate() {
            // Stop/hop identity is independent of path names, allowing two
            // lenses to share a stop or hop while naming their paths differently.
            let key = ("stop".into(), identity(&stop.spec, renames));
            result.entry(key).or_insert_with(|| Item {
                label: path.name.clone(),
                specs: vec![stop.spec.clone()],
                targets: stop.target.iter().cloned().collect(),
                resolved: stop.target.is_some(),
                order: i,
                route: Vec::new(),
            });
        }
        for hop in &path.hops {
            let a = &path.stops[hop.from];
            let b = &path.stops[hop.to];
            let key = (
                "hop".into(),
                format!(
                    "{} → {}",
                    identity(&a.spec, renames),
                    identity(&b.spec, renames)
                ),
            );
            result.entry(key).or_insert_with(|| Item {
                label: path.name.clone(),
                specs: vec![a.spec.clone(), b.spec.clone()],
                targets: a.target.iter().chain(b.target.iter()).cloned().collect(),
                resolved: hop.resolved,
                order: hop.from,
                route: std::iter::once(format!(
                    "{:?}:{:?}:{:?}",
                    hop.source, hop.edge_kind, hop.reason
                ))
                .chain(hop.via.iter().map(|path| identity(path, renames)))
                .collect(),
            });
        }
    }
    result
}

// Pure, sorted output: repeated calls give byte-identical reports and leave
// the supplied map untouched. Head targets are projected by path/rename only.
pub fn changes(
    before: &LensReport,
    after: &LensReport,
    frame: &MapDocument,
    renames: &[FileRename],
) -> Vec<LensDelta> {
    let left = items(before, &[]);
    let right = items(after, renames);
    let keys: BTreeSet<_> = left.keys().chain(right.keys()).cloned().collect();
    keys.into_iter()
        .map(|key| {
            let a = left.get(&key);
            let b = right.get(&key);
            let status =
                match (a, b) {
                    (Some(_), Some(b))
                        if key.0 == "stop"
                            && b.specs.iter().any(|spec| {
                                after.unresolved.iter().any(|u| {
                                    u.spec == *spec && u.kind == UnresolvedKind::RenamedFile
                                })
                            }) =>
                    {
                        LensChange::Renamed
                    }
                    (Some(a), Some(b)) if !b.resolved && a.resolved => LensChange::Broken,
                    (Some(a), Some(b)) if a.specs != b.specs => LensChange::Renamed,
                    (Some(a), Some(b))
                        if a.order != b.order
                            || a.route != b.route
                            || a.targets
                                .iter()
                                .map(|t| &t.symbol)
                                .ne(b.targets.iter().map(|t| &t.symbol)) =>
                    {
                        LensChange::Moved
                    }
                    (Some(_), Some(_)) => LensChange::Shared,
                    (Some(a), None)
                        if key.0 == "area"
                            && a.specs.iter().any(|spec| {
                                after.unresolved.iter().any(|u| {
                                    u.spec == *spec && u.kind == UnresolvedKind::RenamedFile
                                })
                            }) =>
                    {
                        LensChange::Renamed
                    }
                    (Some(_), None) => LensChange::Removed,
                    (None, Some(b)) if !b.resolved => LensChange::Broken,
                    (None, Some(_)) => LensChange::Added,
                    _ => unreachable!(),
                };
            let item = b.or(a).unwrap();
            let mut targets = Vec::new();
            let mut districts = BTreeSet::new();
            for target in a
                .into_iter()
                .flat_map(|item| &item.targets)
                .chain(b.into_iter().flat_map(|item| &item.targets))
            {
                let base_path = renames
                    .iter()
                    .find(|r| r.to == target.path)
                    .map_or(target.path.as_str(), |r| r.from.as_str());
                if let Some(file) = frame.files.iter().position(|path| path == base_path) {
                    let district = frame.nodes[file].district();
                    districts.insert(district);
                    let projected = LensTarget {
                        file,
                        path: frame.files[file].clone(),
                        district,
                        symbol: target.symbol.as_ref().and_then(|symbol| {
                            frame
                                .symbols
                                .get(&file.to_string())
                                .and_then(|rows| rows.iter().find(|row| row.0 .0 == symbol.name))
                                .map(|row| SymbolTarget {
                                    name: row.0 .0.clone(),
                                    kind: row.0 .1,
                                    start: row.0 .2,
                                    end: row.0 .3,
                                })
                        }),
                    };
                    if !targets.contains(&projected) {
                        targets.push(projected);
                    }
                } else if frame.districts.contains_key(&target.district.to_string()) {
                    districts.insert(target.district);
                }
            }
            if key.0 == "hop" {
                // Keep endpoint order, including an identical endpoint for a
                // same-file hop. Mixing old/new endpoint sets would connect
                // the wrong pair when one endpoint is a newly added file.
                let endpoints = a
                    .filter(|item| item.targets.len() == 2)
                    .or_else(|| b.filter(|item| item.targets.len() == 2));
                if let Some(endpoints) = endpoints {
                    targets = endpoints
                        .targets
                        .iter()
                        .filter_map(|target| {
                            let path = renames
                                .iter()
                                .find(|r| r.to == target.path)
                                .map_or(target.path.as_str(), |r| r.from.as_str());
                            let file = frame.files.iter().position(|p| p == path)?;
                            Some(LensTarget {
                                file,
                                path: frame.files[file].clone(),
                                district: frame.nodes[file].district(),
                                symbol: target.symbol.clone(),
                            })
                        })
                        .collect();
                }
            }
            // A broken hop retains its last known base endpoints, so deletion
            // remains visible instead of dropping the route silently.
            LensDelta {
                kind: key.0,
                label: item.label.clone(),
                status,
                before: a.map_or_else(Vec::new, |item| item.specs.clone()),
                after: b.map_or_else(Vec::new, |item| item.specs.clone()),
                targets,
                districts: districts.into_iter().collect(),
            }
        })
        .collect()
}
