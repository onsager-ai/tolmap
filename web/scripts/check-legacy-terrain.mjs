#!/usr/bin/env node
// Stored maps from before removal can still carry a terrain block. Load one
// through the real viewer and confirm it draws and selects identically.
// Usage: pnpm exec vite --port 5176 --strictPort; node scripts/check-legacy-terrain.mjs
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const base = process.argv[2] ?? "http://127.0.0.1:5176";
const fixture = JSON.parse(await readFile(fileURLToPath(new URL("../public/maps/django/django.json", import.meta.url)), "utf8"));
const legacy = structuredClone(fixture);
legacy.terrain = {
  "0": {
    arterials: [{ file: 0, stranded: 2, links: [1] }],
    subdistricts: [{ suffix: 1, members: [0], c: [0, 0], blob: [[[0, 0], [1, 0], [0, 1]]] }],
    parcels: [{ address: "legacy/path", members: [1], rect: [0, 0, 1, 1] }],
    max_suffix: 1,
  },
};

const browser = await chromium.launch({ headless: true });
try {
  const snapshots = [];
  for (const document of [fixture, legacy]) {
    const page = await browser.newPage({ viewport: { width: 1200, height: 800 } });
    let served = false;
    await page.route("**/maps/django/django.json", async (route) => {
      served = true;
      await route.fulfill({ json: document });
    });
    await page.goto(`${base}/django/django`);
    await page.locator('svg.map-svg [data-k^="d:"]').first().waitFor();
    const svg = await page.locator("svg.map-svg").innerHTML();
    await page.locator('svg.map-svg text[data-k^="d:"]').first().click();
    const card = await page.locator("[data-selection-panel]").innerText();
    if (!served || !card.includes("files")) throw new Error("map or district card failed to load");
    snapshots.push({ svg, card });
    await page.close();
  }
  if (snapshots[0].svg !== snapshots[1].svg || snapshots[0].card !== snapshots[1].card) {
    throw new Error("legacy terrain changed the rendered map or selection card");
  }
  console.log("legacy terrain map loaded and ignored; SVG and district card match ordinary map");
} finally {
  await browser.close();
}
