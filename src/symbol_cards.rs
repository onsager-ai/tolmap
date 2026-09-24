//! Vector card geometry inside each displayed file footprint (#82,
//! finding 39).
//!
//! Findings 31 and 34 allocated cards on a raster: a power diagram over grid
//! cells, traced into contours and then smoothed. Every card edge was either
//! a staircase or a rounded copy of one, and a connected raster region could
//! wrap round a sibling into a notched outline. Cards are now a power diagram
//! computed on the polygons themselves. Each level clips convex power cells
//! against its parent region, so a card inside a convex parent is convex;
//! only the file footprint's own notches can reach a top-level card.
//!
//! The raster pass needed a reserved subpixel rectangle so a crowded parent
//! still gave every child a ring. The vector solver has no resolution floor:
//! power weights are capped so that every site stays inside its own cell,
//! which keeps each cell non-empty, and a level that still ends with an
//! empty cell falls back to area-proportional strips, which cannot be empty.
use std::collections::BTreeMap;

use anyhow::{ensure, Result};

use crate::parcels::point_in_polygon;
use crate::schema::{MapDocument, SymbolsDocument};

type Point = [f64; 2];
type Ring = Vec<Point>;
type Rings = Vec<Ring>;

// The smallest reserved fallback cards of the raster pass were roughly 1e-10
// wide. The paired artifact audit found that ten decimals collapses
// exteriors; eleven keeps them. The wire format is unchanged (finding 31).
const CARD_COORDINATE_SCALE: f64 = 1e11;
// Largest relative area error a level accepts before it stops iterating.
// The measured quantity is the within-file Pearson r, which a 2% error per
// card leaves well above the raster pass's .96 (finding 34).
const AREA_TOLERANCE: f64 = 0.02;
// Sites also move to their cell centroids (Lloyd steps), which is what makes
// cells compact rather than slivers; a few steps run even when the first
// weights happen to meet the area tolerance.
const MIN_ITERATIONS: usize = 6;
// Clip against the nearest sites first so the cell shrinks quickly and the
// remaining sites can be rejected by a distance bound without clipping.
const NEAREST_FIRST: usize = 12;
// A power weight may not exceed a neighbour's by this share of their squared
// distance. At or above 1.0 a site can leave its own cell, and a cell can
// then vanish; this is the Voronoi-treemap cap of Nocaj and Brandes.
const NEIGHBOUR_CAP: f64 = 0.9;

/// The half-plane `normal · (x − point) ≤ bound`. Keeping the reference point
/// beside the normal, rather than folding it into one offset, keeps the
/// arithmetic relative to the cell instead of to the world origin.
#[derive(Clone, Copy, Debug)]
struct Plane {
    point: Point,
    normal: Point,
    bound: f64,
}

impl Plane {
    fn eval(&self, p: Point) -> f64 {
        self.normal[0] * (p[0] - self.point[0]) + self.normal[1] * (p[1] - self.point[1])
            - self.bound
    }

    fn horizontal(y: f64, keep_above: bool) -> Self {
        Plane {
            point: [0.0, y],
            normal: if keep_above { [0.0, -1.0] } else { [0.0, 1.0] },
            bound: 0.0,
        }
    }

    fn vertical(x: f64, keep_left: bool) -> Self {
        Plane {
            point: [x, 0.0],
            normal: if keep_left { [1.0, 0.0] } else { [-1.0, 0.0] },
            bound: 0.0,
        }
    }
}

/// The power bisector that keeps site `i`'s side:
/// `|x − s_i|² − W_i ≤ |x − s_j|² − W_j`, rewritten about the midpoint. The
/// `(j, i)` plane is the exact negation of the `(i, j)` plane, so sibling
/// cells classify every vertex complementarily and cannot overlap.
fn bisector(sites: &[Point], weights: &[f64], i: usize, j: usize) -> Plane {
    let (a, b) = (sites[i], sites[j]);
    let normal = [b[0] - a[0], b[1] - a[1]];
    if normal[0] == 0.0 && normal[1] == 0.0 {
        // Coincident sites have no bisector. Splitting by index still gives
        // the two cells complementary halves.
        let sign = if i < j { 1.0 } else { -1.0 };
        return Plane {
            point: a,
            normal: [sign, 0.0],
            bound: 0.0,
        };
    }
    Plane {
        point: [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5],
        normal,
        bound: (weights[i] - weights[j]) * 0.5,
    }
}

fn distance2(a: Point, b: Point) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

