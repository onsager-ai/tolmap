use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::Result;

use crate::partition::Partitioner;
use crate::pipeline::{force_layout, PipelineOutput};
use crate::schema::{RoadRow, WeightedEdge, WeightedGraph};
use crate::terrain;
use crate::SEED;

const GRID: usize = 420;
const SIGMA: f64 = 9.0;

#[derive(Clone, Debug)]
pub struct BlobGeometry {
    pub points: Vec<[f64; 2]>,
    pub blobs: BTreeMap<usize, Vec<Vec<[f64; 2]>>>,
    pub centroids: BTreeMap<usize, [f64; 2]>,
    pub roads: Vec<RoadRow>,
    pub terrain: Option<BTreeMap<usize, TerrainDistrictGeometry>>,
}

#[derive(Clone, Debug)]
pub struct TerrainDistrictGeometry {
    pub arterials: Vec<(usize, usize, Vec<usize>)>,
    pub subdistricts: Vec<TerrainSubdistrictGeometry>,
    pub parcels: Vec<TerrainParcelGeometry>,
}

#[derive(Clone, Debug)]
pub struct TerrainSubdistrictGeometry {
    pub members: Vec<usize>,
    pub center: [f64; 2],
    pub blob: Vec<Vec<[f64; 2]>>,
}

#[derive(Clone, Debug)]
pub struct TerrainParcelGeometry {
    pub address: String,
    pub members: Vec<usize>,
    pub rect: [f64; 4],
}

pub fn build_geometry<P: Partitioner>(
    layout: &PipelineOutput,
    partitioner: &P,
    with_terrain: bool,
) -> Result<BlobGeometry> {
    let mut points = place(layout, partitioner)?;
    relax(&mut points, &layout.membership);
    // Preserve the complete flag-off geometry as the top-level map. Terrain
    // is a second level (`docs/TERRAIN.md` section 2), so its layout replaces
    // points only inside eligible districts after inter-district relaxation;
    // ineligible districts and every top-level contour therefore stay bit
    // for bit on the established path.
    let blobs = contours(&points, &layout.membership);
    let centroids = district_members(&layout.membership)
        .into_iter()
        .map(|(district, members)| (district, mean_points(&points, &members)))
        .collect();
    let terrain = if with_terrain {
        Some(apply_terrain(layout, partitioner, &mut points)?)
    } else {
        None
    };
    let roads = roads(layout);
    Ok(BlobGeometry {
        points,
        blobs,
        centroids,
        roads,
        terrain,
    })
}

#[derive(Clone, Debug)]
struct ParcelWork {
    address: String,
    members: Vec<usize>,
    rect: [f64; 4],
}

fn apply_terrain<P: Partitioner>(
    layout: &PipelineOutput,
    partitioner: &P,
    points: &mut [[f64; 2]],
) -> Result<BTreeMap<usize, TerrainDistrictGeometry>> {
    let paths = layout
        .weighted
        .nodes
        .iter()
        .map(|node| node.file.clone())
        .collect::<Vec<_>>();
    let groups = district_members(&layout.membership);
    let mut result = BTreeMap::new();
    for (district, members) in groups {
        let Some(decomposition) = terrain::decompose(
            &layout.graph,
            &members,
            &paths,
            layout.membership.len(),
            partitioner,
        )?
        else {
            continue;
        };

        let mut parcels = decomposition
            .parcels
            .iter()
            .map(|members| ParcelWork {
                address: parcel_address(members, &paths),
                members: members.clone(),
                rect: [0.0; 4],
            })
            .collect::<Vec<_>>();
        // An address is geometry, not membership: alphabetical row-major
        // order makes a large plat browsable without inventing graph edges.
        parcels.sort_by(|left, right| {
            left.address
                .cmp(&right.address)
                .then_with(|| paths[left.members[0]].cmp(&paths[right.members[0]]))
        });

        place_terrain_district(
            &members,
            &decomposition,
            &layout.graph,
            points,
            &mut parcels,
        );

        // The plat competes as one region while contours are found, exactly
        // as the proposal's compactness measurement does. Individual parcel
        // rectangles are already explicit and should not carve 834 tiny
        // holes into an organic contour in the n8n case.
        let mut contour_groups = decomposition.organic.clone();
        let plat = parcels
            .iter()
            .flat_map(|parcel| parcel.members.iter().copied())
            .collect::<Vec<_>>();
        if !plat.is_empty() {
            contour_groups.push(plat);
        }
        let mut contour_points = Vec::new();
        let mut contour_membership = Vec::new();
        for (group, files) in contour_groups.iter().enumerate() {
            for &file in files {
                contour_points.push(points[file]);
                contour_membership.push(group);
            }
        }
        let sub_blobs = contours(&contour_points, &contour_membership);
        let subdistricts = decomposition
            .organic
            .iter()
            .enumerate()
            .map(|(index, members)| TerrainSubdistrictGeometry {
                members: members.clone(),
                center: mean_points(points, members),
                blob: sub_blobs.get(&index).cloned().unwrap_or_default(),
            })
            .collect();

        let member_set = members.iter().copied().collect::<BTreeSet<_>>();
        let mut neighbours = BTreeMap::<usize, BTreeSet<usize>>::new();
        for edge in &layout.graph.edges {
            if member_set.contains(&edge.a) && member_set.contains(&edge.b) {
                neighbours.entry(edge.a).or_default().insert(edge.b);
                neighbours.entry(edge.b).or_default().insert(edge.a);
            }
        }
        let arterials = decomposition
            .arterials
            .iter()
            .map(|&(file, stranded)| {
                (
                    file,
                    stranded,
                    neighbours
                        .get(&file)
                        .into_iter()
                        .flatten()
                        .copied()
                        .collect(),
                )
            })
            .collect();
        result.insert(
            district,
            TerrainDistrictGeometry {
                arterials,
                subdistricts,
                parcels: parcels
                    .into_iter()
                    .map(|parcel| TerrainParcelGeometry {
                        address: parcel.address,
                        members: parcel.members,
                        rect: parcel.rect,
                    })
                    .collect(),
            },
        );
    }
    Ok(result)
}

