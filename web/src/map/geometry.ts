import type { District, DistrictClass, MapDocument } from "@/types";
import type { Geo } from "./constants";

/** A district's legibility class (issue #34), defaulting to "mainland" when
 * absent or unrecognised. ts-rs types `District.class` as required, but the
 * nine `data/*.json` acceptance fixtures were recorded before this field
 * existed and carry no such key at all -- at runtime `district.class` is
 * `undefined` for every district in those maps. Reading it through this
 * helper everywhere (never `district.class` directly) is what keeps a
 * pre-#34 fixture rendering byte-for-byte as it does on `main`: unclassified
 * reads as mainland, which is what every district effectively was before
 * this feature existed, and is also what `#[serde(default)]` on the Rust
 * struct itself resolves to for the same fixtures. */
export function districtClass(district: District): DistrictClass {
  return district.class === "island" || district.class === "unconnected" ? district.class : "mainland";
}

// ---------- row accessors ----------
// NodeRow = [d, x, y, loc, cplx, churn, fanin, rx, ry, rw, rh] — a fixed tuple
// shape from the Rust side (see bindings/NodeRow.ts). Indexing by position
// instead of decoding into an object matches the reference exactly and keeps
// this file a straight line-for-line port of the accessor block in
// viewer/template.html.
export const D_ = (doc: MapDocument, i: number) => doc.N[i][0];
export const PX = (doc: MapDocument, i: number) => doc.N[i][1];
export const PY = (doc: MapDocument, i: number) => doc.N[i][2];
export const LOC = (doc: MapDocument, i: number) => doc.N[i][3];
export const CX_ = (doc: MapDocument, i: number) => doc.N[i][4];
export const CH = (doc: MapDocument, i: number) => doc.N[i][5];
export const FI = (doc: MapDocument, i: number) => doc.N[i][6];
export const RECT = (doc: MapDocument, i: number): [number, number, number, number] =>
  doc.N[i].slice(7) as [number, number, number, number];

/** Node position: world coords in every geometry mode except treemap, where a
 * file has no fixed point and "position" means the centre of its rect. */
export function px(doc: MapDocument, geo: Geo, i: number): [number, number] {
  if (geo !== "t") return [PX(doc, i), PY(doc, i)];
  const r = RECT(doc, i);
  return [r[0] + r[2] / 2, r[1] + r[3] / 2];
}

export function symbolsOf(doc: MapDocument, i: number) {
  return doc.S?.[String(i)] ?? [];
}

export function worldBounds(doc: MapDocument, geo: Geo): [number, number, number, number] {
  const a: [number, number, number, number] = [1e9, 1e9, -1e9, -1e9];
  const put = (x: number, y: number) => {
    a[0] = Math.min(a[0], x);
    a[1] = Math.min(a[1], y);
    a[2] = Math.max(a[2], x);
    a[3] = Math.max(a[3], y);
  };
  for (let i = 0; i < doc.N.length; i++) {
    const p = px(doc, geo, i);
    put(p[0], p[1]);
  }
  if (geo === "r") {
    for (const d in doc.districts) {
      for (const poly of doc.districts[d].blob) {
        for (const q of poly) put(q[0], q[1]);
      }
    }
  } else {
    for (let i = 0; i < doc.N.length; i++) {
      const r = RECT(doc, i);
      put(r[0] + r[2], r[1] + r[3]);
    }
  }
  return a;
}

/** Same as `worldBounds`, but scoped to mainland districts only (issue #34).
 * This is the box the default view and the "fit" control frame -- islands
 * and unconnected districts sit on rings well outside it
 * (`geometry.rs::relocate_offshore`), and framing the full extent by
 * default would open a repository like n8n on a mostly-empty frame with the
 * actual 28-district map squeezed into a corner of it (measured: mainland
 * is 27.2% of n8n's full extent, 31.6% of dify's). The full extent stays
 * reachable by zooming out -- see `MapRenderer`'s `clampK`, which floors on
 * `fullFitScale`, not this -- offshore districts are meant to be reachable,
 * not foregrounded.
 *
 * When every district is mainland (every pre-#34 fixture: a missing `class`
 * defaults there, see `districtClass`), every `continue` below is a no-op
 * and this returns exactly what `worldBounds` does -- today's framing is
 * unchanged. */
