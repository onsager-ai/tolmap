//! Project generated files onto authored files before mass normalization.
use std::collections::{BTreeMap, BTreeSet};

use crate::schema::{CochangeSignal, FileId, GraphData, SignalEdge};

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct GeneratedCoverage {
    pub collapsed: usize,
    pub excluded: usize,
}

pub struct Projection {
    pub coverage: GeneratedCoverage,
    /// Original indices of surviving nodes, for an already aligned warm start.
    pub retained: Vec<usize>,
}

/// Only the extractor's unique ts-rs match earns a redirect. Unmatched
/// generated files disappear, along with their incident evidence. Authored
/// metrics and symbols stay on their original files; generated LOC, churn and
/// complexity must not inflate the generator's metrics.
pub fn collapse(data: &mut GraphData) -> Projection {
    let generated = data.generated.as_deref().unwrap_or_default();
    let mut projection = Projection {
        coverage: GeneratedCoverage::default(),
        retained: (0..data.nodes.len()).collect(),
    };
    if generated.is_empty() {
        return projection;
    }
    let generated_ids = generated
        .iter()
        .map(|row| row.file)
        .collect::<BTreeSet<_>>();
    projection
        .retained
        .retain(|id| !generated_ids.contains(&(*id as FileId)));
    let mut targets = vec![None; data.nodes.len()];
    for (new, &old) in projection.retained.iter().enumerate() {
        targets[old] = Some(new as FileId);
    }
    for row in generated {
        // A generator must itself be authored. In particular, never follow
        // chains or cycles of generated files from externally supplied data.
        let target = row.generator.and_then(|id| {
            (!generated_ids.contains(&id))
                .then(|| targets.get(id as usize).copied().flatten())
                .flatten()
        });
        targets[row.file as usize] = target;
        if target.is_some() {
            projection.coverage.collapsed += 1;
        } else {
            projection.coverage.excluded += 1;
        }
    }
    let pair = |a: FileId, b: FileId| -> Option<(FileId, FileId)> {
        let (a, b) = (targets[a as usize]?, targets[b as usize]?);
        (a != b).then_some((a, b))
    };
    let mut imports = BTreeMap::<(FileId, FileId), f64>::new();
    for (a, b, weight) in std::mem::take(&mut data.imports) {
        if let Some(pair) = pair(a, b) {
            *imports.entry(pair).or_default() += weight;
        }
    }
    data.imports = imports.into_iter().map(|((a, b), w)| (a, b, w)).collect();
    data.uses = std::mem::take(&mut data.uses)
        .into_iter()
        .filter_map(|(a, b, name)| pair(a, b).map(|(a, b)| (a, b, name)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if let Some(cochange) = data.cochange.as_mut() {
        let mut remapped = BTreeMap::<(FileId, FileId), CochangeSignal>::new();
        for ((a, b), signal) in std::mem::take(cochange) {
            if let Some((a, b)) = pair(a, b) {
                let entry = remapped.entry((a.min(b), a.max(b))).or_insert(signal);
                // Commit sets can overlap. Adding counts would invent
                // co-changes; the strongest observed pair is a lower bound.
                entry.count = entry.count.max(signal.count);
                entry.weight = entry.weight.max(signal.weight);
            }
        }
        *cochange = remapped;
    }
    if let Some(entries) = data.build_entries.as_mut() {
        *entries = entries
            .iter()
            .filter_map(|&id| targets[id as usize])
            .collect();
    }
    if let Some(tree) = data.rust_mod.as_mut() {
        tree.roots = tree
            .roots
            .iter()
            .filter_map(|&id| targets[id as usize])
            .collect();
        tree.pairs = tree.pairs.iter().filter_map(|&(a, b)| pair(a, b)).collect();
        tree.descendant_fanin.clear();
        let mut parents = BTreeMap::<FileId, Vec<FileId>>::new();
        for &(parent, child) in &tree.pairs {
            parents.entry(child).or_default().push(parent);
        }
        for &(source, target, weight) in &data.imports {
            let mut ancestors = BTreeSet::new();
            let mut stack = parents.get(&source).cloned().unwrap_or_default();
            while let Some(parent) = stack.pop() {
                if ancestors.insert(parent) {
                    stack.extend(parents.get(&parent).into_iter().flatten().copied());
                }
            }
            if ancestors.contains(&target) {
                *tree.descendant_fanin.entry(target).or_default() += weight;
            }
        }
    }
    let paths = data
        .nodes
        .iter()
        .map(|node| node.file.clone())
        .collect::<Vec<_>>();
    let mut redirects = BTreeMap::new();
    for (old, target) in targets.iter().enumerate() {
        redirects.insert(
            paths[old].clone(),
            target.map(|id| paths[projection.retained[id as usize]].clone()),
        );
    }
    let mut edges = BTreeMap::<(String, String), SignalEdge>::new();
    for mut edge in std::mem::take(&mut data.edges) {
        let (Some(a), Some(b)) = (&redirects[&edge.a], &redirects[&edge.b]) else {
            continue;
        };
        if a == b {
            continue;
        }
        let key = (a.min(b).clone(), a.max(b).clone());
        edge.a = key.0.clone();
        edge.b = key.1.clone();
        edges
            .entry(key)
            .and_modify(|sum| {
                // Preserve each signal's mass before global normalization.
                sum.static_signal += edge.static_signal;
                sum.cochange += edge.cochange;
                sum.proximity += edge.proximity;
                sum.semantic += edge.semantic;
                sum.weight += edge.weight;
            })
            .or_insert(edge);
    }
    data.edges = edges.into_values().collect();
    data.nodes = std::mem::take(&mut data.nodes)
        .into_iter()
        .enumerate()
        .filter_map(|(id, node)| (!generated_ids.contains(&(id as FileId))).then_some(node))
        .collect();
    for node in &mut data.nodes {
        node.fanin = 0.0;
    }
    for &(_, target, weight) in &data.imports {
        data.nodes[target as usize].fanin += weight;
    }
    data.symbols
        .retain(|path, _| redirects[path].as_ref() == Some(path));
    data.entry_points.retain(|path, _| {
        redirects
            .get(path)
            .is_some_and(|target| target.as_ref() == Some(path))
    });
    // Some(empty) retains the fact that generated-file detection ran; removed
    // file indices must never escape into compact debt evidence.
    data.generated = Some(Vec::new());
    projection
}
