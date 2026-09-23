use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, VecDeque};

use crate::blobs::marching_squares;
use crate::extract::round_to;
use crate::neighbourhoods::Partition;
use crate::schema::{MapDocument, Neighbourhood};

const CONNECTED_ITERATIONS: usize = 12;
const FILE_ITERATIONS: usize = 26;
const FILE_CONTOUR_POINTS: usize = 24;
const NEIGHBOURHOOD_CONTOUR_POINTS: usize = 96;

pub struct Footprints {
    pub parcels: BTreeMap<String, Vec<[f64; 2]>>,
    pub centroids: Vec<[f64; 2]>,
    pub neighbourhoods: BTreeMap<String, Neighbourhood>,
}

pub fn footprint_weight(document: &MapDocument, index: usize) -> usize {
    document
        .code_lines
        .as_ref()
        .and_then(|lines| lines.get(index).copied())
        .unwrap_or_else(|| document.nodes[index].loc())
        .max(1)
}

pub fn build_parcels(document: &MapDocument, partition: &Partition) -> Footprints {
    let mut parcels = BTreeMap::new();
    let mut centroids = document
        .nodes
        .iter()
        .map(|node| node.point())
        .collect::<Vec<_>>();
    let mut neighbourhoods = BTreeMap::new();
    let mut by_district = BTreeMap::<usize, Vec<(&String, &Vec<usize>, &String)>>::new();
    for (id, (district, members, label)) in &partition.groups {
        by_district
            .entry(*district)
            .or_default()
            .push((id, members, label));
    }
    for (district, groups) in by_district {
        let members = groups
            .iter()
            .flat_map(|(_, members, _)| members.iter().copied())
            .collect::<Vec<_>>();
        let blob = &document.districts[&district.to_string()].blob;
        let all_points = members
            .iter()
            .map(|&file| document.nodes[file].point())
            .chain(blob.iter().flatten().copied())
            .collect::<Vec<_>>();
        let (low, span) = bounds(&all_points);
        let mut grid = ((members.len() as f64).sqrt() * 9.0).ceil() as usize;
        grid = grid.clamp(128, 1050);
        let mut mask = rasterize(blob, low, span, grid);
        while mask.iter().filter(|&&inside| inside).count() < groups.len() * 3 && grid < 1680 {
            grid = (grid * 3 / 2).min(1680);
            mask = rasterize(blob, low, span, grid);
        }
        // A district silhouette can be an archipelago. A single
        // neighbourhood cannot be connected across disjoint mask islands;
        // use the largest body for nested geometry while retaining the
        // district's existing outline and layout untouched.
        mask = largest_component(mask, grid);
        // Unconnected districts deliberately have no displayed district blob.
        // They still need file footprints, so use a local square around their
        // existing layout points. Likewise a subpixel district must never
        // silently lose all its files.
        if mask.iter().filter(|&&inside| inside).count() < groups.len() {
            mask.fill(true);
        }
        let sites = groups
            .iter()
            .map(|(_, files, _)| mean_points(document, files))
            .collect::<Vec<_>>();
        let targets = groups
            .iter()
            .map(|(_, files, _)| {
                files
                    .iter()
                    .map(|&file| footprint_weight(document, file) as f64)
                    .sum()
            })
            .collect::<Vec<_>>();
        let owners = solve(&mask, grid, low, span, &sites, &targets, 1, true);
        for (group_index, (id, files, label)) in groups.into_iter().enumerate() {
            let region = owners
                .iter()
                .map(|&owner| owner == group_index)
                .collect::<Vec<_>>();
            let rings = contours(&region, grid, low, span, NEIGHBOURHOOD_CONTOUR_POINTS);
            let rings = if rings.is_empty() {
                vec![pixel_square(
                    owners
                        .iter()
                        .position(|&owner| owner == group_index)
                        .unwrap_or(0),
                    grid,
                    low,
                    span,
                )]
            } else {
                rings
            };
            neighbourhoods.insert(
                id.to_string(),
                Neighbourhood {
                    d: district,
                    size: files.len(),
                    label: label.to_string(),
                    blob: rings.clone(),
                },
            );
            // Rasterize the neighbourhood's own box. Layout sites can be
            // outside it, and including them made a small region subpixel.
            let local_points = rings.iter().flatten().copied().collect::<Vec<_>>();
            let (local_low, local_span) = bounds(&local_points);
            let mut local_grid = ((files.len() as f64).sqrt() * 18.0).ceil() as usize;
            local_grid = local_grid.clamp(112, 350);
            let mut local_mask = rasterize(&rings, local_low, local_span, local_grid);
            while local_mask.iter().filter(|&&inside| inside).count() < files.len() * 4
                && local_grid < 800
            {
                local_grid = (local_grid * 3 / 2).min(800);
                local_mask = rasterize(&rings, local_low, local_span, local_grid);
            }
            if local_mask.iter().filter(|&&inside| inside).count() < files.len() {
                local_mask.fill(true);
            }
            let raw_sites = files
                .iter()
                .map(|&file| document.nodes[file].point())
                .collect::<Vec<_>>();
            // File layout points often lie outside the power cell assigned
            // to their neighbourhood. Projecting all of them to the nearest
            // boundary pixel stacked their seeds together; a single seed
            // then enclosed the others and took ~99% of that region even
            // with area quotas. Preserve their relative layout and scale it
            // into the neighbourhood's own box before solving.
            let source_center = mean(&raw_sites);
            let source_span = raw_sites.iter().fold([f64::INFINITY; 2], |mut low, point| {
                for axis in 0..2 {
                    low[axis] = low[axis].min(point[axis]);
                }
                low
            });
            let source_high = raw_sites
                .iter()
                .fold([f64::NEG_INFINITY; 2], |mut high, point| {
                    for axis in 0..2 {
                        high[axis] = high[axis].max(point[axis]);
                    }
                    high
                });
            let source_span = (source_high[0] - source_span[0])
                .max(source_high[1] - source_span[1])
                .max(1e-9);
            let file_sites = raw_sites
                .iter()
                .enumerate()
                .map(|(local, point)| {
                    let angle = local as f64 * std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
                    let jitter = local_span * 1e-4;
                    [
                        local_low[0]
                            + local_span * 0.5
                            + (point[0] - source_center[0]) / source_span * local_span * 0.7
                            + jitter * angle.cos(),
                        local_low[1]
                            + local_span * 0.5
                            + (point[1] - source_center[1]) / source_span * local_span * 0.7
                            + jitter * angle.sin(),
                    ]
                })
                .collect::<Vec<_>>();
            let file_targets = files
                .iter()
                .map(|&file| footprint_weight(document, file) as f64)
                .collect::<Vec<_>>();
            let file_owners = solve(
                &local_mask,
                local_grid,
                local_low,
                local_span,
                &file_sites,
                &file_targets,
                4,
                false,
            );
            for (local, &file) in files.iter().enumerate() {
                let owned = file_owners
                    .iter()
                    .map(|&owner| owner == local)
                    .collect::<Vec<_>>();
                let polygon = contours(
                    &owned,
                    local_grid,
                    local_low,
                    local_span,
                    FILE_CONTOUR_POINTS,
                )
                .into_iter()
                .max_by(|a, b| polygon_area(a).total_cmp(&polygon_area(b)))
                .unwrap_or_else(|| {
                    pixel_square(
                        file_owners
                            .iter()
                            .position(|&owner| owner == local)
                            .unwrap_or(0),
                        local_grid,
                        local_low,
                        local_span,
                    )
                });
                centroids[file] = polygon_centroid(&polygon);
                parcels.insert(file.to_string(), polygon);
            }
        }
    }
    Footprints {
        parcels,
        centroids,
        neighbourhoods,
    }
}

