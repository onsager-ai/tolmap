#!/usr/bin/env -S npx tsx
// Readable-overview PR, scope item 1: before/after pin counts at fit zoom.
//
// "Before" is the renderer's OLD, unconditional rule (every doc.L pin drawn
// at every zoom, except capitals past zf>3.4) -- reproduced here rather than
// checked out from git history, so both numbers come from one script run
// against one loaded document. "After" calls MapRenderer's own
// map/pins.ts::selectPins -- the SAME function paint() calls -- not a
// reimplementation, so this can't silently drift from what actually renders
// (see pins.ts's top comment).
//
// n8n (11,991 files) and msgraph-sdk-python (16,636 files) are both over
// this task's ultra-band browser ceiling (8,000 files; the largest map
// safe to open in a browser here is dify at 6,347) -- everything below is
// pure Node over the loaded JSON, no browser, matching the CLAUDE.md rule
// that ultra maps get computed from the JSON with the renderer's own pure
// functions rather than opened.
//
// Run: npx tsx web/scripts/pin-counts.ts
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { districtWorldArea, fitScale, mainlandBounds } from "../src/map/geometry";
import { PIN_CAPITAL_HIDE_ZF, selectPins } from "../src/map/pins";
import type { District, MapDocument } from "../src/types";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const MAPS_DIR = path.resolve(HERE, "..", "public", "maps");

const REPOS = ["pallets/flask", "django/django", "langgenius/dify", "n8n-io/n8n", "microsoftgraph/msgraph-sdk-python"];
const VIEWPORTS: Array<[string, number, number]> = [
  ["390x700 (phone)", 390, 700],
  ["1440x900 (desktop)", 1440, 900],
];
const PAD = 46; // MapRenderer.fit()'s own padding constant, duplicated here
// deliberately rather than imported: fit() is the most bug-fought code path
// in this file (#34/#51/fix-keep-view-on-select's history), so this script
// reads its formula rather than pulling a shared helper out of it, to avoid
// putting a reporting script's import on the critical path of that fragile
// function. If fit()'s pad ever changes, this script's numbers and
// MapRenderer's actual view will silently diverge -- acceptable for a
// one-off report; NOT a precedent for anything load-bearing.

/** MapRenderer.fit()'s own (k, tx, ty) derivation, against mainlandBounds
 * (issue #34: the default/fit view frames mainland only). */
function fitTransform(doc: MapDocument, vw: number, vh: number) {
  const b = mainlandBounds(doc, "r");
  const k = fitScale(doc, "r", vw, vh);
  const tx = PAD + (vw - 2 * PAD - (b[2] - b[0]) * k) / 2 - b[0] * k;
  const ty = PAD + (vh - 2 * PAD - (b[3] - b[1]) * k) / 2 - b[1] * k;
  return { k, tx, ty };
}

/** The renderer's OLD rule, reproduced (not imported -- there is nothing
 * left in MapRenderer.ts to import; this IS the before-state it replaced).
 * Same viewport cull margin (30px) the old inline loop used. */
function pinsBefore(doc: MapDocument, k: number, tx: number, ty: number, vw: number, vh: number): number {
  const zf = k / fitScale(doc, "r", vw, vh);
  let n = 0;
  for (const [i, why] of doc.L) {
    if (why === "capital" && zf > PIN_CAPITAL_HIDE_ZF) continue;
    const cx = doc.N[i][1] * k + tx;
    const cy = doc.N[i][2] * k + ty;
    if (cx < -30 || cx > vw + 30 || cy < -30 || cy > vh + 30) continue;
    n++;
  }
  return n;
}

function pinsAfter(doc: MapDocument, k: number, tx: number, ty: number, vw: number, vh: number): number {
  const zf = k / fitScale(doc, "r", vw, vh);
  const districtArea = new Map<number, number>();
  for (const key in doc.districts) districtArea.set(+key, districtWorldArea(doc.districts[key] as District));
  const screenOf = (i: number): [number, number] | null => {
    const cx = doc.N[i][1] * k + tx;
    const cy = doc.N[i][2] * k + ty;
    if (cx < -30 || cx > vw + 30 || cy < -30 || cy > vh + 30) return null;
    return [cx, cy];
  };
  return selectPins(doc, districtArea, screenOf, k, zf, null).length;
}

console.log("## Pin counts at fit zoom, before (unconditional doc.L, zf<=3.4 capitals) vs after (selectPins)\n");
console.log("| map | files | landmarks (doc.L) | viewport | before | after |");
console.log("|---|---:|---:|---|---:|---:|");
for (const slug of REPOS) {
  const file = path.join(MAPS_DIR, ...slug.split("/")) + ".json";
  const doc: MapDocument = JSON.parse(readFileSync(file, "utf8"));
  for (const [label, vw, vh] of VIEWPORTS) {
    const { k, tx, ty } = fitTransform(doc, vw, vh);
    const before = pinsBefore(doc, k, tx, ty, vw, vh);
    const after = pinsAfter(doc, k, tx, ty, vw, vh);
    console.log(`| ${slug} | ${doc.N.length} | ${doc.L.length} | ${label} | ${before} | ${after} |`);
  }
}

// Partition-degeneracy context (issue #57): how many districts msgraph and
// n8n actually have, and what share are the near-singleton tail landmarks()
// gives one capital each -- explains WHY the before/after gap is as large
// as it is for msgraph-sdk-python specifically.
console.log("\n## Partition shape (issue #57 context)\n");
console.log("| map | districts | singleton districts | q |");
console.log("|---|---:|---:|---:|");
for (const slug of ["n8n-io/n8n", "microsoftgraph/msgraph-sdk-python"]) {
  const file = path.join(MAPS_DIR, ...slug.split("/")) + ".json";
  const doc: MapDocument = JSON.parse(readFileSync(file, "utf8"));
  const ids = Object.keys(doc.districts);
  const singleton = ids.filter((d) => (doc.districts[d] as District).size === 1).length;
  console.log(`| ${slug} | ${ids.length} | ${singleton} | ${doc.q.toFixed(4)} |`);
}
