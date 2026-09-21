use std::ffi::CStr;
use std::os::raw::{c_char, c_double, c_int};
use std::sync::Mutex;

use anyhow::{bail, ensure, Result};

use super::{PartitionResult, Partitioner};
use crate::schema::WeightedGraph;

// igraph keeps a process-global finally stack, so concurrent calls through the
// C API corrupt its cleanup state even when each graph is otherwise isolated.
static LEIDEN_LOCK: Mutex<()> = Mutex::new(());

unsafe extern "C" {
    fn tolmap_leiden_partition(
        node_count: usize,
        endpoints: *const usize,
        weights: *const c_double,
        edge_count: usize,
        resolution: c_double,
        seed: u64,
        initial_membership: *const usize,
        membership_out: *mut usize,
        modularity_out: *mut c_double,
        error_out: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LeidenFfi;

impl Partitioner for LeidenFfi {
    fn partition(
        &self,
        graph: &WeightedGraph,
        resolution: f64,
        seed: u64,
        initial: Option<&[usize]>,
    ) -> Result<PartitionResult> {
        let _guard = LEIDEN_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("Leiden FFI lock poisoned"))?;
        if let Some(membership) = initial {
            ensure!(
                membership.len() == graph.node_count,
                "initial membership has {} entries for {} nodes",
                membership.len(),
                graph.node_count
            );
        }
        let mut endpoints = Vec::with_capacity(graph.edges.len() * 2);
        let mut weights = Vec::with_capacity(graph.edges.len());
        for edge in &graph.edges {
            ensure!(
                edge.a < graph.node_count && edge.b < graph.node_count,
                "edge endpoint outside graph"
            );
            ensure!(edge.weight.is_finite(), "non-finite edge weight");
            endpoints.extend([edge.a, edge.b]);
            weights.push(edge.weight);
        }

        // An edgeless graph has no community structure, and libleidenalg's
        // modularity for it is 0/0 -- which the `non-finite modularity` check
        // below turns into a hard error instead of an answer.
        //
        // The path is real, not hypothetical: `blobs::subdivide` induces a
        // subgraph on one district's members, and a district whose files have
        // no kept edge to each other induces a graph with no edges at all.
        // Issue #41's tail is full of such districts (68 of n8n's 369, 121 of
        // dify's 226). Whether a repository actually reaches the failure also
        // needs one of them to clear `subdivide`'s 40-file floor, and measured
        // on issue #41's four reference repositories at their pinned commits
        // none of them does: crawlab, codex, dify and n8n all produce a map on
        // this tree without this guard. So it changes no current output -- it
        // is here because the alternative to answering is aborting a whole
        // build, and the answer is not in doubt. Modularity of a graph with no
        // edges is 0 by convention, and one community is the only partition
        // available.
        //
        // Guarded at the FFI boundary rather than inside `subdivide` so both
        // of that function's call sites are covered.
        if graph.edges.is_empty() {
            return Ok(PartitionResult {
                membership: vec![0; graph.node_count],
                modularity: 0.0,
            });
        }

        let mut membership = vec![0; graph.node_count];
        let mut modularity = 0.0;
        let mut error = vec![0_i8; 1024];
        let initial_ptr = initial.map_or(std::ptr::null(), <[usize]>::as_ptr);
        // SAFETY: all slices remain alive for the call, output buffers have the
        // declared lengths, and the C++ bridge catches exceptions at the ABI.
        let status = unsafe {
            tolmap_leiden_partition(
                graph.node_count,
                endpoints.as_ptr(),
                weights.as_ptr(),
                graph.edges.len(),
                resolution,
                seed,
                initial_ptr,
                membership.as_mut_ptr(),
                &mut modularity,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if status != 0 {
            // SAFETY: the bridge always NUL-terminates this initialized buffer.
            let message = unsafe { CStr::from_ptr(error.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            bail!("libleidenalg failed ({status}): {message}");
        }
        ensure!(
            modularity.is_finite(),
            "libleidenalg returned non-finite modularity"
        );
        Ok(PartitionResult {
            membership,
            modularity,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::WeightedEdge;

    #[test]
    fn partitions_a_pair_of_triangles_deterministically() {
        let graph = WeightedGraph {
            node_count: 6,
            edges: vec![
                (0, 1, 1.0),
                (1, 2, 1.0),
                (2, 0, 1.0),
                (3, 4, 1.0),
                (4, 5, 1.0),
                (5, 3, 1.0),
                (2, 3, 0.01),
            ]
            .into_iter()
            .map(|(a, b, weight)| WeightedEdge { a, b, weight })
            .collect(),
        };
        let first = LeidenFfi.partition(&graph, 1.1, 7, None).unwrap();
        let second = LeidenFfi.partition(&graph, 1.1, 7, None).unwrap();
        assert_eq!(first.membership, second.membership);
        assert_eq!(first.membership[0], first.membership[2]);
        assert_eq!(first.membership[3], first.membership[5]);
        assert_ne!(first.membership[0], first.membership[3]);
    }

    // Not a degenerate corner for its own sake: `blobs::subdivide` hands the
    // partitioner a district's induced subgraph, and issue #41's zero-edge
    // districts induce an edgeless one. Without the guard libleidenalg returns
    // 0/0 here and the build fails instead of drawing the district.
    #[test]
    fn an_edgeless_graph_is_one_community_at_zero_modularity() {
        let graph = WeightedGraph {
            node_count: 5,
            edges: Vec::new(),
        };
        let result = LeidenFfi.partition(&graph, 1.1, 7, None).unwrap();
        assert_eq!(result.membership, vec![0; 5]);
        assert_eq!(result.modularity, 0.0);
    }

    #[test]
    fn an_edgeless_graph_ignores_a_warm_start() {
        let graph = WeightedGraph {
            node_count: 3,
            edges: Vec::new(),
        };
        let result = LeidenFfi
            .partition(&graph, 1.1, 7, Some(&[2, 1, 0]))
            .unwrap();
        // One community is the only answer whatever the seed membership said:
        // with no edges there is nothing for a warm start to preserve.
        assert_eq!(result.membership, vec![0; 3]);
        assert_eq!(result.modularity, 0.0);
    }

    #[test]
    fn accepts_an_initial_membership() {
        let graph = WeightedGraph {
            node_count: 2,
            edges: vec![WeightedEdge {
                a: 0,
                b: 1,
                weight: 1.0,
            }],
        };
        let result = LeidenFfi.partition(&graph, 1.1, 7, Some(&[0, 0])).unwrap();
        assert_eq!(result.membership.len(), 2);
    }
}
