//! Raster card ownership inside each displayed file footprint. Keeping the
//! raster solver preserves its connected weighted quotas and the subpixel
//! reserve for crowded parents; a new vector power diagram would have to
//! recover both guarantees. Contours smooth only within shared ownership.
use std::collections::BTreeMap;

use anyhow::{ensure, Result};

use crate::blobs::marching_squares;
use crate::parcels::{largest_component, point_in_polygon, polygon_area, rasterize, solve};
use crate::schema::{MapDocument, SymbolsDocument};

type Ring = Vec<[f64; 2]>;
type Rings = Vec<Ring>;
// The prototype used roughly 9–12 vertices per card. A larger contour made
// dify's separate symbol document far heavier without adding visible detail.
const CARD_CONTOUR_POINTS: usize = 12;
// The smallest reserved fallback cards are roughly 1e-10 wide. The paired
// artifact audit found that ten decimals collapses exteriors; eleven keeps
// them once reserves sit clear of parent boundaries.
const CARD_COORDINATE_SCALE: f64 = 1e11;

struct Canvas {
    grid: usize,
    low: [f64; 2],
    step: f64,
}

fn ring(mask: &[bool], canvas: &Canvas, compact_tiny: bool) -> Option<Rings> {
    ring_with_smoothing(mask, canvas, compact_tiny, true)
}

fn ring_raw(mask: &[bool], canvas: &Canvas) -> Option<Rings> {
    ring_with_smoothing(mask, canvas, false, false)
}

fn ring_with_smoothing(
    mask: &[bool],
    canvas: &Canvas,
    compact_tiny: bool,
    allow_smoothing: bool,
) -> Option<Rings> {
    let mask = largest_component(mask.to_vec(), canvas.grid);
    let cells = mask.iter().filter(|&&owned| owned).count();
    // A one-cell seed and its few-cell cross are artifacts of connected
    // raster growth, not meaningful card shapes. Draw a compact octagon in
    // an owned cell; its siblings keep their original cells and cannot gain
    // any part of this card. The minimum-area reserve remains separate.
    // A container must retain its full owned region so its descendants can
    // be drawn within it; compacting that parent would strand child cells.
    if compact_tiny && cells > 0 && cells <= 5 {
        let mean = mask.iter().enumerate().filter(|(_, owned)| **owned).fold(
            [0.0, 0.0],
            |mut sum, (cell, _)| {
                sum[0] += (cell % canvas.grid) as f64;
                sum[1] += (cell / canvas.grid) as f64;
                sum
            },
        );
        let mean = [mean[0] / cells as f64, mean[1] / cells as f64];
        let center = mask
            .iter()
            .enumerate()
            .filter(|(_, owned)| **owned)
            .min_by(|(a, _), (b, _)| {
                let distance = |cell: usize| {
                    ((cell % canvas.grid) as f64 - mean[0]).powi(2)
                        + ((cell / canvas.grid) as f64 - mean[1]).powi(2)
                };
                distance(*a).total_cmp(&distance(*b)).then_with(|| a.cmp(b))
            })
            .map(|(cell, _)| {
                [
                    canvas.low[0] + (cell % canvas.grid) as f64 * canvas.step,
                    canvas.low[1] + (cell / canvas.grid) as f64 * canvas.step,
                ]
            })?;
        let radius = canvas.step * 0.38;
        let bevel = radius * 0.42;
        let [x, y] = center;
        let mut compact = vec![vec![
            [x - radius + bevel, y - radius],
            [x + radius - bevel, y - radius],
            [x + radius, y - radius + bevel],
            [x + radius, y + radius - bevel],
            [x + radius - bevel, y + radius],
            [x - radius + bevel, y + radius],
            [x - radius, y + radius - bevel],
            [x - radius, y - radius + bevel],
        ]];
        if quantize_rings(&mut compact).is_ok() {
            return Some(compact);
        }
    }
    let mut rings = marching_squares(&mask, canvas.grid);
    for points in &mut rings {
        let original = std::mem::take(points);
        let length = original.len();
        if length < 3 {
            *points = original;
            continue;
        }
        *points = original
            .iter()
            .enumerate()
            .filter_map(|(i, &current)| {
                let previous = original[(i + length - 1) % length];
                let next = original[(i + 1) % length];
                let a = [current[0] - previous[0], current[1] - previous[1]];
                let b = [next[0] - current[0], next[1] - current[1]];
                let cross = a[0] * b[1] - a[1] * b[0];
                let dot = a[0] * b[0] + a[1] * b[1];
                (cross != 0.0 || dot <= 0.0).then_some(current)
            })
            .collect();
    }
    rings.sort_by(|a, b| {
        polygon_area(b)
            .total_cmp(&polygon_area(a))
            .then_with(|| a.len().cmp(&b.len()))
    });
    // One corner-cutting pass suppresses grid steps. Every sibling starts
    // from the same raster ownership, and the quarter-cell audit below
    // rejects a rounded corner if it enters any unowned cell (including a
    // hole). When a tight boundary has no room to round, use its original
    // contour instead of moving ownership across the boundary.
    if allow_smoothing {
        let smoothed = rings.iter().map(|ring| smooth(ring)).collect::<Rings>();
        if let Some(output) = simplify(&smoothed, &mask, canvas) {
            return Some(output);
        }
    }
    simplify(&rings, &mask, canvas)
}

