#!/usr/bin/env -S npx tsx
// A2 (district colour, issue #82): every fixture map in web/public/maps must
// come out of map/colour.ts's greedy colouring with zero adjacent district
// pairs sharing a hue -- UNLESS a district has six or more coloured
// neighbours, in which case six hues cannot avoid a repeat and this reports
// it rather than asserting something impossible. Also reports hue-use counts
// per map, so a lopsided assignment (five districts on hue 0, one each on
// the rest) is visible even when the "no adjacent repeat" check passes.
//
// Run: npx tsx web/scripts/check-district-colours.ts
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { assignDistrictHues, districtAdjacency, HUE_COUNT } from "../src/map/colour";
import type { MapDocument } from "../src/types";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const MAPS_DIR = path.resolve(HERE, "..", "public", "maps");

function findFixtures(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = path.join(dir, entry);
    if (statSync(full).isDirectory()) {
      out.push(...findFixtures(full));
    } else if (entry.endsWith(".json") && entry !== "index.json") {
      out.push(full);
    }
  }
  return out.sort();
}

let failed = false;
const fixtures = findFixtures(MAPS_DIR);
if (fixtures.length === 0) {
  console.error(`No fixture maps found under ${MAPS_DIR}. Copy the corpus fixtures into web/public/maps first.`);
  process.exit(1);
}

console.log("## District colour check: adjacent-pair hue collisions and hue-use counts\n");
for (const file of fixtures) {
  const doc: MapDocument = JSON.parse(readFileSync(file, "utf8"));
  const rel = path.relative(MAPS_DIR, file);
  const { neighbours, threshold } = districtAdjacency(doc);
  const hue = assignDistrictHues(doc);

  const collisions: Array<{ a: number; b: number; hue: number; degreeA: number; degreeB: number }> = [];
  const seen = new Set<string>();
  for (const [a, ns] of neighbours) {
    for (const b of ns) {
      const key = a < b ? `${a}-${b}` : `${b}-${a}`;
      if (seen.has(key)) continue;
      seen.add(key);
      if (hue.get(a) === hue.get(b)) {
        collisions.push({ a, b, hue: hue.get(a)!, degreeA: neighbours.get(a)!.length, degreeB: neighbours.get(b)!.length });
      }
    }
  }
  // A collision is only a bug if NEITHER endpoint has >=6 coloured
  // neighbours (six hues genuinely cannot avoid a repeat there).
  const unexplained = collisions.filter((c) => c.degreeA < HUE_COUNT && c.degreeB < HUE_COUNT);

  const counts = new Array(HUE_COUNT).fill(0);
  for (const h of hue.values()) counts[h]++;

  console.log(`### ${rel}`);
  console.log(`districts coloured: ${hue.size} · adjacency threshold: ${threshold.toFixed(4)} world units`);
  console.log(`hue use counts: ${counts.map((c, i) => `H${i}=${c}`).join(" ")}`);
  if (collisions.length === 0) {
    console.log("adjacent-pair collisions: none");
  } else {
    console.log(`adjacent-pair collisions: ${collisions.length} (${unexplained.length} unexplained, ${collisions.length - unexplained.length} at a district with >=${HUE_COUNT} coloured neighbours)`);
    for (const c of collisions) {
      const explained = c.degreeA >= HUE_COUNT || c.degreeB >= HUE_COUNT;
      console.log(`  d:${c.a} (${c.degreeA} neighbours) <-> d:${c.b} (${c.degreeB} neighbours) both hue H${c.hue}${explained ? "  [expected: >=6 neighbours somewhere]" : "  [UNEXPECTED]"}`);
    }
  }
  if (unexplained.length > 0) failed = true;
  console.log("");
}

if (failed) {
  console.error("FAIL: at least one fixture has two adjacent districts (each with <6 coloured neighbours) sharing a hue.");
  process.exit(1);
}
console.log("PASS: every fixture has zero unexplained adjacent-pair hue collisions.");
