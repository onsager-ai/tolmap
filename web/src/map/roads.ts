// A3 (import roads, issue #82): district-to-district import roads, ported
// from the prototype's road()/exitPt()/inRings() (arch20.body.html) as pure
// geometry + aggregation functions rather than 1:1 vanilla-JS, so
// MapRenderer only has to turn the result into SVG elements. Pure and
// deterministic: no DOM here (matches map/pins.ts, map/graph.ts), so the
// selection/aggregation half can be exercised from a plain Node script the
// same way check-district-colours.ts exercises colour.ts.
import type { District, MapDocument } from "@/types";
import { D_, districtClass, districtWorldArea } from "./geometry";

type Pt = [number, number];

// ---------- aggregation (loadDocument calls this once per document) ----------

export interface DistrictFlow {
  a: number;
  b: number;
  /** directed import count a -> b */
  ab: number;
  /** directed import count b -> a */
  ba: number;
  total: number;
}

/** Aggregate doc.E (file->file import edges) into DIRECTED counts between
 * every pair of MAINLAND districts, skipping intra-district edges (not a
 * road: a road connects two PLACES) and any edge touching an island or
 * unconnected district. Islands already have their own de-emphasis (issue
 * #34's fade/thin-outline treatment) -- a road out to one runs from the
 * mainland cluster to a point on the offshore ring well outside
 * mainlandBounds (geometry.rs::relocate_offshore), which is exactly the
 * "many thin grey ribbons run far past the mainland, out to islands and off
 * screen" clutter a PR review measured on dify (18 of its 37 non-unconnected
 * districts are islands). One entry per unordered pair, keyed
 * `min(a,b)-max(a,b)` so `ab`/`ba` always mean "from the pair's lower id to
 * its higher id" / the reverse, consistently. */
export function aggregateDistrictFlows(doc: MapDocument): Map<string, DistrictFlow> {
  const flows = new Map<string, DistrictFlow>();
  for (const [x, y] of doc.E) {
    const da = D_(doc, x);
    const db = D_(doc, y);
    if (da === db) continue;
    if (districtClass(doc.districts[String(da)]) !== "mainland") continue;
    if (districtClass(doc.districts[String(db)]) !== "mainland") continue;
    const a = Math.min(da, db);
    const b = Math.max(da, db);
    const key = `${a}-${b}`;
    let f = flows.get(key);
    if (!f) {
      f = { a, b, ab: 0, ba: 0, total: 0 };
      flows.set(key, f);
    }
    if (da === a) f.ab++;
    else f.ba++;
    f.total++;
  }
  return flows;
}

const byTotalDesc = (x: DistrictFlow, y: DistrictFlow) => y.total - x.total || x.a - y.a || x.b - y.b;

/** Which district pairs actually get drawn as a road: the top `topN` pairs
 * by total (both directions summed), plus every district's own `perDistrict`
 * strongest pairs -- so a district with no pair large enough for the global
 * top N still gets its own busiest connections shown, the same "top N
 * overall, plus each place's own strongest" rule the prototype's district
 * drill-down (`drawRoads`'s `keep`/`for (const l in mem)` loop) uses one
 * level down, at the lobe/district-drilldown scope. Deterministic: ties
 * break on the lower district id, then the higher. */
export function selectRoadPairs(flows: Map<string, DistrictFlow>, topN = 12, perDistrict = 2): DistrictFlow[] {
  const all = [...flows.values()];
  const sorted = [...all].sort(byTotalDesc);
  const keep = new Set<string>();
  for (const f of sorted.slice(0, topN)) keep.add(`${f.a}-${f.b}`);
  const byDistrict = new Map<number, DistrictFlow[]>();
  for (const f of all) {
    for (const id of [f.a, f.b]) {
      let list = byDistrict.get(id);
      if (!list) {
        list = [];
        byDistrict.set(id, list);
      }
      list.push(f);
    }
  }
  for (const list of byDistrict.values()) {
    for (const f of [...list].sort(byTotalDesc).slice(0, perDistrict)) keep.add(`${f.a}-${f.b}`);
  }
  return sorted.filter((f) => keep.has(`${f.a}-${f.b}`));
}

// ---------- per-district approximate radius (mouth flare + exitPt fallback) ----------

/** A district's approximate on-map radius from its world-space blob area
 * (area of a circle of that radius = area of the blob) -- used both as
 * exitPt()'s fallback distance (if the centre-to-centre ray somehow starts
 * outside the district's own polygon, or the district has no polygon at
 * all) and as the road ribbon's mouth-flare base (see buildRoadGeom). */
export function districtRadius(area: number): number {
  return Math.sqrt(Math.max(area, 0) / Math.PI);
}

// ---------- exitPt / inRings (ported from arch20.body.html) ----------

/** Even-odd ray casting: is `p` inside any of `rings` (a district's `blob`,
 * a list of one or more closed polygons -- a district can be several
 * disjoint blobs)? Ported directly from the prototype's `inRings`. */
