//! Second-level communities. This module only reads the kept weighted graph;
//! it never mutates district membership or its layout.

use std::collections::BTreeMap;

use anyhow::Result;

use crate::partition::Partitioner;
use crate::pipeline::PipelineOutput;
use crate::schema::{WeightedEdge, WeightedGraph};
use crate::SEED;

const TARGET: usize = 30;
const SINGLE_LIMIT: usize = 40;

#[derive(Clone, Debug)]
pub struct Partition {
    pub groups: BTreeMap<String, (usize, Vec<usize>, String)>,
    pub file_ids: Vec<String>,
}

pub fn partition<P: Partitioner>(layout: &PipelineOutput, partitioner: &P) -> Result<Partition> {
    let mut adjacency = vec![Vec::<(usize, f64)>::new(); layout.membership.len()];
    for edge in &layout.graph.edges {
        adjacency[edge.a].push((edge.b, edge.weight));
        adjacency[edge.b].push((edge.a, edge.weight));
    }
    for row in &mut adjacency {
        row.sort_by_key(|&(node, _)| node);
    }
    let mut districts = BTreeMap::<usize, Vec<usize>>::new();
    for (file, &district) in layout.membership.iter().enumerate() {
        districts.entry(district).or_default().push(file);
    }
    let mut groups = BTreeMap::new();
    let mut file_ids = vec![String::new(); layout.membership.len()];
    for (district, members) in districts {
        let mut parts = split(&members, &adjacency, partitioner, 0)?;
        fold_small(&mut parts, &adjacency);
        parts.sort_by_key(|part| part[0]);
        let parents = parts
            .iter()
            .map(|part| dominant_parent(part, layout))
            .collect::<Vec<_>>();
        for (ordinal, part) in parts.into_iter().enumerate() {
            let id = format!("{district}-{ordinal}");
            let label = unique_suffix(&parents, ordinal);
            for &file in &part {
                file_ids[file] = id.clone();
            }
            groups.insert(id, (district, part, label));
        }
    }
    Ok(Partition { groups, file_ids })
}

fn split<P: Partitioner>(
    members: &[usize],
    adjacency: &[Vec<(usize, f64)>],
    partitioner: &P,
    depth: usize,
) -> Result<Vec<Vec<usize>>> {
    if members.len() <= SINGLE_LIMIT {
        return Ok(vec![members.to_vec()]);
    }
    if depth >= 12 {
        return Ok(path_chunks(members));
    }
    let local = members
        .iter()
        .enumerate()
        .map(|(i, &n)| (n, i))
        .collect::<BTreeMap<_, _>>();
    let mut edges = Vec::new();
    for &a in members {
        for &(b, weight) in &adjacency[a] {
            if a < b {
                if let Some(&local_b) = local.get(&b) {
                    edges.push(WeightedEdge {
                        a: local[&a],
                        b: local_b,
                        weight,
                    });
                }
            }
        }
    }
    if edges.is_empty() {
        return Ok(path_chunks(members));
    }
    let graph = WeightedGraph {
        node_count: members.len(),
        edges,
    };
    // Recurse at a fixed resolution instead of asking one high-resolution
    // pass to split a huge district into near-singletons. The latter made
    // folding quadratic and lost the graph's useful larger communities.
    // Raise resolution only if this induced component does not split.
    let base = 1.1;
    let mut parts = Vec::new();
    for multiplier in [1.0, 2.0, 4.0] {
        let result = partitioner.partition(&graph, base * multiplier, SEED, None)?;
        let mut by_id = BTreeMap::<usize, Vec<usize>>::new();
        for (&file, community) in members.iter().zip(result.membership) {
            by_id.entry(community).or_default().push(file);
        }
        parts = by_id.into_values().collect();
        if parts.len() > 1 {
            break;
        }
    }
    if parts.len() <= 1 {
        return Ok(path_chunks(members));
    }
    let mut output = Vec::new();
    for part in parts {
        if part.len() == members.len() {
            output.push(part);
        } else {
            output.extend(split(&part, adjacency, partitioner, depth + 1)?);
        }
    }
    Ok(output)
}

fn path_chunks(members: &[usize]) -> Vec<Vec<usize>> {
    // A connected induced component can remain indivisible at all three
    // tested resolutions (or peel one singleton at each recursive pass).
    // The global file order is lexical path order, so contiguous chunks
    // preserve a useful directory fallback and cap the geometry workload.
    members.chunks(TARGET).map(|chunk| chunk.to_vec()).collect()
}

fn fold_small(groups: &mut Vec<Vec<usize>>, adjacency: &[Vec<(usize, f64)>]) {
    loop {
        let Some(source) = groups
            .iter()
            .enumerate()
            .filter(|(_, group)| group.len() < 3)
            .min_by_key(|(_, group)| (group.len(), group[0]))
            .map(|(index, _)| index)
        else {
            break;
        };
        if groups.len() == 1 {
            break;
        }
        let mut owner = BTreeMap::new();
        for (index, group) in groups.iter().enumerate() {
            for &file in group {
                owner.insert(file, index);
            }
        }
        let mut weights = vec![0.0; groups.len()];
        for &file in &groups[source] {
            for &(neighbour, weight) in &adjacency[file] {
                if let Some(&target) = owner.get(&neighbour) {
                    if target != source {
                        weights[target] += weight;
                    }
                }
            }
        }
        let target = (0..groups.len())
            .filter(|&i| i != source)
            .max_by(|&a, &b| {
                weights[a]
                    .total_cmp(&weights[b])
                    .then_with(|| groups[b][0].cmp(&groups[a][0]))
            })
            .expect("at least two groups");
        let moved = groups.remove(source);
        let target = if target > source { target - 1 } else { target };
        groups[target].extend(moved);
        groups[target].sort_unstable();
        if groups[target].len() > SINGLE_LIMIT + 2 {
            // Folding hundreds of weakly attached singletons into the same
            // strongest neighbour produced 124- to 545-file regions on the
            // first large-repo run. Keep the merge decision, then divide
            // its lexical path run back into tractable neighbourhoods.
            let split = path_chunks(&groups.remove(target));
            groups.extend(split);
        }
    }
}

fn dominant_parent(members: &[usize], layout: &PipelineOutput) -> String {
    let mut counts = BTreeMap::<String, usize>::new();
    for &file in members {
        let path = &layout.weighted.nodes[file].file;
        let parent = path.rsplit_once('/').map_or(".", |(parent, _)| parent);
        *counts.entry(parent.to_owned()).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(parent, _)| parent)
        .unwrap_or_else(|| ".".to_owned())
}

fn unique_suffix(parents: &[String], index: usize) -> String {
    let parts = parents[index].split('/').collect::<Vec<_>>();
    for length in 1..=parts.len() {
        let suffix = parts[parts.len() - length..].join("/");
        if parents.iter().enumerate().all(|(other, parent)| {
            other == index || !parent.ends_with(&format!("/{suffix}")) && parent != &suffix
        }) {
            return suffix;
        }
    }
    format!("{} #{}", parents[index], index + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_does_not_make_one_huge_neighbourhood() {
        let adjacency = vec![Vec::new(); 70];
        let mut groups = vec![(0..40).collect::<Vec<_>>()];
        groups.extend((40..70).map(|file| vec![file]));
        fold_small(&mut groups, &adjacency);
        assert_eq!(groups.iter().map(Vec::len).sum::<usize>(), 70);
        assert!(groups.iter().all(|group| group.len() <= SINGLE_LIMIT + 2));
    }
}
