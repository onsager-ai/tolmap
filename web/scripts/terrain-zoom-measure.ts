#!/usr/bin/env -S npx tsx
// Issue #54, scope item 2: measure at which zf (zoom relative to fitScale)
// each terrain element (sub-district contour/label, parcel cell/label,
// arterial road) FIRST clears a legibility floor already established
// elsewhere on this map, so the zoom thresholds MapRenderer.drawTerrain
// gates on can be justified from a measured quantity instead of a picked
// constant.
//
// Reuses the same two floors the renderer already trusts for an identical
// question ("is this place big enough on screen to read"):
//   - PIN_ESTABLISH_AREA (pins.ts): 3 file-dots' worth of screen area, the
//     bar a district's own CAPITAL PIN has to clear before it draws. A
//     sub-district is a smaller place than its parent district but the
//     same kind of place, so the same bar is reused rather than inventing a
//     second "is this legible" constant.
//   - DOT_DENSITY_FLOOR (constants.ts): px² per file before #49's budget
//     draws a file's own dot at all. A parcel packs >=1 files into one grid
//     cell and reads as ONE thing, so the natural floor for the cell itself
//     is "at least as legible as a single file dot" -- one DOT_DENSITY_FLOOR
//     of screen area, not a fraction or a multiple of it.
//
// zf is k/fitScale(doc,"r",vw,vh) at each measured viewport -- the SAME zf
// MapRenderer.paint() computes once per frame as zf0 and passes into
// drawTerrain. Solving screenArea(zf) = worldArea * (zf*fitScale)^2 >= floor
// for zf gives zf_establish = sqrt(floor / worldArea) / fitScale directly,
// no search needed.
//
// n8n-io/n8n and aws/aws-sdk-go-v2 are both over this task's browser
// ceiling (see HANDOFF.md/CLAUDE.md: ~6.5k files, dify is the largest safe
// to open locally) -- this script never opens a page, it is pure Node over
// the fetched terrain-build JSON, exactly like pin-counts.ts.
//
// Run: npx tsx web/scripts/terrain-zoom-measure.ts [slug ...]
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { districtWorldArea, fitScale, polygonArea } from "../src/map/geometry";
import { DOT_DENSITY_FLOOR } from "../src/map/constants";
import { PIN_ESTABLISH_AREA } from "../src/map/pins";
import type { District, MapDocument, TerrainSubdistrict, TerrainParcel } from "../src/types";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const MAPS_DIR = path.resolve(HERE, "..", "public", "maps");

const DEFAULT_SLUGS = [
  "crawlab-team/crawlab",
  "django/django",
  "langgenius/dify",
  "n8n-io/n8n",
  "aws/aws-sdk-go-v2",
  "microsoft/vscode",
  "elastic/kibana",
  "twentyhq/twenty",
];
const VIEWPORTS: Array<[string, number, number]> = [
  ["390x700 (phone)", 390, 700],
  ["1440x900 (desktop)", 1440, 900],
];

function quantile(xs: number[], q: number): number {
  if (!xs.length) return NaN;
  const s = [...xs].sort((a, b) => a - b);
  const idx = Math.min(s.length - 1, Math.max(0, Math.round(q * (s.length - 1))));
  return s[idx];
}
function summarize(xs: number[]): string {
  if (!xs.length) return "n/a";
  return `${quantile(xs, 0.5).toFixed(3)} [${quantile(xs, 0.1).toFixed(3)}–${quantile(xs, 0.9).toFixed(3)}]`;
}

function subArea(sd: TerrainSubdistrict): number {
  let a = 0;
  for (const poly of sd.blob) a += polygonArea(poly);
  return a;
}
function parcelArea(p: TerrainParcel): number {
  return p.rect[2] * p.rect[3];
}

const slugs = process.argv.slice(2).length ? process.argv.slice(2) : DEFAULT_SLUGS;

console.log("## Terrain element establish-zf, per map/viewport\n");
console.log("zf_establish = sqrt(floor / worldArea) / fitScale -- the zf at which the element's own");
console.log("on-screen area first clears the reused floor (PIN_ESTABLISH_AREA=" + PIN_ESTABLISH_AREA + " for");
console.log("sub-district contours, DOT_DENSITY_FLOOR=" + DOT_DENSITY_FLOOR + " for parcel cells).\n");
console.log(
  "| map | viewport | district (eligible, files) | zf full reveal | subdistricts n | sub zf_establish median [p10-p90] | parcels n | parcel zf_establish median [p10-p90] | arterials |",
);
console.log("|---|---|---|---:|---:|---|---:|---|---:|");

for (const slug of slugs) {
  const file = path.join(MAPS_DIR, ...slug.split("/")) + ".json";
  let doc: MapDocument;
  try {
    doc = JSON.parse(readFileSync(file, "utf8"));
  } catch {
    console.log(`| ${slug} | -- | (map not fetched: ${file}) | | | | | | |`);
    continue;
  }
  if (!doc.terrain) {
    console.log(`| ${slug} | -- | (no terrain block -- not built with --terrain) | | | | | | |`);
    continue;
  }
  for (const [label, vw, vh] of VIEWPORTS) {
    const fs = fitScale(doc, "r", vw, vh);
    for (const [dkey, terrain] of Object.entries(doc.terrain)) {
      const d = +dkey;
      const district = doc.districts[String(d)] as District;
      const dArea = districtWorldArea(district);
      const zfFullReveal = Math.sqrt((district.size * DOT_DENSITY_FLOOR) / dArea) / fs;
      const subZfs = terrain.subdistricts.map((sd) => Math.sqrt(PIN_ESTABLISH_AREA / subArea(sd)) / fs);
      const parcelZfs = terrain.parcels.map((p) => Math.sqrt(DOT_DENSITY_FLOOR / parcelArea(p)) / fs);
      console.log(
        `| ${slug} | ${label} | ${doc.names[dkey]} (${district.size}) | ${zfFullReveal.toFixed(3)} | ${terrain.subdistricts.length} | ${summarize(subZfs)} | ${terrain.parcels.length} | ${summarize(parcelZfs)} | ${terrain.arterials.length} |`,
      );
    }
  }
}