fn mean_points(document: &MapDocument, files: &[usize]) -> [f64; 2] {
    let mut sum = [0.0, 0.0];
    for &file in files {
        let point = document.nodes[file].point();
        sum[0] += point[0];
        sum[1] += point[1];
    }
    [sum[0] / files.len() as f64, sum[1] / files.len() as f64]
}

fn mean(points: &[[f64; 2]]) -> [f64; 2] {
    let n = points.len().max(1) as f64;
    [
        points.iter().map(|p| p[0]).sum::<f64>() / n,
        points.iter().map(|p| p[1]).sum::<f64>() / n,
    ]
}

fn bounds(points: &[[f64; 2]]) -> ([f64; 2], f64) {
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    for point in points {
        for axis in 0..2 {
            low[axis] = low[axis].min(point[axis]);
            high[axis] = high[axis].max(point[axis]);
        }
    }
    if points.is_empty() {
        return ([0.0, 0.0], 1.0);
    }
    let span = (high[0] - low[0]).max(high[1] - low[1]).max(0.0001);
    let padding = span * 0.04;
    ([low[0] - padding, low[1] - padding], span + 2.0 * padding)
}

fn rasterize(rings: &[Vec<[f64; 2]>], low: [f64; 2], span: f64, grid: usize) -> Vec<bool> {
    let mut mask = vec![false; grid * grid];
    if rings.is_empty() {
        mask.fill(true);
        return mask;
    }
    // Restrict each ring's test to its bounding box. Scanning every polygon
    // at every grid cell made the prototype scale with the whole district's
    // square rather than the visible region's area.
    for ring in rings {
        if ring.len() < 3 {
            continue;
        }
        let (ring_low, ring_span) = bounds(ring);
        let ring_high = [ring_low[0] + ring_span, ring_low[1] + ring_span];
        let x0 =
            (((ring_low[0] - low[0]) / span * (grid - 1) as f64).floor() as isize).max(0) as usize;
        let y0 =
            (((ring_low[1] - low[1]) / span * (grid - 1) as f64).floor() as isize).max(0) as usize;
        let x1 =
            (((ring_high[0] - low[0]) / span * (grid - 1) as f64).ceil() as usize).min(grid - 1);
        let y1 =
            (((ring_high[1] - low[1]) / span * (grid - 1) as f64).ceil() as usize).min(grid - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let point = cell_point(y * grid + x, grid, low, span);
                if point_in_polygon(point, ring) {
                    // Rings alternate exterior and hole, as emitted by
                    // marching_squares. Unioning them filled holes and let
                    // file cells claim area outside their neighbourhood.
                    mask[y * grid + x] = !mask[y * grid + x];
                }
            }
        }
    }
    mask
}

