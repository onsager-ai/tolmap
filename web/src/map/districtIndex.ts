// Issue #82 "district index" (owner decision, AskUserQuestion 2026-09-24):
// the left rail's three stacked parts -- a jargon-heavy Landmarks list, a
// separate Hubs list, and district rows whose colour chips no longer
// identify anything once the six shared hues repeat -- collapse into ONE
// "Districts" list. Each mainland row states its size, its dominant folder
// (reusing packageLayout's own "mostly" computation, so the sidebar and the
// district card can never disagree about what counts as dominant) and up to
// a few key files in plain words, each tappable.
//
// This module is the pure data side of that row: no JSX, so
// check-view-stability.mjs (or any other script) can build the same rows
// this component renders without a browser, the same reason hubs.ts and
// packageLayout.ts are their own modules.
import type { LandmarkRow, MapDocument } from "@/types";
import { D_, districtClass, FI } from "./geometry";
import { formatDirectory, type PackageLayout } from "./packageLayout";

// Reuses the district card's own threshold (SelectionPanel.tsx's
// DistrictBody: "largest.share >= 40") -- the same folder is either worth
// calling out or it isn't, regardless of which UI surface is asking.
const MOSTLY_SHARE_THRESHOLD = 40;

// Two or three key files is the normal case (spec: "most imported", plus
// whichever of entry/bridge this district actually has). A hazard file is
// added past that only as a last resort -- see buildDistrictIndexRow's own
// comment -- so this is a hard ceiling, not a target every row reaches.
const MAX_KEY_FILES = 4;

export type DistrictKeyFileKind = "most-imported" | "entry" | "bridge" | "hazard";

export interface DistrictKeyFile {
  kind: DistrictKeyFileKind;
  /** File index -- tapping the row selects this file (SidebarProps.onPickKeyFile). */
  file: number;
  /** Plain-words label, glossary terms only -- no "capital", no "betweenness", no raw decimals. */
  text: string;
}

export interface DistrictIndexRow {
  d: number;
  name: string;
  size: number;
  /** Formatted dominant folder ("src/components/", "(repo root)"), or `null`
   * when no folder clears MOSTLY_SHARE_THRESHOLD -- same omission rule the
   * district card already uses. */
  mostly: string | null;
  keyFiles: readonly DistrictKeyFile[];
}

export interface DistrictIndex {
  totalFiles: number;
  /** Mainland districts, sorted by size descending -- ties broken by id for
   * CLAUDE.md's determinism rule (two districts can tie on size and nothing
   * upstream promises a stable iteration order otherwise). */
  mainland: readonly DistrictIndexRow[];
  /** Island district ids, sorted the same way as `mainland`, collapsed under
   * "> N islands" exactly as today (Sidebar.tsx's CollapsibleSection) -- no
   * key-file detail for these, unchanged from the pre-existing behaviour. */
  islandIds: readonly string[];
}

function basename(path: string): string {
  return path.split("/").pop() ?? path;
}

/** Which OTHER district a bridge file actually links to: the district its
 * import edges (doc.E, either direction) most often cross into. A file
 * landmarks() calls "bridge" for its file-graph betweenness can, in
 * principle, have every one of its edges stay inside its own district (a
 * structural bridge between neighbourhoods, not districts) -- that's the
 * `null` case below, and the caller omits the "links A <-> B" line entirely
 * rather than naming a district the file doesn't actually connect to. */
function bridgeTarget(doc: MapDocument, file: number, ownDistrict: number): number | null {
  const counts = new Map<number, number>();
  for (const [a, b] of doc.E) {
    if (a === file) {
      const bd = D_(doc, b);
      if (bd !== ownDistrict) counts.set(bd, (counts.get(bd) ?? 0) + 1);
    } else if (b === file) {
      const ad = D_(doc, a);
      if (ad !== ownDistrict) counts.set(ad, (counts.get(ad) ?? 0) + 1);
    }
  }
  let best: number | null = null;
  let bestCount = -1;
  for (const [district, count] of counts) {
    if (count > bestCount || (count === bestCount && (best == null || district < best))) {
      best = district;
      bestCount = count;
    }
  }
  return best;
}

/** The file(s) doc.L flags with this `why`, restricted to one district,
 * lowest rank first (doc.L is already rank-ascending -- pipeline.py's
 * landmarks() assigns rank by enumerate() over the order it pushed picks
 * in). Landmarks are global (only up to 2 entries/bridges/hazards, 2 hubs,
 * total), so a given district very rarely has more than one of a kind, but
 * this picks deterministically when it does. */
function landmarksInDistrict(doc: MapDocument, why: LandmarkRow[1], d: number): LandmarkRow[] {
  return doc.L.filter((row) => row[1] === why && D_(doc, row[0]) === d);
}

