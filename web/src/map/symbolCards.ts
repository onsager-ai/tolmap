// Symbol cards inside file footprints (#82 C2). Pure, DOM-free geometry and
// gating logic for the hierarchical symbols sibling document
// (docs/API.md's `/symbols?district=`, src/symbols.rs's `HierSymbolRow`,
// src/symbol_cards.rs's ring encoding) -- kept separate from MapRenderer.ts
// the same way footprints.ts/geometry.ts/colour.ts already are, so the
// layout math is testable without a browser (CLAUDE.md's no-browser-locally
// rule leaves this the only place that testing can happen at all).
//
// Behaviour ported from the owner's prototype (arch20.body.html's drawSyms/
// symOn/expanded/rolled, ~lines 247-315), adapted to this product's richer
// server-side geometry: the prototype had one ring per symbol and a single
// per-file "rest" polygon; this product's Rust pass (finding 31) additionally
// splits an expanded class into a `header_rings` band plus its members'
// own cards, which the prototype never had to draw separately.
import type { DistrictSymbols, HierSymbolRow, MapDocument } from "@/types";
import { polygonArea } from "./geometry";

/** Per-file world-space footprint area, computed once per document
 * (MapRenderer.loadDocument, the same "per-document work never repeats per
 * paint" rule footprints.ts's own caches follow) -- the input to the 40px
 * symbol gate below, recomputed against the CURRENT `k` every paint. */
export function computeFileFootprintAreas(doc: MapDocument): Map<number, number> {
  const out = new Map<number, number>();
  if (!doc.P) return out;
  for (let i = 0; i < doc.F.length; i++) {
    const poly = doc.P[String(i)];
    if (poly && poly.length >= 3) out.set(i, polygonArea(poly));
  }
  return out;
}

// docs/API.md: "Kinds are 0 class, 1 function, 2 method, 3 nested function,
// 4 interface, 5 type, 6 constant."
export const KIND_CLASS = 0;
export const KIND_FUNCTION = 1;
export const KIND_METHOD = 2;
export const KIND_NESTED_FUNCTION = 3;
export const KIND_INTERFACE = 4;
export const KIND_TYPE = 5;
export const KIND_CONST = 6;

const ROW_FILE = 0;
const ROW_NAME = 1;
const ROW_KIND = 2;
const ROW_START = 3;
const ROW_END = 4;
const ROW_PARENT = 5;
const ROW_CODE_LINES = 6;

export const rowFile = (r: HierSymbolRow) => r[ROW_FILE];
export const rowName = (r: HierSymbolRow) => r[ROW_NAME];
export const rowKind = (r: HierSymbolRow) => r[ROW_KIND];
export const rowStart = (r: HierSymbolRow) => r[ROW_START];
export const rowEnd = (r: HierSymbolRow) => r[ROW_END];
export const rowParent = (r: HierSymbolRow) => r[ROW_PARENT];
export const rowCodeLines = (r: HierSymbolRow) => r[ROW_CODE_LINES];

/** A card, in world coordinates: `[x, y]` points, closed implicitly (no
 * repeated last point). Later contours (holes) are rare in practice but kept
 * for even-odd fill fidelity, per docs/API.md. */
export type WorldRing = [number, number][];
export type WorldContours = WorldRing[];

const CARD_COORDINATE_SCALE = 1e11;

/** docs/API.md: "Each contour is a flat integer list `[x0, y0, dx1, dy1,
 * ...]` in units of 1e-11 world coordinates. Accumulate the deltas, then
 * divide by 1e11 to draw it." Mirrors src/symbol_cards.rs's `encode_rings`
 * exactly (that function's `previous` starts at `[0,0]`, so the first pair
 * IS the absolute quantised point once accumulated the same way here). Plain
 * JS numbers are safe: a world coordinate's quantised magnitude for any real
 * map is well under 2^53. */
export function decodeRing(flat: readonly number[]): WorldRing {
  const points: WorldRing = [];
  let x = 0;
  let y = 0;
  for (let i = 0; i + 1 < flat.length; i += 2) {
    x += flat[i];
    y += flat[i + 1];
    points.push([x / CARD_COORDINATE_SCALE, y / CARD_COORDINATE_SCALE]);
  }
  return points;
}

export function decodeContours(rings: readonly (readonly number[])[] | null | undefined): WorldContours | null {
  if (!rings || rings.length === 0) return null;
  return rings.map(decodeRing);
}

