//! Actionable district cycles (issue #258): mutual pairs and a small cut.
//!
//! A district cycle on a real map is often most of the map (finding 63:
//! 6 of semon's 7 districts, 519 cross-district edges), which nobody can act
//! on. Two smaller answers are computed from the cycle's bundles, where a
//! bundle is every file edge from one district to another and its weight is
//! how many file edges it holds:
//!
//! - **Mutual pairs**: two districts with bundles in both directions. The
//!   lighter direction is the one to look at.
//! - **A small cut**: bundles whose removal leaves the cycle's districts
//!   acyclic. Finding the minimum is NP-hard (minimum feedback arc set), so
//!   this is a heuristic and is never reported as the minimum. It is the
//!   weighted greedy ordering of Eades, Lin and Smyth (1993): repeatedly
//!   take sinks to the back and sources to the front, otherwise the district
//!   whose outgoing weight most exceeds its incoming weight to the front.
//!   Bundles pointing backwards in that order form the cut, which favours
//!   light bundles. A cut bundle whose return would leave the graph acyclic
//!   is then returned, heaviest first, so no bundle in the cut is redundant.
//!
//! Every tie breaks on the smaller district id, then the bundle's
//! `(from, to)`, so one map always gives one answer.

use std::collections::{BTreeMap, BTreeSet};

/// `(from, to)` district ids -> file edges from `from` to `to`.
pub type Bundles = BTreeMap<(usize, usize), usize>;

/// One `(from, to)` per pair of districts with bundles both ways: the
/// lighter direction, or, at equal weight, the one whose `from` is smaller.
/// Ordered by `(from, to)`.
pub fn mutual_pairs(bundles: &Bundles) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    for (&(a, b), &forward) in bundles {
        if a >= b {
            continue;
        }
        let Some(&back) = bundles.get(&(b, a)) else {
            continue;
        };
        pairs.push(if back < forward { (b, a) } else { (a, b) });
    }
    pairs.sort_unstable();
    pairs
}

fn acyclic(nodes: &BTreeSet<usize>, arcs: &BTreeSet<(usize, usize)>) -> bool {
    // Kahn's algorithm: acyclic iff every node can be removed in-degree 0.
    let mut indegree = nodes
        .iter()
        .map(|&n| (n, 0usize))
        .collect::<BTreeMap<_, _>>();
    for &(_, to) in arcs {
        *indegree.entry(to).or_default() += 1;
    }
    let mut ready = indegree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(&n, _)| n)
        .collect::<Vec<_>>();
    let mut removed = 0;
    while let Some(node) = ready.pop() {
        removed += 1;
        for &(_, to) in arcs.range((node, 0)..=(node, usize::MAX)) {
            let degree = indegree.get_mut(&to).expect("arc endpoint is a node");
            *degree -= 1;
            if *degree == 0 {
                ready.push(to);
            }
        }
    }
    removed == indegree.len()
}