fn crossing(a: Point, b: Point, da: f64, db: f64) -> Point {
    let t = da / (da - db);
    [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
}

/// Shoelace area about the first vertex. Card coordinates are world units
/// near 1 while small cards are 1e-6 wide; subtracting the origin first
/// avoids cancelling most of the significant digits.
fn signed_area(ring: &[Point]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let origin = ring[0];
    let mut sum = 0.0;
    for k in 1..ring.len() - 1 {
        let a = [ring[k][0] - origin[0], ring[k][1] - origin[1]];
        let b = [ring[k + 1][0] - origin[0], ring[k + 1][1] - origin[1]];
        sum += a[0] * b[1] - a[1] * b[0];
    }
    sum * 0.5
}

fn rings_area(rings: &[Ring]) -> f64 {
    rings
        .iter()
        .map(|ring| signed_area(ring))
        .sum::<f64>()
        .abs()
}

fn centroid(ring: &[Point]) -> Point {
    let origin = ring[0];
    let mut area2 = 0.0;
    let mut x = 0.0;
    let mut y = 0.0;
    for k in 0..ring.len() {
        let a = [ring[k][0] - origin[0], ring[k][1] - origin[1]];
        let next = ring[(k + 1) % ring.len()];
        let b = [next[0] - origin[0], next[1] - origin[1]];
        let cross = a[0] * b[1] - a[1] * b[0];
        area2 += cross;
        x += (a[0] + b[0]) * cross;
        y += (a[1] + b[1]) * cross;
    }
    if area2 == 0.0 {
        let n = ring.len() as f64;
        return [
            ring.iter().map(|p| p[0]).sum::<f64>() / n,
            ring.iter().map(|p| p[1]).sum::<f64>() / n,
        ];
    }
    [origin[0] + x / (3.0 * area2), origin[1] + y / (3.0 * area2)]
}

fn bounds(rings: &[Ring]) -> (Point, Point) {
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    for point in rings.iter().flatten() {
        for axis in 0..2 {
            low[axis] = low[axis].min(point[axis]);
            high[axis] = high[axis].max(point[axis]);
        }
    }
    (low, high)
}

fn extent(ring: &[Point]) -> f64 {
    let (low, high) = bounds(std::slice::from_ref(&ring.to_vec()));
    (high[0] - low[0]).max(high[1] - low[1])
}

/// Removes repeated points, straight-through points and zero-width spikes.
/// Sutherland–Hodgman clipping leaves all three where a cut passes through or
/// near an existing vertex, and each would count as a vertex on the wire.
fn clean(ring: &[Point]) -> Ring {
    let size = extent(ring);
    let eps = size * 1e-12;
    let same = |a: Point, b: Point| (a[0] - b[0]).abs() <= eps && (a[1] - b[1]).abs() <= eps;
    let mut points: Ring = ring.to_vec();
    loop {
        let before = points.len();
        let mut deduped: Ring = Vec::with_capacity(points.len());
        for &p in &points {
            if deduped.last().is_none_or(|&q| !same(p, q)) {
                deduped.push(p);
            }
        }
        while deduped.len() > 1 && same(deduped[0], *deduped.last().unwrap()) {
            deduped.pop();
        }
        let n = deduped.len();
        points = if n < 3 {
            deduped
        } else {
            (0..n)
                .filter(|&k| {
                    let a = deduped[(k + n - 1) % n];
                    let b = deduped[k];
                    let c = deduped[(k + 1) % n];
                    let u = [b[0] - a[0], b[1] - a[1]];
                    let v = [c[0] - b[0], c[1] - b[1]];
                    let cross = u[0] * v[1] - u[1] * v[0];
                    cross.abs() > 1e-12 * u[0].hypot(u[1]) * v[0].hypot(v[1])
                })
                .map(|k| deduped[k])
                .collect()
        };
        if points.len() == before || points.len() < 3 {
            return points;
        }
    }
}

/// Counter-clockwise, cleaned copy of a region ring, or `None` when it has
/// no area.
fn normalise(ring: &[Point]) -> Option<Ring> {
    let mut ring = clean(ring);
    if ring.len() < 3 {
        return None;
    }
    let area = signed_area(&ring);
    if area == 0.0 || !area.is_finite() {
        return None;
    }
    if area < 0.0 {
        ring.reverse();
    }
    Some(ring)
}

fn clip_points(polygon: &[Point], plane: &Plane) -> Ring {
    let n = polygon.len();
    let mut out = Vec::with_capacity(n + 2);
    for k in 0..n {
        let current = polygon[k];
        let next = polygon[(k + 1) % n];
        let dc = plane.eval(current);
        let dn = plane.eval(next);
        if dc <= 0.0 {
            out.push(current);
            if dn > 0.0 {
                out.push(crossing(current, next, dc, dn));
            }
        } else if dn <= 0.0 {
            out.push(crossing(current, next, dc, dn));
        }
    }
    out
}

/// A convex-cell vertex and the index of the site whose bisector carries the
/// edge that leaves it (−1 for the region's own boundary). The labels give
/// the neighbour list and the shared edge lengths the weight update uses.
#[derive(Clone, Copy, Debug)]
struct Vertex {
    point: Point,
    edge: isize,
}

fn clip_labelled(polygon: &[Vertex], plane: &Plane, label: isize) -> Vec<Vertex> {
    let n = polygon.len();
    let mut out = Vec::with_capacity(n + 2);
    for k in 0..n {
        let current = polygon[k];
        let next = polygon[(k + 1) % n];
        let dc = plane.eval(current.point);
        let dn = plane.eval(next.point);
        if dc <= 0.0 {
            out.push(current);
            if dn > 0.0 {
                out.push(Vertex {
                    point: crossing(current.point, next.point, dc, dn),
                    edge: label,
                });
            }
        } else if dn <= 0.0 {
            out.push(Vertex {
                point: crossing(current.point, next.point, dc, dn),
                edge: current.edge,
            });
        }
    }
    out
}

/// Intersects a simple counter-clockwise ring with a half-plane and returns
/// each connected piece. Sutherland–Hodgman alone joins the pieces of a
/// non-convex ring with zero-width bridges along the cut; filled, they are
/// invisible, but stroked they draw a line through the neighbouring file.
/// Each inside chain runs from an entry crossing to an exit crossing; along
/// the cut line the crossings pair up as (exit, entry) for every interval
/// inside the ring, which says which chain follows which. `None` means the
/// pairing failed (a degenerate touch or a self-intersecting ring).
fn split(ring: &[Point], plane: &Plane) -> Option<Rings> {
    let n = ring.len();
    if n < 3 {
        return Some(Vec::new());
    }
    let values = ring.iter().map(|&p| plane.eval(p)).collect::<Vec<_>>();
    let inside = values.iter().filter(|&&value| value <= 0.0).count();
    if inside == n {
        return Some(vec![ring.to_vec()]);
    }
    if inside == 0 {
        return Some(Vec::new());
    }
    let start = (0..n).find(|&k| values[k] > 0.0 && values[(k + 1) % n] <= 0.0)?;
    let mut chains: Rings = Vec::new();
    let mut current: Ring = Vec::new();
    for step in 0..n {
        let k = (start + step) % n;
        let next = (k + 1) % n;
        let (a, b, da, db) = (ring[k], ring[next], values[k], values[next]);
        if da > 0.0 && db <= 0.0 {
            current = vec![crossing(a, b, da, db)];
        }
        if db <= 0.0 {
            current.push(b);
        } else if da <= 0.0 {
            current.push(crossing(a, b, da, db));
            chains.push(std::mem::take(&mut current));
        }
    }
    let direction = [-plane.normal[1], plane.normal[0]];
    let along = |p: Point| direction[0] * p[0] + direction[1] * p[1];
    // (position along the cut, 0 = exit / 1 = entry, chain)
    let mut crossings = Vec::with_capacity(chains.len() * 2);
    for (index, chain) in chains.iter().enumerate() {
        crossings.push((along(*chain.last()?), 0u8, index));
        crossings.push((along(chain[0]), 1u8, index));
    }
    crossings.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut following = vec![usize::MAX; chains.len()];
    for pair in crossings.chunks(2) {
        if pair.len() != 2 || pair[0].1 != 0 || pair[1].1 != 1 {
            return None;
        }
        following[pair[0].2] = pair[1].2;
    }
    let mut visited = vec![false; chains.len()];
    let mut pieces = Vec::new();
    for first in 0..chains.len() {
        if visited[first] {
            continue;
        }
        let mut piece = Vec::new();
        let mut chain = first;
        loop {
            if chain == usize::MAX || visited[chain] {
                return None;
            }
            visited[chain] = true;
            piece.extend_from_slice(&chains[chain]);
            chain = following[chain];
            if chain == first {
                break;
            }
        }
        pieces.push(piece);
    }
    Some(pieces)
}

/// `split`, checked against the Sutherland–Hodgman area (which is exact for
/// any ring, bridges included). A failed pairing is retried with the cut
/// moved by 1e-10 of the ring's size; the last resort keeps the bridged
/// ring, which fills correctly.
fn split_robust(ring: &[Point], plane: &Plane) -> Rings {
    let whole = signed_area(ring).abs();
    let size = extent(ring);
    let norm = plane.normal[0].hypot(plane.normal[1]);
    let keep = |pieces: Rings| {
        pieces
            .into_iter()
            .map(|piece| clean(&piece))
            .filter(|piece| piece.len() >= 3 && signed_area(piece) > whole * 1e-12)
            .collect::<Rings>()
    };
    for attempt in 0..4 {
        let mut moved = *plane;
        if attempt > 0 {
            let sign = if attempt % 2 == 0 { 1.0 } else { -1.0 };
            moved.bound += sign * attempt as f64 * norm * size * 1e-10;
        }
        if let Some(pieces) = split(ring, &moved) {
            let expected = signed_area(&clip_points(ring, &moved));
            let got = pieces.iter().map(|piece| signed_area(piece)).sum::<f64>();
            if (got - expected).abs() <= whole * 1e-9 {
                return keep(pieces);
            }
        }
    }
    keep(vec![clip_points(ring, plane)])
}

fn largest(pieces: &[Ring]) -> Option<&Ring> {
    let mut best: Option<(&Ring, f64)> = None;
    for piece in pieces {
        let area = signed_area(piece);
        if area > 0.0 && best.is_none_or(|(_, top)| area > top) {
            best = Some((piece, area));
        }
    }
    best.map(|(piece, _)| piece)
}

fn cross(o: Point, a: Point, b: Point) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn hull(points: &[Point]) -> Ring {
    let mut sorted = points.to_vec();
    sorted.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    sorted.dedup();
    if sorted.len() < 3 {
        return sorted;
    }
    let mut lower: Ring = Vec::new();
    for &q in &sorted {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], q) <= 0.0 {
            lower.pop();
        }
        lower.push(q);
    }
    let mut upper: Ring = Vec::new();
    for &q in sorted.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], q) <= 0.0 {
            upper.pop();
        }
        upper.push(q);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Counter-clockwise and convex. The hull-area comparison rejects a