export function ringBounds(ring: WorldRing): [number, number, number, number] {
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const [x, y] of ring) {
    if (x < x0) x0 = x;
    if (x > x1) x1 = x;
    if (y < y0) y0 = y;
    if (y > y1) y1 = y;
  }
  return [x0, y0, x1, y1];
}

export function ringCentroid(ring: WorldRing): [number, number] {
  let sx = 0, sy = 0;
  for (const [x, y] of ring) {
    sx += x;
    sy += y;
  }
  return ring.length ? [sx / ring.length, sy / ring.length] : [0, 0];
}

/** Decoded per-district index: local-array positions resolved against the
 * document's GLOBAL symbol indices (docs/API.md: "`parent` and every edge
 * endpoint are global symbol indices"), children built once per fetched
 * district rather than re-scanned per paint. Built lazily and cached by
 * MapRenderer, keyed by the `DistrictSymbols` object identity (one per
 * successful fetch, never mutated after decode). */
export interface DecodedDistrictSymbols {
  raw: DistrictSymbols;
  /** global symbol index -> local array index */
  globalToLocal: Map<number, number>;
  /** local index -> local indices of its children (parent field resolved to
   * a LOCAL index already, or -1 for a top-level symbol). */
  children: number[][];
  /** local index -> local parent index, or -1 */
  localParent: Int32Array;
  /** file (global file index) -> local indices of its top-level symbols,
   * in source order. */
  topByFile: Map<number, number[]>;
  /** local index -> decoded card contours (world coords), or null if this
   * symbol has no ring (its file has no footprint, or it fell outside the
   * geometry pass -- see src/symbol_cards.rs's own "eligible" gate). */
  cardRings: (WorldContours | null)[];
  /** local index -> decoded header-band contours, for a class/interface with
   * members. Only present for symbols `header_rings` actually covers. */
  headerRings: Map<number, WorldContours>;
  /** file (global file index) -> decoded module-level-code region. */
  moduleRings: Map<number, WorldContours>;
}

export function decodeDistrictSymbols(raw: DistrictSymbols): DecodedDistrictSymbols {
  const n = raw.symbols.length;
  const globalToLocal = new Map<number, number>();
  for (let i = 0; i < n; i++) globalToLocal.set(raw.symbol_indices[i], i);
  const localParent = new Int32Array(n).fill(-1);
  const children: number[][] = Array.from({ length: n }, () => []);
  const topByFile = new Map<number, number[]>();
  for (let i = 0; i < n; i++) {
    const row = raw.symbols[i];
    const parentGlobal = rowParent(row);
    const parentLocal = parentGlobal >= 0 ? globalToLocal.get(parentGlobal) : undefined;
    // A parent global index that IS in this document but belongs to a
    // DIFFERENT file is a cross-file Go receiver method's original symbol
    // parent (src/symbol_cards.rs's local_parent has the identical file
    // check) -- treat it as top-level here too, since its card was placed
    // at file level, not inside that other file's card.
    if (parentLocal != null && rowFile(raw.symbols[parentLocal]) === rowFile(row)) {
      localParent[i] = parentLocal;
      children[parentLocal].push(i);
    } else {
      let bucket = topByFile.get(rowFile(row));
      if (!bucket) {
        bucket = [];
        topByFile.set(rowFile(row), bucket);
      }
      bucket.push(i);
    }
  }
  for (const bucket of topByFile.values()) bucket.sort((a, b) => rowStart(raw.symbols[a]) - rowStart(raw.symbols[b]));
  for (const bucket of children) bucket.sort((a, b) => rowStart(raw.symbols[a]) - rowStart(raw.symbols[b]));

  const cardRings = raw.symbol_rings
    ? raw.symbol_rings.map((r) => decodeContours(r))
    : new Array<WorldContours | null>(n).fill(null);
  const headerRings = new Map<number, WorldContours>();
  if (raw.header_rings) {
    for (const [globalIdxStr, rings] of Object.entries(raw.header_rings)) {
      const local = globalToLocal.get(Number(globalIdxStr));
      const decoded = decodeContours(rings);
      if (local != null && decoded) headerRings.set(local, decoded);
    }
  }
  const moduleRings = new Map<number, WorldContours>();
  if (raw.module_rings) {
    for (const [fileIdxStr, rings] of Object.entries(raw.module_rings)) {
      const decoded = decodeContours(rings);
      if (decoded) moduleRings.set(Number(fileIdxStr), decoded);
    }
  }
  return { raw, globalToLocal, children, localParent, topByFile, cardRings, headerRings, moduleRings };
}