fn largest_component(mask: Vec<bool>, grid: usize) -> Vec<bool> {
    let mut seen = vec![false; mask.len()];
    let mut largest = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || seen[start] {
            continue;
        }
        let mut cells = Vec::new();
        let mut queue = VecDeque::from([start]);
        seen[start] = true;
        while let Some(cell) = queue.pop_front() {
            cells.push(cell);
            let x = cell % grid;
            let y = cell / grid;
            let neighbours = [
                (x > 0).then(|| cell.saturating_sub(1)),
                (x + 1 < grid).then_some(cell + 1),
                (y > 0).then(|| cell.saturating_sub(grid)),
                (y + 1 < grid).then_some(cell + grid),
            ];
            for neighbour in neighbours.into_iter().flatten() {
                if mask[neighbour] && !seen[neighbour] {
                    seen[neighbour] = true;
                    queue.push_back(neighbour);
                }
            }
        }
        if cells.len() > largest.len() {
            largest = cells;
        }
    }
    let mut result = vec![false; mask.len()];
    for cell in largest {
        result[cell] = true;
    }
    result
}

fn cell_point(cell: usize, grid: usize, low: [f64; 2], span: f64) -> [f64; 2] {
    [
        low[0] + (cell % grid) as f64 * span / (grid - 1) as f64,
        low[1] + (cell / grid) as f64 * span / (grid - 1) as f64,
    ]
}

