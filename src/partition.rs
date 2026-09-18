mod leiden_ffi;

use anyhow::Result;

use crate::schema::WeightedGraph;

#[derive(Clone, Debug)]
pub struct PartitionResult {
    pub membership: Vec<usize>,
    pub modularity: f64,
}

pub trait Partitioner {
    fn partition(
        &self,
        graph: &WeightedGraph,
        resolution: f64,
        seed: u64,
        initial: Option<&[usize]>,
    ) -> Result<PartitionResult>;
}

pub use leiden_ffi::LeidenFfi;