/** The immediate parent's GLOBAL symbol index, or null for a top-level
 * symbol -- MapView's step-back chain (spec item 3: "symbol -> parent
 * symbol -> file -> district -> none") needs exactly this, one level at a
 * time, without reaching into `DecodedDistrictSymbols`' internal arrays
 * itself. */
export function parentGlobalOf(decoded: DecodedDistrictSymbols, global: number): number | null {
  const local = decoded.globalToLocal.get(global);
  if (local == null) return null;
  const parentLocal = decoded.localParent[local];
  return parentLocal >= 0 ? decoded.raw.symbol_indices[parentLocal] : null;
}

/** D1 (issue #82 finding 27): "A file draws its symbols once its footprint
 * is >= 40px on screen" -- the square root of the file's ON-SCREEN area
 * (world area * k^2), matching the prototype's own `symOn`
 * (`Math.sqrt(PAREA[fi])/px >= 40`, where `1/px` there is this codebase's
 * `k`). The selected file is always eligible regardless of size (spec item
 * 2 / prototype: `if(fi===focusF) return true`). */
export function fileCrossesSymbolGate(worldArea: number, k: number, isSelected: boolean): boolean {
  if (isSelected) return true;
  if (!(worldArea > 0)) return false;
  return Math.sqrt(worldArea) * k >= 40;
}

/** D1: "a class expands its members once its short side is >= 110px", or
 * when it (or a descendant) is the current selection -- prototype's
 * `expanded()`. `ancestorsGlobal` is the selected symbol's ancestor chain
 * (itself excluded), as GLOBAL indices, since a selection can point at a
 * symbol from a different `DistrictSymbols` fetch than the class being
 * tested (a cross-district reference target). */
export function isClassExpanded(
  hasChildren: boolean,
  shortSidePx: number,
  isSelfOrAncestorOfSelection: boolean,
): boolean {
  if (!hasChildren) return false;
  if (isSelfOrAncestorOfSelection) return true;
  return shortSidePx >= 110;
}

/** Ancestor chain (local indices), nearest first, self excluded. */
export function ancestorsOf(decoded: DecodedDistrictSymbols, local: number): number[] {
  const out: number[] = [];
  let p = decoded.localParent[local];
  while (p >= 0) {
    out.push(p);
    p = decoded.localParent[p];
  }
  return out;
}

/** Every local index in the subtree rooted at `local`, including itself. */
export function subtreeOf(decoded: DecodedDistrictSymbols, local: number): Set<number> {
  const out = new Set<number>([local]);
  const stack = [local];
  while (stack.length) {
    const x = stack.pop()!;
    for (const c of decoded.children[x]) {
      if (!out.has(c)) {
        out.add(c);
        stack.push(c);
      }
    }
  }
  return out;
}

/** Kind label used for a card/label/outline row: functions and methods (and
 * nested functions) carry `()`; classes and interfaces are bold, never
 * parenthesised (GLOSSARY.md: code objects keep their code names -- this is
 * display punctuation, not a rename). */
export function symbolLabel(row: HierSymbolRow, memberCount: number, collapsed: boolean): string {
  const name = rowName(row);
  const kind = rowKind(row);
  const call = kind === KIND_FUNCTION || kind === KIND_METHOD || kind === KIND_NESTED_FUNCTION;
  const suffix = collapsed && memberCount > 0 ? ` ▸${memberCount}` : "";
  return `${name}${call ? "()" : ""}${suffix}`;
}

export function isBoldKind(kind: number): boolean {
  return kind === KIND_CLASS || kind === KIND_INTERFACE;
}

export function isDashedKind(kind: number): boolean {
  return kind === KIND_NESTED_FUNCTION;
}

/** Fills: "each depth gets lighter... roughly 0.55, 0.85 and 0.6 by depth"
 * (spec item 2), lifted directly from the prototype's own ternary
 * (`depth===0?.55:depth===1?.85:.6`) -- depth 0 is a top-level symbol,
 * depth increases one per level of nesting actually drawn (i.e. only when
 * an ancestor class is expanded; a collapsed class's members are never
 * drawn, so they never contribute a depth). */
