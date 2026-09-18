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
        ensure!(modularity.is_finite(), "libleidenalg returned non-finite modularity");
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
