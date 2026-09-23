// B4 (nested footprints, issue #82, scope item 4): "streets" -- roads one
// level down, drawn only inside a FOCUSED district (docs/GLOSSARY.md: a
// street is a road between neighbourhoods, or neighbourhood <-> district).
// Aggregation mirrors roads.ts's aggregateDistrictFlows/selectRoadPairs
// exactly, at the neighbourhood scale, and the actual ribbon geometry reuses
// roads.ts's buildRoadGeom/districtRadius/exitPt unchanged -- those are
// already generic over "two places with a centre and a set of rings," not
// District-specific, so a neighbourhood is just another place to hand them.
// Pure and DOM-free for the same reason roads.ts/colour.ts are.
import type { MapDocument } from "@/types";
import { D_, districtClass } from "./geometry";

export interface StreetFlow {
  a: string;
  b: string;
  ab: number;
  ba: number;
  total: number;
}

const byTotalDesc = (x: StreetFlow, y: StreetFlow) => y.total - x.total || (x.a < y.a ? -1 : x.a > y.a ? 1 : 0) || (x.b < y.b ? -1 : x.b > y.b ? 1 : 0);

/** doc.E aggregated into directed neighbourhood-pair counts, scoped to files
 * whose DISTRICT is `districtId` on both ends (an intra-district edge whose
 * two files landed in different neighbourhoods -- same-neighbourhood edges
 * aren't a street, a street connects two PLACES, exactly roads.ts's own
 * district-level rule one level up). */
export function aggregateIntraDistrictFlows(doc: MapDocument, districtId: number): Map<string, StreetFlow> {
  const flows = new Map<string, StreetFlow>();
  const nb = doc.file_neighbourhoods;
  if (!nb) return flows;
  for (const [x, y] of doc.E) {
    if (D_(doc, x) !== districtId || D_(doc, y) !== districtId) continue;
    const na = nb[x];
    const nbb = nb[y];
    if (na == null || nbb == null || na === nbb) continue;
    const a = na < nbb ? na : nbb;
    const b = na < nbb ? nbb : na;
    const key = `${a}\0${b}`;
    let f = flows.get(key);
    if (!f) {
      f = { a, b, ab: 0, ba: 0, total: 0 };
      flows.set(key, f);
    }
    if (na === a) f.ab++;
    else f.ba++;
    f.total++;
  }
  return flows;
}

/** Top `topN` intra-district pairs by total, plus every neighbourhood's own
 * single strongest pair (spec: "the top 20 pairs ... plus each
 * neighbourhood's strongest pair") -- same "global top N, plus each place's
 * own best" rule as roads.ts's selectRoadPairs. */
export function selectStreetPairs(flows: Map<string, StreetFlow>, topN = 20): StreetFlow[] {
  const all = [...flows.values()];
  const sorted = [...all].sort(byTotalDesc);
  const keep = new Set<string>();
  for (const f of sorted.slice(0, topN)) keep.add(`${f.a}\0${f.b}`);
  const byNeighbourhood = new Map<string, StreetFlow[]>();
  for (const f of all) {
    for (const id of [f.a, f.b]) {
      let list = byNeighbourhood.get(id);
      if (!list) {
        list = [];
        byNeighbourhood.set(id, list);
      }
      list.push(f);
    }
  }
  for (const list of byNeighbourhood.values()) {
    const best = [...list].sort(byTotalDesc)[0];
    if (best) keep.add(`${best.a}\0${best.b}`);
  }
  return sorted.filter((f) => keep.has(`${f.a}\0${f.b}`));
}

export interface CrossFlow {
  /** neighbourhood id inside the focused district */
  n: string;
  /** the OTHER (mainland) district id */
  d: number;
  /** focused-neighbourhood -> other-district import count */
  out: number;
  /** other-district -> focused-neighbourhood import count */
  in: number;
  total: number;
}

/** doc.E aggregated into (neighbourhood inside `districtId`) <-> (other
 * MAINLAND district) counts -- the cross-district half of item 4. Islands and
 * unconnected districts are excluded on the far end for the same reason
 * roads.ts's aggregateDistrictFlows excludes them: a thin grey ribbon running
 * off to an island (or nowhere, for unconnected) is clutter a PR review
 * already measured and removed one level up. */
export function aggregateCrossDistrictFlows(doc: MapDocument, districtId: number): Map<string, CrossFlow> {
  const flows = new Map<string, CrossFlow>();
  const nb = doc.file_neighbourhoods;
  if (!nb) return flows;
  for (const [x, y] of doc.E) {
    const dx = D_(doc, x);
    const dy = D_(doc, y);
    if (dx === dy) continue;
    const fromFocused = dx === districtId ? x : dy === districtId ? y : -1;
    if (fromFocused === -1) continue;
    const other = fromFocused === x ? y : x;
    const otherDistrict = D_(doc, other);
    if (districtClass(doc.districts[String(otherDistrict)]) !== "mainland") continue;
    const n = nb[fromFocused];
    if (n == null) continue;
    const key = `${n}\0${otherDistrict}`;
    let f = flows.get(key);
    if (!f) {
      f = { n, d: otherDistrict, out: 0, in: 0, total: 0 };
      flows.set(key, f);
    }
    if (fromFocused === x) f.out++;
    else f.in++;
    f.total++;
  }
  return flows;
}

const byCrossTotalDesc = (x: CrossFlow, y: CrossFlow) => y.total - x.total || (x.n < y.n ? -1 : x.n > y.n ? 1 : 0) || x.d - y.d;

/** Top `topN` cross-district pairs (spec: "top 12"), dashed/fainter at the
 * call site -- these connect a neighbourhood to a whole other district, not
 * two peers, so there's no "each neighbourhood's strongest" floor the way
 * `selectStreetPairs` has one: a neighbourhood with no cross-district traffic
 * in the global top 12 simply draws none, same as roads.ts's own top-N-only
 * half would for a pair that never contends. */
export function selectCrossDistrictPairs(flows: Map<string, CrossFlow>, topN = 12): CrossFlow[] {
  return [...flows.values()].sort(byCrossTotalDesc).slice(0, topN);
}
