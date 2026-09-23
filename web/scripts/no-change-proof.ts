#!/usr/bin/env -S npx tsx
// Issue #48: no-change proof for the file-dot density budget.
//
// The budget (constants.ts DOT_DENSITY_FLOOR, MapRenderer.dotFactor) must
// never engage on the nine acceptance fixtures at the fit zoom -- every
// district's on-screen area per file has to clear the floor there. This
// re-derives that per-district number using the renderer's own pure
// geometry functions (fitScale, districtWorldArea, districtClass) rather
// than reimplementing the math a second time, so it can't silently drift
// from what MapRenderer actually computes at runtime.
//
// What this DOES NOT prove, as of issue #82 (district hues, import roads,
// hub rings, always-on district names): the DOM is no longer byte-identical
// to the pre-#48 renderer at fit zoom on these fixtures, and that was never
// this script's own claim -- #48's ORIGINAL comment reasoned "budget never
// engages here" -> "DOM stays byte-identical", which held only because
// nothing else in the renderer changed anything #48 didn't already touch.
// #82 intentionally changes district fill/stroke colour (A2), adds road
// ribbons and hub rings that did not exist before (A3/A4), and makes every
// mainland district's name label unconditional (A5) -- all deliberate DOM
// changes this PR's own scope calls for, none of them related to the
// density gate this script actually checks. The assertion below (every
// district's px²/file clears DOT_DENSITY_FLOOR at fit zoom on the nine
// fixtures) is unaffected by any of #82's changes and still holds; read this
// script as "the dot-thinning gate doesn't engage on these fixtures," not as
// a DOM snapshot test.
//
// Run: npx tsx web/scripts/no-change-proof.ts
// (no test runner exists in this project yet -- see web/README.md -- so this
// is a standalone script, not a `*.test.ts` file)
//
// Optionally also reports (not asserted -- these are not committed
// acceptance fixtures) the same numbers for extra maps, e.g. crawlab/dify/n8n
// from issue #48's measurement, via TOLMAP_EXTRA_MAPS_DIR=/path/to/*.json:
//   TOLMAP_EXTRA_MAPS_DIR=/tmp/islands-maps npx tsx web/scripts/no-change-proof.ts
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { DOT_DENSITY_FLOOR } from "../src/map/constants";
import { districtClass, districtWorldArea, fitScale } from "../src/map/geometry";
import type { District, MapDocument } from "../src/types";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DATA_DIR = path.resolve(HERE, "..", "..", "data");

const VIEWPORTS: Array<[string, number, number]> = [
  ["390x700 (phone)", 390, 700],
  ["1440x900 (desktop)", 1440, 900],
];

function loadFixtures(dir: string): Array<{ name: string; doc: MapDocument }> {
  return readdirSync(dir)
    .filter((f) => f.endsWith(".json"))
    .sort()
    .map((f) => ({ name: f.replace(/\.json$/, ""), doc: JSON.parse(readFileSync(path.join(dir, f), "utf8")) }));
}

/** Per-district on-screen px²/file at scale `k`, skipping districts the
 * budget never applies to (unconnected: no polygon, see MapRenderer.dotFactor
 * and districtWorldArea's doc comment; empty: nothing to divide by). */
function densityByDistrict(doc: MapDocument, k: number): Array<{ id: string; pxPerFile: number; size: number }> {
  const out: Array<{ id: string; pxPerFile: number; size: number }> = [];
  for (const [id, district] of Object.entries(doc.districts) as [string, District][]) {
    if (districtClass(district) === "unconnected" || district.size <= 0) continue;
    const area = districtWorldArea(district);
    out.push({ id, pxPerFile: (area * k * k) / district.size, size: district.size });
  }
  return out;
}

/** How many of the document's files are drawn (dotFactor > 0, reproduced
 * inline since MapRenderer's version is a private method) at scale `k`. */
function drawnCount(doc: MapDocument, k: number): number {
  let n = 0;
  for (const [, district] of Object.entries(doc.districts) as [string, District][]) {
    if (districtClass(district) === "unconnected" || district.size <= 0) {
      n += district.size;
      continue;
    }
    const area = districtWorldArea(district);
    const edge = (area * k * k) / DOT_DENSITY_FLOOR;
    n += Math.min(district.size, Math.max(0, Math.ceil(edge)));
  }
  return n;
}

/** Smallest k (as a multiple of fitScale) at which every district's budget
 * covers its whole file count -- i.e. the zoom past which nothing more
 * reveals. */
function fullRevealZoom(doc: MapDocument, geo: "r", vw: number, vh: number): number {
  const fit = fitScale(doc, geo, vw, vh);
  let maxK = 0;
  for (const [, district] of Object.entries(doc.districts) as [string, District][]) {
    if (districtClass(district) === "unconnected" || district.size <= 0) continue;
    const area = districtWorldArea(district);
    if (area <= 0) continue; // degenerate (zero-area polygon); budget can't reveal it by zooming
    const kNeeded = Math.sqrt((DOT_DENSITY_FLOOR * district.size) / area);
    maxK = Math.max(maxK, kNeeded);
  }
  return maxK / fit;
}

let failed = false;

console.log("## No-change proof: nine acceptance fixtures (data/*.json), gate must not engage at fit zoom\n");
console.log(`floor = ${DOT_DENSITY_FLOOR} px²/file\n`);
console.log("| fixture | viewport | min px²/file | districts checked | status |");
console.log("|---|---|---|---|---|");
for (const { name, doc } of loadFixtures(DATA_DIR)) {
  for (const [label, vw, vh] of VIEWPORTS) {
    const k = fitScale(doc, "r", vw, vh);
    const rows = densityByDistrict(doc, k);
    const min = rows.length ? Math.min(...rows.map((r) => r.pxPerFile)) : Infinity;
    const ok = rows.every((r) => r.pxPerFile >= DOT_DENSITY_FLOOR);
    if (!ok) failed = true;
    console.log(`| ${name} | ${label} | ${min.toFixed(1)} | ${rows.length} | ${ok ? "PASS" : "FAIL"} |`);
  }
}

const extraDir = process.env.TOLMAP_EXTRA_MAPS_DIR;
if (extraDir) {
  console.log(`\n## Extra maps (not committed fixtures, reported only): ${extraDir}\n`);
  console.log("| map | viewport | files drawn @fit | @2x | @4x | full-reveal zoom (x fit) |");
  console.log("|---|---|---|---|---|---|");
  for (const { name, doc } of loadFixtures(extraDir)) {
    const total = doc.F.length;
    for (const [label, vw, vh] of VIEWPORTS) {
      const fit = fitScale(doc, "r", vw, vh);
      const at1 = drawnCount(doc, fit);
      const at2 = drawnCount(doc, fit * 2);
      const at4 = drawnCount(doc, fit * 4);
      const reveal = fullRevealZoom(doc, "r", vw, vh);
      console.log(
        `| ${name} (${total} files) | ${label} | ${at1} | ${at2} | ${at4} | ${reveal.toFixed(2)}x |`,
      );
    }
  }
}

if (failed) {
  console.error("\nFAIL: at least one district in a committed fixture falls below the density floor at fit zoom.");
  process.exit(1);
}
console.log("\nPASS: every district in every committed fixture clears the density floor at both viewports.");
