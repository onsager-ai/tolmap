// A2 (district colour, issue #82): a district's hue is no longer `id % 12`
// against a 12-colour wheel with no adjacency awareness -- it's a greedy
// graph colouring over six colour-blind-validated hues (--H0..--H5, matching
// the prototype's arch20.head.html palette), so two districts that actually
// TOUCH on the map never render the same colour. This module is pure and
// deterministic: no DOM, no randomness, so `check-district-colours.ts` can
// run it over a fixture from plain Node and so the SAME assignment always
// comes out of the SAME map JSON (CLAUDE.md's determinism rule, applied to
// colour rather than geometry).
import type { District, MapDocument } from "@/types";
import { districtClass } from "./geometry";

export const HUE_COUNT = 6;

type Point = [number, number];
type Blob = Point[][];

/** Adjacency threshold, in world units (the same units `district.blob`
 * coordinates are already in -- no `k` scaling here, this runs once per
 * document, before any viewport exists).
 *
 * The prototype (arch20.body.html: `near()`, right above its own greedy
 * colouring loop) used a flat 0.14 on a world extent of about 4 units --
 * 3.5% of span. Measured on this port's own fixtures (dify: span 5.30x4.29,
 * django: span 3.84x3.91 -- see the district blob bounding box below), the
 * two builds land in the same ~4-5 unit range the prototype was tuned
 * against, so the same RATIO (not the same flat constant, since the compact
 * schema's coordinates are not pinned to a fixed 0..4 box across every
 * fixture) is what actually travels: `ADJACENCY_SPAN_FRACTION * span` gives
 * 0.14-0.19 on this port's own maps, i.e. materially the same absolute
 * distance the prototype validated by eye. A flat constant would either
 * under-trigger on a map whose layout happens to pack into a smaller box, or
 * over-trigger (colouring distant districts as "neighbours") on a larger
 * one.
 */
export const ADJACENCY_SPAN_FRACTION = 0.035;

function blobBounds(districts: Record<string, District>, ids: string[]): [number, number, number, number] {
  let x0 = 1e9, y0 = 1e9, x1 = -1e9, y1 = -1e9;
  for (const id of ids) {
    for (const poly of districts[id].blob) {
      for (const [x, y] of poly) {
        if (x < x0) x0 = x;
        if (y < y0) y0 = y;
        if (x > x1) x1 = x;
        if (y > y1) y1 = y;
      }
    }
  }
  if (x1 < x0) return [0, 0, 1, 1];
  return [x0, y0, x1, y1];
}

/** Bounding box of any blob (a district's or a neighbourhood's -- both are
 * `Point[][]`, possibly several disjoint rings). Exported so
 * neighbourhoods.ts's own adjacency check reuses the exact box/grid/near
 * machinery below instead of a second copy at the smaller (neighbourhood)
 * scale. */
export function blobBox(blob: Blob): [number, number, number, number] {
  let x0 = 1e9, y0 = 1e9, x1 = -1e9, y1 = -1e9;
  for (const poly of blob) {
    for (const [x, y] of poly) {
      if (x < x0) x0 = x;
      if (y < y0) y0 = y;
      if (x > x1) x1 = x;
      if (y > y1) y1 = y;
    }
  }
  return [x0, y0, x1, y1];
}
/** A uniform grid over `pts` (cell size = `threshold`), so `near()` below
 * never does an O(points_a * points_b) double loop -- dify alone has a
 * district with 984 outline points, and an all-pairs check across 78
 * districts would be billions of distance calls. Each point only needs to
 * check its own cell plus the 8 neighbours (a point in an adjacent cell can
 * still be within `threshold`, but nothing two cells away can). */
function buildGrid(pts: Point[], cell: number): Map<string, Point[]> {
  const grid = new Map<string, Point[]>();
  for (const p of pts) {
    const key = `${Math.floor(p[0] / cell)},${Math.floor(p[1] / cell)}`;
    let bucket = grid.get(key);
    if (!bucket) {
      bucket = [];
      grid.set(key, bucket);
    }
    bucket.push(p);
  }
  return grid;
}

function flatten(blob: Blob): Point[] {
  const out: Point[] = [];
  for (const poly of blob) for (const p of poly) out.push(p);
  return out;
}

/** True once any point of `a`'s outline comes within `threshold` of any
 * point of `b`'s outline. Grid-accelerated (see buildGrid); bbox-rejected
 * first so two blobs nowhere near each other never even build a grid.
 * Exported (as `nearBlobs`) so neighbourhoods.ts's smaller-scale adjacency
 * check is the SAME algorithm, not a second implementation -- only the
 * threshold and which blobs get compared differ at that scope. */