/// A small set of bundles (`(from, to)`, ordered) whose removal leaves the
/// districts of `bundles` acyclic. A heuristic; see the module comment.
pub fn greedy_cut(bundles: &Bundles) -> Vec<(usize, usize)> {
    let arcs = bundles
        .iter()
        .filter(|(&(a, b), &w)| a != b && w > 0)
        .map(|(&arc, &w)| (arc, w))
        .collect::<BTreeMap<_, _>>();
    let nodes = arcs
        .keys()
        .flat_map(|&(a, b)| [a, b])
        .collect::<BTreeSet<_>>();
    let mut left = nodes.clone();
    let mut front = Vec::new();
    let mut back = Vec::new();
    // Weights to and from the districts still left.
    let flows = |left: &BTreeSet<usize>, node: usize| {
        let mut out = 0usize;
        let mut inn = 0usize;
        for (&(a, b), &w) in &arcs {
            if a == node && left.contains(&b) {
                out += w;
            } else if b == node && left.contains(&a) {
                inn += w;
            }
        }
        (out, inn)
    };
    while !left.is_empty() {
        let sink = left.iter().copied().find(|&n| flows(&left, n).0 == 0);
        if let Some(node) = sink {
            left.remove(&node);
            back.push(node);
            continue;
        }
        let source = left.iter().copied().find(|&n| flows(&left, n).1 == 0);
        if let Some(node) = source {
            left.remove(&node);
            front.push(node);
            continue;
        }
        // The largest out - in; the first (smallest id) on a tie.
        let mut best: Option<(i64, usize)> = None;
        for &node in &left {
            let (out, inn) = flows(&left, node);
            let delta = out as i64 - inn as i64;
            if best.is_none_or(|(d, _)| delta > d) {
                best = Some((delta, node));
            }
        }
        let (_, node) = best.expect("left is not empty");
        left.remove(&node);
        front.push(node);
    }
    back.reverse();
    front.extend(back);
    let position = front
        .iter()
        .enumerate()
        .map(|(i, &n)| (n, i))
        .collect::<BTreeMap<_, _>>();
    let mut cut = arcs
        .keys()
        .copied()
        .filter(|&(a, b)| position[&a] > position[&b])
        .collect::<BTreeSet<_>>();
    let mut kept = arcs
        .keys()
        .copied()
        .filter(|arc| !cut.contains(arc))
        .collect::<BTreeSet<_>>();
    // Return what the cut does not need, heaviest first.
    let mut candidates = cut.iter().copied().collect::<Vec<_>>();
    candidates.sort_by_key(|arc| (std::cmp::Reverse(arcs[arc]), *arc));
    for arc in candidates {
        kept.insert(arc);
        if acyclic(&nodes, &kept) {
            cut.remove(&arc);
        } else {
            kept.remove(&arc);
        }
    }
    cut.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundles(rows: &[(usize, usize, usize)]) -> Bundles {
        rows.iter().map(|&(a, b, w)| ((a, b), w)).collect()
    }

    fn without(all: &Bundles, cut: &[(usize, usize)]) -> bool {
        let nodes = all.keys().flat_map(|&(a, b)| [a, b]).collect();
        let arcs = all
            .keys()
            .copied()
            .filter(|arc| !cut.contains(arc))
            .collect();
        acyclic(&nodes, &arcs)
    }

    #[test]
    fn a_three_district_cycle_is_cut_at_its_light_back_bundle() {
        let graph = bundles(&[(0, 1, 5), (1, 2, 5), (2, 0, 1)]);
        assert_eq!(greedy_cut(&graph), vec![(2, 0)]);
        // Whichever bundle is light, it is the cut.
        let graph = bundles(&[(0, 1, 1), (1, 2, 5), (2, 0, 5)]);
        assert_eq!(greedy_cut(&graph), vec![(0, 1)]);
    }

    #[test]
    fn a_mutual_pair_reports_its_lighter_direction() {
        let graph = bundles(&[(0, 1, 7), (1, 0, 2), (1, 2, 3)]);
        assert_eq!(mutual_pairs(&graph), vec![(1, 0)]);
        assert_eq!(greedy_cut(&graph), vec![(1, 0)]);
    }

    #[test]
    fn ties_break_on_the_smaller_district() {
        let graph = bundles(&[(0, 1, 4), (1, 0, 4)]);
        assert_eq!(mutual_pairs(&graph), vec![(0, 1)]);
        assert_eq!(greedy_cut(&graph), vec![(1, 0)]);
        let graph = bundles(&[(0, 1, 1), (1, 2, 1), (2, 0, 1)]);
        assert_eq!(greedy_cut(&graph), vec![(2, 0)]);
    }

    #[test]
    fn no_cut_bundle_is_redundant() {
        // Two cycles sharing the light bundle 3 -> 0: one cut suffices.
        let graph = bundles(&[
            (0, 1, 6),
            (1, 3, 6),
            (0, 2, 6),
            (2, 3, 6),
            (3, 0, 1),
            (1, 2, 2),
        ]);
        let cut = greedy_cut(&graph);
        assert_eq!(cut, vec![(3, 0)]);
        assert!(without(&graph, &cut));
        for i in 0..cut.len() {
            let mut less = cut.clone();
            less.remove(i);
            assert!(!without(&graph, &less));
        }
    }

    #[test]
    fn an_acyclic_graph_needs_no_cut() {
        let graph = bundles(&[(0, 1, 3), (1, 2, 3), (0, 2, 1)]);
        assert!(greedy_cut(&graph).is_empty());
        assert!(mutual_pairs(&graph).is_empty());
    }
}