fn smooth(ring: &Ring) -> Ring {
    if signed_area(ring) <= 0.0 {
        // Reducing a hole exposes its enclosed sibling, so keep holes on
        // their exact raster boundary.
        return ring.clone();
    }
    (0..ring.len())
        .map(|i| {
            let previous = ring[(i + ring.len() - 1) % ring.len()];
            let point = ring[i];
            let next = ring[(i + 1) % ring.len()];
            let incoming = [point[0] - previous[0], point[1] - previous[1]];
            let outgoing = [next[0] - point[0], next[1] - point[1]];
            let cross = incoming[0] * outgoing[1] - incoming[1] * outgoing[0];
            if cross <= 0.0 {
                return point;
            }
            let before_length = incoming[0].abs().max(incoming[1].abs());
            let after_length = outgoing[0].abs().max(outgoing[1].abs());
            if before_length == 0.0 || after_length == 0.0 {
                return point;
            }
            [
                point[0] - incoming[0] / before_length * 0.25 + outgoing[0] / after_length * 0.25,
                point[1] - incoming[1] / before_length * 0.25 + outgoing[1] / after_length * 0.25,
            ]
        })
        .collect()
}

fn simplify(rings: &Rings, mask: &[bool], canvas: &Canvas) -> Option<Rings> {
    let maximum = rings.iter().map(Vec::len).max()?;
    let mut limit = CARD_CONTOUR_POINTS;
    loop {
        let mut output = rings
            .iter()
            .filter_map(|points| {
                let stride = points.len().div_ceil(limit).max(1);
                let polygon = points
                    .iter()
                    .step_by(stride)
                    .map(|p| {
                        [
                            canvas.low[0] + p[0] * canvas.step,
                            canvas.low[1] + p[1] * canvas.step,
                        ]
                    })
                    .collect::<Ring>();
                (polygon.len() >= 3 && polygon_area(&polygon) > 0.0).then_some(polygon)
            })
            .collect::<Rings>();
        if output.is_empty() {
            return None;
        }
        // A sparse sample of a thin diagonal can have apparent floating
        // area but become collinear at export precision. Keep more of the
        // raster contour before falling back to a reserved rectangle.
        if quantize_rings(&mut output).is_err() {
            if limit >= maximum {
                return None;
            }
            limit = (limit * 2).min(maximum);
            continue;
        }
        if !covers_unowned_pixel(mask, canvas, &output) {
            return Some(output);
        }
        if limit >= maximum {
            return None;
        }
        limit = (limit * 2).min(maximum);
    }
}

