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
} finally {
  await browser.close();
}
