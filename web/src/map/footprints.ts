// B4 perf follow-up (issue #82, owner review of the first B4 PR): footprint
// mode's per-file paint cost was the blocker -- one <path> element per file,
// with hit-testing riding on the browser's own native hit-test for each of
// them, put dify's paint well over budget (measured: desktop drag p50
// 198.8ms, phone p50 1325ms against a ~150ms target -- see the PR
// description). The fix has two halves:
//
//   1. Fills for SMALL-on-screen files batch into one <path> per (district,
//      shade), pointer-events:none (MapRenderer's own batching logic).
//   2. Hit-testing for those files moves off the DOM entirely: a world-space
//      spatial index over every file's `P` polygon, queried by point-in-
//      polygon on click/hover instead of relying on an element existing to
//      hit-test against at all. This module is that index -- pure and
//      DOM-free, like every other map/*.ts geometry module, so it's testable
//      without a browser.
import type { MapDocument } from "@/types";
import { D_, polygonArea } from "./geometry";
import { inRings } from "./roads";

type Pt = [number, number];

/** Each district's MEDIAN file footprint area (world units^2) -- the input
 * to MapRenderer's own "is this district's footprints large enough on
 * screen to draw individually" decision (perf follow-up, issue #82): the
 * caller multiplies by k^2 and compares the square root against a px
 * threshold every paint, but the per-file areas themselves are static
 * geometry, computed once per document like every other loadDocument()
 * cache. Median, not mean: a district with one huge landmark file and many
 * tiny ones should still batch the tiny ones, which a mean would resist. */
export function districtMedianFootprintArea(doc: MapDocument): Map<number, number> {
  const byDistrict = new Map<number, number[]>();
  if (doc.P) {
    for (let i = 0; i < doc.F.length; i++) {
      const poly = doc.P[String(i)];
      if (!poly || poly.length < 3) continue;
      const d = D_(doc, i);
      let list = byDistrict.get(d);
      if (!list) {
        list = [];
        byDistrict.set(d, list);
      }
      list.push(polygonArea(poly));
    }
  }
  const out = new Map<number, number>();
  for (const [d, areas] of byDistrict) {
    areas.sort((a, b) => a - b);
    out.set(d, areas[areas.length >> 1] || 0);
  }
  return out;
}

export interface FootprintIndex {
  /** World-space grid cell size. */
  cell: number;
  /** cell key ("cx,cy") -> file indices whose bbox overlaps that cell. A
   * file can appear in several cells (its bbox can span more than one), so a
   * lookup only ever needs to check ONE cell -- the one containing the query
   * point -- not a 3x3 neighbourhood, unlike the point-to-point grids in
   * colour.ts/footprints.ts's own former near-neighbour search: those find
   * the nearest of many discrete POINTS, which can sit just across a cell
   * boundary from the query cell, while this index inserts by AREA overlap,
   * so any polygon covering the query point was already registered in the
   * query point's own cell when it was built. */
  grid: Map<string, number[]>;
}

/** Every file with a `P` polygon, bucketed by world-space bbox overlap. Cell
 * size is the MEDIAN file bbox extent across the document -- large enough
 * that most files land in a small, boundable number of cells (not one cell
 * per pixel), small enough that a query's single cell doesn't end up
 * holding a large fraction of the document (dify: 6,347 files, cells sized
 * to its own median footprint keep bucket counts in the tens, not
 * thousands). Built once per document (MapRenderer.loadDocument), the same
 * "per-document work never repeats per paint" rule every other cache in
 * that method follows. */
export function buildFootprintIndex(doc: MapDocument): FootprintIndex {
  const grid = new Map<string, number[]>();
  if (!doc.P) return { cell: 1, grid };
  const extents: number[] = [];
  const boxes = new Map<number, [number, number, number, number]>();
  for (let i = 0; i < doc.F.length; i++) {
    const poly = doc.P[String(i)];
    if (!poly || poly.length < 3) continue;
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const [x, y] of poly) {
      if (x < x0) x0 = x;
      if (y < y0) y0 = y;
      if (x > x1) x1 = x;
      if (y > y1) y1 = y;
    }
    boxes.set(i, [x0, y0, x1, y1]);
    extents.push(Math.max(x1 - x0, y1 - y0));
  }
  if (extents.length === 0) return { cell: 1, grid };
  extents.sort((a, b) => a - b);
  const median = extents[extents.length >> 1] || 1e-6;
  const cell = Math.max(median, 1e-6);
  for (const [i, [x0, y0, x1, y1]] of boxes) {
    const cx0 = Math.floor(x0 / cell);
    const cx1 = Math.floor(x1 / cell);
    const cy0 = Math.floor(y0 / cell);
    const cy1 = Math.floor(y1 / cell);
    for (let cx = cx0; cx <= cx1; cx++) {
      for (let cy = cy0; cy <= cy1; cy++) {
        const key = `${cx},${cy}`;
        let bucket = grid.get(key);
        if (!bucket) {
          bucket = [];
          grid.set(key, bucket);
        }
        bucket.push(i);
      }
    }
  }
  return { cell, grid };
}

/** Which file's footprint (if any) contains world point `(wx, wy)` --
 * point-in-polygon (roads.ts's own `inRings`, one ring at a time) over just
 * the query cell's bucket, not the whole document. Ties (touching polygons
 * at a shared edge, vanishingly rare with a Voronoi-style diagram) resolve
 * to the lowest file index in the bucket, deterministically. Called from
 * MapRenderer's click/hover resolution, replacing the DOM hit-test a
 * batched (pointer-events:none) footprint has no element to receive.
 *
 * Known limitation, measured on the django fixture rather than assumed: a
 * file's own reported `footprint_centroids` entry does not always land
 * inside that file's own `P` polygon (roughly 15% of django's files, all in
 * districts with many near-degenerate, reserved-minimum-pixel cells --
 * finding 27's own prototype measurement of the same phenomenon, "only 17%
 * of sites fell inside their own cell", was never claimed fully closed by
 * finding 29's rewrite at this sub-pixel scale). For those files, a tap
 * exactly on the reported centroid can resolve to a geometrically adjacent
 * file instead. This is a layout/backend precision question, not a viewer
 * bug -- `hitTestFootprint` faithfully tests the polygon it's given -- and
 * is out of scope for this (viewer-only) change; not observed to affect
 * anything larger than the smallest slivers in a district. */
export function hitTestFootprint(doc: MapDocument, index: FootprintIndex, wx: number, wy: number): number | null {
  if (!doc.P) return null;
  const key = `${Math.floor(wx / index.cell)},${Math.floor(wy / index.cell)}`;
  const bucket = index.grid.get(key);
  if (!bucket) return null;
  const p: Pt = [wx, wy];
  for (const i of bucket) {
    const poly = doc.P[String(i)];
    if (poly && inRings(p, [poly])) return i;
  }
  return null;
}