fn parcel_address(members: &[usize], paths: &[String]) -> String {
    let parents = members
        .iter()
        .map(|&member| {
            let mut parts = paths[member].split('/').collect::<Vec<_>>();
            parts.pop();
            parts
        })
        .collect::<Vec<_>>();
    let common_len = (0..parents.iter().map(Vec::len).min().unwrap_or(0))
        .take_while(|&index| {
            parents
                .iter()
                .all(|parts| parts[index] == parents[0][index])
        })
        .count();
    if common_len == 0 {
        ".".to_owned()
    } else {
        parents[0][..common_len].join("/")
    }
}

fn place_terrain_district(
    members: &[usize],
    decomposition: &terrain::Decomposition,
    graph: &WeightedGraph,
    points: &mut [[f64; 2]],
    parcels: &mut [ParcelWork],
) {
    let baseline_center = mean_points(points, members);
    let mut baseline_distances = members
        .iter()
        .map(|&file| {
            norm([
                points[file][0] - baseline_center[0],
                points[file][1] - baseline_center[1],
            ])
        })
        .collect::<Vec<_>>();
    let baseline_radius = percentile(&mut baseline_distances, 90.0).max(1e-12);

    let parcel_files = parcels
        .iter()
        .map(|parcel| parcel.members.len())
        .sum::<usize>();
    let mut major_sizes = decomposition
        .organic
        .iter()
        .map(Vec::len)
        .collect::<Vec<_>>();
    if parcel_files > 0 {
        major_sizes.push(parcel_files);
    }
    let mut centers = if major_sizes.len() <= 1 {
        vec![[0.0, 0.0]]
    } else {
        // Removing arterials makes these components disconnected by
        // definition. `pack` is the existing group fallback for exactly
        // that case; forcing nonexistent links would assert structure the
        // graph does not contain.
        pack(
            &major_sizes
                .iter()
                .enumerate()
                .map(|(index, &size)| (index, size))
                .collect::<Vec<_>>(),
        )
    };
    centers = normalize_spacing(centers);
    let mean_group_size =
        major_sizes.iter().sum::<usize>() as f64 / major_sizes.len().max(1) as f64;
    let mut local_points = BTreeMap::<usize, [f64; 2]>::new();

    for (group_index, files) in decomposition.organic.iter().enumerate() {
        let local_edges = induced_edges(files, graph);
        let mut inner = if files.len() > 1 {
            force_layout(files.len(), &local_edges, 200, 1.0, SEED)
        } else {
            vec![[0.0, 0.0]]
        };
        center_points(&mut inner);
        let radius = percentile(
            &mut inner.iter().map(|point| norm(*point)).collect::<Vec<_>>(),
            88.0,
        )
        .max(1e-12);
        // Terrain contours describe each group separately, unlike the
        // top-level layout whose 0.62 spread describes the union of all its
        // internal groups. Keeping 0.62 here made packed siblings interleave
        // and produced long ribbon contours (the first PP falsification run
        // in finding 16). A smaller, still area-scaled footprint leaves the
        // normalized one-unit centre spacing room to express the split.
        let spread = 0.12 * (files.len() as f64 / mean_group_size.max(1.0)).sqrt();
        for (&file, point) in files.iter().zip(inner) {
            local_points.insert(
                file,
                [
                    point[0] / radius * spread + centers[group_index][0],
                    point[1] / radius * spread + centers[group_index][1],
                ],
            );
        }
    }

    if parcel_files > 0 {
        let group_index = decomposition.organic.len();
        let spread = 1.24 * (parcel_files as f64 / mean_group_size.max(1.0)).sqrt();
        let grid = parcel_grid(parcels);
        for (parcel, normalized) in parcels.iter_mut().zip(grid) {
            let rect = [
                centers[group_index][0] + (normalized[0] - 0.5) * spread,
                centers[group_index][1] + (normalized[1] - 0.5) * spread,
                normalized[2] * spread,
                normalized[3] * spread,
            ];
            parcel.rect = rect;
            let columns = (parcel.members.len() as f64).sqrt().ceil().max(1.0) as usize;
            let rows = parcel.members.len().div_ceil(columns).max(1);
            for (offset, &file) in parcel.members.iter().enumerate() {
                let column = offset % columns;
                let row = offset / columns;
                local_points.insert(
                    file,
                    [
                        rect[0] + rect[2] * (column as f64 + 0.5) / columns as f64,
                        rect[1] + rect[3] * (row as f64 + 0.5) / rows as f64,
                    ],
                );
            }
        }
    }

    // An arterial is a file, not a new synthetic node. Solve its stated
    // placement (weighted centroid of every in-district neighbour) by fixed
    // point iteration so arterial-to-arterial links participate too. The
    // non-arterial neighbours anchor every observed arterial chain; a fully
    // isolated arterial falls back to the district centre.
    let arterial_set = decomposition
        .arterials
        .iter()
        .map(|(file, _)| *file)
        .collect::<BTreeSet<_>>();
    let member_set = members.iter().copied().collect::<BTreeSet<_>>();
    let mut district_adjacency = BTreeMap::<usize, Vec<(usize, f64)>>::new();
    for edge in &graph.edges {
        if member_set.contains(&edge.a) && member_set.contains(&edge.b) {
            district_adjacency
                .entry(edge.a)
                .or_default()
                .push((edge.b, edge.weight));
            district_adjacency
                .entry(edge.b)
                .or_default()
                .push((edge.a, edge.weight));
        }
    }
    for &arterial in &arterial_set {
        local_points.insert(arterial, [0.0, 0.0]);
    }
    for _ in 0..200 {
        let previous = arterial_set
            .iter()
            .map(|&file| (file, local_points[&file]))
            .collect::<BTreeMap<_, _>>();
        for &arterial in &arterial_set {
            let mut weighted = [0.0, 0.0];
            let mut mass = 0.0;
            for &(neighbour, weight) in district_adjacency.get(&arterial).into_iter().flatten() {
                let point = previous
                    .get(&neighbour)
                    .copied()
                    .or_else(|| local_points.get(&neighbour).copied())
                    .unwrap_or([0.0, 0.0]);
                weighted[0] += point[0] * weight;
                weighted[1] += point[1] * weight;
                mass += weight;
            }
            if mass > 0.0 {
                local_points.insert(arterial, [weighted[0] / mass, weighted[1] / mass]);
            }
        }
    }

    let local_mean = members.iter().fold([0.0, 0.0], |mut sum, file| {
        let point = local_points[file];
        sum[0] += point[0];
        sum[1] += point[1];
        sum
    });
    let local_mean = [
        local_mean[0] / members.len().max(1) as f64,
        local_mean[1] / members.len().max(1) as f64,
    ];
    let mut local_distances = members
        .iter()
        .map(|file| {
            let point = local_points[file];
            norm([point[0] - local_mean[0], point[1] - local_mean[1]])
        })
        .collect::<Vec<_>>();
    let local_radius = percentile(&mut local_distances, 90.0).max(1e-12);
    let scale = baseline_radius / local_radius;
    for &file in members {
        let point = local_points[&file];
        points[file] = [
            (point[0] - local_mean[0]) * scale + baseline_center[0],
            (point[1] - local_mean[1]) * scale + baseline_center[1],
        ];
    }
    for parcel in parcels {
        parcel.rect = [
            (parcel.rect[0] - local_mean[0]) * scale + baseline_center[0],
            (parcel.rect[1] - local_mean[1]) * scale + baseline_center[1],
            parcel.rect[2] * scale,
            parcel.rect[3] * scale,
        ];
    }
}

