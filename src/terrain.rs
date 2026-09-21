//! Terrain decomposition for oversized top-level districts.
//!
//! This module deliberately knows nothing about repositories, rendering, or
//! the compact map schema.  Its inputs are the weighted, pruned graph the
//! top-level partitioner saw, a sorted member list, and the corresponding file
//! paths.  That keeps the mechanism in `docs/TERRAIN.md` section 2 directly
//! unit-testable and prevents terrain from feeding back into the top level.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::Result;

use crate::parity;
use crate::partition::Partitioner;
use crate::schema::{WeightedEdge, WeightedGraph};
use crate::SEED;

/// Median `districts / sqrt(files)` over the acceptance fixtures with at
/// least 50 files.  Derived once in `docs/TERRAIN.md` section 1; runtime data
/// must not silently retune the scale of an existing map.
pub const C_REF: f64 = 0.517;
pub const ELIGIBILITY_FLOOR: usize = 50;
pub const SPLIT_RATIO: f64 = 2.0;
pub const MERGE_RATIO: f64 = 0.3;
pub const RESOLUTION: f64 = 1.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub target: f64,
    pub hi: f64,
    pub lo: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Decomposition {
    /// `(file index, files stranded)`, in removal order.
    pub arterials: Vec<(usize, usize)>,
    pub parcels: Vec<Vec<usize>>,
    pub organic: Vec<Vec<usize>>,
    pub band: Band,
}

pub fn band(total_files: usize) -> Band {
    let target = (total_files as f64).sqrt() / C_REF;
    Band {
        target,
        hi: SPLIT_RATIO * target,
        lo: MERGE_RATIO * target,
    }
}

pub fn eligible(total_files: usize, district_size: usize) -> bool {
    district_size >= ELIGIBILITY_FLOOR && district_size as f64 > band(total_files).hi
}

/// Reproduce `eval/terrain_spike.py::decompose`, except that the articulation
/// search is deliberately uncapped as required by `docs/TERRAIN.md` section
/// 2.  Members and every returned group are sorted by global file index.
pub fn decompose<P: Partitioner>(
    graph: &WeightedGraph,
    members: &[usize],
    paths: &[String],
    total_files: usize,
    partitioner: &P,
) -> Result<Option<Decomposition>> {
    if !eligible(total_files, members.len()) {
        return Ok(None);
    }
    let terrain_band = band(total_files);
    let adjacency = induced_adjacency(graph, members);
    let (arterials, rest) = find_arterials(&adjacency, members, paths, terrain_band.lo);
    let mut parcels = Vec::new();
    let mut organic_components = Vec::new();
    for component in components(&adjacency, &rest) {
        if (component.len() as f64) < terrain_band.lo {
            parcels.push(component);
        } else {
            organic_components.push(component);
        }
    }
    let mut organic = Vec::new();
    for component in organic_components {
        organic.extend(organic_split(
            &adjacency,
            &component,
            terrain_band.lo,
            terrain_band.hi,
            partitioner,
        )?);
    }
    Ok(Some(Decomposition {
        arterials,
        parcels,
        organic,
        band: terrain_band,
    }))
}

type Adjacency = BTreeMap<usize, BTreeMap<usize, f64>>;

fn induced_adjacency(graph: &WeightedGraph, members: &[usize]) -> Adjacency {
    let member_set = members.iter().copied().collect::<BTreeSet<_>>();
    let mut adjacency = members
        .iter()
        .copied()
        .map(|member| (member, BTreeMap::new()))
        .collect::<Adjacency>();
    for edge in &graph.edges {
        if edge.a == edge.b || !member_set.contains(&edge.a) || !member_set.contains(&edge.b) {
            continue;
        }
        *adjacency
            .get_mut(&edge.a)
            .expect("member was inserted")
            .entry(edge.b)
            .or_default() += edge.weight;
        *adjacency
            .get_mut(&edge.b)
            .expect("member was inserted")
            .entry(edge.a)
            .or_default() += edge.weight;
    }
    adjacency
}

fn components(adjacency: &Adjacency, nodes: &BTreeSet<usize>) -> Vec<Vec<usize>> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for &start in nodes {
        if !seen.insert(start) {
            continue;
        }
        let mut queue = VecDeque::from([start]);
        let mut component = Vec::new();
        while let Some(node) = queue.pop_front() {
            component.push(node);
            if let Some(neighbours) = adjacency.get(&node) {
                for &neighbour in neighbours.keys() {
                    if nodes.contains(&neighbour) && seen.insert(neighbour) {
                        queue.push_back(neighbour);
                    }
                }
            }
        }
        component.sort_unstable();
        result.push(component);
    }
    result
}