export function cardFillRatio(depth: number): number {
  if (depth <= 0) return 0.55;
  if (depth === 1) return 0.85;
  return 0.6;
}

/** Module-level code's own, lighter mix ratio (spec: "a dashed, light
 * region") -- the prototype's own `srest` fill (`mix(surface, fileCol, .25)`). */
export const MODULE_FILL_RATIO = 0.25;

export interface RolledReferences {
  /** rep-key ("s:<global>" or "f:<global file>") -> aggregated occurrence count */
  out: Map<string, number>;
  in: Map<string, number>;
}

/** Roll a symbol's (or, for a hover/selection on a class, its whole
 * subtree's) references up to whatever is actually drawn on screen this
 * paint -- spec item 4 / prototype's `rolled`/`rep`. `visibleGlobals` is the
 * set of GLOBAL symbol indices that got a card drawn this paint (VIS, in the
 * prototype); `rep` walks a target's ancestor chain until it finds one that
 * IS drawn, falling back to the target's own file. Edges internal to the
 * subtree, and edges to the subtree's own ancestors (a member referencing
 * its own class, for instance), are dropped -- spec: "Drop edges internal to
 * the subtree and edges to its own ancestors." */
export function rollReferences(
  decoded: DecodedDistrictSymbols,
  local: number,
  visibleGlobals: ReadonlySet<number>,
): RolledReferences {
  const subtree = subtreeOf(decoded, local);
  const subtreeGlobals = new Set<number>([...subtree].map((i) => decoded.raw.symbol_indices[i]));
  const ancestorGlobals = new Set<number>(ancestorsOf(decoded, local).map((i) => decoded.raw.symbol_indices[i]));

  const rep = (globalIdx: number): string => {
    if (visibleGlobals.has(globalIdx)) return `s:${globalIdx}`;
    const localIdx = decoded.globalToLocal.get(globalIdx);
    if (localIdx != null) {
      for (const a of ancestorsOf(decoded, localIdx)) {
        const ga = decoded.raw.symbol_indices[a];
        if (visibleGlobals.has(ga)) return `s:${ga}`;
      }
      return `f:${rowFile(decoded.raw.symbols[localIdx])}`;
    }
    // The far endpoint of a crossing edge is included in `symbols` per
    // docs/API.md, so this branch is defensive only (a document that
    // somehow references a global index it didn't also ship a row for).
    return `f:${globalIdx}`;
  };

  const out = new Map<string, number>();
  const inn = new Map<string, number>();
  for (const [source, target, occurrences] of decoded.raw.edges) {
    const sourceInSubtree = subtreeGlobals.has(source);
    const targetInSubtree = subtreeGlobals.has(target);
    if (sourceInSubtree === targetInSubtree) continue; // internal, or touches neither
    if (sourceInSubtree) {
      if (ancestorGlobals.has(target) || subtreeGlobals.has(target)) continue;
      const key = rep(target);
      if (key === `s:${decoded.raw.symbol_indices[local]}`) continue;
      out.set(key, (out.get(key) ?? 0) + occurrences);
    } else {
      if (ancestorGlobals.has(source) || subtreeGlobals.has(source)) continue;
      const key = rep(source);
      if (key === `s:${decoded.raw.symbol_indices[local]}`) continue;
      inn.set(key, (inn.get(key) ?? 0) + occurrences);
    }
  }
  return { out, in: inn };
}

export function topN(m: ReadonlyMap<string, number>, n: number): [string, number][] {
  return [...m.entries()].sort((a, b) => b[1] - a[1]).slice(0, n);
}

/** Reference-line width: "Width grows with log(count)" -- the prototype's
 * own `1 + min(3, log2(1+n)) * .7`, kept identical so the visual scale
 * matches the reference screenshots. */
export function referenceLineWidth(count: number): number {
  return 1 + Math.min(3, Math.log2(1 + count)) * 0.7;
}

/** Resolve a rep-key ("s:<global>" or "f:<global>") back to a drawable
 * world point: a visible symbol's card centroid, or (for "f:...", or a
 * symbol this district doc doesn't carry geometry for) the file's `fileXY`.
 * `cardCentroids` is built once per paint over every symbol actually drawn
 * (screen-independent, so it's world-space and reusable for a hover-only
 * redraw). */
export function repKeyFile(key: string): number {
  return Number(key.slice(2));
}

export function isSymbolRepKey(key: string): boolean {
  return key.startsWith("s:");
}