export function inRings(p: Pt, rings: Pt[][]): boolean {
  let c = false;
  for (const r of rings) {
    for (let a = 0, b = r.length - 1; a < r.length; b = a++) {
      const [xi, yi] = r[a];
      const [xj, yj] = r[b];
      if (yi > p[1] !== yj > p[1] && p[0] < ((xj - xi) * (p[1] - yi)) / (yj - yi) + xi) c = !c;
    }
  }
  return c;
}

/** Where the straight line from district centre `A` toward `B` exits `A`'s
 * own blob -- coarse 40-step march to bracket the crossing, then 12-step
 * bisection to refine it (ported from the prototype's `exitPt`, same step
 * counts). Falls back to a point `fallR` out along the A->B direction when
 * there's no polygon to exit (an island/mainland district always has one;
 * this only matters for a degenerate zero-area district) or `A` isn't even
 * inside its own polygon (shouldn't happen -- `district.c` is the blob's own
 * centroid/placement point -- kept as a defensive fallback rather than an
 * assertion, consistent with this codebase's other geometry fallbacks). */
export function exitPt(A: Pt, B: Pt, rings: Pt[][], fallR: number): Pt {
  if (!rings.length || !inRings(A, rings)) {
    const dx = B[0] - A[0];
    const dy = B[1] - A[1];
    const L = Math.hypot(dx, dy) || 1;
    return [A[0] + (dx / L) * fallR, A[1] + (dy / L) * fallR];
  }
  let lo = 0;
  let hi = 1;
  for (let k = 0; k < 40; k++) {
    const t = k / 40;
    const p: Pt = [A[0] + (B[0] - A[0]) * t, A[1] + (B[1] - A[1]) * t];
    if (!inRings(p, rings)) {
      hi = t;
      break;
    }
    lo = t;
  }
  for (let k = 0; k < 12; k++) {
    const m = (lo + hi) / 2;
    const p: Pt = [A[0] + (B[0] - A[0]) * m, A[1] + (B[1] - A[1]) * m];
    if (inRings(p, rings)) lo = m;
    else hi = m;
  }
  return [A[0] + (B[0] - A[0]) * hi, A[1] + (B[1] - A[1]) * hi];
}

// ---------- ribbon + chevron geometry ----------

export interface RoadChevron {
  tip: Pt;
  back1: Pt;
  back2: Pt;
}

export interface RoadGeom {
  /** Closed ribbon polygon, in SCREEN space, ready for an SVG `path` `d`. */
  polygon: Pt[];
  /** Open chevron(s) -- one v-shaped mark per entry, SCREEN space. */
  chevrons: RoadChevron[];
  /** Quadratic-bezier hit path, SCREEN space, for a wide transparent
   * "stroke-width" hit target (>= 10-12px, spec) -- constant-width strokes
   * are cheaper to hit-test as a stroked path than trying to keep a second
   * filled polygon in sync. */
  hitD: string;
}

/** `k`: current world->screen scale -- needed to convert the two world-space
 * fallback/mouth radii (`raWorld`/`rbWorld`, from `districtRadius`) into the
 * screen-px flare this ribbon's ends actually use. `widthPx` is the
 * ribbon's OWN constant on-screen width (already computed by the caller
 * from flow/maxFlow, see MapRenderer) -- everything in this function after
 * that is screen-space geometry; `k` never touches point positions
 * (`toScreen` does that), only the two radii. */
