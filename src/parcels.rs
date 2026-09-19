use std::collections::BTreeMap;

use crate::blobs::marching_squares;
use crate::schema::MapDocument;

const GRID: usize = 168;
const ITERATIONS: usize = 26;
const DECIMATE: usize = 18;

pub fn build_parcels(document: &MapDocument) -> BTreeMap<String, Vec<[f64; 2]>> {
    let mut groups = BTreeMap::<usize, Vec<usize>>::new();
    for (index, node) in document.nodes.iter().enumerate() {
        groups.entry(node.district()).or_default().push(index);
    }
    let mut parcels = BTreeMap::new();
    for (district, members) in groups {
        let Some(district_geometry) = document.districts.get(&district.to_string()) else {
            continue;
        };
        if district_geometry.blob.is_empty() {
            continue;
        }
        let points = members
            .iter()
            .map(|&index| document.nodes[index].point())
            .collect::<Vec<_>>();
        let mut low = [f64::INFINITY; 2];
        let mut high = [f64::NEG_INFINITY; 2];
        for point in points.iter().chain(district_geometry.blob.iter().flatten()) {
            for dimension in 0..2 {
                low[dimension] = low[dimension].min(point[dimension]);
                high[dimension] = high[dimension].max(point[dimension]);
            }
        }
        let padding = (high[0] - low[0]).max(high[1] - low[1]) * 0.06;
        low[0] -= padding;
        low[1] -= padding;
        let span = (high[0] - low[0] + padding)
            .max(high[1] - low[1] + padding)
            .max(1e-9);
        let grid = GRID
            .max(((members.len() as f64).sqrt() * 26.0) as usize)
            .min(320);
        let mask = district_mask(&district_geometry.blob, low, span, grid);
        if mask.iter().filter(|value| **value).count() < members.len() * 4 {
            continue;
        }
        let targets = members
            .iter()
            .map(|&index| document.nodes[index].loc().max(1) as f64)
            .collect::<Vec<_>>();
        let owners = power_cells(&points, &targets, &mask, low, span, grid);
        for (local, polygon) in cell_polygons(&owners, &mask, members.len(), low, span, grid) {
            parcels.insert(members[local].to_string(), polygon);
        }
    }
    parcels
}

fn district_mask(blob: &[Vec<[f64; 2]>], low: [f64; 2], span: f64, grid: usize) -> Vec<bool> {
    let mut mask = vec![false; grid * grid];
    for y in 0..grid {
        for x in 0..grid {
            let point = [
                x as f64 / (grid - 1) as f64 * span + low[0],
                y as f64 / (grid - 1) as f64 * span + low[1],
            ];
            mask[y * grid + x] = blob.iter().any(|polygon| point_in_polygon(point, polygon));
        }
    }
    mask
}

fn power_cells(
    points: &[[f64; 2]],
    targets: &[f64],
    mask: &[bool],
    low: [f64; 2],
    span: f64,
    grid: usize,
) -> Vec<usize> {
    let cells = mask
        .iter()
        .enumerate()
        .filter_map(|(index, &inside)| inside.then_some(index))
        .collect::<Vec<_>>();
    let total = cells.len();
    let target_sum = targets.iter().sum::<f64>().max(1.0);
    let target_area = targets
        .iter()
        .map(|value| value / target_sum * total as f64)
        .collect::<Vec<_>>();
    let distances = cells
        .iter()
        .flat_map(|&cell| {
            let x = cell % grid;
            let y = cell / grid;
            let point = [
                x as f64 / (grid - 1) as f64 * span + low[0],
                y as f64 / (grid - 1) as f64 * span + low[1],
            ];
            points
                .iter()
                .map(move |site| (point[0] - site[0]).powi(2) + (point[1] - site[1]).powi(2))
        })
        .collect::<Vec<_>>();
    let mut weights = vec![0.0; points.len()];
    let mut owners = vec![0; total];
    let step = span * span / points.len().max(1) as f64 * 0.18;
    for iteration in 0..ITERATIONS {
        let mut area = vec![0_usize; points.len()];
        for (cell_index, owner) in owners.iter_mut().enumerate() {
            let start = cell_index * points.len();
            let mut best = f64::INFINITY;
            let mut best_index = 0;
            for point in 0..points.len() {
                let distance = distances[start + point] - weights[point];
                if distance < best {
                    best = distance;
                    best_index = point;
                }
            }
            *owner = best_index;
            area[best_index] += 1;
        }
        let errors = area
            .iter()
            .zip(&target_area)
            .map(|(&actual, &target)| (target - actual as f64) / target.max(1.0))
            .collect::<Vec<_>>();
        if errors
            .iter()
            .map(|value| value.abs())
            .fold(0.0_f64, f64::max)
            < 0.08
        {
            break;
        }
        for index in 0..weights.len() {
            weights[index] += step
                * errors[index].clamp(-1.5, 1.5)
                * (1.0 - 0.55 * iteration as f64 / ITERATIONS as f64);
        }
        let mean = weights.iter().sum::<f64>() / weights.len().max(1) as f64;
        for weight in &mut weights {
            *weight -= mean;
        }
    }
    let mut output = vec![usize::MAX; mask.len()];
    for (cell, owner) in cells.into_iter().zip(owners) {
        output[cell] = owner;
    }
    output
}

