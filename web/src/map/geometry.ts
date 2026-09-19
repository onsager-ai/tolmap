import type { MapDocument } from "@/types";
import type { Geo } from "./constants";

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

export function fitScale(doc: MapDocument, geo: Geo, vw: number, vh: number): number {
  const b = worldBounds(doc, geo);
  const pad = 46;
  return Math.min((vw - 2 * pad) / (b[2] - b[0] || 1), (vh - 2 * pad) / (b[3] - b[1] || 1));
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