export function nearBlobs(a: Blob, b: Blob, threshold: number): boolean {
  const boxA = blobBox(a);
  const boxB = blobBox(b);
  if (boxA[2] + threshold < boxB[0] || boxB[2] + threshold < boxA[0]) return false;
  if (boxA[3] + threshold < boxB[1] || boxB[3] + threshold < boxA[1]) return false;
  const ptsA = flatten(a);
  const ptsB = flatten(b);
  if (ptsA.length === 0 || ptsB.length === 0) return false;
  const grid = buildGrid(ptsA.length <= ptsB.length ? ptsA : ptsB, threshold);
  const probe = ptsA.length <= ptsB.length ? ptsB : ptsA;
  const t2 = threshold * threshold;
  for (const p of probe) {
    const cx = Math.floor(p[0] / threshold);
    const cy = Math.floor(p[1] / threshold);
    for (let dx = -1; dx <= 1; dx++) {
      for (let dy = -1; dy <= 1; dy++) {
        const bucket = grid.get(`${cx + dx},${cy + dy}`);
        if (!bucket) continue;
        for (const q of bucket) {
          const ddx = p[0] - q[0];
          const ddy = p[1] - q[1];
          if (ddx * ddx + ddy * ddy < t2) return true;
        }
      }
    }
  }
  return false;
}
function near(a: District, b: District, threshold: number): boolean {
  return nearBlobs(a.blob, b.blob, threshold);
}

export interface DistrictAdjacency {
  /** district id (numeric) -> ids of districts whose blob comes within the
   * adjacency threshold. */
  neighbours: Map<number, number[]>;
  threshold: number;
}

/** Every mainland/island district's neighbour set -- exported on its own
 * (not just the final hue map) because `check-district-colours.ts` needs it
 * to report "a district with >=6 coloured neighbours" as the one case six
 * hues cannot avoid a repeat for. */
export function districtAdjacency(doc: MapDocument): DistrictAdjacency {
  const ids = Object.keys(doc.districts).filter((id) => districtClass(doc.districts[id]) !== "unconnected");
  const bounds = blobBounds(doc.districts, ids);
  const span = Math.max(bounds[2] - bounds[0], bounds[3] - bounds[1]) || 1;
  const threshold = span * ADJACENCY_SPAN_FRACTION;
  const neighbours = new Map<number, number[]>();
  for (const id of ids) neighbours.set(+id, []);
  for (let i = 0; i < ids.length; i++) {
    for (let j = i + 1; j < ids.length; j++) {
      if (near(doc.districts[ids[i]], doc.districts[ids[j]], threshold)) {
        neighbours.get(+ids[i])!.push(+ids[j]);
        neighbours.get(+ids[j])!.push(+ids[i]);
      }
    }
  }
  return { neighbours, threshold };
}

/** Greedy colouring: districts visited size-descending (ties by id
 * ascending, so two same-sized districts always resolve the same way --
 * CLAUDE.md's determinism rule), each picking the lowest-index hue not
 * already used by an ALREADY-COLOURED neighbour (later neighbours haven't
 * been assigned yet and don't constrain this pick -- same as the prototype's
 * `adj[k].map(n=>DC[n])`, which reads whatever's in `DC` so far). If every
 * hue is taken among coloured neighbours (only possible once a district has
 * six or more of them, which six hues can't avoid), fall back to the
 * globally least-used hue, ties to the lowest index -- keeps the six hues
 * roughly balanced across the whole map rather than piling onto hue 0. */
export function assignDistrictHues(doc: MapDocument): Map<number, number> {
  const { neighbours } = districtAdjacency(doc);
  const ids = [...neighbours.keys()].sort((a, b) => doc.districts[String(b)].size - doc.districts[String(a)].size || a - b);
  const hue = new Map<number, number>();
  const used = new Array(HUE_COUNT).fill(0);
  for (const id of ids) {
    const takenByNeighbour = new Set((neighbours.get(id) ?? []).map((n) => hue.get(n)).filter((h): h is number => h != null));
    let pick = -1;
    for (let h = 0; h < HUE_COUNT; h++) {
      if (!takenByNeighbour.has(h)) {
        pick = h;
        break;
      }
    }
    if (pick === -1) {
      pick = 0;
      for (let h = 1; h < HUE_COUNT; h++) if (used[h] < used[pick]) pick = h;
    }
    hue.set(id, pick);
    used[pick]++;
  }
  return hue;
}