fn cell_polygons(
    owners: &[usize],
    mask: &[bool],
    count: usize,
    low: [f64; 2],
    span: f64,
    grid: usize,
) -> BTreeMap<usize, Vec<[f64; 2]>> {
    let mut result = BTreeMap::new();
    for owner in 0..count {
        let owned = owners
            .iter()
            .zip(mask)
            .map(|(&candidate, &inside)| inside && candidate == owner)
            .collect::<Vec<_>>();
        if !owned.iter().any(|value| *value) {
            continue;
        }
        let mut contours = marching_squares(&owned, grid);
        contours.sort_by_key(|contour| std::cmp::Reverse(contour.len()));
        let Some(mut polygon) = contours.into_iter().next() else {
            continue;
        };
        if polygon.len() > DECIMATE {
            let stride = (polygon.len() / DECIMATE).max(1);
            polygon = polygon.into_iter().step_by(stride).collect();
        }
        if polygon.len() < 3 {
            continue;
        }
        result.insert(
            owner,
            polygon
                .into_iter()
                .map(|point| {
                    [
                        crate::extract::round_to(point[0] / (grid - 1) as f64 * span + low[0], 4),
                        crate::extract::round_to(point[1] / (grid - 1) as f64 * span + low[1], 4),
                    ]
                })
                .collect(),
        );
    }
    result
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
        let crosses = (a[1] > point[1]) != (b[1] > point[1])
            && point[0] < (b[0] - a[0]) * (point[1] - a[1]) / (b[1] - a[1]) + a[0];
        if crosses {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

pub fn polygon_area(polygon: &[[f64; 2]]) -> f64 {
    if polygon.len() < 3 {
        return 0.0;
    }
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .map(|(a, b)| a[0] * b[1] - a[1] * b[0])
        .sum::<f64>()
        .abs()
        * 0.5
}

pub fn area_correlation(document: &MapDocument) -> Option<f64> {
    let parcels = document.parcels.as_ref()?;
    if parcels.len() <= 3 {
        return None;
    }
    let got = parcels
        .iter()
        .map(|(_, polygon)| polygon_area(polygon))
        .collect::<Vec<_>>();
    let wanted = parcels
        .keys()
        .filter_map(|key| key.parse::<usize>().ok())
        .map(|index| document.nodes[index].loc() as f64)
        .collect::<Vec<_>>();
    pearson(&got, &wanted)
}

fn pearson(left: &[f64], right: &[f64]) -> Option<f64> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let left_mean = left.iter().sum::<f64>() / left.len() as f64;
    let right_mean = right.iter().sum::<f64>() / right.len() as f64;
    let numerator = left
        .iter()
        .zip(right)
        .map(|(a, b)| (a - left_mean) * (b - right_mean))
        .sum::<f64>();
    let left_norm = left
        .iter()
        .map(|value| (value - left_mean).powi(2))
        .sum::<f64>();
    let right_norm = right
        .iter()
        .map(|value| (value - right_mean).powi(2))
        .sum::<f64>();
    let denominator = (left_norm * right_norm).sqrt();
    (denominator > 0.0).then_some(numerator / denominator)
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
}