/// self-intersecting star, whose corners all turn left too.
fn is_convex(ring: &[Point]) -> bool {
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let turns_left = (0..n).all(|k| {
        let a = ring[(k + n - 1) % n];
        let b = ring[k];
        let c = ring[(k + 1) % n];
        let u = [b[0] - a[0], b[1] - a[1]];
        let v = [c[0] - b[0], c[1] - b[1]];
        u[0] * v[1] - u[1] * v[0] >= -1e-9 * u[0].hypot(u[1]) * v[0].hypot(v[1])
    });
    if !turns_left {
        return false;
    }
    let hull_area = signed_area(&hull(ring));
    (hull_area - signed_area(ring)).abs() <= hull_area * 1e-9
}

fn is_simple(ring: &[Point]) -> bool {
    let n = ring.len();
    for i in 0..n {
        let (p1, p2) = (ring[i], ring[(i + 1) % n]);
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            let (q1, q2) = (ring[j], ring[(j + 1) % n]);
            let d1 = cross(q1, q2, p1);
            let d2 = cross(q1, q2, p2);
            let d3 = cross(p1, p2, q1);
            let d4 = cross(p1, p2, q2);
            if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
                return false;
            }
        }
    }
    true
}

fn inset_convex(ring: &[Point], delta: f64) -> Option<Ring> {
    let n = ring.len();
    let mut out = ring.to_vec();
    for k in 0..n {
        let p = ring[k];
        let q = ring[(k + 1) % n];
        let e = [q[0] - p[0], q[1] - p[1]];
        let length = e[0].hypot(e[1]);
        if length == 0.0 {
            continue;
        }
        // The interior of a counter-clockwise ring is on the left.
        let inward = [-e[1] / length, e[0] / length];
        let plane = Plane {
            point: p,
            normal: [-inward[0], -inward[1]],
            bound: -delta,
        };
        out = clip_points(&out, &plane);
        if out.len() < 3 {
            return None;
        }
    }
    let out = clean(&out);
    (out.len() >= 3 && signed_area(&out) > 0.0).then_some(out)
}

