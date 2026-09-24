use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::str::FromStr;

use anyhow::{ensure, Result};

use crate::extract::round_to;
use crate::partition::{PartitionResult, Partitioner};
use crate::schema::{GraphData, SignalEdge, WeightedEdge, WeightedGraph};
use crate::SEED;

const SHARE_STATIC: f64 = 0.45;
const SHARE_COCHANGE: f64 = 0.35;
const SHARE_PROXIMITY: f64 = 0.08;
const SHARE_SEMANTIC: f64 = 0.12;

pub const PRUNE_KEEP_PER_NODE: usize = 14;
pub const ABSOLUTE_FLOOR: f64 = 0.02;

// Calibrated from the nine acceptance fixtures at their data/fixtures.toml
// pins. See docs/PRUNE_VARIANTS.md for the full empirical derivation and
// remote run. Type-7 quantiles make this percentile's numerical floor 0.02
// on celery, the median fixture by the percentile rank of 0.02.
const PERCENTILE_FLOOR: f64 = 0.000_419_289_047_680_761_6;
const NODE_RELATIVE_FRACTION: f64 = 0.02;
// Nearest attainable empirical match to absolute's median below-floor share
// after moving the comparison before max-rescale (the distributions are
// discrete, so the target falls between adjacent steps).
const PRE_RESCALE_FLOOR: f64 = 0.000_077_526_049_820_726_55;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PruneVariant {
    Absolute,
    Percentile,
    #[default]
    NodeRelative,
    PreRescale,
}

impl fmt::Display for PruneVariant {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::Absolute => "absolute",
            Self::Percentile => "percentile",
            Self::NodeRelative => "node-relative",
            Self::PreRescale => "pre-rescale",
        })
    }
}