/// Alphabetical row-major shelves.  A row's height is its share of all
/// parcel files and each cell's width is its share of that row, so every
/// rectangle has area exactly proportional to file count (finding 8) while
/// preserving address order in both axes.
fn parcel_grid(parcels: &[ParcelWork]) -> Vec<[f64; 4]> {
    if parcels.is_empty() {
        return Vec::new();
    }
    let columns = (parcels.len() as f64).sqrt().ceil().max(1.0) as usize;
    let total = parcels
        .iter()
        .map(|parcel| parcel.members.len())
        .sum::<usize>() as f64;
    let mut output = vec![[0.0; 4]; parcels.len()];
    let mut y = 0.0;
    for start in (0..parcels.len()).step_by(columns) {
        let end = (start + columns).min(parcels.len());
        let row_total = parcels[start..end]
            .iter()
            .map(|parcel| parcel.members.len())
            .sum::<usize>() as f64;
        let height = row_total / total.max(1.0);
        let mut x = 0.0;
        for index in start..end {
            let width = parcels[index].members.len() as f64 / row_total.max(1.0);
            output[index] = [x, y, width, height];
            x += width;
        }
        y += height;
    }
    output
}

fn place<P: Partitioner>(layout: &PipelineOutput, partitioner: &P) -> Result<Vec<[f64; 2]>> {
    let groups = district_members(&layout.membership);
    let total = layout.membership.len().max(1);
    let mut output = vec![[0.0; 2]; layout.membership.len()];
    for (district, members) in groups {
        let sub_membership = subdivide(&members, &layout.graph, partitioner)?;
        let mut subgroups = BTreeMap::<usize, Vec<usize>>::new();
        let mut sub_order = Vec::new();
        for (&file, &subgroup) in members.iter().zip(&sub_membership) {
            if !subgroups.contains_key(&subgroup) {
                sub_order.push(subgroup);
            }
            subgroups.entry(subgroup).or_default().push(file);
        }
        let sub_edges = aggregate_sub_edges(&members, &sub_membership, &layout.graph);
        let centers = if sub_order.len() <= 1 {
            vec![[0.0, 0.0]]
        } else if connected(sub_order.len(), &sub_edges) && sub_edges.len() >= sub_order.len() {
            force_layout(sub_order.len(), &sub_edges, 300, 1.0, SEED)
        } else {
            pack(
                &sub_order
                    .iter()
                    .enumerate()
                    .map(|(index, subgroup)| (index, subgroups[subgroup].len()))
                    .collect::<Vec<_>>(),
            )
        };
        let centers = normalize_spacing(centers);
        let mean_group_size =
            subgroups.values().map(Vec::len).sum::<usize>() as f64 / subgroups.len().max(1) as f64;
        let mut district_points = Vec::<(usize, [f64; 2])>::new();
        for (sub_index, subgroup) in sub_order.iter().enumerate() {
            let files = &subgroups[subgroup];
            let local_edges = induced_edges(files, &layout.graph);
            let mut inner = if files.len() > 1 {
                force_layout(files.len(), &local_edges, 200, 1.0, SEED)
            } else {
                vec![[0.0, 0.0]]
            };
            center_points(&mut inner);
            let radius = percentile(
                &mut inner.iter().map(|point| norm(*point)).collect::<Vec<_>>(),
                88.0,
            )
            .max(1e-12);
            let spread = 0.62 * (files.len() as f64 / mean_group_size.max(1.0)).sqrt();
            for (file, point) in files.iter().zip(inner) {
                district_points.push((
                    *file,
                    [
                        point[0] / radius * spread + centers[sub_index][0],
                        point[1] / radius * spread + centers[sub_index][1],
                    ],
                ));
            }
        }
        let mean = district_points
            .iter()
            .fold([0.0, 0.0], |mut sum, (_, point)| {
                sum[0] += point[0];
                sum[1] += point[1];
                sum
            });
        let mean = [
            mean[0] / district_points.len().max(1) as f64,
            mean[1] / district_points.len().max(1) as f64,
        ];
        for (_, point) in &mut district_points {
            point[0] -= mean[0];
            point[1] -= mean[1];
        }
        let mut distances = district_points
            .iter()
            .map(|(_, point)| norm(*point))
            .collect::<Vec<_>>();
        let radius = percentile(&mut distances, 90.0).max(1e-12);
        let cap = radius * 1.5;
        let scale = 1.30 * (members.len() as f64 / total as f64).sqrt();
        let center = layout.districts[&district].centroid;
        for (file, mut point) in district_points {
            let distance = norm(point);
            if distance > cap {
                point[0] *= cap / distance;
                point[1] *= cap / distance;
            }
            output[file] = [
                point[0] / radius * scale + center[0],
                point[1] / radius * scale + center[1],
            ];
        }
    }
    Ok(output)
}