/// Mitred inward offset of a non-convex ring. It is only accepted before
/// the offset's first topological event: every edge keeps its direction,
/// the ring stays simple, and every vertex stays inside the original.
fn inset_mitred(ring: &[Point], delta: f64) -> Option<Ring> {
    let n = ring.len();
    let mut normals = Vec::with_capacity(n);
    for k in 0..n {
        let p = ring[k];
        let q = ring[(k + 1) % n];
        let e = [q[0] - p[0], q[1] - p[1]];
        let length = e[0].hypot(e[1]);
        if length == 0.0 {
            return None;
        }
        normals.push([-e[1] / length, e[0] / length]);
    }
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let a = normals[(k + n - 1) % n];
        let b = normals[k];
        let denominator = 1.0 + a[0] * b[0] + a[1] * b[1];
        if denominator < 0.1 {
            return None;
        }
        out.push([
            ring[k][0] + delta * (a[0] + b[0]) / denominator,
            ring[k][1] + delta * (a[1] + b[1]) / denominator,
        ]);
    }
    for k in 0..n {
        let before = [
            ring[(k + 1) % n][0] - ring[k][0],
            ring[(k + 1) % n][1] - ring[k][1],
        ];
        let after = [
            out[(k + 1) % n][0] - out[k][0],
            out[(k + 1) % n][1] - out[k][1],
        ];
        if before[0] * after[0] + before[1] * after[1] <= 0.0 {
            return None;
        }
    }
    let valid = signed_area(&out) > 0.0
        && is_simple(&out)
        && out.iter().all(|&point| point_in_polygon(point, ring));
    valid.then(|| clean(&out)).filter(|out| out.len() >= 3)
}

fn inset(ring: &[Point], delta: f64) -> Option<Ring> {
    if is_convex(ring) {
        inset_convex(ring, delta)
    } else {
        inset_mitred(ring, delta)
    }
}

/// The drawn card for an allocated piece: inset by the raster pass's gutter
/// distances, halving the distance when a thin piece cannot take the full
/// gutter, and never a ring that collapses at the wire precision.
fn card_ring(piece: &[Point], depth: usize) -> Option<Ring> {
    let area = signed_area(piece).max(0.0);
    let mut delta = if depth == 0 {
        (0.035 * area.sqrt()).min(0.004)
    } else {
        (0.05 * area.sqrt()).min(0.003)
    };
    for _ in 0..6 {
        if let Some(ring) = inset(piece, delta) {
            let mut rings = vec![ring];
            if quantize_rings(&mut rings).is_ok() {
                return rings.pop();
            }
        }
        delta *= 0.5;
    }
    let mut rings = vec![piece.to_vec()];
    quantize_rings(&mut rings).ok()?;
    rings.pop()
}

/// A class card's header band: the top `share` of the card's area above one
/// horizontal cut, with the members' body below it.
fn split_band(card: &[Point], share: f64) -> (Option<Ring>, Rings) {
    let total = signed_area(card);
    let (low, high) = bounds(std::slice::from_ref(&card.to_vec()));
    let target = total * share;
    let (mut below, mut above) = (low[1], high[1]);
    for _ in 0..64 {
        let middle = 0.5 * (below + above);
        let area = signed_area(&clip_points(card, &Plane::horizontal(middle, true)));
        if area > target {
            below = middle;
        } else {
            above = middle;
        }
    }
    let cut = 0.5 * (below + above);
    let header = largest(&split_robust(card, &Plane::horizontal(cut, true))).cloned();
    let body = split_robust(card, &Plane::horizontal(cut, false));
    (header, body)
}

fn in_region(point: Point, rings: &[Ring]) -> bool {
    rings
        .iter()
        .filter(|ring| point_in_polygon(point, ring))
        .count()
        % 2
        == 1
}