/// Repeatedly remove the articulation point that strands the most files.
/// One Tarjan traversal computes every candidate exactly per iteration; no
/// degree-based preselection or 64-node cap is involved.
fn find_arterials(
    adjacency: &Adjacency,
    members: &[usize],
    paths: &[String],
    floor: f64,
) -> (Vec<(usize, usize)>, BTreeSet<usize>) {
    let mut remaining = members.iter().copied().collect::<BTreeSet<_>>();
    let mut found = Vec::new();
    loop {
        let stranded = articulation_stranding(adjacency, &remaining);
        let best = stranded
            .into_iter()
            .filter(|(_, count)| (*count as f64) >= floor)
            .min_by(|(left_node, left_count), (right_node, right_count)| {
                right_count
                    .cmp(left_count)
                    .then_with(|| paths[*left_node].cmp(&paths[*right_node]))
            });
        let Some((node, count)) = best else {
            break;
        };
        found.push((node, count));
        remaining.remove(&node);
    }
    (found, remaining)
}

#[derive(Default)]
struct TarjanState {
    next_time: usize,
    discovered: BTreeMap<usize, usize>,
    low: BTreeMap<usize, usize>,
    parent: BTreeMap<usize, usize>,
    subtree: BTreeMap<usize, usize>,
    separating_children: BTreeMap<usize, Vec<usize>>,
}

fn tarjan_visit(
    node: usize,
    root: usize,
    adjacency: &Adjacency,
    nodes: &BTreeSet<usize>,
    state: &mut TarjanState,
) {
    let time = state.next_time;
    state.next_time += 1;
    state.discovered.insert(node, time);
    state.low.insert(node, time);
    let mut subtree = 1;
    let mut root_children = 0;

    for &neighbour in adjacency[&node].keys() {
        if !nodes.contains(&neighbour) {
            continue;
        }
        if !state.discovered.contains_key(&neighbour) {
            state.parent.insert(neighbour, node);
            root_children += 1;
            tarjan_visit(neighbour, root, adjacency, nodes, state);
            subtree += state.subtree[&neighbour];
            let child_low = state.low[&neighbour];
            state
                .low
                .entry(node)
                .and_modify(|low| *low = (*low).min(child_low));
            if node == root || child_low >= state.discovered[&node] {
                state
                    .separating_children
                    .entry(node)
                    .or_default()
                    .push(state.subtree[&neighbour]);
            }
        } else if state.parent.get(&node).copied() != Some(neighbour) {
            let back = state.discovered[&neighbour];
            state
                .low
                .entry(node)
                .and_modify(|low| *low = (*low).min(back));
        }
    }
    state.subtree.insert(node, subtree);
    if node == root && root_children <= 1 {
        state.separating_children.remove(&node);
    }
}

/// Exact `largest_before - 1 - largest_after` for every articulation point.
/// The parent-side remainder represented by the block-cut tree is included,
/// and an already-disconnected graph retains its unaffected largest
/// component, which is why disconnection alone never creates an arterial.
fn articulation_stranding(
    adjacency: &Adjacency,
    nodes: &BTreeSet<usize>,
) -> BTreeMap<usize, usize> {
    let graph_components = components(adjacency, nodes);
    let before = graph_components.iter().map(Vec::len).max().unwrap_or(0);
    if before <= 1 {
        return BTreeMap::new();
    }
    let mut component_of = BTreeMap::new();
    let mut component_sizes = Vec::new();
    for (index, component) in graph_components.iter().enumerate() {
        component_sizes.push(component.len());
        for &node in component {
            component_of.insert(node, index);
        }
    }
    let largest_count = component_sizes
        .iter()
        .filter(|&&size| size == before)
        .count();
    let second_largest = component_sizes
        .iter()
        .copied()
        .filter(|&size| size < before)
        .max()
        .unwrap_or(0);
    let mut state = TarjanState::default();
    for component in &graph_components {
        if let Some(&root) = component.first() {
            tarjan_visit(root, root, adjacency, nodes, &mut state);
        }
    }
    let mut result = BTreeMap::new();
    for (&node, separated) in &state.separating_children {
        let component_index = component_of[&node];
        let own_size = component_sizes[component_index];
        let separated_total = separated.iter().sum::<usize>();
        let remainder = own_size.saturating_sub(1 + separated_total);
        let largest_own_piece = separated
            .iter()
            .copied()
            .chain([remainder])
            .max()
            .unwrap_or(0);
        let largest_other = if own_size == before && largest_count == 1 {
            second_largest
        } else {
            before
        };
        let largest_after = largest_own_piece.max(largest_other);
        if before > largest_after + 1 {
            result.insert(node, before - 1 - largest_after);
        }
    }
    result
}