fn subdivide<P: Partitioner>(
    members: &[usize],
    graph: &WeightedGraph,
    partitioner: &P,
) -> Result<Vec<usize>> {
    if members.len() < 40 {
        return Ok(vec![0; members.len()]);
    }
    let local = members
        .iter()
        .enumerate()
        .map(|(local, &global)| (global, local))
        .collect::<BTreeMap<_, _>>();
    let edges = graph
        .edges
        .iter()
        .filter_map(|edge| {
            Some(WeightedEdge {
                a: *local.get(&edge.a)?,
                b: *local.get(&edge.b)?,
                weight: edge.weight,
            })
        })
        .collect();
    let induced = WeightedGraph {
        node_count: members.len(),
        edges,
    };
    let resolution = 0.6_f64.max(members.len() as f64 / 14.0 / 3.0);
    Ok(partitioner
        .partition(&induced, resolution, SEED, None)?
        .membership)
}

fn induced_edges(members: &[usize], graph: &WeightedGraph) -> Vec<WeightedEdge> {
    let local = members
        .iter()
        .enumerate()
        .map(|(local, &global)| (global, local))
        .collect::<BTreeMap<_, _>>();
    graph
        .edges
        .iter()
        .filter_map(|edge| {
            Some(WeightedEdge {
                a: *local.get(&edge.a)?,
                b: *local.get(&edge.b)?,
                weight: edge.weight,
            })
        })
        .collect()
}