/// A point strictly inside the ring: the area centroid when it is inside,
/// otherwise the middle of the longest inside run of the horizontal line
/// through it.
fn interior_point(ring: &[Point]) -> Option<Point> {
    if ring.len() < 3 {
        return None;
    }
    let center = centroid(ring);
    if point_in_polygon(center, ring) {
        return Some(center);
    }
    let y = center[1];
    let n = ring.len();
    let mut xs = Vec::new();
    for k in 0..n {
        let a = ring[k];
        let b = ring[(k + 1) % n];
        if (a[1] > y) != (b[1] > y) {
            xs.push(a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
        }
    }
    xs.sort_by(f64::total_cmp);
    let mut best: Option<(f64, Point)> = None;
    for pair in xs.chunks_exact(2) {
        let width = pair[1] - pair[0];
        if width > 0.0 && best.is_none_or(|(top, _)| width > top) {
            best = Some((width, [(pair[0] + pair[1]) * 0.5, y]));
        }
    }
    best.map(|(_, point)| point)
}

/// SplitMix64: a fixed, dependency-free generator, so a level's first sites
/// depend only on `SEED` and the level's identity.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn level_seed(file: usize, symbol: Option<usize>) -> u64 {
    let key = ((file as u64) << 32) ^ symbol.map_or(0xFFFF_FFFF, |symbol| symbol as u64);
    crate::SEED ^ key.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// Seeded rejection sampling inside the region. Sites are then ordered from
/// the top down, so children in source order start from the top of their
/// parent, just below its header band.
fn initial_sites(rings: &[Ring], count: usize, seed: u64) -> Option<Vec<Point>> {
    let (low, high) = bounds(rings);
    let gap2 = ((high[0] - low[0]).max(high[1] - low[1]) * 1e-9).powi(2);
    let mut rng = Rng(seed);
    let mut sites = Vec::with_capacity(count);
    let mut attempts = 0;
    while sites.len() < count && attempts < 200 * count + 1000 {
        attempts += 1;
        let point = [
            low[0] + rng.unit() * (high[0] - low[0]),
            low[1] + rng.unit() * (high[1] - low[1]),
        ];
        if !in_region(point, rings) || sites.iter().any(|&site| distance2(site, point) <= gap2) {
            continue;
        }
        sites.push(point);
    }
    if sites.len() < count {
        return None;
    }
    sites.sort_by(|a, b| b[1].total_cmp(&a[1]).then(a[0].total_cmp(&b[0])));
    Some(sites)
}

struct Region<'a> {
    rings: &'a [Ring],
    convex: bool,
    start: Vec<Vertex>,
}

struct Cell {
    pieces: Rings,
    area: f64,
    /// Σ shared-edge length / (2 · site distance): how fast this cell's area
    /// grows per unit of its own power weight (the diagonal of the
    /// semi-discrete optimal-transport Hessian).
    growth: f64,
    neighbours: Vec<usize>,
}

fn reach_of(polygon: &[Vertex], site: Point) -> f64 {
    polygon
        .iter()
        .map(|vertex| distance2(vertex.point, site))
        .fold(0.0, f64::max)
        .sqrt()
}

fn compute_cell(region: &Region, sites: &[Point], weights: &[f64], i: usize) -> Cell {
    let n = sites.len();
    let site = sites[i];
    let order_key = |a: &(f64, usize), b: &(f64, usize)| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1));
    let mut order = (0..n)
        .filter(|&j| j != i)
        .map(|j| (distance2(site, sites[j]), j))
        .collect::<Vec<_>>();
    let nearest = NEAREST_FIRST.min(order.len());
    if nearest < order.len() {
        order.select_nth_unstable_by(nearest, order_key);
    }
    order[..nearest].sort_by(order_key);
    let mut polygon = region.start.clone();
    let mut reach = reach_of(&polygon, site);
    for (position, &(d2, j)) in order.iter().enumerate() {
        if position >= nearest {
            // The bisector lies (d² + W_i − W_j) / 2d from the site; no
            // vertex is further than `reach`, so it cannot cut the cell.
            if d2 + weights[i] - weights[j] >= 2.0 * d2.sqrt() * reach {
                continue;
            }
        }
        let plane = bisector(sites, weights, i, j);
        if polygon.iter().all(|vertex| plane.eval(vertex.point) <= 0.0) {
            continue;
        }
        polygon = clip_labelled(&polygon, &plane, j as isize);
        if polygon.len() < 3 {
            polygon.clear();
            break;
        }
        reach = reach_of(&polygon, site);
    }
    let mut growth = 0.0;
    let mut neighbours = Vec::new();
    for k in 0..polygon.len() {
        let Ok(j) = usize::try_from(polygon[k].edge) else {
            continue;
        };
        let next = polygon[(k + 1) % polygon.len()].point;
        let length = distance2(polygon[k].point, next).sqrt();
        let distance = distance2(site, sites[j]).sqrt();
        if distance > 0.0 {
            growth += length / (2.0 * distance);
        }
        neighbours.push(j);
    }
    neighbours.sort_unstable();
    neighbours.dedup();
    let pieces = if polygon.len() < 3 {
        Vec::new()
    } else if region.convex {
        let ring = clean(&polygon.iter().map(|vertex| vertex.point).collect::<Ring>());
        if ring.len() >= 3 && signed_area(&ring) > 0.0 {
            vec![ring]
        } else {
            Vec::new()
        }
    } else {
        // Within the region's hull only the bisectors that bound the convex
        // cell can cut; the others contain it.
        let mut pieces = region.rings.to_vec();
        for &j in &neighbours {
            let plane = bisector(sites, weights, i, j);
            pieces = pieces
                .iter()
                .flat_map(|piece| split_robust(piece, &plane))
                .collect();
            if pieces.is_empty() {
                break;
            }
        }
        pieces
    };
    let area = pieces.iter().map(|piece| signed_area(piece)).sum();
    Cell {
        pieces,
        area,
        growth,
        neighbours,
    }
}