fn leiden_groups<P: Partitioner>(
    adjacency: &Adjacency,
    nodes: &[usize],
    partitioner: &P,
) -> Result<Vec<Vec<usize>>> {
    let order = nodes.iter().copied().collect::<BTreeSet<_>>();
    let order = order.into_iter().collect::<Vec<_>>();
    let local = order
        .iter()
        .enumerate()
        .map(|(index, &node)| (node, index))
        .collect::<BTreeMap<_, _>>();
    let node_set = order.iter().copied().collect::<BTreeSet<_>>();
    let mut edges = Vec::new();
    for &a in &order {
        for (&b, &weight) in &adjacency[&a] {
            if a < b && node_set.contains(&b) {
                edges.push(WeightedEdge {
                    a: local[&a],
                    b: local[&b],
                    weight,
                });
            }
        }
    }
    let graph = WeightedGraph {
        node_count: order.len(),
        edges,
    };
    let partition = partitioner.partition(&graph, RESOLUTION, SEED, None)?;
    let mut groups = BTreeMap::<usize, Vec<usize>>::new();
    for (&node, community) in order.iter().zip(partition.membership) {
        groups.entry(community).or_default().push(node);
    }
    Ok(groups.into_values().collect())
}

fn fold_small(
    adjacency: &Adjacency,
    groups: Vec<Vec<usize>>,
    floor: f64,
    ceiling: f64,
) -> Vec<Vec<usize>> {
    let mut groups = groups
        .into_iter()
        .map(|group| group.into_iter().collect::<BTreeSet<_>>())
        .collect::<Vec<_>>();
    loop {
        let smallest = groups
            .iter()
            .enumerate()
            .filter(|(_, group)| (group.len() as f64) < floor)
            .min_by_key(|(_, group)| (group.len(), group.first().copied().unwrap_or(usize::MAX)))
            .map(|(index, _)| index);
        let Some(source) = smallest else {
            break;
        };
        if groups.len() == 1 {
            break;
        }
        let owner = groups
            .iter()
            .enumerate()
            .flat_map(|(index, group)| group.iter().map(move |&node| (node, index)))
            .collect::<BTreeMap<_, _>>();
        let mut weights = BTreeMap::<usize, f64>::new();
        for &node in &groups[source] {
            for (&neighbour, &weight) in &adjacency[&node] {
                if let Some(&target) = owner.get(&neighbour) {
                    if target != source {
                        *weights.entry(target).or_default() += weight;
                    }
                }
            }
        }
        if weights.is_empty() {
            break;
        }
        let fits = weights
            .keys()
            .copied()
            .filter(|&target| groups[target].len() + groups[source].len() <= ceiling as usize)
            .collect::<Vec<_>>();
        let pool = if fits.is_empty() {
            weights.keys().copied().collect::<Vec<_>>()
        } else {
            fits
        };
        let target = pool
            .into_iter()
            .max_by(|left, right| {
                weights[left]
                    .total_cmp(&weights[right])
                    .then_with(|| right.cmp(left))
            })
            .expect("a weighted neighbour exists");
        let source_members = groups[source].clone();
        groups[target].extend(source_members);
        groups.remove(source);
    }
    groups
        .into_iter()
        .map(|group| group.into_iter().collect())
        .collect()
}

fn organic_split<P: Partitioner>(
    adjacency: &Adjacency,
    component: &[usize],
    lo: f64,
    hi: f64,
    partitioner: &P,
) -> Result<Vec<Vec<usize>>> {
    if (component.len() as f64) <= hi {
        return Ok(vec![component.to_vec()]);
    }
    let parts = fold_small(
        adjacency,
        leiden_groups(adjacency, component, partitioner)?,
        lo,
        hi,
    );
    // A locally indivisible component is an explicit terminal condition;
    // recursing would simply ask Leiden the same question forever.
    if parts.len() == 1 {
        return Ok(parts);
    }
    let mut result = Vec::new();
    for part in parts {
        result.extend(organic_split(adjacency, &part, lo, hi, partitioner)?);
    }
    Ok(result)
}