fn aggregate_sub_edges(
    members: &[usize],
    sub_membership: &[usize],
    graph: &WeightedGraph,
) -> Vec<WeightedEdge> {
    let local = members
        .iter()
        .enumerate()
        .map(|(local, &global)| (global, local))
        .collect::<BTreeMap<_, _>>();
    let mut sub_order = Vec::new();
    for &subgroup in sub_membership {
        if !sub_order.contains(&subgroup) {
            sub_order.push(subgroup);
        }
    }
    let sub_index = sub_order
        .into_iter()
        .enumerate()
        .map(|(index, subgroup)| (subgroup, index))
        .collect::<BTreeMap<_, _>>();
    let mut weights = BTreeMap::<(usize, usize), f64>::new();
    for edge in &graph.edges {
        let (Some(&a), Some(&b)) = (local.get(&edge.a), local.get(&edge.b)) else {
            continue;
        };
        let a = sub_index[&sub_membership[a]];
        let b = sub_index[&sub_membership[b]];
        if a == b {
            continue;
        }
        let pair = if a < b { (a, b) } else { (b, a) };
        *weights.entry(pair).or_default() += edge.weight;
    }
    weights
        .into_iter()
        .map(|((a, b), weight)| WeightedEdge { a, b, weight })
        .collect()
}

fn connected(count: usize, edges: &[WeightedEdge]) -> bool {
    if count <= 1 {
        return true;
    }
    let mut adjacency = vec![Vec::new(); count];
    for edge in edges {
        adjacency[edge.a].push(edge.b);
        adjacency[edge.b].push(edge.a);
    }
    let mut seen = BTreeSet::from([0]);
    let mut queue = VecDeque::from([0]);
    while let Some(node) = queue.pop_front() {
        for &neighbour in &adjacency[node] {
            if seen.insert(neighbour) {
                queue.push_back(neighbour);
            }
        }
    }
    seen.len() == count
}

fn pack(sizes: &[(usize, usize)]) -> Vec<[f64; 2]> {
    let mut items = sizes.to_vec();
    items.sort_by_key(|(key, size)| (std::cmp::Reverse(*size), *key));
    let mut placed = Vec::<(f64, f64, f64)>::new();
    let mut output = vec![[0.0; 2]; sizes.len()];
    for (key, size) in items {
        let radius = (size.max(1) as f64).sqrt() * 0.30;
        if placed.is_empty() {
            placed.push((0.0, 0.0, radius));
            output[key] = [0.0, 0.0];
            continue;
        }
        let mut best = None;
        let mut best_distance = f64::INFINITY;
        for &(x, y, previous_radius) in &placed {
            for step in 0..48 {
                let angle = std::f64::consts::TAU * step as f64 / 48.0;
                let candidate = [
                    x + (previous_radius + radius) * angle.cos(),
                    y + (previous_radius + radius) * angle.sin(),
                ];
                if placed.iter().any(|&(other_x, other_y, other_radius)| {
                    (candidate[0] - other_x).powi(2) + (candidate[1] - other_y).powi(2)
                        < (other_radius + radius).powi(2) - 1e-9
                }) {
                    continue;
                }
                let distance = candidate[0].powi(2) + candidate[1].powi(2);
                if distance < best_distance {
                    best = Some(candidate);
                    best_distance = distance;
                }
            }
        }
        let point = best.unwrap_or_else(|| {
            let angle = 2.399_963 * placed.len() as f64;
            let distance = 0.4 * (placed.len() as f64 + 1.0).sqrt();
            [distance * angle.cos(), distance * angle.sin()]
        });
        output[key] = point;
        placed.push((point[0], point[1], radius));
    }
    output
}