/** docs/GLOSSARY.md kind names, for tooltips/outline rows -- never shown as
 * a metaphor, always the code's own word. */
export const KIND_NAMES: Record<number, string> = {
  [KIND_CLASS]: "class",
  [KIND_FUNCTION]: "function",
  [KIND_METHOD]: "method",
  [KIND_NESTED_FUNCTION]: "nested function",
  [KIND_INTERFACE]: "interface",
  [KIND_TYPE]: "type",
  [KIND_CONST]: "constant",
};

/** A file's top-level outline, in source order, each with its own
 * (unrolled) incoming reference count -- SelectionPanel's outline tree
 * (spec item 5). Nesting mirrors `children`; a row's `refsIn` is the RAW
 * in-degree of that one symbol (not rolled up to what's drawn), since the
 * sidebar shows the file regardless of what the map happens to have
 * expanded. */
export interface OutlineRow {
  local: number;
  global: number;
  row: HierSymbolRow;
  refsIn: number;
  children: OutlineRow[];
}

export function fileOutline(decoded: DecodedDistrictSymbols, fileIdx: number): OutlineRow[] {
  const inCounts = new Map<number, number>();
  for (const [, target, occurrences] of decoded.raw.edges) {
    inCounts.set(target, (inCounts.get(target) ?? 0) + occurrences);
  }
  const build = (local: number): OutlineRow => ({
    local,
    global: decoded.raw.symbol_indices[local],
    row: decoded.raw.symbols[local],
    refsIn: inCounts.get(decoded.raw.symbol_indices[local]) ?? 0,
    children: decoded.children[local].map(build),
  });
  return (decoded.topByFile.get(fileIdx) ?? []).map(build);
}

/** Total symbol count across a file's outline -- every top-level row plus
 * every nested descendant. Issue #82 C2 follow-up: SelectionPanel's
 * "symbols" count switches to this the moment hierarchical data has loaded
 * for the file, replacing the old flat, truncated `S`-based count. */
export function countOutlineSymbols(rows: readonly OutlineRow[]): number {
  let n = 0;
  for (const r of rows) n += 1 + countOutlineSymbols(r.children);
  return n;
}

/** External references for the sidebar, grouped by top-level class/function
 * (spec item 5) -- every out-edge from any symbol in the file, keyed by the
 * ancestor-or-self top-level symbol on THIS side, rolled to the target's own
 * top-level symbol (or file, if the target has no symbol data at hand). */
export interface ExternalRefGroup {
  from: OutlineRow;
  targets: Array<{ key: string; count: number; label: string }>;
}

export function externalReferences(
  decoded: DecodedDistrictSymbols,
  map: MapDocument,
  fileIdx: number,
): ExternalRefGroup[] {
  const topLocals = decoded.topByFile.get(fileIdx) ?? [];
  const groups: ExternalRefGroup[] = [];
  for (const topLocal of topLocals) {
    const subtree = subtreeOf(decoded, topLocal);
    const subtreeGlobals = new Set([...subtree].map((i) => decoded.raw.symbol_indices[i]));
    const targets = new Map<string, { count: number; label: string }>();
    for (const [source, target, occurrences] of decoded.raw.edges) {
      if (!subtreeGlobals.has(source) || subtreeGlobals.has(target)) continue;
      const targetLocal = decoded.globalToLocal.get(target);
      let key: string;
      let label: string;
      if (targetLocal != null) {
        let top = targetLocal;
        for (const a of ancestorsOf(decoded, targetLocal)) top = a;
        key = `s:${decoded.raw.symbol_indices[top]}`;
        label = `${map.F[rowFile(decoded.raw.symbols[top])]} › ${rowName(decoded.raw.symbols[top])}`;
      } else {
        key = `f:${target}`;
        label = map.F[target] ?? `file ${target}`;
      }
      const existing = targets.get(key);
      if (existing) existing.count += occurrences;
      else targets.set(key, { count: occurrences, label });
    }
    if (targets.size === 0) continue;
    groups.push({
      from: {
        local: topLocal,
        global: decoded.raw.symbol_indices[topLocal],
        row: decoded.raw.symbols[topLocal],
        refsIn: 0,
        children: [],
      },
      targets: [...targets.entries()]
        .sort((a, b) => b[1].count - a[1].count)
        .map(([key, { count, label }]) => ({ key, count, label })),
    });
  }
  return groups;
}
