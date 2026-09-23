#!/usr/bin/env node
// Viewer screenshots for review, taken in CI (never on the maintainer's
// laptop: headless Chromium renders in software and drove that machine to
// 95-99 °C on 2026-09-23, and its cooling cannot take it). The owner reviews
// viewer changes from a phone, so phone frames come first.
//
// Usage: node scripts/screenshots.mjs --base http://127.0.0.1:5176 --out shots
//        [--slugs django/django,langgenius/dify]
import { chromium } from "playwright";
import { mkdir } from "node:fs/promises";

const arg = (name, fallback) => {
  const i = process.argv.indexOf(`--${name}`);
  return i > 0 ? process.argv[i + 1] : fallback;
};
const base = arg("base", "http://127.0.0.1:5176");
const out = arg("out", "shots");
const slugs = arg("slugs", "django/django,langgenius/dify").split(",");

const PROFILES = [
  { name: "phone", viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 },
  { name: "desktop", viewport: { width: 1440, height: 900 }, isMobile: false, hasTouch: false, deviceScaleFactor: 1 },
];
// Zoom steps are clicks on the Zoom in button, so the frames match what a
// person gets from the same control rather than an internal zoom value.
const STEPS = [0, 2, 4];

await mkdir(out, { recursive: true });
const browser = await chromium.launch();
try {
  for (const profile of PROFILES) {
    for (const slug of slugs) {
      const context = await browser.newContext(profile);
      const page = await context.newPage();
      await page.goto(`${base}/${slug}`, { waitUntil: "domcontentloaded" });
      await page.locator("svg [data-k]").first().waitFor({ timeout: 30_000 });
      await page.waitForTimeout(500);
      let clicks = 0;
      for (const step of STEPS) {
        while (clicks < step) {
          await page.locator('button[aria-label="Zoom in"]').click();
          clicks++;
        }
        await page.waitForTimeout(400);
        const file = `${out}/${profile.name}-${slug.replace("/", "__")}-zoom${step}.png`;
        await page.screenshot({ path: file });
        console.log(file);
      }
      await context.close();
    }
  }

  // B4 (nested footprints, issue #82 scope item 10): three extra dify frames
  // per profile -- a focused district (to show streets), a file selected
  // (import lines), and a zoom deep enough for neighbourhood labels. Only
  // for dify: it's the one fixture in `slugs` with enough districts and
  // neighbourhoods for these to be worth a dedicated look.
  const DIFY_SLUG = "langgenius/dify";
  if (slugs.includes(DIFY_SLUG)) {
    const doc = await (await fetch(`${base}/maps/${DIFY_SLUG}.json`)).json();
    const workflowFile = doc.F.findIndex((p) => p === "web/app/components/workflow/types.ts");
    const workflowDistrict = doc.N[workflowFile][0];
    const stem = `${out}/${DIFY_SLUG.replace("/", "__")}`;

    for (const profile of PROFILES) {
      const context = await browser.newContext(profile);
      const page = await context.newPage();

      // A district focused: streets draw inside it (scope item 4).
      await page.goto(`${base}/${DIFY_SLUG}?d=${workflowDistrict}`, { waitUntil: "domcontentloaded" });
      await page.locator('button[aria-label="Zoom to district"]').waitFor({ timeout: 30_000 });
      await page.locator('button[aria-label="Zoom to district"]').click({ force: true });
      await page.waitForTimeout(900);
      await page.screenshot({ path: `${stem}-${profile.name}-district-focused.png` });
      console.log(`${stem}-${profile.name}-district-focused.png`);

      // A file selected: persistent import lines anchor on its footprint
      // (scope item 3).
      await page.goto(`${base}/${DIFY_SLUG}?file=${encodeURIComponent(doc.F[workflowFile])}`, { waitUntil: "domcontentloaded" });
      await page.locator("svg [data-k]").first().waitFor({ timeout: 30_000 });
      await page.waitForTimeout(700);
      if (profile.isMobile) await page.locator("[data-selection-panel] > div").first().tap().catch(() => {});
      await page.screenshot({ path: `${stem}-${profile.name}-file-selected.png` });
      console.log(`${stem}-${profile.name}-file-selected.png`);

      // A zoom deep enough for neighbourhood labels (scope item 5): focused
      // district, one more zoom step past "Zoom to district" itself.
      await page.goto(`${base}/${DIFY_SLUG}?d=${workflowDistrict}`, { waitUntil: "domcontentloaded" });
      await page.locator('button[aria-label="Zoom to district"]').waitFor({ timeout: 30_000 });
      await page.locator('button[aria-label="Zoom to district"]').click({ force: true });
      await page.waitForTimeout(900);
      await page.locator('button[aria-label="Zoom in"]').click();
      await page.waitForTimeout(400);
      await page.screenshot({ path: `${stem}-${profile.name}-neighbourhood-labels.png` });
      console.log(`${stem}-${profile.name}-neighbourhood-labels.png`);

      await context.close();
    }
  }

  // Issue #82 C2 scope item 10: three more dify frames per profile -- a deep
  // zoom showing cards inside a large file, a selected class with its
  // reference lines, and the file card's outline tree. Targets are picked
  // dynamically off the bundled district-0 symbols fixture (the same
  // approach check-view-stability.mjs's new C2 checks use), never hardcoded.
  if (slugs.includes(DIFY_SLUG)) {
    const doc = await (await fetch(`${base}/maps/${DIFY_SLUG}.json`)).json();
    const symbols = await (await fetch(`${base}/maps/${DIFY_SLUG}.symbols/0.json`)).json();
    const stem = `${out}/${DIFY_SLUG.replace("/", "__")}`;

    const fileSymbolCounts = new Map();
    for (const row of symbols.symbols) fileSymbolCounts.set(row[0], (fileSymbolCounts.get(row[0]) ?? 0) + 1);
    let bigFile = null;
    let bigFileCount = -1;
    for (const [f, n] of fileSymbolCounts) {
      if (n > bigFileCount) {
        bigFile = f;
        bigFileCount = n;
      }
    }

    const childCount = new Map();
    symbols.symbols.forEach((row) => {
      if (row[5] >= 0) childCount.set(row[5], (childCount.get(row[5]) ?? 0) + 1);
    });
    let classGlobal = null;
    let classLocal = -1;
    let classMembers = -1;
    symbols.symbols.forEach((row, local) => {
      if (row[2] !== 0) return;
      const global = symbols.symbol_indices[local];
      const n = childCount.get(global) ?? 0;
      if (n > classMembers) {
        classMembers = n;
        classGlobal = global;
        classLocal = local;
      }
    });
    const classFile = classLocal >= 0 ? symbols.symbols[classLocal][0] : null;

    if (bigFile != null) {
      for (const profile of PROFILES) {
        const context = await browser.newContext(profile);
        const page = await context.newPage();
        // Deep zoom on a large, symbol-dense file -- selecting it makes it
        // gate-eligible regardless of on-screen size, then zooming in grows
        // its (and its neighbours') footprints past the 40px card gate.
        await page.goto(`${base}/${DIFY_SLUG}?file=${encodeURIComponent(doc.F[bigFile])}`, { waitUntil: "domcontentloaded" });
        await page.locator("svg [data-k]").first().waitFor({ timeout: 30_000 });
        await page.waitForTimeout(700);
        for (let i = 0; i < 6; i++) {
          await page.locator('button[aria-label="Zoom in"]').click();
          await page.waitForTimeout(150);
        }
        await page.waitForTimeout(400);
        await page.screenshot({ path: `${stem}-${profile.name}-symbol-cards-deep-zoom.png` });
        console.log(`${stem}-${profile.name}-symbol-cards-deep-zoom.png`);

        if (profile.isMobile) await page.locator("[data-selection-panel] > div").first().tap().catch(() => {});
        await page.screenshot({ path: `${stem}-${profile.name}-outline-tree.png` });
        console.log(`${stem}-${profile.name}-outline-tree.png`);

        await context.close();
      }
    }

    if (classGlobal != null && classFile != null) {
      for (const profile of PROFILES) {
        const context = await browser.newContext(profile);
        const page = await context.newPage();
        // A selected class with its rolled-up reference lines (scope item 4)
        // -- set directly via hsym, since a card tap is exercised by the
        // check script, not needed again here just to reach this state.
        await page.goto(`${base}/${DIFY_SLUG}?file=${encodeURIComponent(doc.F[classFile])}&hsym=${classGlobal}`, { waitUntil: "domcontentloaded" });
        await page.locator("svg [data-k]").first().waitFor({ timeout: 30_000 });
        await page.waitForTimeout(700);
        for (let i = 0; i < 4; i++) {
          await page.locator('button[aria-label="Zoom in"]').click();
          await page.waitForTimeout(150);
        }
        await page.waitForTimeout(400);
        await page.screenshot({ path: `${stem}-${profile.name}-symbol-references.png` });
        console.log(`${stem}-${profile.name}-symbol-references.png`);
        await context.close();
      }
    }
  }
} finally {
  await browser.close();
}
