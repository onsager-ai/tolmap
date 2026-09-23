// B4 (nested footprints, issue #82, scope item 6): small-footprint hit
// circles. A file whose on-screen footprint area is under pi*8^2 also gets an
// invisible circular hit target, radius `min(8px, half the distance to the
// nearest OTHER visible file centroid)` -- generous enough to tap reliably,
// capped so two small, close-together footprints never get overlapping hit
// circles that would fight over the same tap. The "nearest visible centroid"
// is necessarily a per-PAINT, screen-space quantity (what's visible changes
// every pan/zoom), so this can't be precomputed once per document the way
// districtOrder/roadPlan/hubSet are -- it's recomputed each paint(), but only
// over the files actually drawn that frame (viewport-culled already), not
// the whole document, so the cost is proportional to what's on screen.
// Grid-accelerated for the same reason colour.ts's `near()` is: an all-pairs
// scan across a few thousand visible files would be millions of distance
// calls for what is, per file, a handful of realistic neighbours. Pure and
// DOM-free so it's unit-testable without a browser.
export const SMALL_FOOTPRINT_HIT_CAP_PX = 8;

export interface ScreenPoint {
  i: number;
  x: number;
  y: number;
}

/** For every point in `points`, half the distance to its nearest OTHER point
 * in the same list, capped at `capPx` -- exactly the radius scope item 6
 * specifies. A point with no other point within `capPx*2` (so nothing could
 * possibly beat the cap) gets `capPx` outright without a distance check. */
export function smallFootprintHitRadii(points: readonly ScreenPoint[], capPx: number = SMALL_FOOTPRINT_HIT_CAP_PX): Map<number, number> {
  const cell = Math.max(1, capPx * 2);
  const grid = new Map<string, ScreenPoint[]>();
  const keyOf = (x: number, y: number) => `${Math.floor(x / cell)},${Math.floor(y / cell)}`;
  for (const p of points) {
    const k = keyOf(p.x, p.y);
    let bucket = grid.get(k);
    if (!bucket) {
      bucket = [];
      grid.set(k, bucket);
    }
    bucket.push(p);
  }
  const out = new Map<number, number>();
  for (const p of points) {
    const cx = Math.floor(p.x / cell);
    const cy = Math.floor(p.y / cell);
    let best = Infinity;
    for (let dx = -1; dx <= 1; dx++) {
      for (let dy = -1; dy <= 1; dy++) {
        const bucket = grid.get(`${cx + dx},${cy + dy}`);
        if (!bucket) continue;
        for (const q of bucket) {
          if (q.i === p.i) continue;
          const d = Math.hypot(p.x - q.x, p.y - q.y);
          if (d < best) best = d;
        }
      }
    }
    out.set(p.i, Math.min(capPx, best === Infinity ? capPx : best / 2));
  }
  return out;
}

/** On-screen area a footprint's own bounding box occupies -- cheap stand-in
 * for the polygon's true (shoelace) area, computed from the same x0/y0/x1/y1
 * extrema the renderer already tracks while building the path's `d` string,
 * so this costs nothing extra per file. A bbox is >= the true polygon area,
 * so this only ever OVER-estimates -- a file that's genuinely small enough to
 * need the extra hit circle is never missed by this approximation; the worst
 * case is an oddly elongated footprint that could have qualified on true
 * area but doesn't on bbox area, which just means it relies on its own
 * (larger) polygon shape for hit-testing instead, which is exactly what a
 * bigger footprint does anyway. */
export function bboxArea(x0: number, y0: number, x1: number, y1: number): number {
  return Math.max(0, x1 - x0) * Math.max(0, y1 - y0);
}