impl FromStr for PruneVariant {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "absolute" => Ok(Self::Absolute),
            "percentile" => Ok(Self::Percentile),
            "node-relative" => Ok(Self::NodeRelative),
            "pre-rescale" => Ok(Self::PreRescale),
            _ => Err(format!(
                "unknown prune variant {value:?}; expected absolute, percentile, node-relative, or pre-rescale"
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PruneStats {
    pub variant: PruneVariant,
    pub floor: f64,
    pub floor_basis: &'static str,
    pub candidate_edges: usize,
    /// Candidate-edge-order mask. Empty on the map-building path, which does
    /// not need instrumentation; populated by [`apply_prune_variant`].
    pub below_floor_edges: Vec<bool>,
    pub below_floor_count: usize,
    pub weight_sum_pre_prune: f64,
}

impl PruneStats {
    pub fn below_floor_share(&self) -> f64 {
        if self.candidate_edges == 0 {
            0.0
        } else {
            self.below_floor_count as f64 / self.candidate_edges as f64
        }
    }
}

#[derive(Clone, Debug)]
pub struct LayoutDistrict {
    pub centroid: [f64; 2],
    pub area: f64,
    pub rect: [f64; 4],
    pub size: usize,
}

#[derive(Clone, Debug)]
pub struct LayoutNode {
    pub district: usize,
    pub rect: [f64; 4],
    pub point: [f64; 2],
}

#[derive(Clone, Debug)]
pub struct Landmark {
    pub node: usize,
    pub why: String,
    pub detail: String,
    pub rank: usize,
}

#[derive(Clone, Debug)]
pub struct PipelineOutput {
    pub membership: Vec<usize>,
    pub modularity: f64,
    pub districts: BTreeMap<usize, LayoutDistrict>,
    pub nodes: Vec<LayoutNode>,
    pub landmarks: Vec<Landmark>,
    pub weighted: GraphData,
    pub graph: WeightedGraph,
}

pub fn run<P: Partitioner>(
    data: GraphData,
    resolution: f64,
    partitioner: &P,
    initial_membership: Option<&[usize]>,
) -> Result<PipelineOutput> {
    run_with_variant(
        data,
        resolution,
        partitioner,
        initial_membership,
        PruneVariant::default(),
    )
}

pub fn run_with_variant<P: Partitioner>(
    mut data: GraphData,
    resolution: f64,
    partitioner: &P,
    initial_membership: Option<&[usize]>,
    prune_variant: PruneVariant,
) -> Result<PipelineOutput> {
    run_with_variant_progress(
        data,
        resolution,
        partitioner,
        initial_membership,
        prune_variant,
        &crate::progress::Progress::silent(),
    )
}

pub fn run_with_variant_progress<P: Partitioner>(
    mut data: GraphData,
    resolution: f64,
    partitioner: &P,
    initial_membership: Option<&[usize]>,
    prune_variant: PruneVariant,
    progress: &crate::progress::Progress,
) -> Result<PipelineOutput> {
    let blend_stage = progress.stage(crate::progress::StageId::BlendPrune, Some(1));
    apply_prune_variant_inner(&mut data, prune_variant, false)?;
    blend_stage.set(1);
    blend_stage.finish();
    let graph = weighted_graph(&data)?;
    let partition_stage = progress.stage(crate::progress::StageId::Partition, None);
    let PartitionResult {
        membership,
        modularity,
    } = partitioner.partition(&graph, resolution, SEED, initial_membership)?;
    partition_stage.finish();
    let membership = merge_tiny(&membership, &data, 4);
    let (districts, nodes) = layout(&membership, &data);
    let landmarks = landmarks(&membership, &data, &graph);
    Ok(PipelineOutput {
        membership,
        modularity: round_to(modularity, 4),
        districts,
        nodes,
        landmarks,
        weighted: data,
        graph,
    })
}

/// Builds the `initial_membership` argument `run` passes to the partitioner
/// from a previous commit's file -> district map (finding 4: this is the
/// single highest-leverage step in the pipeline, 46% -> 88% district
/// retention on django at no modularity cost). Node order must match
/// `data.nodes` -- `weighted_graph` builds its node ids in that same order
/// and neither `blend` nor `prune` reorders or drops nodes (`prune` removes
/// only edges), so the caller does not need to know that internal detail to
/// get this right.
///
/// A file the previous commit did not have gets its own fresh singleton
/// community rather than joining an existing one or being left unset:
/// leidenalg requires every node to have an initial community, and seeding
/// a new file into an arbitrary existing one would bias it towards that
/// district before the algorithm has seen a single edge for it. This
/// mirrors `eval/batch_stability.py::partition_seeded`, which is the script
/// finding 4's numbers came from.
pub fn align_initial_membership(
    data: &GraphData,
    previous: &BTreeMap<String, usize>,
) -> Vec<usize> {
    let mut next_id = previous.values().copied().max().map_or(0, |max| max + 1);
    data.nodes
        .iter()
        .map(|node| {
            previous.get(&node.file).copied().unwrap_or_else(|| {
                let id = next_id;
                next_id += 1;
                id
            })
        })
        .collect()
}

pub fn blend(data: &mut GraphData) -> Result<()> {
    blend_mass_normalized(data)?;
    max_rescale(data)
}

/// Applies finding 1's per-signal mass normalisation without the final
/// global-maximum rescale. Kept separate so `dump-blend` can measure the
/// pre-rescale distribution finding 10 identifies; the shipped [`blend`]
/// still performs these same operations in the same order and then calls
/// [`max_rescale`].
pub fn blend_mass_normalized(data: &mut GraphData) -> Result<()> {
    ensure!(!data.edges.is_empty(), "cannot blend an empty graph");
    let (raw_static, raw_cochange, raw_proximity, raw_semantic) = signal_masses(data);
    for edge in &mut data.edges {
        edge.weight =
            mass_normalized_weight(edge, raw_static, raw_cochange, raw_proximity, raw_semantic);
    }
    Ok(())
}

/// The same pre-rescale weights as [`blend_mass_normalized`], without
/// cloning or mutating the graph. `dump-blend` uses this on repositories
/// whose extracted graph is already close to the runner's memory ceiling.
pub fn mass_normalized_weights(data: &GraphData) -> Result<Vec<f64>> {
    ensure!(!data.edges.is_empty(), "cannot blend an empty graph");
    let (raw_static, raw_cochange, raw_proximity, raw_semantic) = signal_masses(data);
    Ok(data
        .edges
        .iter()
        .map(|edge| {
            mass_normalized_weight(edge, raw_static, raw_cochange, raw_proximity, raw_semantic)
        })
        .collect())
}

fn signal_masses(data: &GraphData) -> (f64, f64, f64, f64) {
    let raw_static = data
        .edges
        .iter()
        .map(|edge| edge.static_signal)
        .sum::<f64>();
    let raw_cochange = data.edges.iter().map(|edge| edge.cochange).sum::<f64>();
    let raw_proximity = data.edges.iter().map(|edge| edge.proximity).sum::<f64>();
    let raw_semantic = data.edges.iter().map(|edge| edge.semantic).sum::<f64>();

    // Python's `sum(...) or 1.0` uses one only for an exactly zero mass.
    let raw_static = if raw_static == 0.0 { 1.0 } else { raw_static };
    let raw_cochange = if raw_cochange == 0.0 {
        1.0
    } else {
        raw_cochange
    };
    let raw_proximity = if raw_proximity == 0.0 {
        1.0
    } else {
        raw_proximity
    };
    let raw_semantic = if raw_semantic == 0.0 {
        1.0
    } else {
        raw_semantic
    };
    (raw_static, raw_cochange, raw_proximity, raw_semantic)
}

fn mass_normalized_weight(
    edge: &SignalEdge,
    raw_static: f64,
    raw_cochange: f64,
    raw_proximity: f64,
    raw_semantic: f64,
) -> f64 {
    SHARE_STATIC / raw_static * edge.static_signal
        + SHARE_COCHANGE / raw_cochange * edge.cochange
        + SHARE_PROXIMITY / raw_proximity * edge.proximity
        + SHARE_SEMANTIC / raw_semantic * edge.semantic
}

fn max_rescale(data: &mut GraphData) -> Result<()> {
    let maximum = data
        .edges
        .iter()
        .map(|edge| edge.weight)
        .fold(f64::NEG_INFINITY, f64::max);
    ensure!(
        maximum.is_finite() && maximum > 0.0,
        "invalid blended edge mass"
    );
    for edge in &mut data.edges {
        edge.weight /= maximum;
    }
    Ok(())
}

pub fn prune(data: &mut GraphData, keep_per_node: usize, floor: f64) {
    let mut incident = BTreeMap::<String, Vec<usize>>::new();
    for (index, edge) in data.edges.iter().enumerate() {
        incident.entry(edge.a.clone()).or_default().push(index);
        incident.entry(edge.b.clone()).or_default().push(index);
    }
    let mut keep = BTreeSet::<(String, String)>::new();
    for edges in incident.values_mut() {
        edges.sort_by(|left, right| {
            data.edges[*right]
                .weight
                .partial_cmp(&data.edges[*left].weight)
                .unwrap_or(Ordering::Equal)
        });
        for &index in edges.iter().take(keep_per_node) {
            let edge = &data.edges[index];
            if edge.weight >= floor {
                keep.insert((edge.a.clone(), edge.b.clone()));
            }
        }
    }
    data.edges
        .retain(|edge| keep.contains(&(edge.a.clone(), edge.b.clone())));
}

/// Applies one complete blend/prune route. `Absolute` preserves the original
/// two calls verbatim, including comparison strictness and edge ordering, so
/// explicit `absolute` measurements remain comparable with the old default.
pub fn apply_prune_variant(data: &mut GraphData, variant: PruneVariant) -> Result<PruneStats> {
    apply_prune_variant_inner(data, variant, true)
}

fn apply_prune_variant_inner(
    data: &mut GraphData,
    variant: PruneVariant,
    collect_floor_edges: bool,
) -> Result<PruneStats> {
    match variant {
        PruneVariant::Absolute => {
            blend(data)?;
            let stats = prune_stats(
                data,
                variant,
                ABSOLUTE_FLOOR,
                "max-rescaled absolute",
                collect_floor_edges,
            );
            prune(data, PRUNE_KEEP_PER_NODE, ABSOLUTE_FLOOR);
            Ok(stats)
        }
        PruneVariant::Percentile => {
            blend(data)?;
            let floor = quantile_type7(
                &data
                    .edges
                    .iter()
                    .map(|edge| edge.weight)
                    .collect::<Vec<_>>(),
                PERCENTILE_FLOOR,
            );
            let stats = prune_stats(
                data,
                variant,
                floor,
                "max-rescaled percentile",
                collect_floor_edges,
            );
            prune(data, PRUNE_KEEP_PER_NODE, floor);
            Ok(stats)
        }
        PruneVariant::NodeRelative => {
            blend(data)?;
            let strongest = strongest_incident(data);
            let below_floor = if collect_floor_edges {
                data.edges
                    .iter()
                    .filter(|edge| {
                        edge.weight < NODE_RELATIVE_FRACTION * strongest[edge.a.as_str()]
                            && edge.weight < NODE_RELATIVE_FRACTION * strongest[edge.b.as_str()]
                    })
                    .count()
            } else {
                0
            };
            let below_floor_edges = if collect_floor_edges {
                data.edges
                    .iter()
                    .map(|edge| {
                        edge.weight < NODE_RELATIVE_FRACTION * strongest[edge.a.as_str()]
                            && edge.weight < NODE_RELATIVE_FRACTION * strongest[edge.b.as_str()]
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let stats = PruneStats {
                variant,
                floor: NODE_RELATIVE_FRACTION,
                floor_basis: "fraction of strongest incident edge",
                candidate_edges: data.edges.len(),
                below_floor_edges,
                below_floor_count: below_floor,
                weight_sum_pre_prune: if collect_floor_edges {
                    data.edges.iter().map(|edge| edge.weight).sum()
                } else {
                    0.0
                },
            };
            prune_node_relative(
                data,
                PRUNE_KEEP_PER_NODE,
                NODE_RELATIVE_FRACTION,
                &strongest,
            );
            Ok(stats)
        }
        PruneVariant::PreRescale => {
            blend_mass_normalized(data)?;
            let stats = prune_stats(
                data,
                variant,
                PRE_RESCALE_FLOOR,
                "mass-normalized pre-rescale absolute",
                collect_floor_edges,
            );
            prune(data, PRUNE_KEEP_PER_NODE, PRE_RESCALE_FLOOR);
            max_rescale(data)?;
            Ok(stats)
        }
    }
}

fn prune_stats(
    data: &GraphData,
    variant: PruneVariant,
    floor: f64,
    floor_basis: &'static str,
    collect_floor_edges: bool,
) -> PruneStats {
    let below_floor_count = if collect_floor_edges {
        data.edges.iter().filter(|edge| edge.weight < floor).count()
    } else {
        0
    };
    PruneStats {
        variant,
        floor,
        floor_basis,
        candidate_edges: data.edges.len(),
        below_floor_edges: if collect_floor_edges {
            data.edges.iter().map(|edge| edge.weight < floor).collect()
        } else {
            Vec::new()
        },
        below_floor_count,
        weight_sum_pre_prune: if collect_floor_edges {
            data.edges.iter().map(|edge| edge.weight).sum()
        } else {
            0.0
        },
    }
}

fn edge_key(edge: &SignalEdge) -> (String, String) {
    (edge.a.clone(), edge.b.clone())
}

fn quantile_type7(values: &[f64], percentile: f64) -> f64 {
    debug_assert!(!values.is_empty());
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    if sorted.len() == 1 {
        return sorted[0];
    }
    let position = percentile.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let fraction = position - lower as f64;
    sorted[lower] + fraction * (sorted[upper] - sorted[lower])
}

fn strongest_incident(data: &GraphData) -> BTreeMap<String, f64> {
    let mut strongest = BTreeMap::<String, f64>::new();
    for edge in &data.edges {
        for file in [&edge.a, &edge.b] {
            strongest
                .entry(file.clone())
                .and_modify(|weight| *weight = weight.max(edge.weight))
                .or_insert(edge.weight);
        }
    }
    strongest
}

fn prune_node_relative(
    data: &mut GraphData,
    keep_per_node: usize,
    fraction: f64,
    strongest: &BTreeMap<String, f64>,
) {
    let mut incident = BTreeMap::<String, Vec<usize>>::new();
    for (index, edge) in data.edges.iter().enumerate() {
        incident.entry(edge.a.clone()).or_default().push(index);
        incident.entry(edge.b.clone()).or_default().push(index);
    }
    let mut keep = BTreeSet::<(String, String)>::new();
    for (file, edges) in &mut incident {
        edges.sort_by(|left, right| {
            data.edges[*right]
                .weight
                .partial_cmp(&data.edges[*left].weight)
                .unwrap_or(Ordering::Equal)
        });
        let floor = fraction * strongest[file.as_str()];
        for &index in edges.iter().take(keep_per_node) {
            let edge = &data.edges[index];
            if edge.weight >= floor {
                keep.insert(edge_key(edge));
            }
        }
    }
    data.edges.retain(|edge| keep.contains(&edge_key(edge)));
}

fn weighted_graph(data: &GraphData) -> Result<WeightedGraph> {
    let index = data
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.file.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut edges = Vec::with_capacity(data.edges.len());
    for edge in &data.edges {
        edges.push(WeightedEdge {
            a: *index
                .get(edge.a.as_str())
                .ok_or_else(|| anyhow::anyhow!("edge references missing file {}", edge.a))?,
            b: *index
                .get(edge.b.as_str())
                .ok_or_else(|| anyhow::anyhow!("edge references missing file {}", edge.b))?,
            weight: edge.weight,
        });
    }
    Ok(WeightedGraph {
        node_count: data.nodes.len(),
        edges,
    })
}

pub fn merge_tiny(membership: &[usize], data: &GraphData, minimum: usize) -> Vec<usize> {
    let mut membership = membership.to_vec();
    let mut size_order = Vec::new();
    let mut sizes = BTreeMap::<usize, usize>::new();
    for &community in &membership {
        if !sizes.contains_key(&community) {
            size_order.push(community);
        }
        *sizes.entry(community).or_default() += 1;
    }
    let index = data
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.file.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut between = BTreeMap::<(usize, usize), f64>::new();
    for edge in &data.edges {
        let a = membership[index[edge.a.as_str()]];
        let b = membership[index[edge.b.as_str()]];
        if a != b {
            *between.entry((a, b)).or_default() += edge.weight;
            *between.entry((b, a)).or_default() += edge.weight;
        }
    }
    let order_position = size_order
        .iter()
        .enumerate()
        .map(|(position, &community)| (community, position))
        .collect::<BTreeMap<_, _>>();
    let mut communities = size_order.clone();
    communities.sort_by_key(|community| (sizes[community], order_position[community]));
    for community in communities {
        let count = sizes[&community];
        if count >= minimum || count == 0 {
            continue;
        }
        let mut options = between
            .iter()
            .filter_map(|(&(from, target), &weight)| {
                (from == community && sizes.get(&target).copied().unwrap_or(0) >= minimum)
                    .then_some((weight, target))
            })
            .collect::<Vec<_>>();
        options.sort_by(|left, right| {
            left.0
                .partial_cmp(&right.0)
                .unwrap_or(Ordering::Equal)
                .then(left.1.cmp(&right.1))
        });
        let target = if let Some((_, target)) = options.last() {
            *target
        } else {
            let orphan = membership
                .iter()
                .position(|value| *value == community)
                .expect("community has a member");
            let directory = directory_name(&data.nodes[orphan].file);
            sibling_target(&membership, data, community, directory, &sizes, minimum)
                .or_else(|| {
                    let parent = directory_name(directory);
                    parent_target(&membership, data, community, parent, &sizes, minimum)
                })
                .unwrap_or(community)
        };
        if target == community {
            continue;
        }
        for value in &mut membership {
            if *value == community {
                *value = target;
            }
        }
        *sizes.entry(target).or_default() += count;
        sizes.insert(community, 0);
    }

    let mut encounter = Vec::new();
    let mut final_sizes = BTreeMap::<usize, usize>::new();
    for &community in &membership {
        if !final_sizes.contains_key(&community) {
            encounter.push(community);
        }
        *final_sizes.entry(community).or_default() += 1;
    }
    let encounter_position = encounter
        .iter()
        .enumerate()
        .map(|(index, &community)| (community, index))
        .collect::<BTreeMap<_, _>>();
    encounter.sort_by_key(|community| {
        (
            std::cmp::Reverse(final_sizes[community]),
            encounter_position[community],
        )
    });
    let remap = encounter
        .into_iter()
        .enumerate()
        .map(|(new, old)| (old, new))
        .collect::<BTreeMap<_, _>>();
    membership.into_iter().map(|value| remap[&value]).collect()
}

fn sibling_target(
    membership: &[usize],
    data: &GraphData,
    community: usize,
    directory: &str,
    _sizes: &BTreeMap<usize, usize>,
    _minimum: usize,
) -> Option<usize> {
    counter_mode(
        membership
            .iter()
            .enumerate()
            .filter(|(index, value)| {
                **value != community && directory_name(&data.nodes[*index].file) == directory
            })
            .map(|(_, value)| *value),
    )
}

fn parent_target(
    membership: &[usize],
    data: &GraphData,
    community: usize,
    parent: &str,
    sizes: &BTreeMap<usize, usize>,
    minimum: usize,
) -> Option<usize> {
    counter_mode(
        membership
            .iter()
            .enumerate()
            .filter(|(index, value)| {
                **value != community
                    && sizes.get(value).copied().unwrap_or(0) >= minimum
                    && directory_name(&data.nodes[*index].file).starts_with(parent)
            })
            .map(|(_, value)| *value),
    )
}

fn counter_mode(values: impl Iterator<Item = usize>) -> Option<usize> {
    let mut counts = BTreeMap::<usize, (usize, usize)>::new();
    let mut next_order = 0;
    for value in values {
        let entry = counts.entry(value).or_insert_with(|| {
            let order = next_order;
            next_order += 1;
            (0, order)
        });
        entry.0 += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, (count, order))| (*count, std::cmp::Reverse(*order)))
        .map(|(value, _)| value)
}

fn layout(
    membership: &[usize],
    data: &GraphData,
) -> (BTreeMap<usize, LayoutDistrict>, Vec<LayoutNode>) {
    let district_count = membership
        .iter()
        .copied()
        .max()
        .map_or(0, |value| value + 1);
    let district_edges = aggregate_edges(membership, &data.edges, data);
    let centroids = force_layout(district_count, &district_edges, 400, 1.1, SEED);
    let centroids = normalize_points(&centroids);
    let mut members = vec![Vec::<usize>::new(); district_count];
    for (index, &district) in membership.iter().enumerate() {
        members[district].push(index);
    }
    let total = membership.len().max(1) as f64;
    let mut districts = BTreeMap::new();
    let mut nodes = vec![
        LayoutNode {
            district: 0,
            rect: [0.0; 4],
            point: [0.0; 2],
        };
        membership.len()
    ];
    for (district, district_members) in members.iter_mut().enumerate() {
        let area = district_members.len() as f64 / total;
        let side = area.sqrt() * 0.62;
        let [center_x, center_y] = centroids[district];
        let rect = [center_x - side / 2.0, center_y - side / 2.0, side, side];
        districts.insert(
            district,
            LayoutDistrict {
                centroid: [round_to(center_x, 4), round_to(center_y, 4)],
                area: round_to(area, 4),
                rect: rect.map(|value| round_to(value, 4)),
                size: district_members.len(),
            },
        );
        district_members.sort_by(|left, right| {
            data.nodes[*right]
                .loc
                .cmp(&data.nodes[*left].loc)
                .then(data.nodes[*left].file.cmp(&data.nodes[*right].file))
        });
        let values = district_members
            .iter()
            .map(|index| data.nodes[*index].loc as f64)
            .collect::<Vec<_>>();
        for (&index, packed) in district_members.iter().zip(squarify(&values, rect)) {
            nodes[index] = LayoutNode {
                district,
                rect: packed.map(|value| round_to(value, 5)),
                point: [
                    round_to(packed[0] + packed[2] / 2.0, 5),
                    round_to(packed[1] + packed[3] / 2.0, 5),
                ],
            };
        }
    }
    (districts, nodes)
}

fn aggregate_edges(
    membership: &[usize],
    edges: &[SignalEdge],
    data: &GraphData,
) -> Vec<WeightedEdge> {
    let index = data
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.file.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut weights = BTreeMap::<(usize, usize), f64>::new();
    for edge in edges {
        let a = membership[index[edge.a.as_str()]];
        let b = membership[index[edge.b.as_str()]];
        if a != b {
            let pair = if a < b { (a, b) } else { (b, a) };
            *weights.entry(pair).or_default() += edge.weight;
        }
    }
    weights
        .into_iter()
        .map(|((a, b), weight)| WeightedEdge { a, b, weight })
        .collect()
}

pub(crate) fn force_layout(
    count: usize,
    edges: &[WeightedEdge],
    iterations: usize,
    ideal: f64,
    seed: u64,
) -> Vec<[f64; 2]> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![[0.0, 0.0]];
    }
    let mut points = (0..count)
        .map(|index| {
            let angle =
                (index as f64 * 2.399_963_229_728_653 + seed as f64 * 0.17) % std::f64::consts::TAU;
            let radius = 0.35 + 0.65 * ((index + 1) as f64 / count as f64).sqrt();
            [radius * angle.cos(), radius * angle.sin()]
        })
        .collect::<Vec<_>>();
    let area = 4.0;
    let k = ideal * (area / count as f64).sqrt();
    for iteration in 0..iterations {
        let mut displacement = vec![[0.0, 0.0]; count];
        for i in 0..count {
            for j in i + 1..count {
                let delta = [points[i][0] - points[j][0], points[i][1] - points[j][1]];
                let distance = hypot(delta).max(1e-9);
                let force = k * k / distance;
                let vector = [delta[0] / distance * force, delta[1] / distance * force];
                displacement[i][0] += vector[0];
                displacement[i][1] += vector[1];
                displacement[j][0] -= vector[0];
                displacement[j][1] -= vector[1];
            }
        }
        for edge in edges {
            let delta = [
                points[edge.a][0] - points[edge.b][0],
                points[edge.a][1] - points[edge.b][1],
            ];
            let distance = hypot(delta).max(1e-9);
            let force = distance * distance / k * edge.weight.max(1e-9).sqrt();
            let vector = [delta[0] / distance * force, delta[1] / distance * force];
            displacement[edge.a][0] -= vector[0];
            displacement[edge.a][1] -= vector[1];
            displacement[edge.b][0] += vector[0];
            displacement[edge.b][1] += vector[1];
        }
        let temperature = 0.2 * (1.0 - iteration as f64 / iterations as f64).max(0.01);
        for index in 0..count {
            let length = hypot(displacement[index]).max(1e-9);
            points[index][0] += displacement[index][0] / length * length.min(temperature);
            points[index][1] += displacement[index][1] / length * length.min(temperature);
        }
        let mean = points.iter().fold([0.0, 0.0], |mut total, point| {
            total[0] += point[0];
            total[1] += point[1];
            total
        });
        for point in &mut points {
            point[0] -= mean[0] / count as f64;
            point[1] -= mean[1] / count as f64;
        }
    }
    points
}

fn normalize_points(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    if points.is_empty() {
        return Vec::new();
    }
    let x_min = points
        .iter()
        .map(|point| point[0])
        .fold(f64::INFINITY, f64::min);
    let x_max = points
        .iter()
        .map(|point| point[0])
        .fold(f64::NEG_INFINITY, f64::max);
    let y_min = points
        .iter()
        .map(|point| point[1])
        .fold(f64::INFINITY, f64::min);
    let y_max = points
        .iter()
        .map(|point| point[1])
        .fold(f64::NEG_INFINITY, f64::max);
    let x_span = (x_max - x_min).max(f64::EPSILON);
    let y_span = (y_max - y_min).max(f64::EPSILON);
    points
        .iter()
        .map(|point| [(point[0] - x_min) / x_span, (point[1] - y_min) / y_span])
        .collect()
}

pub(crate) fn squarify(values: &[f64], rect: [f64; 4]) -> Vec<[f64; 4]> {
    let [mut x, mut y, mut width, mut height] = rect;
    let total = values.iter().sum::<f64>().max(1.0);
    let values = values
        .iter()
        .map(|value| value * width * height / total)
        .collect::<Vec<_>>();
    let mut output = vec![[0.0; 4]; values.len()];
    let mut index = 0;
    while index < values.len() {
        let mut row = Vec::<(usize, f64)>::new();
        let length = width.min(height);
        while index < values.len() {
            let mut candidate = row.clone();
            candidate.push((index, values[index]));
            if !row.is_empty() && worst(&candidate, length) > worst(&row, length) {
                break;
            }
            row = candidate;
            index += 1;
        }
        let sum = row.iter().map(|(_, value)| value).sum::<f64>();
        if width >= height {
            let row_width = if height == 0.0 { 0.0 } else { sum / height };
            let mut offset_y = y;
            for (item, value) in row {
                let row_height = if sum == 0.0 {
                    0.0
                } else {
                    value / sum * height
                };
                output[item] = [x, offset_y, row_width, row_height];
                offset_y += row_height;
            }
            x += row_width;
            width -= row_width;
        } else {
            let row_height = if width == 0.0 { 0.0 } else { sum / width };
            let mut offset_x = x;
            for (item, value) in row {
                let row_width = if sum == 0.0 { 0.0 } else { value / sum * width };
                output[item] = [offset_x, y, row_width, row_height];
                offset_x += row_width;
            }
            y += row_height;
            height -= row_height;
        }
    }
    output
}

fn worst(row: &[(usize, f64)], length: f64) -> f64 {
    if row.is_empty() || length == 0.0 {
        return f64::INFINITY;
    }
    let sum = row.iter().map(|(_, value)| value).sum::<f64>();
    let maximum = row
        .iter()
        .map(|(_, value)| *value)
        .fold(f64::NEG_INFINITY, f64::max);
    let minimum = row
        .iter()
        .map(|(_, value)| *value)
        .fold(f64::INFINITY, f64::min);
    if sum == 0.0 || minimum == 0.0 {
        return f64::INFINITY;
    }
    (length * length * maximum / (sum * sum)).max(sum * sum / (length * length * minimum))
}

fn landmarks(membership: &[usize], data: &GraphData, graph: &WeightedGraph) -> Vec<Landmark> {
    let adjacency = adjacency(graph);
    let betweenness = betweenness_centrality(&adjacency, 120, SEED);
    let mut picks = Vec::<Landmark>::new();
    let structural = |index: usize| {
        let base = data.nodes[index]
            .file
            .rsplit('/')
            .next()
            .unwrap_or(&data.nodes[index].file);
        base != "__init__.py" && !matches!(base, "exceptions.py" | "errors.py")
    };
    let entry_hints = [
        "__init__.py",
        "cmdline.py",
        "app.py",
        "main.py",
        "cli.py",
        "crawler.py",
        "__main__.py",
    ];
    let mut entries = (0..data.nodes.len())
        .filter(|&index| {
            let base = data.nodes[index].file.rsplit('/').next().unwrap_or("");
            entry_hints.contains(&base) && data.nodes[index].loc > 40
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        data.nodes[*right]
            .fanin
            .partial_cmp(&data.nodes[*left].fanin)
            .unwrap_or(Ordering::Equal)
    });
    for index in entries.into_iter().take(2) {
        add_landmark(
            &mut picks,
            index,
            "entry",
            format!("{} loc", data.nodes[index].loc),
        );
    }

    let mut bridges = (0..data.nodes.len())
        .filter(|index| structural(*index))
        .collect::<Vec<_>>();
    bridges.sort_by(|left, right| {
        betweenness[*right]
            .partial_cmp(&betweenness[*left])
            .unwrap_or(Ordering::Equal)
    });
    for index in bridges.into_iter().take(2) {
        add_landmark(
            &mut picks,
            index,
            "bridge",
            format!("betweenness {:.3}", betweenness[index]),
        );
    }

    let mut hubs = (0..data.nodes.len())
        .filter(|index| structural(*index))
        .collect::<Vec<_>>();
    hubs.sort_by(|left, right| {
        data.nodes[*right]
            .fanin
            .partial_cmp(&data.nodes[*left].fanin)
            .unwrap_or(Ordering::Equal)
    });
    for index in hubs.into_iter().take(2) {
        add_landmark(
            &mut picks,
            index,
            "hub",
            format!(
                "fan-in {}",
                python_number(data.nodes[index].fanin, &data.lang)
            ),
        );
    }

    let degree = weighted_degree(graph);
    let mut by_district = BTreeMap::<usize, Vec<usize>>::new();
    for (index, &district) in membership.iter().enumerate() {
        by_district.entry(district).or_default().push(index);
    }
    let mut districts = by_district.into_iter().collect::<Vec<_>>();
    districts.sort_by_key(|(district, files)| (std::cmp::Reverse(files.len()), *district));
    for (district, mut files) in districts {
        files.retain(|index| structural(*index));
        files.sort_by(|left, right| {
            degree[*right]
                .partial_cmp(&degree[*left])
                .unwrap_or(Ordering::Equal)
        });
        if let Some(index) = files
            .into_iter()
            .find(|index| !picks.iter().any(|pick| pick.node == *index))
        {
            add_landmark(
                &mut picks,
                index,
                "capital",
                format!("district {district} centre"),
            );
        }
    }

    let mut hazards = (0..data.nodes.len()).collect::<Vec<_>>();
    hazards.sort_by_key(|index| {
        std::cmp::Reverse(data.nodes[*index].churn * data.nodes[*index].complexity)
    });
    for index in hazards.into_iter().take(2) {
        add_landmark(
            &mut picks,
            index,
            "hazard",
            format!(
                "churn {} x cplx {}",
                data.nodes[index].churn, data.nodes[index].complexity
            ),
        );
    }
    for (rank, pick) in picks.iter_mut().enumerate() {
        pick.rank = rank + 1;
    }
    picks
}

fn add_landmark(picks: &mut Vec<Landmark>, index: usize, why: &str, detail: String) {
    if picks.iter().any(|pick| pick.node == index) {
        return;
    }
    picks.push(Landmark {
        node: index,
        why: why.to_owned(),
        detail,
        rank: 0,
    });
}

fn python_number(value: f64, language: &str) -> String {
    if language == "py" || value == 0.0 {
        return format!("{value:.0}");
    }
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

fn adjacency(graph: &WeightedGraph) -> Vec<Vec<usize>> {
    let mut adjacency = vec![Vec::new(); graph.node_count];
    for edge in &graph.edges {
        adjacency[edge.a].push(edge.b);
        adjacency[edge.b].push(edge.a);
    }
    for neighbours in &mut adjacency {
        neighbours.sort_unstable();
        neighbours.dedup();
    }
    adjacency
}

fn weighted_degree(graph: &WeightedGraph) -> Vec<f64> {
    let mut result = vec![0.0; graph.node_count];
    for edge in &graph.edges {
        result[edge.a] += edge.weight;
        result[edge.b] += edge.weight;
    }
    result
}

fn betweenness_centrality(adjacency: &[Vec<usize>], sample_size: usize, seed: u64) -> Vec<f64> {
    let count = adjacency.len();
    if count == 0 {
        return Vec::new();
    }
    let sample_count = count.min(sample_size);
    let sampled = sample_count != count;
    let sources = if sampled {
        python_sample(count, sample_count, seed)
    } else {
        (0..count).collect::<Vec<_>>()
    };
    // Needed only to tell a sampled source from a non-source when rescaling
    // below -- see the comment there for why that distinction matters.
    let source_set: BTreeSet<usize> = if sampled {
        sources.iter().copied().collect()
    } else {
        BTreeSet::new()
    };
    let mut centrality = vec![0.0; count];
    for source in sources {
        let mut stack = Vec::with_capacity(count);
        let mut predecessors = vec![Vec::<usize>::new(); count];
        let mut sigma = vec![0.0; count];
        sigma[source] = 1.0;
        let mut distance = vec![-1_isize; count];
        distance[source] = 0;
        let mut queue = VecDeque::from([source]);
        while let Some(vertex) = queue.pop_front() {
            stack.push(vertex);
            for &neighbour in &adjacency[vertex] {
                if distance[neighbour] < 0 {
                    queue.push_back(neighbour);
                    distance[neighbour] = distance[vertex] + 1;
                }
                if distance[neighbour] == distance[vertex] + 1 {
                    sigma[neighbour] += sigma[vertex];
                    predecessors[neighbour].push(vertex);
                }
            }
        }
        let mut dependency = vec![0.0; count];
        while let Some(vertex) = stack.pop() {
            let coefficient = (1.0 + dependency[vertex]) / sigma[vertex];
            for &predecessor in &predecessors[vertex] {
                dependency[predecessor] += sigma[predecessor] * coefficient;
            }
            if vertex != source {
                centrality[vertex] += dependency[vertex];
            }
        }
    }
    if count > 2 {
        if sampled {
            // networkx's `_rescale` (networkx.algorithms.centrality.betweenness),
            // called with `endpoints=False` and a sample of `k` source nodes,
            // does NOT apply one scale to every node. A node that was itself
            // one of the k sampled sources can't be its own source, so its
            // count of possible (s, t) pairs runs over K_source - 1 choices of
            // s; every other node's runs over the full K_source. Collapsing
            // that to a single uniform `n / (k * (n-1) * (n-2))` factor (as an
            // earlier version of this function did) overstates every
            // non-source node's centrality by a `(k-1)/k`-ish factor --
            // measured on scrapy: 0.03868767645547809 (uniform scale) against
            // networkx's 0.03848189094241703 for scrapy/utils/python.py, a
            // node that was not sampled -- 0.53% high, enough on its own to
            // flip the landmark's rounded digit from .038 to .039. The raw
            // (pre-scale) accumulated dependency sums already matched
            // networkx's to 1e-13; only the rescale differed.
            let k = sample_count as f64;
            let n_minus_2 = (count - 2) as f64;
            let scale_nonsource = 1.0 / (k * n_minus_2);
            let scale_source = if sample_count > 1 {
                1.0 / ((k - 1.0) * n_minus_2)
            } else {
                f64::NAN
            };
            for (index, value) in centrality.iter_mut().enumerate() {
                *value *= if source_set.contains(&index) {
                    scale_source
                } else {
                    scale_nonsource
                };
            }
        } else {
            let scale = 1.0 / ((count - 1) * (count - 2)) as f64;
            for value in &mut centrality {
                *value *= scale;
            }
        }
    }
    centrality
}

fn python_sample(population: usize, count: usize, seed: u64) -> Vec<usize> {
    let mut random = PythonRandom::new(seed);
    let mut pool = (0..population).collect::<Vec<_>>();
    let mut result = Vec::with_capacity(count);
    for index in 0..count {
        let choice = random.rand_below(population - index);
        result.push(pool[choice]);
        pool[choice] = pool[population - index - 1];
    }
    result
}

struct PythonRandom {
    state: [u32; 624],
    index: usize,
}

impl PythonRandom {
    fn new(seed: u64) -> Self {
        let mut random = Self {
            state: [0; 624],
            index: 624,
        };
        let key = [seed as u32, (seed >> 32) as u32];
        let key = if key[1] == 0 { &key[..1] } else { &key[..] };
        random.init_by_array(key);
        random
    }

    fn init_genrand(&mut self, seed: u32) {
        self.state[0] = seed;
        for index in 1..624 {
            self.state[index] = 1_812_433_253_u32
                .wrapping_mul(self.state[index - 1] ^ (self.state[index - 1] >> 30))
                .wrapping_add(index as u32);
        }
        self.index = 624;
    }

    fn init_by_array(&mut self, key: &[u32]) {
        self.init_genrand(19_650_218);
        let mut i = 1;
        let mut j = 0;
        for _ in 0..624.max(key.len()) {
            self.state[i] = (self.state[i]
                ^ (self.state[i - 1] ^ (self.state[i - 1] >> 30)).wrapping_mul(1_664_525))
            .wrapping_add(key[j])
            .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= 624 {
                self.state[0] = self.state[623];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..623 {
            self.state[i] = (self.state[i]
                ^ (self.state[i - 1] ^ (self.state[i - 1] >> 30)).wrapping_mul(1_566_083_941))
            .wrapping_sub(i as u32);
            i += 1;
            if i >= 624 {
                self.state[0] = self.state[623];
                i = 1;
            }
        }
        self.state[0] = 0x8000_0000;
    }

    fn next_u32(&mut self) -> u32 {
        if self.index >= 624 {
            for index in 0..624 {
                let value = (self.state[index] & 0x8000_0000)
                    | (self.state[(index + 1) % 624] & 0x7fff_ffff);
                self.state[index] = self.state[(index + 397) % 624]
                    ^ (value >> 1)
                    ^ if value & 1 == 0 { 0 } else { 0x9908_b0df };
            }
            self.index = 0;
        }
        let mut value = self.state[self.index];
        self.index += 1;
        value ^= value >> 11;
        value ^= (value << 7) & 0x9d2c_5680;
        value ^= (value << 15) & 0xefc6_0000;
        value ^= value >> 18;
        value
    }

    fn rand_below(&mut self, limit: usize) -> usize {
        let bits = usize::BITS - limit.leading_zeros();
        loop {
            let value = (self.next_u32() >> (32 - bits)) as usize;
            if value < limit {
                return value;
            }
        }
    }
}

fn directory_name(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(directory, _)| directory)
}

fn hypot(point: [f64; 2]) -> f64 {
    (point[0] * point[0] + point[1] * point[1]).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dominant_outlier_graph() -> GraphData {
        let edge = |a: &str, b: &str, static_signal: f64| SignalEdge {
            a: a.to_owned(),
            b: b.to_owned(),
            weight: 0.0,
            static_signal,
            cochange: 0.0,
            proximity: 0.0,
            semantic: 0.0,
        };
        GraphData {
            repo: "outlier".to_owned(),
            pkg: ".".to_owned(),
            lang: "py".to_owned(),
            sources: Vec::new(),
            imports: Vec::new(),
            symbols: BTreeMap::new(),
            uses: Vec::new(),
            commits_scanned: 0,
            nodes: Vec::new(),
            edges: vec![
                edge("a", "b", 10.0),
                edge("b", "c", 10.0),
                edge("d", "e", 10.0),
                edge("e", "f", 10.0),
                edge("x", "y", 1_000.0),
            ],
        }
    }

    fn edge_names(data: &GraphData) -> BTreeSet<(String, String)> {
        data.edges.iter().map(edge_key).collect()
    }

    fn local_structure() -> BTreeSet<(String, String)> {
        [("a", "b"), ("b", "c"), ("d", "e"), ("e", "f")]
            .into_iter()
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
            .collect()
    }

    #[test]
    fn python_rng_matches_cpython_seed_seven() {
        let mut random = PythonRandom::new(7);
        // random.Random(7).getrandbits(32), repeated.
        assert_eq!(random.next_u32(), 1_390_851_128);
        assert_eq!(random.next_u32(), 4_071_050_724);
    }

    #[test]
    fn sampled_betweenness_scales_a_sampled_source_differently_from_a_non_source() {
        // Ground truth from the reference's own library:
        //   nx.betweenness_centrality(nx.path_graph(5), weight=None, seed=7, k=3)
        //   -> {0: 0.0, 1: 0.333333, 2: 0.666667, 3: 0.333333, 4: 0.0}
        // random.Random(7).sample([0,1,2,3,4], 3) == [2, 1, 3] -- nodes 1, 2, 3
        // are themselves sampled sources, 0 and 4 are not, so this single graph
        // exercises both branches of the rescale. Before the fix, a single
        // uniform scale (n / (k*(n-1)*(n-2))) was applied to every node
        // regardless, which is wrong whenever endpoints=False and k < n (see
        // the comment on the rescale below) -- it reproduced neither the
        // source nor the non-source figure here.
        let adjacency = vec![vec![1], vec![0, 2], vec![1, 3], vec![2, 4], vec![3]];
        let result = betweenness_centrality(&adjacency, 3, 7);
        let expected = [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0 / 3.0, 0.0];
        for (value, expect) in result.iter().zip(expected) {
            assert!(
                (value - expect).abs() < 1e-9,
                "got {result:?}, want {expected:?}"
            );
        }
    }

    #[test]
    fn mass_normalization_assigns_each_signal_its_share() {
        let mut data = GraphData {
            repo: String::new(),
            pkg: String::new(),
            lang: "py".to_owned(),
            sources: Vec::new(),
            imports: Vec::new(),
            symbols: BTreeMap::new(),
            uses: Vec::new(),
            commits_scanned: 0,
            nodes: Vec::new(),
            edges: vec![SignalEdge {
                a: "a".to_owned(),
                b: "b".to_owned(),
                weight: 0.0,
                static_signal: 2.0,
                cochange: 3.0,
                proximity: 4.0,
                semantic: 5.0,
            }],
        };
        blend(&mut data).unwrap();
        assert_eq!(data.edges[0].weight, 1.0);
    }

    #[test]
    fn absolute_variant_loses_structure_below_the_global_outlier_floor() {
        let mut data = dominant_outlier_graph();
        apply_prune_variant(&mut data, PruneVariant::Absolute).unwrap();
        assert_eq!(edge_names(&data), [("x".to_owned(), "y".to_owned())].into());
    }

    #[test]
    fn percentile_variant_keeps_the_per_node_structure_around_an_outlier() {
        let mut data = dominant_outlier_graph();
        apply_prune_variant(&mut data, PruneVariant::Percentile).unwrap();
        assert!(local_structure().is_subset(&edge_names(&data)));
    }

    #[test]
    fn node_relative_variant_keeps_the_per_node_structure_around_an_outlier() {
        let mut data = dominant_outlier_graph();
        apply_prune_variant(&mut data, PruneVariant::NodeRelative).unwrap();
        assert!(local_structure().is_subset(&edge_names(&data)));
    }

    #[test]
    fn pre_rescale_variant_keeps_the_per_node_structure_around_an_outlier() {
        let mut data = dominant_outlier_graph();
        apply_prune_variant(&mut data, PruneVariant::PreRescale).unwrap();
        assert!(local_structure().is_subset(&edge_names(&data)));
    }

    #[test]
    fn squarified_rectangles_preserve_total_area() {
        let rows = squarify(&[3.0, 2.0, 1.0], [0.0, 0.0, 2.0, 3.0]);
        let area: f64 = rows.iter().map(|row| row[2] * row[3]).sum();
        assert!((area - 6.0).abs() < 1e-9);
    }
}