export function mainlandBounds(doc: MapDocument, geo: Geo): [number, number, number, number] {
  const a: [number, number, number, number] = [1e9, 1e9, -1e9, -1e9];
  const put = (x: number, y: number) => {
    a[0] = Math.min(a[0], x);
    a[1] = Math.min(a[1], y);
    a[2] = Math.max(a[2], x);
    a[3] = Math.max(a[3], y);
  };
  const isMainland = (i: number) => districtClass(doc.districts[String(D_(doc, i))]) === "mainland";
  for (let i = 0; i < doc.N.length; i++) {
    if (!isMainland(i)) continue;
    const p = px(doc, geo, i);
    put(p[0], p[1]);
  }
  if (geo === "r") {
    for (const d in doc.districts) {
      if (districtClass(doc.districts[d]) !== "mainland") continue;
      for (const poly of doc.districts[d].blob) {
        for (const q of poly) put(q[0], q[1]);
      }
    }
  } else {
    for (let i = 0; i < doc.N.length; i++) {
      if (!isMainland(i)) continue;
      const r = RECT(doc, i);
      put(r[0] + r[2], r[1] + r[3]);
    }
  }
  // A document with no mainland district at all is a degenerate case no
  // real corpus reaches (it needs >=100 districts of near-equal size to
  // clear MAINLAND_SHARE_PERCENT's 1% floor nowhere -- geometry.rs's own
  // classify_districts doc comment) -- fall back to the full extent rather
  // than hand the caller the untouched +-1e9 sentinel.
  if (a[2] < a[0]) return worldBounds(doc, geo);
  return a;
}

// Split out of fitScale/fullFitScale so MapRenderer can reuse it against a
// CACHED bounds box (issue #51 perf follow-up). MapRenderer.paint() used to
// call fitScale() -- which recomputes mainlandBounds(), an O(N + blob
// points) walk -- once PER FILE DOT, turning an O(N) paint into O(N^2)
// (profiled on langgenius/dify, 6.3k files: 3.1s of 3.5s paint CPU was
// mainlandBounds's own self time). The fix caches the BOUNDS, not the
// scale -- a viewport resize changes the scale but not the bounds, and
// caching the final number would have to be invalidated on VW/VH anyway --
// so this is the one place both the uncached callers below and
// MapRenderer's cached ones turn a bounds box back into a scale.
const FIT_PAD = 46;
export function scaleToFit(b: [number, number, number, number], vw: number, vh: number): number {
  return Math.min((vw - 2 * FIT_PAD) / (b[2] - b[0] || 1), (vh - 2 * FIT_PAD) / (b[3] - b[1] || 1));
}

export function fitScale(doc: MapDocument, geo: Geo, vw: number, vh: number): number {
  return scaleToFit(mainlandBounds(doc, geo), vw, vh);
}

/** Like `fitScale`, but against the FULL extent (mainland + islands +
 * unconnected). `MapRenderer` uses this only as the floor for how far a
 * viewer can zoom OUT -- never as the default framing, which is
 * `fitScale`/`mainlandBounds` (see that function's doc comment for why). */
export function fullFitScale(doc: MapDocument, geo: Geo, vw: number, vh: number): number {
  return scaleToFit(worldBounds(doc, geo), vw, vh);
}

// ---------- colour ----------
const hx = (c: string) => [1, 3, 5].map((i) => parseInt(c.slice(i, i + 2), 16));
const mix = (a: number[], b: number[], t: number) => a.map((v, i) => Math.round(v + (b[i] - v) * t));

/** Churn/complexity ramp: cool → warm, three fixed stops. Same palette as the
 * reference so screenshots and colour-blind-safe review stay comparable. */
export function ramp(t: number): string {
  const A = hx("#3E6E88");
  const B = hx("#B8B06A");
  const C = hx("#C0472F");
  const r = t < 0.5 ? mix(A, B, t * 2) : mix(B, C, (t - 0.5) * 2);
  return `rgb(${r.join(",")})`;
}

let cssCache: CSSStyleDeclaration | null = null;
/** District colour is a pure function of `district id % 12` against the
 * --c0..--c11 custom properties in index.css — never randomised, never a
 * hash of the district's name (names can change on rename; ids are stable
 * within one build). This is the determinism rule from CLAUDE.md applied to
 * colour instead of geometry. */
export function districtColor(d: number): string {
  if (!cssCache) cssCache = getComputedStyle(document.documentElement);
  return cssCache.getPropertyValue(`--c${((d % 12) + 12) % 12}`).trim();
}