fn covers_unowned_pixel(mask: &[bool], canvas: &Canvas, rings: &Rings) -> bool {
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    for point in rings.iter().flatten() {
        for axis in 0..2 {
            low[axis] = low[axis].min(point[axis]);
            high[axis] = high[axis].max(point[axis]);
        }
    }
    let x0 = (((low[0] - canvas.low[0]) / canvas.step).floor() as isize).max(0) as usize;
    let y0 = (((low[1] - canvas.low[1]) / canvas.step).floor() as isize).max(0) as usize;
    let x1 = (((high[0] - canvas.low[0]) / canvas.step).ceil() as usize).min(canvas.grid - 1);
    let y1 = (((high[1] - canvas.low[1]) / canvas.step).ceil() as usize).min(canvas.grid - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            if mask[y * canvas.grid + x] {
                continue;
            }
            let center = [
                canvas.low[0] + x as f64 * canvas.step,
                canvas.low[1] + y as f64 * canvas.step,
            ];
            // A long diagonal shortcut can cross several neighbouring
            // pixels without containing any of their centres. Quarter-cell
            // probes keep that overlap below the raster tolerance.
            for dy in [-0.25, 0.0, 0.25] {
                for dx in [-0.25, 0.0, 0.25] {
                    let point = [center[0] + dx * canvas.step, center[1] + dy * canvas.step];
                    if point_in_rings(point, rings) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn point_in_rings(point: [f64; 2], rings: &Rings) -> bool {
    rings
        .iter()
        .filter(|ring| point_in_polygon(point, ring))
        .count()
        % 2
        == 1
}

fn boundary_distance2(point: [f64; 2], rings: &Rings) -> f64 {
    rings
        .iter()
        .flat_map(|ring| ring.iter().zip(ring.iter().cycle().skip(1)))
        .map(|(a, b)| {
            let dx = b[0] - a[0];
            let dy = b[1] - a[1];
            let length2 = dx * dx + dy * dy;
            let t = if length2 > 0.0 {
                ((point[0] - a[0]) * dx + (point[1] - a[1]) * dy) / length2
            } else {
                0.0
            }
            .clamp(0.0, 1.0);
            let ex = point[0] - (a[0] + t * dx);
            let ey = point[1] - (a[1] + t * dy);
            ex * ex + ey * ey
        })
        .fold(f64::INFINITY, f64::min)
}

fn signed_area(ring: &Ring) -> f64 {
    ring.iter()
        .zip(ring.iter().cycle().skip(1))
        .map(|(a, b)| a[0] * b[1] - a[1] * b[0])
        .sum::<f64>()
        * 0.5
}

fn rings_area(rings: &Rings) -> f64 {
    rings.iter().map(signed_area).sum::<f64>().abs()
}

fn inset(mask: &[bool], canvas: &Canvas, depth: usize, minimum_pixels: usize) -> Vec<bool> {
    let area = mask.iter().filter(|&&pixel| pixel).count() as f64 * canvas.step.powi(2);
    let distance = (if depth == 0 { 0.035 } else { 0.05 }) * area.sqrt();
    let cap = if depth == 0 { 0.004 } else { 0.003 };
    let radius = (distance.min(cap) / canvas.step).round() as usize;
    if radius == 0 {
        return mask.to_vec();
    }
    let g = canvas.grid;
    let mut result = mask.to_vec();
    for _ in 0..radius {
        let previous = result.clone();
        for (i, pixel) in result.iter_mut().enumerate() {
            if !previous[i] {
                continue;
            }
            let x = i % g;
            let y = i / g;
            *pixel = x > 0
                && x + 1 < g
                && y > 0
                && y + 1 < g
                && previous[i - 1]
                && previous[i + 1]
                && previous[i - g]
                && previous[i + g];
        }
    }
    // A thin but valid card keeps its one-cell sliver instead of vanishing.
    if result.iter().filter(|&&yes| yes).count() >= minimum_pixels {
        result
    } else {
        mask.to_vec()
    }
}

fn allocate(mask: &[bool], canvas: &Canvas, weights: &[f64]) -> Vec<Vec<bool>> {
    let count = weights.len();
    if count == 0 {
        return Vec::new();
    }
    let available = mask
        .iter()
        .enumerate()
        .filter_map(|(i, &yes)| yes.then_some(i))
        .collect::<Vec<_>>();
    if available.len() < count {
        return vec![vec![false; mask.len()]; count];
    }
    let sites = (0..count)
        .map(|i| {
            let cell = available[(i * available.len() / count).min(available.len() - 1)];
            [
                canvas.low[0] + (cell % canvas.grid) as f64 * canvas.step,
                canvas.low[1] + (cell / canvas.grid) as f64 * canvas.step,
            ]
        })
        .collect::<Vec<_>>();
    let owners = solve(
        mask,
        canvas.grid,
        canvas.low,
        canvas.step * (canvas.grid - 1) as f64,
        &sites,
        weights,
        1,
        true,
    );
    (0..count)
        .map(|owner| owners.iter().map(|&value| value == owner).collect())
        .collect()
}

fn reserve(mask: &mut [bool], canvas: &Canvas, parent: &Rings) -> Option<[f64; 4]> {
    let outer = parent.first()?;
    let target = [
        outer.iter().map(|p| p[0]).sum::<f64>() / outer.len() as f64,
        outer.iter().map(|p| p[1]).sum::<f64>() / outer.len() as f64,
    ];
    let selected = mask
        .iter()
        .enumerate()
        .filter(|(_, yes)| **yes)
        .filter_map(|(i, _)| {
            let center = [
                canvas.low[0] + (i % canvas.grid) as f64 * canvas.step,
                canvas.low[1] + (i / canvas.grid) as f64 * canvas.step,
            ];
            point_in_rings(center, parent).then_some((i, center))
        })
        .min_by(|a, b| {
            let da = (a.1[0] - target[0]).powi(2) + (a.1[1] - target[1]).powi(2);
            let db = (b.1[0] - target[0]).powi(2) + (b.1[1] - target[1]).powi(2);
            da.total_cmp(&db).then_with(|| a.0.cmp(&b.0))
        })?;
    mask[selected.0] = false;
    let inside_square = |center: [f64; 2], half: f64| {
        [
            [center[0] - half, center[1] - half],
            [center[0] + half, center[1] - half],
            [center[0] + half, center[1] + half],
            [center[0] - half, center[1] + half],
        ]
        .iter()
        .all(|&corner| point_in_rings(corner, parent))
    };
    let mut center = selected.1;
    let mut half = canvas.step * 0.2;
    if !inside_square(center, half) {
        // A raster cell can straddle a concave parent's boundary. Shrinking
        // a rectangle at its centre produced sub-1e-11 slivers and a child
        // centroid outside the rounded parent on dify. Search only within
        // the reserved cell for a point with a real interior margin.
        let mut clearance = boundary_distance2(center, parent);
        for level in 0..20 {
            let offset = canvas.step * 0.25 / 2f64.powi(level);
            for dx in [-1.0, 0.0, 1.0] {
                for dy in [-1.0, 0.0, 1.0] {
                    let candidate = [selected.1[0] + dx * offset, selected.1[1] + dy * offset];
                    if !point_in_rings(candidate, parent) {
                        continue;
                    }
                    let margin = boundary_distance2(candidate, parent);
                    if margin > clearance {
                        center = candidate;
                        clearance = margin;
                    }
                }
            }
            if inside_square(center, half) {
                break;
            }
        }
    }
    for _ in 0..24 {
        if inside_square(center, half) {
            break;
        }
        half *= 0.5;
    }
    Some([
        center[0] - half,
        center[1] - half,
        center[0] + half,
        center[1] + half,
    ])
}

fn valid_ring(ring: Option<&Rings>, parent: &Rings) -> bool {
    let Some(ring) = ring else { return false };
    let Some(outer) = ring.first() else {
        return false;
    };
    let centroid = [
        outer.iter().map(|p| p[0]).sum::<f64>() / outer.len() as f64,
        outer.iter().map(|p| p[1]).sum::<f64>() / outer.len() as f64,
    ];
    let points = parent.iter().flatten().collect::<Vec<_>>();
    let low = points.iter().fold([f64::INFINITY; 2], |mut bounds, point| {
        bounds[0] = bounds[0].min(point[0]);
        bounds[1] = bounds[1].min(point[1]);
        bounds
    });
    let high = points
        .iter()
        .fold([f64::NEG_INFINITY; 2], |mut bounds, point| {
            bounds[0] = bounds[0].max(point[0]);
            bounds[1] = bounds[1].max(point[1]);
            bounds
        });
    let epsilon = (high[0] - low[0]).max(high[1] - low[1]) * 1e-9;
    let near_edge = boundary_distance2(centroid, parent) <= epsilon * epsilon;
    point_in_rings(centroid, parent) && !near_edge && rings_area(ring) <= rings_area(parent)
}

fn rectangle(rect: [f64; 4]) -> Rings {
    vec![vec![
        [rect[0], rect[1]],
        [rect[2], rect[1]],
        [rect[2], rect[3]],
        [rect[0], rect[3]],
    ]]
}

fn quantize_rings(rings: &mut Rings) -> Result<()> {
    let mut output = Rings::with_capacity(rings.len());
    for (ring_index, ring) in rings.iter().enumerate() {
        let mut points = Vec::<[i128; 2]>::with_capacity(ring.len());
        for point in ring.iter() {
            let rounded = quantized_point(*point);
            if points.last() != Some(&rounded) {
                points.push(rounded);
            }
        }
        if points.len() > 1 && points.first() == points.last() {
            points.pop();
        }
        loop {
            let length = points.len();
            if length < 3 {
                break;
            }
            let kept = points
                .iter()
                .enumerate()
                .filter_map(|(i, &point)| {
                    let previous = points[(i + length - 1) % length];
                    let next = points[(i + 1) % length];
                    let a = [point[0] - previous[0], point[1] - previous[1]];
                    let b = [next[0] - point[0], next[1] - point[1]];
                    let cross = a[0] * b[1] - a[1] * b[0];
                    let dot = a[0] * b[0] + a[1] * b[1];
                    (cross != 0 || dot <= 0).then_some(point)
                })
                .collect::<Vec<_>>();
            if kept.len() == length {
                break;
            }
            points = kept;
        }
        let doubled_area = points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .map(|(a, b)| a[0] * b[1] - a[1] * b[0])
            .sum::<i128>();
        if points.len() < 3 || doubled_area == 0 {
            // A vanishing hole carries no fill area after rounding; retaining
            // it would emit an invalid contour. Exteriors must remain valid.
            ensure!(
                ring_index > 0,
                "card exterior collapsed at 11 decimals: {ring:?}"
            );
            continue;
        }
        output.push(
            points
                .into_iter()
                .map(|p| {
                    [
                        p[0] as f64 / CARD_COORDINATE_SCALE,
                        p[1] as f64 / CARD_COORDINATE_SCALE,
                    ]
                })
                .collect(),
        );
    }
    *rings = output;
    Ok(())
}

fn quantized_point(point: [f64; 2]) -> [i128; 2] {
    [
        (point[0] * CARD_COORDINATE_SCALE).round() as i128,
        (point[1] * CARD_COORDINATE_SCALE).round() as i128,
    ]
}

fn encode_rings(rings: Rings) -> Vec<Vec<i64>> {
    rings
        .into_iter()
        .map(|ring| {
            let mut encoded = Vec::with_capacity(ring.len() * 2);
            let mut previous = [0i128; 2];
            for point in ring {
                let current = quantized_point(point);
                encoded.push((current[0] - previous[0]) as i64);
                encoded.push((current[1] - previous[1]) as i64);
                previous = current;
            }
            encoded
        })
        .collect()
}

struct Cards<'a> {
    document: &'a SymbolsDocument,
    children: Vec<Vec<usize>>,
    rings: Vec<Option<Rings>>,
    headers: BTreeMap<usize, Rings>,
    modules: BTreeMap<usize, Rings>,
}

fn local_parent(document: &SymbolsDocument, symbol: usize) -> Option<usize> {
    let row = &document.symbols[symbol].0;
    (row.5 >= 0)
        .then_some(row.5 as usize)
        .filter(|&parent| document.symbols[parent].0 .0 == row.0)
}

impl Cards<'_> {
    fn own_lines(&self, symbol: usize) -> usize {
        let parent = &self.document.symbols[symbol].0;
        self.document.symbols[symbol].0 .6.saturating_sub(
            self.children[symbol]
                .iter()
                .filter(|&&child| {
                    let span = &self.document.symbols[child].0;
                    span.3 >= parent.3 && span.4 <= parent.4
                })
                .map(|&child| self.document.symbols[child].0 .6)
                .sum(),
        )
    }

    fn mass(&self, symbol: usize) -> usize {
        self.own_lines(symbol).max(1)
            + self.children[symbol]
                .iter()
                .map(|&child| self.mass(child))
                .sum::<usize>()
    }

    fn fallback(&mut self, symbol: usize, rect: [f64; 4]) {
        self.rings[symbol] = Some(rectangle(rect));
        let children = self.children[symbol].clone();
        if children.is_empty() {
            return;
        }
        let height = rect[3] - rect[1];
        let own = self.own_lines(symbol).max(1) as f64;
        let share = (own / self.mass(symbol) as f64).clamp(0.12, 0.4);
        let header_y = rect[3] - height * share;
        self.headers
            .insert(symbol, rectangle([rect[0], header_y, rect[2], rect[3]]));
        let sum = children
            .iter()
            .map(|&child| self.mass(child) as f64)
            .sum::<f64>();
        let mut x = rect[0];
        for child in children {
            let width = (rect[2] - rect[0]) * self.mass(child) as f64 / sum;
            let gap = width * 0.02;
            self.fallback(
                child,
                [
                    x + gap,
                    rect[1] + height * 0.02,
                    x + width - gap,
                    header_y - height * 0.02,
                ],
            );
            x += width;
        }
    }

    fn place(&mut self, symbol: usize, mask: &[bool], canvas: &Canvas, depth: usize) {
        let children = self.children[symbol].clone();
        let display = largest_component(
            inset(mask, canvas, depth, children.len() * 2 + 1),
            canvas.grid,
        );
        self.rings[symbol] = ring(&display, canvas, children.is_empty());
        if self.rings[symbol].is_none() {
            if let Some(pixel) = display.iter().position(|&yes| yes) {
                let center = [
                    canvas.low[0] + (pixel % canvas.grid) as f64 * canvas.step,
                    canvas.low[1] + (pixel / canvas.grid) as f64 * canvas.step,
                ];
                let half = canvas.step * 0.2;
                self.fallback(
                    symbol,
                    [
                        center[0] - half,
                        center[1] - half,
                        center[0] + half,
                        center[1] + half,
                    ],
                );
            }
            return;
        }
        if children.is_empty() {
            return;
        }
        let mut pixels = display
            .iter()
            .enumerate()
            .filter_map(|(i, &yes)| yes.then_some(i))
            .collect::<Vec<_>>();
        pixels.sort_by_key(|&i| (std::cmp::Reverse(i / canvas.grid), i % canvas.grid));
        let own = self.own_lines(symbol).max(1) as f64;
        let total = self.mass(symbol) as f64;
        let share = (own / total).clamp(0.12, 0.40);
        let header_count = ((pixels.len() as f64 * share).round() as usize)
            .max(1)
            .min(pixels.len().saturating_sub(children.len()));
        let mut header = vec![false; mask.len()];
        let mut body = display;
        for &pixel in pixels.iter().take(header_count) {
            header[pixel] = true;
            body[pixel] = false;
        }
        if let Some(outline) = ring(&header, canvas, true) {
            self.headers.insert(symbol, outline);
        }
        let reserve_rect = self.rings[symbol]
            .as_ref()
            .and_then(|parent| reserve(&mut body, canvas, parent));
        let header_valid = valid_ring(
            self.headers.get(&symbol),
            self.rings[symbol].as_ref().unwrap(),
        );
        if !header_valid {
            if let Some(rect) = reserve_rect {
                self.headers.insert(
                    symbol,
                    rectangle([
                        rect[0],
                        rect[1] + (rect[3] - rect[1]) * 0.8,
                        rect[2],
                        rect[3],
                    ]),
                );
            }
        }
        let weights = children
            .iter()
            .map(|&child| self.mass(child) as f64)
            .collect::<Vec<_>>();
        let regions = allocate(&body, canvas, &weights);
        for (&child, region) in children.iter().zip(&regions) {
            if region.contains(&true) {
                self.place(child, region, canvas, depth + 1);
            }
        }
        let parent = self.rings[symbol].as_ref().unwrap();
        let mut missing = children
            .iter()
            .copied()
            .filter(|&child| !valid_ring(self.rings[child].as_ref(), parent))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            // A smoothed parent can clip the centre of a child that owns a
            // boundary cell. Restore the exact owned outline before using
            // the subpixel reserve; most such children need no fallback.
            let raw_display = largest_component(
                inset(mask, canvas, depth, children.len() * 2 + 1),
                canvas.grid,
            );
            if let Some(raw) = ring_raw(&raw_display, canvas) {
                self.rings[symbol] = Some(raw);
                let parent = self.rings[symbol].as_ref().unwrap();
                missing.retain(|&child| !valid_ring(self.rings[child].as_ref(), parent));
            }
        }
        if !missing.is_empty() {
            if let Some(rect) = reserve_rect {
                let step = (rect[2] - rect[0]) / missing.len() as f64;
                let top = if !header_valid {
                    rect[1] + (rect[3] - rect[1]) * 0.78
                } else {
                    rect[3]
                };
                for (slot, child) in missing.iter().enumerate() {
                    let x = rect[0] + slot as f64 * step;
                    self.fallback(*child, [x + step * 0.02, rect[1], x + step * 0.98, top]);
                }
            }
        }
    }
}

pub fn attach(map: &MapDocument, document: &mut SymbolsDocument) -> Result<()> {
    let Some(parcels) = &map.parcels else {
        return Ok(());
    };
    let mut by_file = vec![Vec::new(); map.files.len()];
    let mut children = vec![Vec::new(); document.symbols.len()];
    for (i, row) in document.symbols.iter().enumerate() {
        by_file[row.0 .0].push(i);
        if let Some(parent) = local_parent(document, i) {
            children[parent].push(i);
        }
    }
    let mut cards = Cards {
        document,
        children,
        rings: vec![None; document.symbols.len()],
        headers: BTreeMap::new(),
        modules: BTreeMap::new(),
    };
    for (file, symbols) in by_file.iter().enumerate() {
        let Some(polygon) = parcels.get(&file.to_string()) else {
            continue;
        };
        let file_outline = vec![polygon.clone()];
        let top = symbols
            .iter()
            .copied()
            .filter(|&symbol| local_parent(document, symbol).is_none())
            .collect::<Vec<_>>();
        let module_lines = document.module_code_lines.get(&file).copied().unwrap_or(0);
        if top.is_empty() && module_lines == 0 {
            continue;
        }
        let mut low = [f64::INFINITY; 2];
        let mut high = [f64::NEG_INFINITY; 2];
        for point in polygon {
            for axis in 0..2 {
                low[axis] = low[axis].min(point[axis]);
                high[axis] = high[axis].max(point[axis]);
            }
        }
        let span = (high[0] - low[0]).max(high[1] - low[1]).max(1e-9);
        let padding = span * 0.01;
        low = [low[0] - padding, low[1] - padding];
        let span = span + 2.0 * padding;
        let mut grid = ((symbols.len() + usize::from(module_lines > 0)) as f64 * 40.0)
            .sqrt()
            .ceil() as usize;
        grid = grid.clamp(32, 320);
        let has_module = module_lines > 0;
        let weights = (if has_module {
            vec![module_lines as f64]
        } else {
            vec![]
        })
        .into_iter()
        .chain(top.iter().map(|&i| cards.mass(i) as f64))
        .collect::<Vec<_>>();
        loop {
            let mut mask = rasterize(std::slice::from_ref(polygon), low, span, grid);
            let canvas = Canvas {
                grid,
                low,
                step: span / (grid - 1) as f64,
            };
            for &symbol in symbols {
                cards.rings[symbol] = None;
                cards.headers.remove(&symbol);
            }
            cards.modules.remove(&file);
            if mask.iter().filter(|&&yes| yes).count()
                >= symbols.len() * 12 + usize::from(has_module)
                || (grid == 1024 && mask.contains(&true))
            {
                let reserve_rect = reserve(&mut mask, &canvas, &file_outline);
                let regions = allocate(&mask, &canvas, &weights);
                let mut offset = 0;
                if has_module {
                    if let Some(outline) = ring(&inset(&regions[0], &canvas, 0, 1), &canvas, true) {
                        cards.modules.insert(file, outline);
                    }
                    offset = 1;
                }
                for (&symbol, region) in top.iter().zip(regions.iter().skip(offset)) {
                    if region.contains(&true) {
                        cards.place(symbol, region, &canvas, 0);
                    }
                }
                let mut missing = Vec::new();
                if has_module && !valid_ring(cards.modules.get(&file), &file_outline) {
                    missing.push(None);
                }
                missing.extend(
                    top.iter()
                        .copied()
                        .filter(|&i| !valid_ring(cards.rings[i].as_ref(), &file_outline))
                        .map(Some),
                );
                if let Some(rect) = reserve_rect {
                    let step = (rect[2] - rect[0]) / missing.len().max(1) as f64;
                    for (slot, entry) in missing.into_iter().enumerate() {
                        let x = rect[0] + slot as f64 * step;
                        let sub = [x + step * 0.02, rect[1], x + step * 0.98, rect[3]];
                        if let Some(symbol) = entry {
                            cards.fallback(symbol, sub);
                        } else {
                            cards.modules.insert(file, rectangle(sub));
                        }
                    }
                }
            }
            let complete = symbols.iter().all(|&i| {
                document.symbols[i].0 .6 == 0 || valid_ring(cards.rings[i].as_ref(), &file_outline)
            }) && (!has_module
                || valid_ring(cards.modules.get(&file), &file_outline));
            if complete {
                break;
            }
            ensure!(
                grid < 1024,
                "missing card in file {file} at maximum raster resolution"
            );
            grid = (grid * 2).min(1024);
        }
    }
    for (i, row) in document.symbols.iter().enumerate() {
        if row.0 .6 >= 1 && parcels.contains_key(&row.0 .0.to_string()) {
            ensure!(cards.rings[i].is_some(), "missing card for symbol {i}");
        }
    }
    let Cards {
        mut rings,
        mut modules,
        mut headers,
        ..
    } = cards;
    // All exported card coordinates, including fallback and header rectangles,
    // pass through the same rounding step after the raster ownership is final.
    for contours in rings.iter_mut().flatten() {
        quantize_rings(contours)?;
    }
    for contours in modules.values_mut().chain(headers.values_mut()) {
        quantize_rings(contours)?;
    }
    document.symbol_rings = Some(
        rings
            .into_iter()
            .map(|item| item.map(encode_rings))
            .collect(),
    );
    document.module_rings = Some(
        modules
            .into_iter()
            .map(|(key, value)| (key, encode_rings(value)))
            .collect(),
    );
    document.header_rings = Some(
        headers
            .into_iter()
            .map(|(key, value)| (key, encode_rings(value)))
            .collect(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_ring_starts_absolute_then_uses_integer_deltas() {
        assert_eq!(
            encode_rings(rectangle([0.0, 0.0, 1e-11, 1e-11])),
            vec![vec![0, 0, 1, 0, 0, 1, -1, 0]]
        );
    }

    #[test]
    fn reserve_moves_within_boundary_cell_before_shrinking() {
        let canvas = Canvas {
            grid: 3,
            low: [-1.0, -1.0 - 1e-12],
            step: 1.0,
        };
        let mut mask = vec![false; 9];
        mask[4] = true;
        let mut parent = vec![vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0]]];
        let rect = reserve(&mut mask, &canvas, &parent).unwrap();
        assert!(rect[2] - rect[0] > 0.01);
        assert!(rect[3] - rect[1] > 0.01);
        let mut child = rectangle(rect);
        quantize_rings(&mut parent).unwrap();
        quantize_rings(&mut child).unwrap();
        assert!(valid_ring(Some(&child), &parent));
    }

    #[test]
    fn output_quantization_keeps_tiny_cards_and_removes_redundant_points() {
        let mut contours = vec![vec![
            [1.0, 1.0],
            [1.0 + 5e-11, 1.0],
            [1.0 + 1e-10, 1.0],
            [1.0 + 1e-10, 1.0],
            [1.0 + 1e-10, 1.0 + 1e-10],
            [1.0, 1.0 + 1e-10],
        ]];
        quantize_rings(&mut contours).unwrap();
        assert_eq!(contours[0].len(), 4);
        assert_ne!(contours[0][0], contours[0][1]);
        let mut collapsed = rectangle([1.0, 1.0, 1.0 + 1e-12, 1.0 + 1e-12]);
        assert!(quantize_rings(&mut collapsed).is_err());
    }

    fn square_canvas(grid: usize) -> (Canvas, Vec<bool>) {
        (
            Canvas {
                grid,
                low: [0.0, 0.0],
                step: 1.0 / (grid - 1) as f64,
            },
            vec![true; grid * grid],
        )
    }

    #[test]
    fn one_and_two_hundred_symbols_keep_a_connected_nonoverlapping_region() {
        for count in [1, 200] {
            let (canvas, mask) = square_canvas(100);
            let regions = allocate(&mask, &canvas, &vec![1.0; count]);
            let mut claimed = vec![false; mask.len()];
            for region in &regions {
                assert!(region.contains(&true));
                let outline = ring(region, &canvas, true).expect("every owner has an outline");
                assert!(rings_area(&outline) > 0.0);
                let orientation = signed_area(&outline[0]).signum();
                assert_eq!(
                    outline
                        .iter()
                        .filter(|ring| signed_area(ring).signum() == orientation)
                        .count(),
                    1,
                    "one card must have one exterior ring"
                );
                for (i, &yes) in region.iter().enumerate() {
                    if yes {
                        assert!(!claimed[i], "sibling masks overlap");
                        claimed[i] = true;
                    }
                }
            }
        }
    }

    #[test]
    fn child_card_centroid_and_area_stay_inside_parent() {
        let (canvas, mask) = square_canvas(80);
        let parent = ring(&inset(&mask, &canvas, 0, 1), &canvas, false).unwrap();
        let body = inset(&mask, &canvas, 0, 1);
        for child in allocate(&body, &canvas, &[1.0, 3.0, 2.0]) {
            let child = ring(&inset(&child, &canvas, 1, 1), &canvas, true).unwrap();
            let outer = &child[0];
            let center = [
                outer.iter().map(|p| p[0]).sum::<f64>() / outer.len() as f64,
                outer.iter().map(|p| p[1]).sum::<f64>() / outer.len() as f64,
            ];
            assert!(point_in_rings(center, &parent));
            assert!(rings_area(&child) < rings_area(&parent));
        }
    }

    #[test]
    fn card_areas_follow_unequal_code_lines() {
        let (canvas, mask) = square_canvas(80);
        let regions = allocate(&mask, &canvas, &[1.0, 2.0, 4.0]);
        let areas = regions
            .iter()
            .map(|region| region.iter().filter(|&&yes| yes).count())
            .collect::<Vec<_>>();
        assert!(areas[0] < areas[1] && areas[1] < areas[2], "{areas:?}");
    }

    #[test]
    fn type_mass_includes_methods_outside_its_source_span() {
        let document: SymbolsDocument = serde_json::from_value(serde_json::json!({
            "files": [0],
            "symbols": [[0,"T",0,1,2,-1,2],[0,"A",2,10,12,0,3],[0,"B",2,14,16,0,3]],
            "edges": [],
            "module_code_lines": {"0": 0},
            "coverage": {"calls_total":0,"calls_resolved":0,"unresolved":{}}
        }))
        .unwrap();
        let cards = Cards {
            rings: vec![None; 3],
            children: vec![vec![1, 2], vec![], vec![]],
            headers: BTreeMap::new(),
            modules: BTreeMap::new(),
            document: &document,
        };
        assert_eq!(cards.mass(0), 8);
    }

    #[test]
    fn one_pixel_parent_still_gives_two_hundred_children_distinct_rings() {
        use crate::schema::{HierSymbolRow, SymbolCoverage};
        let mut symbols = vec![HierSymbolRow((0, "Parent".into(), 0, 1, 201, -1, 201))];
        for i in 0..200 {
            symbols.push(HierSymbolRow((
                0,
                format!("child{i}"),
                2,
                i + 2,
                i + 2,
                0,
                1,
            )));
        }
        let document = SymbolsDocument {
            files: vec![0],
            symbols,
            edges: Vec::new(),
            module_code_lines: BTreeMap::new(),
            coverage: SymbolCoverage::default(),
            symbol_rings: None,
            module_rings: None,
            header_rings: None,
        };
        let (canvas, _) = square_canvas(8);
        let mut mask = vec![false; 64];
        mask[27] = true;
        let mut cards = Cards {
            rings: vec![None; 201],
            children: std::iter::once((1..201).collect())
                .chain(std::iter::repeat_with(Vec::new).take(200))
                .collect(),
            headers: BTreeMap::new(),
            modules: BTreeMap::new(),
            document: &document,
        };
        cards.place(0, &mask, &canvas, 0);
        let parent = cards.rings[0].as_ref().unwrap();
        let mut previous_right = f64::NEG_INFINITY;
        for child in cards.rings.iter().skip(1) {
            let child = child.as_ref().unwrap();
            assert!(point_in_rings(child[0][0], parent));
            assert!(child[0][0][0] > previous_right);
            previous_right = child[0][1][0];
        }
    }

    #[test]
    fn go_method_with_receiver_in_another_file_gets_local_card() {
        let document: SymbolsDocument = serde_json::from_value(serde_json::json!({
            "files": [0, 1],
            "symbols": [[0,"Receiver",0,1,2,-1,2],[1,"Method",2,10,12,0,3]],
            "edges": [],
            "module_code_lines": {"0": 0, "1": 0},
            "coverage": {"calls_total":0,"calls_resolved":0,"unresolved":{}}
        }))
        .unwrap();
        assert_eq!(local_parent(&document, 0), None);
        assert_eq!(local_parent(&document, 1), None);
        assert_eq!(document.symbols[1].0 .5, 0);
    }

    #[test]
    fn boundary_centroid_is_not_accepted_as_contained() {
        let parent = rectangle([0.0, 0.0, 1.0, 1.0]);
        let touching = rectangle([0.9, 0.4, 1.1, 0.6]);
        assert!(!valid_ring(Some(&touching), &parent));
        let interior = rectangle([0.7, 0.4, 0.9, 0.6]);
        assert!(valid_ring(Some(&interior), &parent));
    }

    #[test]
    fn exported_hole_excludes_an_enclosed_sibling() {
        let (canvas, mut mask) = square_canvas(24);
        for y in 8..16 {
            for x in 8..16 {
                mask[y * canvas.grid + x] = false;
            }
        }
        let outline = ring(&mask, &canvas, false).unwrap();
        assert_eq!(outline.len(), 2);
        assert!(point_in_rings([0.1, 0.1], &outline));
        assert!(!point_in_rings([0.5, 0.5], &outline));
        assert!(rings_area(&outline) < polygon_area(&outline[0]));
    }

    #[test]
    fn simplified_card_does_not_cover_another_owners_pixel() {
        let (canvas, mask) = square_canvas(60);
        let regions = allocate(&mask, &canvas, &[1.0, 3.0, 9.0, 2.0]);
        for (owner, region) in regions.iter().enumerate() {
            let outline = ring(region, &canvas, true).unwrap();
            for (pixel, &claimed) in mask.iter().enumerate() {
                if claimed && !region[pixel] {
                    let center = [
                        canvas.low[0] + (pixel % canvas.grid) as f64 * canvas.step,
                        canvas.low[1] + (pixel / canvas.grid) as f64 * canvas.step,
                    ];
                    for dy in [-0.25, 0.0, 0.25] {
                        for dx in [-0.25, 0.0, 0.25] {
                            let sample =
                                [center[0] + dx * canvas.step, center[1] + dy * canvas.step];
                            assert!(
                                !point_in_rings(sample, &outline),
                                "owner {owner} covers {pixel}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn straight_raster_edges_need_only_corner_vertices() {
        let (canvas, mask) = square_canvas(60);
        let outline = ring(&mask, &canvas, true).unwrap();
        assert_eq!(outline.len(), 1);
        assert!(outline[0].len() <= 12);
    }

    #[test]
    fn convex_corners_move_inward_without_adding_vertices() {
        let staircase = vec![
            [0.0, 0.0],
            [3.0, 0.0],
            [3.0, 1.0],
            [4.0, 1.0],
            [4.0, 4.0],
            [0.0, 4.0],
        ];
        let rounded = smooth(&staircase);
        assert_eq!(rounded.len(), staircase.len());
        assert!(rounded[1][0] < staircase[1][0]);
        assert!(rounded[1][1] > staircase[1][1]);
        assert_eq!(rounded[2], staircase[2]);
        assert!(polygon_area(&rounded) < polygon_area(&staircase));
    }

    #[test]
    fn five_cell_cross_becomes_a_compact_convex_card() {
        let (canvas, mut mask) = square_canvas(16);
        mask.fill(false);
        for cell in [7 * 16 + 7, 6 * 16 + 7, 8 * 16 + 7, 7 * 16 + 6, 7 * 16 + 8] {
            mask[cell] = true;
        }
        let outline = ring(&mask, &canvas, true).unwrap();
        assert_eq!(outline.len(), 1);
        assert_eq!(outline[0].len(), 8);
        let corner_signs = outline[0]
            .iter()
            .enumerate()
            .map(|(i, &point)| {
                let previous = outline[0][(i + 7) % 8];
                let next = outline[0][(i + 1) % 8];
                (point[0] - previous[0]) * (next[1] - point[1])
                    - (point[1] - previous[1]) * (next[0] - point[0])
            })
            .collect::<Vec<_>>();
        assert!(corner_signs.iter().all(|&cross| cross > 0.0));
    }
}
