// B4 (nested footprints, issue #82, owner decision D3): a neighbourhood is a
// second-level group of files inside a district (docs/GLOSSARY.md). D3 draws
// them as white gutters plus "three alternating shades" -- no grid, no
// per-neighbourhood colour of their own (finding 24 removed the parcel-grid
// terrain layer specifically for looking artificial; this is a different,
// approved presentation, not its return). This module picks which of three
// shades each neighbourhood gets: a greedy graph colouring, same algorithm as
// colour.ts's district hues (assignDistrictHues), reusing its adjacency
// primitive (nearBlobs) at the smaller neighbourhood scale rather than a
// second implementation. Pure and DOM-free for the same reason colour.ts is:
// deterministic, and testable from plain Node.
import type { MapDocument, Neighbourhood } from "@/types";
import { blobBox, nearBlobs } from "./colour";
import { fileXY } from "./geometry";

export const SHADE_COUNT = 3;

/** Adjacency threshold as a fraction of the OWNING DISTRICT's own blob span
 * -- not the whole map's span (colour.ts's ADJACENCY_SPAN_FRACTION), because
 * a neighbourhood is a fraction of a district, not of the map, and a flat
 * map-wide constant would either never trigger inside a small district or
 * over-trigger inside a large one. A looser fraction than the district-hue
 * threshold (colour.ts's 0.035): neighbourhoods inside one district sit much
 * closer together than districts do to each other, so the same ratio applied
 * at this scale would rarely register two adjacent blobs as neighbours at
 * all. */
export const NEIGHBOURHOOD_ADJACENCY_SPAN_FRACTION = 0.06;

/** Every neighbourhood id (e.g. "0-0") -> its shade index (0..SHADE_COUNT-1).
 * Computed once per document (MapRenderer.loadDocument). Districts are
 * independent: two neighbourhoods in different districts are never drawn
 * adjacent to each other on the map (a district outline always separates
 * them), so adjacency is only ever checked within one district's own
 * neighbourhood set -- O(k^2) per district for its own k neighbourhoods
 * rather than O(n^2) over the whole document's neighbourhood count (dify: 512
 * neighbourhoods total, but at most a few dozen per district). */
export function assignNeighbourhoodShades(doc: MapDocument): Map<string, number> {
  const shade = new Map<string, number>();
  const neighbourhoods = doc.neighbourhoods;
  if (!neighbourhoods) return shade;
  const byDistrict = new Map<number, string[]>();
  for (const [id, n] of Object.entries(neighbourhoods)) {
    if (!byDistrict.has(n.d)) byDistrict.set(n.d, []);
    byDistrict.get(n.d)!.push(id);
  }
  for (const [, ids] of byDistrict) {
    let x0 = 1e9, y0 = 1e9, x1 = -1e9, y1 = -1e9;
    for (const id of ids) {
      const [bx0, by0, bx1, by1] = blobBox(neighbourhoods[id].blob);
      x0 = Math.min(x0, bx0);
      y0 = Math.min(y0, by0);
      x1 = Math.max(x1, bx1);
      y1 = Math.max(y1, by1);
    }
    const span = Math.max(x1 - x0, y1 - y0) || 1;
    const threshold = span * NEIGHBOURHOOD_ADJACENCY_SPAN_FRACTION;
    // Determinism (CLAUDE.md): visited size-descending, ties by id ascending
    // -- the exact tie-break rule assignDistrictHues uses one scale up.
    const ordered = [...ids].sort((a, b) => neighbourhoods[b].size - neighbourhoods[a].size || (a < b ? -1 : a > b ? 1 : 0));
    const neighbourSets = new Map<string, string[]>();
    for (const id of ordered) neighbourSets.set(id, []);
    for (let i = 0; i < ordered.length; i++) {
      for (let j = i + 1; j < ordered.length; j++) {
        if (nearBlobs(neighbourhoods[ordered[i]].blob, neighbourhoods[ordered[j]].blob, threshold)) {
          neighbourSets.get(ordered[i])!.push(ordered[j]);
          neighbourSets.get(ordered[j])!.push(ordered[i]);
        }
      }
    }
    const used = new Array(SHADE_COUNT).fill(0);
    for (const id of ordered) {
      const takenByNeighbour = new Set((neighbourSets.get(id) ?? []).map((n) => shade.get(n)).filter((s): s is number => s != null));
      let pick = -1;
      for (let s = 0; s < SHADE_COUNT; s++) {
        if (!takenByNeighbour.has(s)) {
          pick = s;
          break;
        }
      }
      if (pick === -1) {
        pick = 0;
        for (let s = 1; s < SHADE_COUNT; s++) if (used[s] < used[pick]) pick = s;
      }
      shade.set(id, pick);
      used[pick]++;
    }
  }
  return shade;
}

export function neighbourhoodOf(doc: MapDocument, i: number): Neighbourhood | null {
  const id = doc.file_neighbourhoods?.[i];
  if (id == null) return null;
  return doc.neighbourhoods?.[id] ?? null;
}

/** Every neighbourhood id -> the mean of its own MEMBER files' displayed
 * footprint centroids (map/geometry.ts's `fileXY`) -- streets and labels
 * anchor here rather than on a bbox centre of the neighbourhood's blob,
 * because item 3's own rule ("anchor everything on displayed footprint
 * centroids") applies one level up too: a bbox centre can land outside an
 * L-shaped or thin neighbourhood blob, but the mean of its own files' real
 * positions cannot land further from the mass than the mass itself extends.
 * Computed once per document (MapRenderer.loadDocument), not per paint. */
export function neighbourhoodCentroids(doc: MapDocument): Map<string, [number, number]> {
  const sums = new Map<string, [number, number, number]>();
  const ids = doc.file_neighbourhoods;
  if (!ids) return new Map();
  for (let i = 0; i < ids.length; i++) {
    const id = ids[i];
    if (id == null) continue;
    const [x, y] = fileXY(doc, "r", i);
    const cur = sums.get(id) ?? [0, 0, 0];
    cur[0] += x;
    cur[1] += y;
    cur[2] += 1;
    sums.set(id, cur);
  }
  const out = new Map<string, [number, number]>();
  for (const [id, [sx, sy, n]] of sums) out.set(id, [sx / n, sy / n]);
  return out;
}