/// Area-proportional vertical strips: the fallback for a level whose power
/// cells did not all survive. A strip with a positive target cannot be
/// empty, but strips are not compact, so they are a last resort.
fn strips(rings: &[Ring], weights: &[f64]) -> Vec<Rings> {
    let total = rings_area(rings);
    let sum = weights.iter().sum::<f64>();
    let (low, high) = bounds(rings);
    let left_area = |x: f64| {
        rings
            .iter()
            .map(|ring| signed_area(&clip_points(ring, &Plane::vertical(x, true))))
            .sum::<f64>()
    };
    let mut cuts = vec![low[0]];
    let mut cumulative = 0.0;
    for weight in &weights[..weights.len().saturating_sub(1)] {
        cumulative += weight;
        let target = total * cumulative / sum;
        let (mut left, mut right) = (*cuts.last().unwrap(), high[0]);
        for _ in 0..64 {
            let middle = 0.5 * (left + right);
            if left_area(middle) < target {
                left = middle;
            } else {
                right = middle;
            }
        }
        cuts.push(0.5 * (left + right));
    }
    cuts.push(high[0]);
    (0..weights.len())
        .map(|i| {
            rings
                .iter()
                .flat_map(|ring| split_robust(ring, &Plane::vertical(cuts[i], false)))
                .flat_map(|piece| split_robust(&piece, &Plane::vertical(cuts[i + 1], true)))
                .collect()
        })
        .collect()
}