fn normalize_spacing(mut points: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    if points.len() <= 1 {
        return points;
    }
    let mut nearest = Vec::with_capacity(points.len());
    for (index, point) in points.iter().enumerate() {
        nearest.push(
            points
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .map(|(_, other)| norm([point[0] - other[0], point[1] - other[1]]))
                .fold(f64::INFINITY, f64::min),
        );
    }
    let spacing = percentile(&mut nearest, 50.0).max(1e-12);
    for point in &mut points {
        point[0] /= spacing;
        point[1] /= spacing;
    }
    points
}

fn relax(points: &mut [[f64; 2]], membership: &[usize]) {
    let groups = district_members(membership);
    let mut centroids = groups
        .iter()
        .map(|(&district, members)| (district, mean_points(points, members)))
        .collect::<BTreeMap<_, _>>();
    let radii = groups
        .iter()
        .map(|(&district, members)| {
            let center = centroids[&district];
            let mut distances = members
                .iter()
                .map(|&file| norm([points[file][0] - center[0], points[file][1] - center[1]]))
                .collect::<Vec<_>>();
            (district, percentile(&mut distances, 92.0))
        })
        .collect::<BTreeMap<_, _>>();
    let districts = groups.keys().copied().collect::<Vec<_>>();
    for _ in 0..90 {
        let mut moved = 0.0;
        for left in 0..districts.len() {
            for right in left + 1..districts.len() {
                let a = districts[left];
                let b = districts[right];
                let delta = [
                    centroids[&b][0] - centroids[&a][0],
                    centroids[&b][1] - centroids[&a][1],
                ];
                let distance = norm(delta).max(1e-6);
                let wanted = (radii[&a] + radii[&b]) * 1.18;
                if distance < wanted {
                    let push = (wanted - distance) / 2.0;
                    let direction = [delta[0] / distance, delta[1] / distance];
                    centroids.get_mut(&a).unwrap()[0] -= direction[0] * push * 0.5;
                    centroids.get_mut(&a).unwrap()[1] -= direction[1] * push * 0.5;
                    centroids.get_mut(&b).unwrap()[0] += direction[0] * push * 0.5;
                    centroids.get_mut(&b).unwrap()[1] += direction[1] * push * 0.5;
                    moved += push;
                }
            }
        }
        if moved < 1e-4 {
            break;
        }
    }
    for (district, members) in groups {
        let previous = mean_points(points, &members);
        let shift = [
            centroids[&district][0] - previous[0],
            centroids[&district][1] - previous[1],
        ];
        for file in members {
            points[file][0] += shift[0];
            points[file][1] += shift[1];
        }
    }
}