#[derive(Clone, Copy)]
struct Front {
    score: f64,
    cell: usize,
    owner: usize,
}
impl PartialEq for Front {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Front {}
impl PartialOrd for Front {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Front {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .total_cmp(&self.score)
            .then_with(|| other.owner.cmp(&self.owner))
            .then_with(|| other.cell.cmp(&self.cell))
    }
}

fn solve(
    mask: &[bool],
    grid: usize,
    low: [f64; 2],
    span: f64,
    initial: &[[f64; 2]],
    targets: &[f64],
    minimum_cells: usize,
    connected: bool,
) -> Vec<usize> {
    let cells = mask
        .iter()
        .enumerate()
        .filter_map(|(i, &yes)| yes.then_some(i))
        .collect::<Vec<_>>();
    let count = initial.len();
    if count == 1 {
        return mask
            .iter()
            .map(|&yes| if yes { 0 } else { usize::MAX })
            .collect();
    }
    let mut sites = initial.to_vec();
    let mut used = vec![false; mask.len()];
    let mut seeds = Vec::with_capacity(count);
    for site in &mut sites {
        let seed = cells
            .iter()
            .copied()
            .filter(|&cell| !used[cell])
            .min_by(|&a, &b| {
                distance2(cell_point(a, grid, low, span), *site)
                    .total_cmp(&distance2(cell_point(b, grid, low, span), *site))
                    .then_with(|| a.cmp(&b))
            })
            .expect("raster has at least one cell per site");
        used[seed] = true;
        *site = cell_point(seed, grid, low, span);
        seeds.push(seed);
    }
    let total_weight = targets.iter().sum::<f64>().max(1.0);
    let minimum_cells = minimum_cells.min(cells.len() / count);
    let allocatable = cells.len() - minimum_cells * count;
    let raw = targets
        .iter()
        .map(|&weight| weight / total_weight * allocatable as f64)
        .collect::<Vec<_>>();
    let mut quotas = raw
        .iter()
        .map(|value| minimum_cells + value.floor() as usize)
        .collect::<Vec<_>>();
    let remaining = cells.len() - quotas.iter().sum::<usize>();
    let mut remainders = (0..count).collect::<Vec<_>>();
    remainders.sort_by(|&a, &b| {
        (raw[b] - raw[b].floor())
            .total_cmp(&(raw[a] - raw[a].floor()))
            .then_with(|| a.cmp(&b))
    });
    for &owner in remainders.iter().take(remaining) {
        quotas[owner] += 1;
    }
    let target_area = quotas.iter().map(|&quota| quota as f64).collect::<Vec<_>>();
    let mut weights = vec![0.0; count];
    let step = span * span / count as f64 * 0.24;
    let iterations = if connected {
        CONNECTED_ITERATIONS
    } else {
        FILE_ITERATIONS
    };
    for iteration in 0..iterations {
        let mut areas = vec![0_usize; count];
        let mut sums = vec![[0.0; 2]; count];
        for &cell in &cells {
            let point = cell_point(cell, grid, low, span);
            let winner = (0..count)
                .min_by(|&a, &b| {
                    (distance2(point, sites[a]) - weights[a])
                        .total_cmp(&(distance2(point, sites[b]) - weights[b]))
                        .then_with(|| a.cmp(&b))
                })
                .unwrap();
            areas[winner] += 1;
            sums[winner][0] += point[0];
            sums[winner][1] += point[1];
        }
        for site in 0..count {
            if areas[site] > 0 {
                sites[site][0] = 0.5 * sites[site][0] + 0.5 * sums[site][0] / areas[site] as f64;
                sites[site][1] = 0.5 * sites[site][1] + 0.5 * sums[site][1] / areas[site] as f64;
            }
            let error =
                ((target_area[site] - areas[site] as f64) / target_area[site]).clamp(-1.5, 1.5);
            weights[site] += step * error * (1.0 - 0.6 * iteration as f64 / iterations as f64);
        }
        let min_weight = weights.iter().copied().fold(f64::INFINITY, f64::min);
        for weight in &mut weights {
            *weight -= min_weight;
        }
        // Nocaj-Brandes cap: one site's weight advantage cannot exceed its
        // squared distance to another site, or the latter may disappear.
        for _ in 0..3 {
            for a in 0..count {
                for b in 0..count {
                    weights[a] = weights[a].min(weights[b] + 0.98 * distance2(sites[a], sites[b]));
                }
            }
        }
    }
    if !connected {
        // The solved power cells encode file weights more faithfully than
        // a frontier that can trap several file seeds behind one owner.
        // Reserve one distinct pixel per file after assignment; this avoids
        // the prototype's vanished-cell defect without giving up the area
        // solution for the rest of the neighbourhood.
        let mut owners = vec![usize::MAX; mask.len()];
        for &cell in &cells {
            let point = cell_point(cell, grid, low, span);
            owners[cell] = (0..count)
                .min_by(|&a, &b| {
                    (distance2(point, sites[a]) - weights[a])
                        .total_cmp(&(distance2(point, sites[b]) - weights[b]))
                        .then_with(|| a.cmp(&b))
                })
                .unwrap();
        }
        for (owner, &seed) in seeds.iter().enumerate() {
            owners[seed] = owner;
        }
        return owners;
    }
    // Power assignment alone can produce a vanished cell or detached island.
    // Seed each site with a distinct raster cell, then admit pixels only from
    // an owned neighbour. This guarantees a minimum positive footprint and
    // one connected region on each connected component of the district mask.
    let mut owners = vec![usize::MAX; mask.len()];
    let mut heap = BinaryHeap::new();
    let mut counts = vec![1_usize; count];
    for (owner, &seed) in seeds.iter().enumerate() {
        owners[seed] = owner;
        push_front(
            seed, owner, &owners, mask, grid, low, span, &sites, &weights, &mut heap,
        );
    }
    let mut deferred = BinaryHeap::new();
    while let Some(front) = heap.pop() {
        if owners[front.cell] != usize::MAX {
            continue;
        }
        if counts[front.owner] >= quotas[front.owner] {
            deferred.push(front);
            continue;
        }
        owners[front.cell] = front.owner;
        counts[front.owner] += 1;
        push_front(
            front.cell,
            front.owner,
            &owners,
            mask,
            grid,
            low,
            span,
            &sites,
            &weights,
            &mut heap,
        );
    }
    // Exact target quotas may be geometrically unreachable behind another
    // owner's connected frontier. Fill the remaining pixels from deferred
    // boundary candidates, preserving connectivity and a total partition.
    heap = deferred;
    while let Some(front) = heap.pop() {
        if owners[front.cell] != usize::MAX {
            continue;
        }
        owners[front.cell] = front.owner;
        push_front(
            front.cell,
            front.owner,
            &owners,
            mask,
            grid,
            low,
            span,
            &sites,
            &weights,
            &mut heap,
        );
    }
    // A district blob may be an archipelago. Its detached raster components
    // have no path from the first seeds, so allocate them deterministically.
    for &cell in &cells {
        if owners[cell] != usize::MAX {
            continue;
        }
        let point = cell_point(cell, grid, low, span);
        let owner = (0..count)
            .min_by(|&a, &b| {
                (distance2(point, sites[a]) - weights[a])
                    .total_cmp(&(distance2(point, sites[b]) - weights[b]))
                    .then_with(|| a.cmp(&b))
            })
            .unwrap();
        owners[cell] = owner;
        push_front(
            cell, owner, &owners, mask, grid, low, span, &sites, &weights, &mut heap,
        );
        while let Some(front) = heap.pop() {
            if owners[front.cell] != usize::MAX {
                continue;
            }
            owners[front.cell] = front.owner;
            push_front(
                front.cell,
                front.owner,
                &owners,
                mask,
                grid,
                low,
                span,
                &sites,
                &weights,
                &mut heap,
            );
        }
    }
    owners
}

fn push_front(
    cell: usize,
    owner: usize,
    owners: &[usize],
    mask: &[bool],
    grid: usize,
    low: [f64; 2],
    span: f64,
    sites: &[[f64; 2]],
    weights: &[f64],
    heap: &mut BinaryHeap<Front>,
) {
    let x = cell % grid;
    let y = cell / grid;
    let neighbours = [
        if x > 0 { Some(cell - 1) } else { None },
        if x + 1 < grid { Some(cell + 1) } else { None },
        if y > 0 { Some(cell - grid) } else { None },
        if y + 1 < grid {
            Some(cell + grid)
        } else {
            None
        },
    ];
    for neighbour in neighbours.into_iter().flatten() {
        if mask[neighbour] && owners[neighbour] == usize::MAX {
            heap.push(Front {
                score: distance2(cell_point(neighbour, grid, low, span), sites[owner])
                    - weights[owner],
                cell: neighbour,
                owner,
            });
        }
    }
}

fn distance2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

fn contours(
    mask: &[bool],
    grid: usize,
    low: [f64; 2],
    span: f64,
    max_points: usize,
) -> Vec<Vec<[f64; 2]>> {
    let mut rings = marching_squares(mask, grid);
    rings.sort_by(|a, b| b.len().cmp(&a.len()));
    rings
        .into_iter()
        .filter_map(|ring| {
            let stride = ring.len().div_ceil(max_points).max(1);
            let polygon = ring
                .into_iter()
                .step_by(stride)
                .map(|point| {
                    [
                        round_to(low[0] + point[0] * span / (grid - 1) as f64, 6),
                        round_to(low[1] + point[1] * span / (grid - 1) as f64, 6),
                    ]
                })
                .collect::<Vec<_>>();
            (polygon.len() >= 3 && polygon_area(&polygon) > 0.0).then_some(polygon)
        })
        .collect()
}

fn pixel_square(cell: usize, grid: usize, low: [f64; 2], span: f64) -> Vec<[f64; 2]> {
    let center = cell_point(cell, grid, low, span);
    let half = span / (grid - 1) as f64 * 0.5;
    [[-half, -half], [half, -half], [half, half], [-half, half]]
        .into_iter()
        .map(|d| [round_to(center[0] + d[0], 6), round_to(center[1] + d[1], 6)])
        .collect()
}

pub(crate) fn point_in_polygon(point: [f64; 2], polygon: &[[f64; 2]]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut previous = polygon.len() - 1;
    for current in 0..polygon.len() {
        let a = polygon[current];
        let b = polygon[previous];
        if (a[1] > point[1]) != (b[1] > point[1])
            && point[0] < (b[0] - a[0]) * (point[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

pub fn polygon_area(polygon: &[[f64; 2]]) -> f64 {
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .map(|(a, b)| a[0] * b[1] - a[1] * b[0])
        .sum::<f64>()
        .abs()
        * 0.5
}

fn polygon_centroid(polygon: &[[f64; 2]]) -> [f64; 2] {
    let mut cross_sum = 0.0;
    let mut x = 0.0;
    let mut y = 0.0;
    for (a, b) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
        let cross = a[0] * b[1] - b[0] * a[1];
        cross_sum += cross;
        x += (a[0] + b[0]) * cross;
        y += (a[1] + b[1]) * cross;
    }
    if cross_sum.abs() < 1e-12 {
        let n = polygon.len().max(1) as f64;
        return [
            round_to(polygon.iter().map(|p| p[0]).sum::<f64>() / n, 6),
            round_to(polygon.iter().map(|p| p[1]).sum::<f64>() / n, 6),
        ];
    }
    [
        round_to(x / (3.0 * cross_sum), 6),
        round_to(y / (3.0 * cross_sum), 6),
    ]
}

pub fn area_correlation(document: &MapDocument) -> Option<f64> {
    let parcels = document.parcels.as_ref()?;
    let mut got = Vec::new();
    let mut wanted = Vec::new();
    for (key, polygon) in parcels {
        if let Ok(index) = key.parse::<usize>() {
            got.push(polygon_area(polygon));
            wanted.push(footprint_weight(document, index) as f64);
        }
    }
    pearson(&got, &wanted)
}

fn pearson(left: &[f64], right: &[f64]) -> Option<f64> {
    if left.len() != right.len() || left.len() <= 3 {
        return None;
    }
    let n = left.len() as f64;
    let lm = left.iter().sum::<f64>() / n;
    let rm = right.iter().sum::<f64>() / n;
    let num = left
        .iter()
        .zip(right)
        .map(|(a, b)| (a - lm) * (b - rm))
        .sum::<f64>();
    let l = left.iter().map(|a| (a - lm).powi(2)).sum::<f64>();
    let r = right.iter().map(|b| (b - rm).powi(2)).sum::<f64>();
    (l * r > 0.0).then_some(num / (l * r).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polygon_test_handles_inside_and_outside() {
        let square = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!(point_in_polygon([0.5, 0.5], &square));
        assert!(!point_in_polygon([2.0, 0.5], &square));
    }

    #[test]
    fn shoelace_area_is_positive() {
        let square = [[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]];
        assert_eq!(polygon_area(&square), 4.0);
    }

    #[test]
    fn reserved_seeds_keep_coincident_sites_present() {
        let grid = 20;
        let mask = vec![true; grid * grid];
        let owners = solve(
            &mask,
            grid,
            [0.0, 0.0],
            1.0,
            &[[0.5, 0.5]; 8],
            &[1.0; 8],
            4,
            false,
        );
        for owner in 0..8 {
            assert!(owners.contains(&owner));
        }
    }

    #[test]
    fn detached_district_island_does_not_split_a_neighbourhood() {
        let grid = 5;
        let mut mask = vec![false; grid * grid];
        for cell in [0, 1, 5, 6, 24] {
            mask[cell] = true;
        }
        let body = largest_component(mask, grid);
        assert_eq!(body.iter().filter(|&&inside| inside).count(), 4);
        assert!(!body[24]);
    }

    #[test]
    fn rasterized_nested_ring_preserves_hole() {
        let outer = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let hole = vec![[0.3, 0.3], [0.7, 0.3], [0.7, 0.7], [0.3, 0.7]];
        let mask = rasterize(&[outer, hole], [0.0, 0.0], 1.0, 21);
        assert!(mask[5 * 21 + 5]);
        assert!(!mask[10 * 21 + 10]);
    }

    #[test]
    fn connected_growth_tracks_unequal_area_targets() {
        let grid = 32;
        let mask = vec![true; grid * grid];
        let sites = [[0.2, 0.2], [0.8, 0.2], [0.2, 0.8], [0.8, 0.8]];
        let owners = solve(
            &mask,
            grid,
            [0.0, 0.0],
            1.0,
            &sites,
            &[1.0, 2.0, 3.0, 4.0],
            4,
            true,
        );
        let counts = (0..4)
            .map(|owner| owners.iter().filter(|&&value| value == owner).count())
            .collect::<Vec<_>>();
        assert!(
            counts[0] < counts[1] && counts[1] < counts[2] && counts[2] < counts[3],
            "{counts:?}"
        );
    }
}
