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
import type { LandmarkRow, MapDocument } from "@/types";

// PIN_ESTABLISH_DOTS/PIN_ESTABLISH_AREA (the "is this district established
// enough on screen to earn its capital pin" gate) were removed with capital
// pins themselves -- see isRetiredPinKind below (issue #82 A4).
//
// PIN_CAPITAL_HIDE_ZF is kept and still exported even though capital pins no
// longer read it: MapRenderer's island-fade curve (islandFadeOpacity) reuses
// this SAME numeric threshold for an unrelated reason (the zoom past which
// an island's file dots are legible on their own), and renaming/duplicating
// the constant there would be the actual churn -- the comment at each of
// its remaining call sites explains why that reuse, not this one, is what
// keeps it alive.
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
 * Picking a key file from a district row always sets `sel` to that file
 * (MapView.tsx's onPickKeyFile -> selectFile, issue #82 "district index" --
 * formerly onPickLandmark/onPickHub before the sidebar's Landmarks/Hubs
 * sections merged into one Districts list), so there's no separate
 * "sidebar pick" state to thread through here; `sel` already covers it.
 *
 * `_districtArea`/`_k`/`_zf` are kept in the signature, underscore-prefixed
 * (both call sites -- MapRenderer.paint() and pin-counts.ts -- already pass
 * them positionally) even though the only logic that read them, capital-pin
 * establishment, was removed with capital pins themselves (issue #82 A4, see
 * isRetiredPinKind below). Changing this function's signature/call sites is
 * out of this PR's stated scope. */
export function selectPins(
  doc: MapDocument,
  _districtArea: Map<number, number>,
  screenOf: (i: number) => [number, number] | null,
  _k: number,
  _zf: number,
  sel: number | null,
  // A5 (district labels always on, issue #82): boxes already claimed by
  // something with HIGHER placement priority than a pin -- today, a
  // district's own name label. A pin candidate that would land on one of
  // these is skipped, same as if another pin had already taken the spot;
  // this is what "district names take priority over pins" actually means
  // in code (previously pins were selected with no knowledge of where
  // district names ended up, so the two could visually collide even though
  // neither's OWN collision logic ever saw a conflict). Optional and
  // defaulted so pin-counts.ts's reporting call (which doesn't model
  // district-label placement) keeps working unchanged.
  preplaced: ReadonlyArray<[number, number, number, number]> = [],
): PinPlacement[] {
  const placed: [number, number, number, number][] = [...preplaced];
  const box = (cx: number, cy: number): [number, number, number, number] => [cx - PIN_W / 2, cy - PIN_H, PIN_W, PIN_H];
  const hits = (b: [number, number, number, number]) =>
    placed.some((r) => !(b[0] + b[2] < r[0] || b[0] > r[0] + r[2] || b[1] + b[3] < r[1] || b[1] > r[1] + r[3]));

  const out: PinPlacement[] = [];

  // Owner decision (AskUserQuestion, 2026-09-23, issue #82 A4): capital pins
  // are dropped -- district names are unconditional now (A5), so a capital
  // pin's whole job (naming a district that's otherwise anonymous at a
  // glance) is redundant. Hub pins are dropped too, replaced by the new hub
  // RING layer (map/hubs.ts, drawn by MapRenderer.drawHubRings), which
  // covers every hub above the fan-in threshold, not just the top two
  // landmarks() happens to pick. Both kinds stay in doc.L and the sidebar's
  // landmark list (all five kinds, unchanged) -- only the MAP PIN is
  // retired, including for the selected file: a selected capital/hub still
  // gets its selection ring (MapRenderer.paint()'s `ring()` call), just no
  // second, now-redundant pin glyph on top of it.
  const isRetiredPinKind = (why: LandmarkRow[1]) => why === "capital" || why === "hub";

  if (sel != null) {
    const selRow = doc.L.find(([i, why]) => i === sel && !isRetiredPinKind(why));
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
    if (isRetiredPinKind(why)) continue;
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