fn contours(points: &[[f64; 2]], membership: &[usize]) -> BTreeMap<usize, Vec<Vec<[f64; 2]>>> {
    let groups = district_members(membership);
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    for point in points {
        for dimension in 0..2 {
            low[dimension] = low[dimension].min(point[dimension]);
            high[dimension] = high[dimension].max(point[dimension]);
        }
    }
    let padding = (high[0] - low[0]).max(high[1] - low[1]) * 0.10;
    low[0] -= padding;
    low[1] -= padding;
    high[0] += padding;
    high[1] += padding;
    let span = (high[0] - low[0]).max(high[1] - low[1]).max(1e-9);
    low[0] -= (span - (high[0] - low[0])) / 2.0;
    low[1] -= (span - (high[1] - low[1])) / 2.0;

    let districts = groups.keys().copied().collect::<Vec<_>>();
    let mut fields = Vec::with_capacity(districts.len());
    for district in &districts {
        let mut field = vec![0.0; GRID * GRID];
        for &file in &groups[district] {
            let [x, y] = to_grid(points[file], low, span);
            let x = x.round() as isize;
            let y = y.round() as isize;
            if x >= 0 && y >= 0 && x < GRID as isize && y < GRID as isize {
                field[y as usize * GRID + x as usize] += 1.0;
            }
        }
        let sigma = 4.5_f64.max(SIGMA * (groups[district].len() as f64 / 120.0).powf(0.22));
        fields.push(gaussian_blur(&field, GRID, sigma));
    }
    let mut owner = vec![0_usize; GRID * GRID];
    for cell in 0..owner.len() {
        let mut best = f64::NEG_INFINITY;
        for (index, field) in fields.iter().enumerate() {
            if field[cell] > best {
                best = field[cell];
                owner[cell] = index;
            }
        }
    }

    let mut result = BTreeMap::new();
    for (district_index, district) in districts.iter().enumerate() {
        let peak = fields[district_index]
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let cells = groups[district]
            .iter()
            .map(|&file| {
                let [x, y] = to_grid(points[file], low, span);
                (y.round() as isize, x.round() as isize)
            })
            .collect::<Vec<_>>();
        let mut mask = vec![false; GRID * GRID];
        for threshold in [0.16, 0.12, 0.09, 0.07, 0.05, 0.035, 0.02, 0.012, 0.006] {
            for cell in 0..mask.len() {
                mask[cell] = owner[cell] == district_index
                    && fields[district_index][cell] > peak * threshold;
            }
            let inside = cells
                .iter()
                .filter(|&&(y, x)| {
                    x >= 0
                        && y >= 0
                        && x < GRID as isize
                        && y < GRID as isize
                        && mask[y as usize * GRID + x as usize]
                })
                .count();
            if inside as f64 >= 0.95 * cells.len() as f64 {
                break;
            }
        }
        let mut contours = marching_squares(&mask, GRID);
        contours.sort_by_key(|contour| std::cmp::Reverse(contour.len()));
        let largest = contours.first().map_or(0, Vec::len);
        let mut polygons = Vec::new();
        for contour in contours {
            if contour.len() < 12 || (contour.len() as f64) < largest as f64 * 0.12 {
                continue;
            }
            let stride = (contour.len() / 130).max(1);
            let contour = contour.into_iter().step_by(stride).collect::<Vec<_>>();
            if contour.len() < 4 {
                continue;
            }
            let smooth = chaikin(&contour, 2);
            polygons.push(
                smooth
                    .into_iter()
                    .map(|point| {
                        [
                            point[0] / (GRID - 1) as f64 * span + low[0],
                            point[1] / (GRID - 1) as f64 * span + low[1],
                        ]
                    })
                    .collect(),
            );
        }
        if !polygons.is_empty() {
            result.insert(*district, polygons);
        }
    }
    result
}

fn gaussian_blur(input: &[f64], width: usize, sigma: f64) -> Vec<f64> {
    let radius = (sigma * 4.0).ceil() as isize;
    let mut kernel = (-radius..=radius)
        .map(|offset| (-0.5 * (offset as f64 / sigma).powi(2)).exp())
        .collect::<Vec<_>>();
    let sum = kernel.iter().sum::<f64>();
    for value in &mut kernel {
        *value /= sum;
    }
    let mut horizontal = vec![0.0; input.len()];
    for y in 0..width {
        for x in 0..width {
            let mut value = 0.0;
            for (kernel_index, &weight) in kernel.iter().enumerate() {
                let source_x = x as isize + kernel_index as isize - radius;
                if (0..width as isize).contains(&source_x) {
                    value += input[y * width + source_x as usize] * weight;
                }
            }
            horizontal[y * width + x] = value;
        }
    }
    let mut output = vec![0.0; input.len()];
    for y in 0..width {
        for x in 0..width {
            let mut value = 0.0;
            for (kernel_index, &weight) in kernel.iter().enumerate() {
                let source_y = y as isize + kernel_index as isize - radius;
                if (0..width as isize).contains(&source_y) {
                    value += horizontal[source_y as usize * width + x] * weight;
                }
            }
            output[y * width + x] = value;
        }
    }
    output
}

