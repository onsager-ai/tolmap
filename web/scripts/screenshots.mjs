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

// Issue #82 C2 CI review finding: clicking the "Zoom in" button repeatedly
// while a symbol-dense file (a full outline tree, external references) is
// selected timed out on desktop -- that file's card grows the selection
// panel tall enough to cover the corner the zoom button sits in, and
// Playwright's click retries for 30s against an intercepted target before
// giving up, aborting the whole script. Wheel-zooming at a point away from
// the panel (left of centre; the panel is anchored top-right on desktop,
// a bottom sheet on phone) sidesteps the button entirely -- the same
// technique check-view-stability.mjs's own zoomIn() already uses.
async function wheelZoomIn(page, cx, cy, notches) {
  await page.mouse.move(cx, cy);
  for (let i = 0; i < notches; i++) {
    await page.mouse.wheel(0, -200);
    await page.waitForTimeout(50);
  }
  await page.waitForTimeout(650);
}

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

  // Issue #82 "district index": explicit rail (desktop) and opened drawer
  // (phone) frames for dify -- the new Districts list replacing the old
  // Landmarks/Hubs sections is the main subject of this PR, not just an
  // incidental part of the plain zoom-step frames the STEPS loop above
  // already takes for every slug (those show the rail closed on phone, the
  // default collapsed peek).
  if (slugs.includes(DIFY_SLUG)) {
    const stem = `${out}/${DIFY_SLUG.replace("/", "__")}`;
    for (const profile of PROFILES) {
      const context = await browser.newContext(profile);
      const page = await context.newPage();
      await page.goto(`${base}/${DIFY_SLUG}`, { waitUntil: "domcontentloaded" });
      await page.locator("svg [data-k]").first().waitFor({ timeout: 30_000 });
      await page.waitForTimeout(500);
      if (profile.isMobile) {
        await page.locator("aside button", { hasText: /districts/i }).first().click();
        await page.waitForTimeout(400);
        await page.screenshot({ path: `${stem}-${profile.name}-district-drawer-open.png` });
        console.log(`${stem}-${profile.name}-district-drawer-open.png`);
      } else {
        await page.screenshot({ path: `${stem}-${profile.name}-district-rail.png` });
        console.log(`${stem}-${profile.name}-district-rail.png`);
      }
      await context.close();
    }
  }

  // Owner feedback (issue #82, "layer brightness"): "package layer seems to
  // have larger brightness against others". The fix (geometry.ts's
  // LAYER_SURFACE_MIX, now applied to package colours and the churn/
  // complexity ramps via mixTowardCanvas(), not just district hues) needs a
  // side-by-side of the SAME view in both colour layers to judge -- desktop
  // forced into dark theme (the owner's own report named `--p1: #ffc247`,
  // dark mode's own package swatch) and phone, since brightness is a
  // per-theme, per-viewport question. `geo=r` (footprint mode) on both so
  // the comparison is about colour alone, not dot-vs-footprint rendering.
  if (slugs.includes(DIFY_SLUG)) {
    const stem = `${out}/${DIFY_SLUG.replace("/", "__")}`;
    const desktopProfile = PROFILES.find((p) => p.name === "desktop");
    const phoneProfile = PROFILES.find((p) => p.name === "phone");
    const layerFrames = [
      { profile: desktopProfile, label: "desktop-dark", dark: true },
      { profile: phoneProfile, label: "phone", dark: false },
    ];
    for (const { profile, label, dark } of layerFrames) {
      for (const layer of ["d", "p"]) {
        const context = await browser.newContext(profile);
        const page = await context.newPage();
        await page.goto(`${base}/${DIFY_SLUG}?geo=r&layer=${layer}`, { waitUntil: "domcontentloaded" });
        await page.locator("svg [data-k]").first().waitFor({ timeout: 30_000 });
        if (dark) await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));
        await page.waitForTimeout(400);
        await wheelZoomIn(page, profile.viewport.width * 0.35, profile.viewport.height * 0.4, 3);
        await page.screenshot({ path: `${stem}-${label}-layer-${layer}.png` });
        console.log(`${stem}-${label}-layer-${layer}.png`);
        await context.close();
      }
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

    // CI review finding (issue #82 C2): a district's symbols response also
    // carries the FAR END of every crossing edge, whose own file sits in
    // some other, unbundled district (docs/API.md) -- picking one of those
    // as a screenshot target lands on a file that can never decode any
    // cards. Both scans below are restricted to `symbols.files`, this
    // district's actual member files.
    const memberFiles = new Set(symbols.files);
    const fileSymbolCounts = new Map();
    for (const row of symbols.symbols) {
      if (!memberFiles.has(row[0])) continue;
      fileSymbolCounts.set(row[0], (fileSymbolCounts.get(row[0]) ?? 0) + 1);
    }
    let bigFile = null;
    let bigFileCount = -1;
    for (const [f, n] of fileSymbolCounts) {
      if (n > bigFileCount) {
        bigFile = f;
        bigFileCount = n;
      }
    }

    // Ranked by code_lines, not member count -- see check-view-stability.mjs's
    // pickExpandableClasses for why (a class's allocated area follows its
    // code-line "mass", which a raw member count can badly under-predict for
    // a class full of one-line members).
    const childCount = new Map();
    symbols.symbols.forEach((row) => {
      if (row[5] >= 0) childCount.set(row[5], (childCount.get(row[5]) ?? 0) + 1);
    });
    let classGlobal = null;
    let classLocal = -1;
    let classCodeLines = -1;
    symbols.symbols.forEach((row, local) => {
      if (row[2] !== 0) return;
      if (!memberFiles.has(row[0])) return;
      const global = symbols.symbol_indices[local];
      const n = childCount.get(global) ?? 0;
      if (n === 0) return;
      if (row[6] > classCodeLines) {
        classCodeLines = row[6];
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
        // ~1.6^6 via wheel notches (~1.377x each) -- see wheelZoomIn's doc
        // comment for why this isn't the "Zoom in" button.
        await wheelZoomIn(page, profile.viewport.width * 0.35, profile.viewport.height * 0.4, 9);
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
        await wheelZoomIn(page, profile.viewport.width * 0.35, profile.viewport.height * 0.4, 6); // ~1.6^4
        await page.screenshot({ path: `${stem}-${profile.name}-symbol-references.png` });
        console.log(`${stem}-${profile.name}-symbol-references.png`);
        await context.close();
      }
    }
  }

  // Issue #82 "chrome follows the theme" (owner decision, 2026-09-24):
  // the main frames, once per Playwright `colorScheme` -- "light" and
  // "dark" drive the OS-level prefers-color-scheme media query the app
  // already listens to (System, the default choice; every OTHER frame in
  // this file is taken with no colorScheme override, i.e. this runner's
  // default, per CLAUDE.md's "keep all existing checks green, run in the
  // default theme"). Four kinds of frame, named with a `-light`/`-dark`
  // suffix: the opening zoom, the district rail/drawer, a selected file's
  // card (the link-colour legend -- also where the owner's three-digit-
  // count wrap fix lives, see LinkLegend.tsx), and a deep-zoom symbol-card
  // frame.
  if (slugs.includes(DIFY_SLUG)) {
    const doc = await (await fetch(`${base}/maps/${DIFY_SLUG}.json`)).json();
    const symbols = await (await fetch(`${base}/maps/${DIFY_SLUG}.symbols/0.json`)).json();
    const stem = `${out}/${DIFY_SLUG.replace("/", "__")}`;
    const workflowFile = doc.F.findIndex((p) => p === "web/app/components/workflow/types.ts");

    // The same "biggest symbol-dense member file" pick the deep-zoom block
    // above uses, restricted to district 0's own member files (not the far
    // end of a crossing edge -- see that block's own comment for why).
    const memberFiles = new Set(symbols.files);
    const fileSymbolCounts = new Map();
    for (const row of symbols.symbols) {
      if (!memberFiles.has(row[0])) continue;
      fileSymbolCounts.set(row[0], (fileSymbolCounts.get(row[0]) ?? 0) + 1);
    }
    let bigFile = null;
    let bigFileCount = -1;
    for (const [f, n] of fileSymbolCounts) {
      if (n > bigFileCount) {
        bigFile = f;
        bigFileCount = n;
      }
    }

    for (const colorScheme of ["light", "dark"]) {
      for (const profile of PROFILES) {
        const context = await browser.newContext({ ...profile, colorScheme });
        const page = await context.newPage();

        // Opening zoom.
        await page.goto(`${base}/${DIFY_SLUG}`, { waitUntil: "domcontentloaded" });
        await page.locator("svg [data-k]").first().waitFor({ timeout: 30_000 });
        await page.waitForTimeout(500);
        await page.screenshot({ path: `${stem}-${profile.name}-zoom0-${colorScheme}.png` });
        console.log(`${stem}-${profile.name}-zoom0-${colorScheme}.png`);

        // District rail (desktop) / drawer (phone).
        if (profile.isMobile) {
          await page.locator("aside button", { hasText: /districts/i }).first().click();
          await page.waitForTimeout(400);
          await page.screenshot({ path: `${stem}-${profile.name}-district-drawer-open-${colorScheme}.png` });
          console.log(`${stem}-${profile.name}-district-drawer-open-${colorScheme}.png`);
        } else {
          await page.screenshot({ path: `${stem}-${profile.name}-district-rail-${colorScheme}.png` });
          console.log(`${stem}-${profile.name}-district-rail-${colorScheme}.png`);
        }

        await context.close();
      }
    }

    if (workflowFile >= 0) {
      for (const colorScheme of ["light", "dark"]) {
        for (const profile of PROFILES) {
          const context = await browser.newContext({ ...profile, colorScheme });
          const page = await context.newPage();

          // File card with the link-colour legend -- the owner follow-up's
          // own repro file (three-digit "imported by" count).
          await page.goto(`${base}/${DIFY_SLUG}?file=${encodeURIComponent(doc.F[workflowFile])}`, { waitUntil: "domcontentloaded" });
          await page.locator("svg [data-k]").first().waitFor({ timeout: 30_000 });
          await page.waitForTimeout(700);
          if (profile.isMobile) await page.locator("[data-selection-panel] > div").first().tap().catch(() => {});
          await page.screenshot({ path: `${stem}-${profile.name}-file-selected-${colorScheme}.png` });
          console.log(`${stem}-${profile.name}-file-selected-${colorScheme}.png`);

          await context.close();
        }
      }
    }

    if (bigFile != null) {
      for (const colorScheme of ["light", "dark"]) {
        for (const profile of PROFILES) {
          const context = await browser.newContext({ ...profile, colorScheme });
          const page = await context.newPage();

          // Deep zoom on a large, symbol-dense file -- same target and zoom
          // depth as the default-theme "symbol-cards-deep-zoom" frame above.
          await page.goto(`${base}/${DIFY_SLUG}?file=${encodeURIComponent(doc.F[bigFile])}`, { waitUntil: "domcontentloaded" });
          await page.locator("svg [data-k]").first().waitFor({ timeout: 30_000 });
          await page.waitForTimeout(700);
          await wheelZoomIn(page, profile.viewport.width * 0.35, profile.viewport.height * 0.4, 9);
          await page.screenshot({ path: `${stem}-${profile.name}-symbol-cards-deep-zoom-${colorScheme}.png` });
          console.log(`${stem}-${profile.name}-symbol-cards-deep-zoom-${colorScheme}.png`);

          await context.close();
        }
      }
    }

    // #103 build items 1/2/4: a selected class with a drawn extends line
    // (the hollow-triangle arrowhead is --ink, theme-dependent) and its
    // card's own "extends" list -- both themes, same reasoning as every
    // other frame in this block. Target picked dynamically off the bundled
    // fixture's OWN `kinds` legend (never a hardcoded edge-kind index): the
    // first `extends` edge whose class is a real member of this district,
    // same memberFiles restriction the picks above already use.
    const extendsIdx = symbols.kinds ? symbols.kinds.indexOf("extends") : -1;
    let inheritanceClassGlobal = null;
    let inheritanceClassFile = null;
    if (extendsIdx >= 0) {
      const localByGlobal = new Map(symbols.symbol_indices.map((g, local) => [g, local]));
      for (const [source, , , kind] of symbols.edges) {
        if (kind !== extendsIdx) continue;
        const sourceLocal = localByGlobal.get(source);
        if (sourceLocal == null || !memberFiles.has(symbols.symbols[sourceLocal][0])) continue;
        inheritanceClassGlobal = source;
        inheritanceClassFile = symbols.symbols[sourceLocal][0];
        break;
      }
    }

    if (inheritanceClassGlobal != null && inheritanceClassFile != null) {
      for (const colorScheme of ["light", "dark"]) {
        for (const profile of PROFILES) {
          const context = await browser.newContext({ ...profile, colorScheme });
          const page = await context.newPage();

          await page.goto(`${base}/${DIFY_SLUG}?file=${encodeURIComponent(doc.F[inheritanceClassFile])}&hsym=${inheritanceClassGlobal}`, { waitUntil: "domcontentloaded" });
          await page.locator("svg [data-k]").first().waitFor({ timeout: 30_000 });
          await page.waitForTimeout(700);
          await wheelZoomIn(page, profile.viewport.width * 0.35, profile.viewport.height * 0.4, 6); // ~1.6^4, same as the symbol-references frame above
          if (profile.isMobile) await page.locator("[data-selection-panel] > div").first().tap().catch(() => {});
          await page.screenshot({ path: `${stem}-${profile.name}-class-extends-${colorScheme}.png` });
          console.log(`${stem}-${profile.name}-class-extends-${colorScheme}.png`);

          await context.close();
        }
      }
    }
  }
} finally {
  await browser.close();
}
