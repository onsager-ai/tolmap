// Ranked landmark-pin selection, pulled out of MapRenderer's paint() into its
// own pure function for the same reason pinch.ts exists: web/scripts/
// pin-counts.ts (issue: readable-overview PR, scope item 1) has to report
// pin counts for maps too large to open in a browser under the CPU
// constraint (msgraph-sdk-python, n8n), and it can only report numbers that
// actually match what renders if it calls the SAME selection MapRenderer
// calls, not a second copy that could silently drift.
//
// The problem this exists to fix (issue #57): MapRenderer used to draw every
// doc.L pin at every zoom (past a flat zf<=3.4 cutoff for capitals only),
// which is fine when landmarks() picks a handful of global landmarks plus
// one capital per district -- but msgraph-sdk-python's partition degenerated
// into 10,030 districts (85% singleton, q=0.0000), so "one capital per
// district" put 10,034 pins on the map regardless of zoom. The fix is not to
// the partition (that's #57's own scope) -- it's that a capital pin should
// only draw once its district is actually a legible PLACE on screen.
import type { District, LandmarkRow, MapDocument } from "@/types";
import { D_, districtClass, districtWorldArea } from "./geometry";
import { DOT_DENSITY_FLOOR } from "./constants";

// "It should need at least a few dots' worth of area": DOT_DENSITY_FLOOR
// (constants.ts) is the px²/file a district needs before #49's budget draws
// a file's dot at all, so re-using it here (rather than inventing a second,
// unrelated constant) means "established" tracks the same reader-tested
// number "legible" already means elsewhere on this map. 3 dots' worth is
// small enough that an island barely bigger than a handful of files still
// earns its capital pin once it's toe-to-toe with a few visible file dots,
// and large enough that a sliver too small to show ANY dots (#49's fast
// path: edge < 1) can't earn one either.
export const PIN_ESTABLISH_DOTS = 3;
export const PIN_ESTABLISH_AREA = PIN_ESTABLISH_DOTS * DOT_DENSITY_FLOOR;

// Pre-existing cutoff (unchanged from the renderer this replaces): once
// zoomed in this far, individual file dots are legible on their own and a
// capital pin reads as clutter rather than a wayfinding aid. Global
// landmarks (entry/bridge/hub/hazard) were never subject to this and still
// aren't -- they're not tied to a district's own legibility.
export const PIN_CAPITAL_HIDE_ZF = 3.4;

// Approximate on-screen footprint of the pin glyph (the teardrop path plus
// its rank-number text), anchored at its tip (cx, cy) -- see MapRenderer's
// pin drawing block: `M cx cy l -8 -12 a 9.5 9.5 0 1 1 16 0 z` is about 19px
// wide and reaches about 31px above its tip. Padded slightly so two pins
// that are merely adjacent (not literally overlapping paths) still read as
// two separate, tappable targets rather than a fused blob.
const PIN_W = 20;
const PIN_H = 32;

export interface PinPlacement {
  row: LandmarkRow;
  cx: number;
  cy: number;
}

/** `screenOf(i)` maps a file index to its CURRENT on-screen position, or
 * `null` if it's culled (off-screen) -- callers pass MapRenderer's own
 * `X(px(i)[0]), Y(px(i)[1])` (with the same margin the old inline loop used),
 * or the equivalent world->screen transform for a Node-computed fit view, so
 * this function never duplicates that geometry itself.
 *
 * `sel` is the one file (if any) whose pin must draw regardless of
 * establishment or collision -- "the selected file's pin ... always draws."
 * Picking a landmark from the sidebar always sets `sel` to that file
 * (MapView.tsx's onPickLandmark -> selectFile), so there's no separate
 * "sidebar pick" state to thread through here; `sel` already covers it. */
export function selectPins(
  doc: MapDocument,
  districtArea: Map<number, number>,
  screenOf: (i: number) => [number, number] | null,
  k: number,
  zf: number,
  sel: number | null,
): PinPlacement[] {
  const placed: [number, number, number, number][] = [];
  const box = (cx: number, cy: number): [number, number, number, number] => [cx - PIN_W / 2, cy - PIN_H, PIN_W, PIN_H];
  const hits = (b: [number, number, number, number]) =>
    placed.some((r) => !(b[0] + b[2] < r[0] || b[0] > r[0] + r[2] || b[1] + b[3] < r[1] || b[1] > r[1] + r[3]));

  const out: PinPlacement[] = [];

  if (sel != null) {
    const selRow = doc.L.find(([i]) => i === sel);
    if (selRow) {
      const p = screenOf(sel);
      if (p) {
        placed.push(box(p[0], p[1]));
        out.push({ row: selRow, cx: p[0], cy: p[1] });
      }
    }
  }

  type Cand = { row: LandmarkRow; cx: number; cy: number; rank: number };
  const candidates: Cand[] = [];
  for (const row of doc.L) {
    const [i, why, , rank] = row;
    if (i === sel) continue; // already placed above, unconditionally
    if (why === "capital") {
      if (zf > PIN_CAPITAL_HIDE_ZF) continue;
      const d = D_(doc, i);
      const district = doc.districts[String(d)] as District;
      // Mainland is always established -- it's the map's actual subject
      // (issue #34), never gated. Island and unconnected districts share the
      // same area check: an unconnected district has an EMPTY blob
      // (geometry.rs never gives it a polygon -- districtClass's doc
      // comment in geometry.ts), so districtArea reads 0 for it and it can
      // never clear PIN_ESTABLISH_AREA -- correctly, since it has no
      // on-screen place for a pin to anchor to.
      if (districtClass(district) !== "mainland") {
        const area = (districtArea.get(d) ?? districtWorldArea(district)) * k * k;
        if (area < PIN_ESTABLISH_AREA) continue;
      }
    }
    const p = screenOf(i);
    if (!p) continue;
    candidates.push({ row, cx: p[0], cy: p[1], rank });
  }
  // Deterministic priority: ascending landmark rank -- the same order
  // pipeline.rs's landmarks() assigned (up to 2 entries, then bridges, then
  // hubs, then one capital per district biggest-first, then up to 2
  // hazards), tie-broken by file index so two candidates that ever shared a
  // rank would still resolve identically on every run (CLAUDE.md's
  // determinism rule), though `rank` is assigned by a single enumerate() on
  // the Rust side today and never actually ties.
  candidates.sort((a, b) => a.rank - b.rank || a.row[0] - b.row[0]);
  for (const c of candidates) {
    const b = box(c.cx, c.cy);
    if (hits(b)) continue;
    placed.push(b);
    out.push({ row: c.row, cx: c.cx, cy: c.cy });
  }
  return out;
}