/// Splits a region (disjoint counter-clockwise rings) among children in
/// proportion to `weights`, returning each child's pieces. This is a
/// capacity-constrained power diagram: each iteration moves every power
/// weight by a diagonally preconditioned optimal-transport step towards its
/// target area, then moves every site to its cell's centroid.
fn partition(region: &[Ring], weights: &[f64], seed: u64) -> Vec<Rings> {
    let n = weights.len();
    let rings = region
        .iter()
        .filter(|ring| ring.len() >= 3 && signed_area(ring) > 0.0)
        .cloned()
        .collect::<Rings>();
    if n <= 1 {
        return vec![rings; n];
    }
    let total = rings_area(&rings);
    if total <= 0.0 || !total.is_finite() {
        return vec![Vec::new(); n];
    }
    let sum = weights.iter().sum::<f64>();
    let targets = weights
        .iter()
        .map(|weight| weight / sum * total)
        .collect::<Vec<_>>();
    let convex = rings.len() == 1 && is_convex(&rings[0]);
    let start = if convex {
        rings[0].clone()
    } else {
        hull(&rings.iter().flatten().copied().collect::<Vec<_>>())
    };
    let region = Region {
        rings: &rings,
        convex,
        start: start
            .into_iter()
            .map(|point| Vertex { point, edge: -1 })
            .collect(),
    };
    let Some(mut sites) = initial_sites(&rings, n, seed) else {
        return strips(&rings, weights);
    };
    let mut power = vec![0.0; n];
    let iterations = if n <= 64 {
        80
    } else if n <= 256 {
        48
    } else {
        24
    };
    let mut iteration = 0;
    let cells = loop {
        let cells = (0..n)
            .map(|i| compute_cell(&region, &sites, &power, i))
            .collect::<Vec<_>>();
        iteration += 1;
        let error = cells
            .iter()
            .zip(&targets)
            .map(|(cell, target)| (cell.area - target).abs() / target)
            .fold(0.0, f64::max);
        if (error <= AREA_TOLERANCE && iteration > MIN_ITERATIONS) || iteration >= iterations {
            break cells;
        }
        for i in 0..n {
            let mut nearest = cells[i]
                .neighbours
                .iter()
                .map(|&j| distance2(sites[i], sites[j]))
                .fold(f64::INFINITY, f64::min);
            if !nearest.is_finite() {
                nearest = (0..n)
                    .filter(|&j| j != i)
                    .map(|j| distance2(sites[i], sites[j]))
                    .fold(f64::INFINITY, f64::min);
            }
            let step = if cells[i].growth > 0.0 {
                0.5 * (targets[i] - cells[i].area) / cells[i].growth
            } else {
                0.25 * nearest
            };
            power[i] += step.clamp(-0.5 * nearest, 0.5 * nearest);
        }
        for (site, cell) in sites.iter_mut().zip(&cells) {
            if let Some(point) = largest(&cell.pieces).and_then(|piece| interior_point(piece)) {
                *site = point;
            }
        }
        let floor = power.iter().copied().fold(f64::INFINITY, f64::min);
        for weight in &mut power {
            *weight -= floor;
        }
        for _ in 0..3 {
            let mut changed = false;
            for i in 0..n {
                for &j in &cells[i].neighbours {
                    let limit = power[j] + NEIGHBOUR_CAP * distance2(sites[i], sites[j]);
                    if power[i] > limit {
                        power[i] = limit;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    };
    if cells.iter().any(|cell| largest(&cell.pieces).is_none()) {
        return strips(&rings, weights);
    }
    cells.into_iter().map(|cell| cell.pieces).collect()
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

    /// Draws `symbol` in the largest of its allocated pieces, then splits its
    /// card into a header band for its own lines and a body shared by its
    /// members, each member inset inside its own share.
    fn place(&mut self, symbol: usize, region: &[Ring], depth: usize, file: usize) {
        let Some(card) = largest(region).and_then(|piece| card_ring(piece, depth)) else {
            return;
        };
        let children = self.children[symbol].clone();
        if !children.is_empty() {
            let own = self.own_lines(symbol).max(1) as f64;
            let share = (own / self.mass(symbol) as f64).clamp(0.12, 0.40);
            let (header, body) = split_band(&card, share);
            if let Some(header) = header {
                let mut rings = vec![header];
                if quantize_rings(&mut rings).is_ok() {
                    self.headers.insert(symbol, rings);
                }
            }
            let weights = children
                .iter()
                .map(|&child| self.mass(child) as f64)
                .collect::<Vec<_>>();
            let regions = partition(&body, &weights, level_seed(file, Some(symbol)));
            for (&child, region) in children.iter().zip(&regions) {
                self.place(child, region, depth + 1, file);
            }
        }
        self.rings[symbol] = Some(vec![card]);
    }
}

pub fn attach(map: &MapDocument, document: &mut SymbolsDocument) -> Result<()> {
    attach_with_progress(map, document, None)
}

pub fn attach_with_progress(
    map: &MapDocument,
    document: &mut SymbolsDocument,
    progress: Option<&crate::progress::StageCounter>,
) -> Result<()> {
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
        let advance = || {
            if let Some(progress) = progress {
                progress.advance(1);
            }
        };
        let Some(root) = parcels
            .get(&file.to_string())
            .and_then(|polygon| normalise(polygon))
        else {
            advance();
            continue;
        };
        let top = symbols
            .iter()
            .copied()
            .filter(|&symbol| local_parent(document, symbol).is_none())
            .collect::<Vec<_>>();
        let module_lines = document.module_code_lines.get(&file).copied().unwrap_or(0);
        if top.is_empty() && module_lines == 0 {
            advance();
            continue;
        }
        let has_module = module_lines > 0;
        let weights = has_module
            .then_some(module_lines as f64)
            .into_iter()
            .chain(top.iter().map(|&i| cards.mass(i) as f64))
            .collect::<Vec<_>>();
        let regions = partition(&[root], &weights, level_seed(file, None));
        let mut offset = 0;
        if has_module {
            if let Some(ring) = largest(&regions[0]).and_then(|piece| card_ring(piece, 0)) {
                cards.modules.insert(file, vec![ring]);
            }
            offset = 1;
        }
        for (&symbol, region) in top.iter().zip(regions.iter().skip(offset)) {
            cards.place(symbol, region, 0, file);
        }
        advance();
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
    // Every exported coordinate passes through the same rounding step. The
    // cards were already validated at this precision, so this pass only
    // normalises; it is idempotent on an already rounded ring.
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

    fn rectangle(rect: [f64; 4]) -> Ring {
        vec![
            [rect[0], rect[1]],
            [rect[2], rect[1]],
            [rect[2], rect[3]],
            [rect[0], rect[3]],
        ]
    }

    fn l_shape() -> Ring {
        // A 0.1-wide L: the notch sits in the top right.
        vec![
            [0.3, 0.3],
            [0.4, 0.3],
            [0.4, 0.35],
            [0.35, 0.35],
            [0.35, 0.4],
            [0.3, 0.4],
        ]
    }

    fn inside_all(point: Point, pieces: &[Ring]) -> bool {
        pieces.iter().any(|piece| point_in_polygon(point, piece))
    }

    #[test]
    fn wire_ring_starts_absolute_then_uses_integer_deltas() {
        assert_eq!(
            encode_rings(vec![rectangle([0.0, 0.0, 1e-11, 1e-11])]),
            vec![vec![0, 0, 1, 0, 0, 1, -1, 0]]
        );
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
        let mut collapsed = vec![rectangle([1.0, 1.0, 1.0 + 1e-12, 1.0 + 1e-12])];
        assert!(quantize_rings(&mut collapsed).is_err());
    }

    #[test]
    fn split_of_a_u_shape_returns_both_arms_without_a_bridge() {
        let u = vec![
            [0.0, 0.0],
            [3.0, 0.0],
            [3.0, 2.0],
            [2.0, 2.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [0.0, 2.0],
        ];
        let pieces = split_robust(&u, &Plane::horizontal(1.5, true));
        assert_eq!(pieces.len(), 2);
        for piece in &pieces {
            assert_eq!(piece.len(), 4);
            assert!((signed_area(piece) - 0.5).abs() < 1e-12);
        }
        let below = split_robust(&u, &Plane::horizontal(1.5, false));
        assert_eq!(below.len(), 1);
        assert!((rings_area(&below) - 4.0).abs() < 1e-12);
    }

    #[test]
    fn convex_regions_split_into_one_convex_piece() {
        let square = rectangle([0.0, 0.0, 1.0, 1.0]);
        let plane = Plane {
            point: [0.5, 0.5],
            normal: [1.0, 2.0],
            bound: 0.1,
        };
        let pieces = split_robust(&square, &plane);
        assert_eq!(pieces.len(), 1);
        assert!(is_convex(&pieces[0]));
        let expected = signed_area(&clip_points(&square, &plane));
        assert!((signed_area(&pieces[0]) - expected).abs() < 1e-12);
    }

    #[test]
    fn partition_areas_follow_unequal_code_lines() {
        let square = rectangle([0.2, 0.2, 0.3, 0.3]);
        let weights = [1.0, 2.0, 4.0, 8.0, 3.0];
        let regions = partition(std::slice::from_ref(&square), &weights, 7);
        let total = signed_area(&square);
        for (region, weight) in regions.iter().zip(weights) {
            let target = total * weight / 18.0;
            let area = rings_area(region);
            assert!(
                (area / target - 1.0).abs() < 0.05,
                "area {area} target {target}"
            );
            assert_eq!(region.len(), 1, "a convex parent gives one piece");
            assert!(is_convex(&region[0]));
        }
    }

    #[test]
    fn partition_pieces_are_disjoint_and_inside_a_non_convex_parent() {
        let region = vec![l_shape()];
        let weights = (0..30).map(|i| (i % 7 + 1) as f64).collect::<Vec<_>>();
        let regions = partition(&region, &weights, 11);
        assert_eq!(regions.len(), 30);
        for pieces in &regions {
            let piece = largest(pieces).expect("every child has a piece");
            assert!(in_region(interior_point(piece).unwrap(), &region));
            for point in piece {
                assert!(point[0] >= 0.3 - 1e-12 && point[0] <= 0.4 + 1e-12);
                assert!(point[1] >= 0.3 - 1e-12 && point[1] <= 0.4 + 1e-12);
            }
        }
        for ix in 0..80 {
            for iy in 0..80 {
                let point = [
                    0.3 + (ix as f64 + 0.37) * 0.1 / 80.0,
                    0.3 + (iy as f64 + 0.61) * 0.1 / 80.0,
                ];
                let owners = regions
                    .iter()
                    .filter(|pieces| inside_all(point, pieces))
                    .count();
                assert!(owners <= 1, "{point:?} owned by {owners}");
                if owners == 1 {
                    assert!(in_region(point, &region));
                }
            }
        }
    }

    #[test]
    fn partition_is_deterministic() {
        let region = vec![l_shape()];
        let weights = [5.0, 1.0, 1.0, 9.0, 2.0, 3.0, 1.0];
        assert_eq!(
            partition(&region, &weights, 7),
            partition(&region, &weights, 7)
        );
    }

    #[test]
    fn strips_give_every_weight_a_piece() {
        let region = vec![l_shape()];
        let weights = [1.0, 50.0, 1.0, 1.0, 20.0];
        let regions = strips(&region, &weights);
        let total = rings_area(&region);
        for (pieces, weight) in regions.iter().zip(weights) {
            assert!(largest(pieces).is_some());
            let target = total * weight / 73.0;
            assert!((rings_area(pieces) / target - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn convex_card_inset_stays_inside_and_keeps_its_shape() {
        let square = rectangle([0.2, 0.2, 0.3, 0.3]);
        let card = card_ring(&square, 0).unwrap();
        assert_eq!(card.len(), 4);
        assert!(signed_area(&card) < signed_area(&square));
        assert!(card.iter().all(|&p| point_in_polygon(p, &square)));
    }

    #[test]
    fn mitred_inset_handles_a_notched_parent() {
        let region = l_shape();
        let card = inset(&region, 0.002).expect("a thin gutter fits the L");
        assert_eq!(card.len(), region.len());
        assert!(is_simple(&card));
        assert!(card.iter().all(|&p| point_in_polygon(p, &region)));
        assert!(signed_area(&card) < signed_area(&region));
    }

    #[test]
    fn header_band_takes_its_share_of_the_card() {
        let card = rectangle([0.0, 0.0, 0.1, 0.1]);
        let (header, body) = split_band(&card, 0.25);
        let header = header.unwrap();
        assert!((signed_area(&header) - 0.0025).abs() < 1e-12);
        assert!((rings_area(&body) - 0.0075).abs() < 1e-12);
        assert!(header.iter().all(|p| p[1] >= 0.075 - 1e-12));
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
    fn tiny_parent_still_gives_two_hundred_children_disjoint_cards() {
        use crate::schema::{HierSymbolRow, SymbolCoverage};
        let mut symbols = vec![HierSymbolRow((
            0,
            "Parent".into(),
            0,
            1,
            201,
            -1,
            201,
            false,
        ))];
        for i in 0..200 {
            symbols.push(HierSymbolRow((
                0,
                format!("child{i}"),
                2,
                i + 2,
                i + 2,
                0,
                1,
                false,
            )));
        }
        let document = SymbolsDocument {
            files: vec![0],
            symbols,
            edges: Vec::new(),
            kinds: crate::schema::symbol_edge_kinds(),
            module_code_lines: BTreeMap::new(),
            coverage: SymbolCoverage::default(),
            symbol_rings: None,
            module_rings: None,
            header_rings: None,
        };
        let mut cards = Cards {
            rings: vec![None; 201],
            children: std::iter::once((1..201).collect())
                .chain(std::iter::repeat_with(Vec::new).take(200))
                .collect(),
            headers: BTreeMap::new(),
            modules: BTreeMap::new(),
            document: &document,
        };
        let parent_region = rectangle([0.5, 0.5, 0.5 + 1e-4, 0.5 + 1e-4]);
        cards.place(0, std::slice::from_ref(&parent_region), 0, 0);
        let parent = &cards.rings[0].as_ref().unwrap()[0];
        let children = cards.rings[1..]
            .iter()
            .map(|rings| rings.as_ref().expect("every child has a card")[0].clone())
            .collect::<Vec<_>>();
        for (i, child) in children.iter().enumerate() {
            assert!(signed_area(child) > 0.0);
            assert!(child.iter().all(|&p| point_in_polygon(p, parent)));
            let center = centroid(child);
            for (j, other) in children.iter().enumerate() {
                if i != j {
                    assert!(!point_in_polygon(center, other), "{i} inside {j}");
                }
            }
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
}