/** One district's row: header (name, size -- rendered by the caller),
 * "mostly <folder>", and up to a few tappable key files. Every landmark KIND
 * the old rail surfaced (Sidebar.tsx pre-#82-district-index: entry, bridge,
 * hub, capital, hazard) must still be reachable from *some* row -- capital
 * is the one exception (dropped from the map entirely in A4/A5, not needed
 * here either). The other three map onto this row's own vocabulary:
 *   - hub: the global top-fan-in file is, by construction, also the
 *     highest-fan-in file within ITS OWN district (it's the max over every
 *     file, so it's certainly the max over the subset that shares its
 *     district) -- "most imported" already reaches it with no separate hub
 *     line needed.
 *   - entry, bridge: reached directly, when this district has one.
 *   - hazard: NOT one of the three templates the spec calls out ("most
 *     imported" / "entry" / "links"), so it only fills a slot this district
 *     would otherwise leave empty (no entry, no linkable bridge) rather than
 *     always claiming a fourth line -- keeps the common row at the spec's
 *     "two or three key files" while still giving every hazard landmark
 *     *some* row to surface from, which the CLAUDE.md "landmarks listed"
 *     check now verifies file-by-file (checkDistrictIndex in
 *     check-view-stability.mjs) rather than assuming. */
export function buildDistrictIndexRow(doc: MapDocument, packageLayout: PackageLayout, d: number): DistrictIndexRow {
  let mostFile = -1;
  let mostFi = -1;
  for (let i = 0; i < doc.N.length; i++) {
    if (D_(doc, i) !== d) continue;
    const fi = FI(doc, i);
    if (fi > mostFi || (fi === mostFi && (mostFile === -1 || i < mostFile))) {
      mostFi = fi;
      mostFile = i;
    }
  }

  const keyFiles: DistrictKeyFile[] = [];
  if (mostFile !== -1 && mostFi > 0) {
    keyFiles.push({ kind: "most-imported", file: mostFile, text: `most imported: ${basename(doc.F[mostFile])} (${mostFi})` });
  }
  const entryRow = landmarksInDistrict(doc, "entry", d)[0];
  if (entryRow) {
    keyFiles.push({ kind: "entry", file: entryRow[0], text: `entry: ${basename(doc.F[entryRow[0]])}` });
  }
  const bridgeRow = landmarksInDistrict(doc, "bridge", d)[0];
  if (bridgeRow) {
    const other = bridgeTarget(doc, bridgeRow[0], d);
    if (other != null) {
      keyFiles.push({ kind: "bridge", file: bridgeRow[0], text: `links ${doc.names[String(d)]} ↔ ${doc.names[String(other)]}` });
    }
  }
  // Fallback slot (see doc comment above): only when this district has a
  // hazard landmark that isn't already one of the picks above, and only
  // when it would otherwise be missing a second or third line -- a district
  // that already got its most-imported file, its entry AND its bridge line
  // doesn't need a fourth just because one happens to also be a hazard.
  const hazardRow = landmarksInDistrict(doc, "hazard", d)[0];
  if (hazardRow && keyFiles.length < 3 && !keyFiles.some((k) => k.file === hazardRow[0])) {
    keyFiles.push({ kind: "hazard", file: hazardRow[0], text: `hazard: ${basename(doc.F[hazardRow[0]])}` });
  }

  const paths = packageLayout.districtPaths.get(d) ?? [];
  const largest = paths.find((path) => !path.other);
  const mostly = largest && largest.share >= MOSTLY_SHARE_THRESHOLD ? formatDirectory(largest.path!) : null;

  return {
    d,
    name: doc.names[String(d)],
    size: doc.districts[String(d)].size,
    mostly,
    keyFiles: keyFiles.slice(0, MAX_KEY_FILES),
  };
}

/** The whole sidebar list: mainland rows (size descending, ties by id) plus
 * island ids for the existing collapsible section. Built once per
 * [doc, packageLayout] pair -- Sidebar.tsx memoises this the same way it
 * already memoises computeHubs(). */
export function buildDistrictIndex(doc: MapDocument, packageLayout: PackageLayout): DistrictIndex {
  const ids = Object.keys(doc.districts);
  const byClass = (cls: "mainland" | "island") =>
    ids
      .filter((id) => districtClass(doc.districts[id]) === cls)
      .sort((a, b) => doc.districts[b].size - doc.districts[a].size || Number(a) - Number(b));
  const mainland = byClass("mainland").map((id) => buildDistrictIndexRow(doc, packageLayout, Number(id)));
  return { totalFiles: doc.F.length, mainland, islandIds: byClass("island") };
}
