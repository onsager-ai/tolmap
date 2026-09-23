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
} finally {
  await browser.close();
}