/// Assign stable suffixes to current organic groups.  The overlap matching
/// is the established greedy best-Jaccard implementation in `src/parity.rs`,
/// including matches at the 0.35 boundary, not a terrain-specific
/// approximation of it.
pub fn assign_suffixes(
    current: &[Vec<String>],
    previous: Option<&[(usize, Vec<String>)]>,
    previous_max: usize,
) -> (Vec<usize>, usize) {
    if previous.is_none() {
        let mut order = (0..current.len()).collect::<Vec<_>>();
        order.sort_by(|&left, &right| {
            current[right]
                .len()
                .cmp(&current[left].len())
                .then_with(|| current[left].iter().min().cmp(&current[right].iter().min()))
        });
        let mut suffixes = vec![0; current.len()];
        for (offset, index) in order.into_iter().enumerate() {
            suffixes[index] = offset + 1;
        }
        return (suffixes, current.len());
    }

    let previous = previous.expect("checked above");
    let candidate = current
        .iter()
        .enumerate()
        .flat_map(|(group, files)| files.iter().cloned().map(move |file| (file, group)))
        .collect::<BTreeMap<_, _>>();
    let reference = previous
        .iter()
        .enumerate()
        .flat_map(|(group, (_, files))| files.iter().cloned().map(move |file| (file, group)))
        .collect::<BTreeMap<_, _>>();
    let common = candidate
        .keys()
        .filter(|file| reference.contains_key(*file))
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let matches = parity::match_districts(&candidate, &reference, &common);
    let mut suffixes = vec![0; current.len()];
    for (candidate_group, (reference_group, _)) in matches {
        suffixes[candidate_group] = previous[reference_group].0;
    }
    let mut maximum = previous_max.max(
        previous
            .iter()
            .map(|(suffix, _)| *suffix)
            .max()
            .unwrap_or(0),
    );
    let mut unmatched = (0..current.len())
        .filter(|&index| suffixes[index] == 0)
        .collect::<Vec<_>>();
    unmatched.sort_by(|&left, &right| {
        current[right]
            .len()
            .cmp(&current[left].len())
            .then_with(|| current[left].iter().min().cmp(&current[right].iter().min()))
    });
    for index in unmatched {
        maximum += 1;
        suffixes[index] = maximum;
    }
    (suffixes, maximum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::PartitionResult;

    fn graph(node_count: usize, pairs: &[(usize, usize, f64)]) -> WeightedGraph {
        WeightedGraph {
            node_count,
            edges: pairs
                .iter()
                .map(|&(a, b, weight)| WeightedEdge { a, b, weight })
                .collect(),
        }
    }

    fn paths(count: usize) -> Vec<String> {
        (0..count).map(|index| format!("f{index:03}")).collect()
    }

    #[test]
    fn star_hub_is_an_arterial() {
        let graph = graph(
            6,
            &[
                (0, 1, 1.0),
                (0, 2, 1.0),
                (0, 3, 1.0),
                (0, 4, 1.0),
                (0, 5, 1.0),
            ],
        );
        let members = (0..6).collect::<Vec<_>>();
        let adjacency = induced_adjacency(&graph, &members);
        let (arterials, rest) = find_arterials(&adjacency, &members, &paths(6), 1.0);
        assert_eq!(arterials, vec![(0, 4)]);
        assert_eq!(rest, BTreeSet::from([1, 2, 3, 4, 5]));
    }

    #[test]
    fn clique_has_no_arterial() {
        let mut pairs = Vec::new();
        for a in 0..5 {
            for b in a + 1..5 {
                pairs.push((a, b, 1.0));
            }
        }
        let graph = graph(5, &pairs);
        let members = (0..5).collect::<Vec<_>>();
        let adjacency = induced_adjacency(&graph, &members);
        assert!(find_arterials(&adjacency, &members, &paths(5), 1.0)
            .0
            .is_empty());
    }

    #[test]
    fn already_disconnected_district_does_not_call_a_hub_load_bearing() {
        let graph = graph(6, &[(0, 1, 1.0), (0, 2, 1.0), (3, 4, 1.0), (3, 5, 1.0)]);
        let members = (0..6).collect::<Vec<_>>();
        let adjacency = induced_adjacency(&graph, &members);
        assert!(find_arterials(&adjacency, &members, &paths(6), 1.0)
            .0
            .is_empty());
    }

    #[test]
    fn ceiling_cap_prevents_small_folds_from_snowballing() {
        let groups = vec![
            vec![0, 1, 2],
            vec![3, 4, 5],
            vec![6, 7, 8, 9, 10],
            vec![11, 12, 13, 14, 15],
        ];
        let graph = graph(16, &[(0, 6, 10.0), (3, 6, 10.0), (3, 11, 1.0)]);
        let members = (0..16).collect::<Vec<_>>();
        let adjacency = induced_adjacency(&graph, &members);
        let folded = fold_small(&adjacency, groups, 4.0, 8.0);
        let sizes = folded.iter().map(Vec::len).collect::<Vec<_>>();
        assert_eq!(sizes, vec![8, 8]);
        assert!(folded
            .iter()
            .any(|group| group.contains(&0) && group.contains(&6)));
        assert!(folded
            .iter()
            .any(|group| group.contains(&3) && group.contains(&11)));
    }

    struct OnePart;

    impl Partitioner for OnePart {
        fn partition(
            &self,
            graph: &WeightedGraph,
            _resolution: f64,
            _seed: u64,
            _initial: Option<&[usize]>,
        ) -> Result<PartitionResult> {
            Ok(PartitionResult {
                membership: vec![0; graph.node_count],
                modularity: 0.0,
            })
        }
    }

    #[test]
    fn recursive_split_stops_when_leiden_returns_one_part() {
        let graph = graph(5, &[(0, 1, 1.0), (1, 2, 1.0), (2, 3, 1.0), (3, 4, 1.0)]);
        let members = (0..5).collect::<Vec<_>>();
        let adjacency = induced_adjacency(&graph, &members);
        assert_eq!(
            organic_split(&adjacency, &members, 1.0, 2.0, &OnePart).unwrap(),
            vec![members]
        );
    }

    #[test]
    fn eligibility_keeps_the_fifty_file_floor() {
        assert!(!eligible(100, 49));
        assert!(eligible(100, 50));
    }

    #[test]
    fn suffixes_on_first_appearance_follow_size_then_path() {
        let current = vec![
            vec!["z".into()],
            vec!["b".into(), "c".into()],
            vec!["a".into(), "d".into()],
        ];
        assert_eq!(assign_suffixes(&current, None, 0), (vec![3, 2, 1], 3));
    }

    #[test]
    fn suffixes_keep_a_stable_match() {
        let current = vec![vec!["a".into(), "b".into()], vec!["c".into()]];
        let previous = vec![(7, vec!["a".into(), "b".into()]), (3, vec!["c".into()])];
        assert_eq!(
            assign_suffixes(&current, Some(&previous), 7),
            (vec![7, 3], 7)
        );
    }

    #[test]
    fn suffixes_match_at_the_jaccard_boundary() {
        // Seven shared members over a twenty-member union is exactly 0.35.
        // The acceptance gate deliberately includes this boundary, and
        // terrain suffix matching reuses that definition.
        let mut current = vec![(0..7).map(|i| format!("f{i}")).collect::<Vec<_>>()];
        current.extend((7..20).map(|i| vec![format!("f{i}")]));
        let previous = vec![(7, (0..20).map(|i| format!("f{i}")).collect())];
        let (suffixes, maximum) = assign_suffixes(&current, Some(&previous), 7);
        assert_eq!(suffixes[0], 7);
        assert_eq!(maximum, 20);
    }

    #[test]
    fn a_new_subdistrict_gets_the_next_suffix() {
        let current = vec![vec!["a".into()], vec!["new".into()]];
        let previous = vec![(2, vec!["a".into()])];
        assert_eq!(
            assign_suffixes(&current, Some(&previous), 2),
            (vec![2, 3], 3)
        );
    }

    #[test]
    fn a_retired_suffix_is_never_reused() {
        let current = vec![vec!["a".into()], vec!["new".into()]];
        let previous = vec![(2, vec!["a".into()])];
        assert_eq!(
            assign_suffixes(&current, Some(&previous), 9),
            (vec![2, 10], 10)
        );
    }
}