// ---------- rooms / treemap ----------
// A file, opened. Everything on this map is a plan view, so a file is not a
// tower — it is a PLOT, and its symbols are rooms inside the footprint. Rooms
// run in source order along rows, like a floor plan read left to right, and
// the lines that belong to no symbol (imports, module-level code) stay as
// common area so the footprint accounts for the whole file.
export interface Room {
  v: number;
  sm: import("@/types").SymbolRow | null;
}
export function rooms(sy: import("@/types").SymbolRow[], loc: number): Room[] {
  const out: Room[] = [];
  let cursor = 1;
  for (const sm of sy) {
    if (sm[2] > cursor) out.push({ v: sm[2] - cursor, sm: null });
    out.push({ v: Math.max(1, sm[3] - sm[2] + 1), sm });
    cursor = Math.max(cursor, sm[3] + 1);
  }
  if (loc > cursor) out.push({ v: loc - cursor, sm: null });
  return out.filter((r) => r.v > 0);
}

export interface Cell {
  x: number;
  y: number;
  w: number;
  h: number;
  sm: import("@/types").SymbolRow | null;
}
// Strip treemap: rooms stay in source order, but a row closes as soon as
// adding another room would make the row's average aspect ratio worse.
// Order-preserving and sliver-free, which a naive equal-sum split is not.
export function stripRows(items: Room[], w: number, h: number): Cell[] {
  const total = items.reduce((a, b) => a + b.v, 0) || 1;
  const scale = (w * h) / total;
  const A = items.map((it) => ({ a: Math.max(it.v * scale, 0.0001), sm: it.sm }));
  const aspect = (row: typeof A, sum: number) => {
    const rh = sum / w;
    if (rh <= 0) return Infinity;
    let acc = 0;
    for (const r of row) {
      const rw = r.a / rh;
      acc += Math.max(rw / rh, rh / rw);
    }
    return acc / row.length;
  };
  const rowsOut: { items: typeof A; sum: number }[] = [];
  let cur: typeof A = [];
  let sum = 0;
  for (const it of A) {
    if (!cur.length) {
      cur = [it];
      sum = it.a;
      continue;
    }
    if (aspect(cur.concat(it), sum + it.a) <= aspect(cur, sum)) {
      cur.push(it);
      sum += it.a;
    } else {
      rowsOut.push({ items: cur, sum });
      cur = [it];
      sum = it.a;
    }
  }
  if (cur.length) rowsOut.push({ items: cur, sum });
  const out: Cell[] = [];
  let y = 0;
  const totalA = A.reduce((a, b) => a + b.a, 0) || 1;
  for (const r of rowsOut) {
    const rh = h * (r.sum / totalA);
    let x = 0;
    for (const it of r.items) {
      const rw = w * (it.a / (r.sum || 1));
      out.push({ x, y, w: rw, h: rh, sm: it.sm });
      x += rw;
    }
    y += rh;
  }
  return out;
}

// ---------- file-dot density (issue #48) ----------
/** Shoelace formula: absolute area of a simple polygon, in world units. A
 * district's on-screen area is this (summed over its `blob` polygons) times
 * k² -- see MapRenderer.dotFactor, which is why this lives here rather than
 * inline in the renderer: the same area-from-a-polygon math the plots layer
 * would need if it ever wanted a density measure of its own. */
export function polygonArea(poly: Array<[number, number]>): number {
  let a = 0;
  for (let i = 0; i < poly.length; i++) {
    const [x0, y0] = poly[i];
    const [x1, y1] = poly[(i + 1) % poly.length];
    a += x0 * y1 - x1 * y0;
  }
  return Math.abs(a) / 2;
}

/** A district's world-space area: the sum over its (possibly several)
 * `blob` polygons. Zero for a district with an EMPTY blob -- unconnected
 * districts, whose polygon geometry.rs drops entirely (see districtClass's
 * doc comment) -- callers must treat that zero as "no area to budget
 * against", not "budget everything to nothing"; see MapRenderer.dotFactor. */
export function districtWorldArea(district: District): number {
  let a = 0;
  for (const poly of district.blob) a += polygonArea(poly);
  return a;
}

export function tmCentre(doc: MapDocument, d: number): [number, number] {
  const a: [number, number, number, number] = [1e9, 1e9, -1e9, -1e9];
  for (let i = 0; i < doc.N.length; i++) {
    if (D_(doc, i) !== d) continue;
    const r = RECT(doc, i);
    a[0] = Math.min(a[0], r[0]);
    a[1] = Math.min(a[1], r[1]);
    a[2] = Math.max(a[2], r[0] + r[2]);
    a[3] = Math.max(a[3], r[1] + r[3]);
  }
  return [(a[0] + a[2]) / 2, (a[1] + a[3]) / 2];
}