export function buildRoadGeom(opts: {
  a: Pt;
  b: Pt;
  ringsA: Pt[][];
  ringsB: Pt[][];
  raWorld: number;
  rbWorld: number;
  widthPx: number;
  bothDirections: boolean;
  k: number;
  toScreen: (p: Pt) => Pt;
}): RoadGeom {
  const { a, b, ringsA, ringsB, raWorld, rbWorld, widthPx, bothDirections, k, toScreen } = opts;
  const p0 = exitPt(a, b, ringsA, raWorld);
  const p1 = exitPt(b, a, ringsB, rbWorld);
  // Quadratic bezier control point: the chord's midpoint, offset
  // perpendicular by 0.12x the chord's own length (spec) -- computed in
  // WORLD space. The world->screen transform here is an isotropic scale
  // plus translate (no rotation, no shear), so a ratio computed in world
  // space (this offset, and every tangent/normal direction below) survives
  // unchanged into screen space; only ABSOLUTE lengths (the ribbon's width)
  // need to be screen-space quantities, which is why `widthPx` is supplied
  // directly rather than derived from a world-space stroke width.
  const dx = p1[0] - p0[0];
  const dy = p1[1] - p0[1];
  const c: Pt = [(p0[0] + p1[0]) / 2 - dy * 0.12, (p0[1] + p1[1]) / 2 + dx * 0.12];

  const raPx = raWorld * k;
  const rbPx = rbWorld * k;
  const W = widthPx;
  // Mouth flare: capped at 2.8x the base width, otherwise grows with the
  // district's own on-screen size -- a road into a large district gets a
  // wider "harbour mouth" than one into a sliver. Ported ratios from the
  // prototype's `road()`.
  const mA = Math.min(W * 2.8, raPx * 0.9 + W);
  const mB = Math.min(W * 2.8, rbPx * 0.9 + W);

  const N = 24;
  const worldPts: Pt[] = [];
  const tangents: Pt[] = [];
  for (let i = 0; i <= N; i++) {
    const t = i / N;
    const u = 1 - t;
    const x = u * u * p0[0] + 2 * u * t * c[0] + t * t * p1[0];
    const y = u * u * p0[1] + 2 * u * t * c[1] + t * t * p1[1];
    worldPts.push([x, y]);
    let tx = 2 * u * (c[0] - p0[0]) + 2 * t * (p1[0] - c[0]);
    let ty = 2 * u * (c[1] - p0[1]) + 2 * t * (p1[1] - c[1]);
    const tl = Math.hypot(tx, ty) || 1;
    tx /= tl;
    ty /= tl;
    tangents.push([tx, ty]);
  }
  const screenPts = worldPts.map(toScreen);

  const L1: Pt[] = [];
  const L2: Pt[] = [];
  for (let i = 0; i <= N; i++) {
    const t = i / N;
    // Ease the half-width from the mouth flare down to the base width over
    // the first/last 22% of the curve's length (ported ratio) -- a smooth
    // taper rather than a step where the ribbon meets its own mouth.
    const fa = Math.pow(Math.max(0, 1 - t / 0.22), 2);
    const fb = Math.pow(Math.max(0, 1 - (1 - t) / 0.22), 2);
    const hw = (W + (mA - W) * fa + (mB - W) * fb) / 2;
    const [tx, ty] = tangents[i];
    const nx = -ty;
    const ny = tx;
    const [sx, sy] = screenPts[i];
    L1.push([sx + nx * hw, sy + ny * hw]);
    L2.push([sx - nx * hw, sy - ny * hw]);
  }
  const polygon = L1.concat(L2.slice().reverse());

  const chevronSize = Math.max(W * 1.1, 9);
  const chevronAt = (t: number, dir: 1 | -1): RoadChevron => {
    const i0 = Math.round(t * N);
    const i1 = Math.min(N, Math.max(0, i0 + dir));
    const p = screenPts[i0];
    const q = screenPts[i1];
    let tx = (q[0] - p[0]) * dir;
    let ty = (q[1] - p[1]) * dir;
    const tl = Math.hypot(tx, ty) || 1;
    tx /= tl;
    ty /= tl;
    const hx = -ty;
    const hy = tx;
    const s = chevronSize;
    return {
      tip: [p[0] + tx * s * 0.6, p[1] + ty * s * 0.6],
      back1: [p[0] - tx * s * 0.5 + hx * s * 0.7, p[1] - ty * s * 0.5 + hy * s * 0.7],
      back2: [p[0] - tx * s * 0.5 - hx * s * 0.7, p[1] - ty * s * 0.5 - hy * s * 0.7],
    };
  };
  const chevrons = bothDirections ? [chevronAt(0.38, 1), chevronAt(0.62, -1)] : [chevronAt(0.5, 1)];

  const [sp0x, sp0y] = toScreen(p0);
  const [scx, scy] = toScreen(c);
  const [sp1x, sp1y] = toScreen(p1);
  const hitD = `M${sp0x.toFixed(1)} ${sp0y.toFixed(1)} Q${scx.toFixed(1)} ${scy.toFixed(1)} ${sp1x.toFixed(1)} ${sp1y.toFixed(1)}`;

  return { polygon, chevrons, hitD };
}

/** One item ready for MapRenderer to draw: the aggregated flow, plus enough
 * of each endpoint district's own state (district ids) to build the
 * geometry and the hover/tap card. Kept separate from `RoadGeom` (which is
 * screen-space, recomputed every real paint()) so `loadDocument` can select
 * pairs and compute world-space radii ONCE per document -- see
 * MapRenderer.loadDocument's own comment for why per-document work never
 * repeats per paint(). */
export interface RoadPlan {
  a: number;
  b: number;
  ab: number;
  ba: number;
  total: number;
  raWorld: number;
  rbWorld: number;
}

/** Called once per document (loadDocument): picks which district pairs get
 * a road and precomputes their world-space mouth radii. */
export function buildRoadPlan(doc: MapDocument): RoadPlan[] {
  const flows = aggregateDistrictFlows(doc);
  const pairs = selectRoadPairs(flows);
  return pairs.map((f) => ({
    ...f,
    raWorld: districtRadius(districtWorldArea(doc.districts[String(f.a)] as District)),
    rbWorld: districtRadius(districtWorldArea(doc.districts[String(f.b)] as District)),
  }));
}