/// Trace the 0.5 isoline of a binary raster. For binary input, emitting and
/// joining exposed cell edges is the marching-squares contour without the
/// ambiguous interpolated cases.
pub(crate) fn marching_squares(mask: &[bool], width: usize) -> Vec<Vec<[f64; 2]>> {
    type Key = (isize, isize);
    let mut edges = BTreeMap::<Key, Vec<Key>>::new();
    let filled = |x: isize, y: isize| {
        x >= 0
            && y >= 0
            && x < width as isize
            && y < width as isize
            && mask[y as usize * width + x as usize]
    };
    let mut add = |from: Key, to: Key| edges.entry(from).or_default().push(to);
    for y in 0..width as isize {
        for x in 0..width as isize {
            if !filled(x, y) {
                continue;
            }
            let left = 2 * x - 1;
            let right = 2 * x + 1;
            let top = 2 * y - 1;
            let bottom = 2 * y + 1;
            if !filled(x, y - 1) {
                add((left, top), (right, top));
            }
            if !filled(x + 1, y) {
                add((right, top), (right, bottom));
            }
            if !filled(x, y + 1) {
                add((right, bottom), (left, bottom));
            }
            if !filled(x - 1, y) {
                add((left, bottom), (left, top));
            }
        }
    }
    for targets in edges.values_mut() {
        targets.sort_unstable();
        targets.reverse();
    }
    let mut contours = Vec::new();
    loop {
        let Some((&start, _)) = edges.iter().find(|(_, targets)| !targets.is_empty()) else {
            break;
        };
        let maximum = edges.values().map(Vec::len).sum::<usize>() + 1;
        let mut contour = Vec::new();
        let mut current = start;
        for _ in 0..maximum {
            contour.push([current.0 as f64 / 2.0, current.1 as f64 / 2.0]);
            let Some(targets) = edges.get_mut(&current) else {
                break;
            };
            let Some(next) = targets.pop() else {
                break;
            };
            current = next;
            if current == start {
                break;
            }
        }
        if contour.len() >= 3 {
            contours.push(contour);
        }
    }
    contours
}

fn chaikin(points: &[[f64; 2]], rounds: usize) -> Vec<[f64; 2]> {
    let mut points = points.to_vec();
    for _ in 0..rounds {
        let mut output = Vec::with_capacity(points.len() * 2);
        for index in 0..points.len() {
            let a = points[index];
            let b = points[(index + 1) % points.len()];
            output.push([a[0] * 0.75 + b[0] * 0.25, a[1] * 0.75 + b[1] * 0.25]);
            output.push([a[0] * 0.25 + b[0] * 0.75, a[1] * 0.25 + b[1] * 0.75]);
        }
        points = output;
    }
    points
}

fn roads(layout: &PipelineOutput) -> Vec<RoadRow> {
    let index = layout
        .weighted
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.file.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut order = Vec::new();
    let mut weights = BTreeMap::<(usize, usize), f64>::new();
    for edge in &layout.weighted.edges {
        let a = layout.membership[index[edge.a.as_str()]];
        let b = layout.membership[index[edge.b.as_str()]];
        if a == b {
            continue;
        }
        let pair = if a < b { (a, b) } else { (b, a) };
        if !weights.contains_key(&pair) {
            order.push(pair);
        }
        *weights.entry(pair).or_default() += edge.weight;
    }
    let maximum = weights.values().copied().fold(0.0_f64, f64::max).max(1e-12);
    order
        .into_iter()
        .filter_map(|pair| {
            let strength = weights[&pair] / maximum;
            (strength > 0.18)
                .then(|| RoadRow((pair.0, pair.1, crate::extract::round_to(strength, 3))))
        })
        .collect()
}

fn district_members(membership: &[usize]) -> BTreeMap<usize, Vec<usize>> {
    let mut result = BTreeMap::<usize, Vec<usize>>::new();
    for (file, &district) in membership.iter().enumerate() {
        result.entry(district).or_default().push(file);
    }
    result
}

fn mean_points(points: &[[f64; 2]], members: &[usize]) -> [f64; 2] {
    let sum = members.iter().fold([0.0, 0.0], |mut sum, &member| {
        sum[0] += points[member][0];
        sum[1] += points[member][1];
        sum
    });
    [
        sum[0] / members.len().max(1) as f64,
        sum[1] / members.len().max(1) as f64,
    ]
}

fn center_points(points: &mut [[f64; 2]]) {
    let count = points.len().max(1) as f64;
    let mean = points.iter().fold([0.0, 0.0], |mut sum, point| {
        sum[0] += point[0];
        sum[1] += point[1];
        sum
    });
    for point in points {
        point[0] -= mean[0] / count;
        point[1] -= mean[1] / count;
    }
}

fn percentile(values: &mut [f64], percentile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    let position = percentile / 100.0 * (values.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        values[lower]
    } else {
        values[lower] * (upper as f64 - position) + values[upper] * (position - lower as f64)
    }
}

fn to_grid(point: [f64; 2], low: [f64; 2], span: f64) -> [f64; 2] {
    [
        (point[0] - low[0]) / span * (GRID - 1) as f64,
        (point[1] - low[1]) / span * (GRID - 1) as f64,
    ]
}

fn norm(point: [f64; 2]) -> f64 {
    (point[0] * point[0] + point[1] * point[1]).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marching_squares_keeps_disconnected_islands() {
        let mut mask = vec![false; 25];
        mask[6] = true;
        mask[18] = true;
        assert_eq!(marching_squares(&mask, 5).len(), 2);
    }
}
