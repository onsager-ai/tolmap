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
/** docs/UX.md §3.1: the phone's sheet is raised by its grabber (a real
 * button cycling peek -> half -> full), not by tapping a card header. */
async function setSheetDetent(page, detent) {
  for (let i = 0; i < 3; i++) {
    const now = await page.locator("[data-phone-sheet]").getAttribute("data-detent").catch(() => null);
    if (now === detent || now == null) return;
    await page.locator("[data-sheet-grabber]").click();
    await page.waitForTimeout(400);
  }
}

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
      if (profile.isMobile) await setSheetDetent(page, "half");
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
        // docs/UX.md §4.3: the district index is the sheet's Districts tab.
        await setSheetDetent(page, "half");
        await page.screenshot({ path: `${stem}-${profile.name}-district-index-half.png` });
        console.log(`${stem}-${profile.name}-district-index-half.png`);
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

        if (profile.isMobile) await setSheetDetent(page, "full");
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
          await setSheetDetent(page, "half");
          await page.screenshot({ path: `${stem}-${profile.name}-district-index-half-${colorScheme}.png` });
          console.log(`${stem}-${profile.name}-district-index-half-${colorScheme}.png`);
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
          if (profile.isMobile) await setSheetDetent(page, "half");
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
          if (profile.isMobile) await setSheetDetent(page, "half");
          await page.screenshot({ path: `${stem}-${profile.name}-class-extends-${colorScheme}.png` });
          console.log(`${stem}-${profile.name}-class-extends-${colorScheme}.png`);

          await context.close();
        }
      }
    }
  }

  // docs/UX.md §11 phase 2: the phone shell. A selection made by TAPPING THE
  // MAP (sheet at Peek, target above it), a district selected, a file at
  // Half, the district index at Half, the Layers sheet, search, path mode --
  // on the 390 x 844 phone and on 360 x 640 and 320 x 568. The back-button
  // pop order is asserted in check-view-stability.mjs (checkPhoneBackStack).
  if (slugs.includes(DIFY_SLUG)) {
    const doc = await (await fetch(`${base}/maps/${DIFY_SLUG}.json`)).json();
    const workflowFile = doc.F.findIndex((p) => p === "web/app/components/workflow/types.ts");
    const stem = `${out}/phase2`;
    const phones = [
      PROFILES.find((p) => p.name === "phone"),
      { name: "phone360", viewport: { width: 360, height: 640 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 },
      { name: "phone320", viewport: { width: 320, height: 568 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 },
    ];
    const shot = async (page, name) => {
      await page.screenshot({ path: `${stem}-${name}.png` });
      console.log(`${stem}-${name}.png`);
    };
    const open = async (page, query = "") => {
      await page.goto(`${base}/${DIFY_SLUG}${query}`, { waitUntil: "domcontentloaded" });
      await page.locator("svg [data-k]").first().waitFor({ timeout: 30_000 });
      await page.waitForTimeout(700);
    };
    // A point on the map that hit-tests to `prefix`, clear of the chrome.
    const pickOnMap = (page, prefix) =>
      page.evaluate((prefix) => {
        const sel = prefix === "d:" ? 'svg.map-svg text.hit[data-k^="d:"]' : 'svg.map-svg .hit[data-k^="f:"]';
        for (const el of document.querySelectorAll(sel)) {
          const r = el.getBoundingClientRect();
          const x = r.left + r.width / 2;
          const y = r.top + r.height / 2;
          if (y < 90 || y > innerHeight - 200 || x < 20 || x > innerWidth - 80) continue;
          if (document.elementFromPoint(x, y)?.closest?.("[data-k]")?.getAttribute("data-k") !== el.getAttribute("data-k")) continue;
          return { x, y, key: el.getAttribute("data-k") };
        }
        return null;
      }, prefix);

    for (const profile of phones) {
      const context = await browser.newContext(profile);
      const page = await context.newPage();
      const tag = profile.name;

      await open(page);
      await shot(page, `${tag}-overview-peek`);

      // Map taps: a district, then a file in view.
      const d = await pickOnMap(page, "d:");
      if (d) {
        await page.touchscreen.tap(d.x, d.y);
        await page.waitForTimeout(700);
        await shot(page, `${tag}-district-selected-by-map-tap-peek`);
      }
      const f = await pickOnMap(page, "f:");
      if (f) {
        await page.touchscreen.tap(f.x, f.y);
        await page.waitForTimeout(500);
        if (!new URL(page.url()).searchParams.has("file")) {
          await page.touchscreen.tap(f.x, f.y);
          await page.waitForTimeout(500);
        }
        await page.waitForTimeout(400);
        await shot(page, `${tag}-file-selected-by-map-tap-peek`);
      }

      // A file at Half (the approved "File selected · half" state).
      if (workflowFile >= 0) {
        await open(page, `?file=${encodeURIComponent(doc.F[workflowFile])}`);
        await setSheetDetent(page, "half");
        await page.waitForTimeout(500);
        await shot(page, `${tag}-file-selected-half`);
        // Path mode: "Path from here", then a file tapped on the map.
        await setSheetDetent(page, "full");
        await page.locator('[data-path-start="from"]').click();
        await page.waitForTimeout(500);
        await shot(page, `${tag}-path-mode-picking`);
        const g = await pickOnMap(page, "f:");
        if (g) {
          await page.touchscreen.tap(g.x, g.y);
          await page.waitForTimeout(500);
          await setSheetDetent(page, "half");
          await page.waitForTimeout(400);
          await shot(page, `${tag}-path-mode-found-half`);
        }
      }

      // The district index at Half, and a district selected from it.
      await open(page);
      await setSheetDetent(page, "half");
      await shot(page, `${tag}-district-index-half`);
      const row = page.locator("[data-district-index-row] > button").first();
      if (await row.count()) {
        await row.click();
        await page.waitForTimeout(800);
        await shot(page, `${tag}-district-selected-from-index-peek`);
      }

      // The Layers sheet (§4.6), on the churn layer so its ramp shows.
      await open(page);
      await page.locator('button[aria-label="Map layers"]').click();
      await page.waitForTimeout(300);
      await page.locator('[data-layer-option="c"]').click();
      await page.waitForTimeout(300);
      await shot(page, `${tag}-layers-sheet`);

      // Search from the pill (phase 3 frames below cover it in full).
      await open(page);
      await page.locator("[data-open-search]").click();
      await page.locator('[data-search-layer] input[data-search-input]').fill("types");
      await page.waitForTimeout(300);
      await shot(page, `${tag}-search-open`);

      // Map quality (§4.2) as a Full sheet.
      await open(page);
      const quality = page.locator("[data-map-quality]");
      if (await quality.count()) {
        await quality.click();
        await page.waitForTimeout(500);
        await shot(page, `${tag}-map-quality-full`);
      }
      await context.close();
    }

    // Light theme, the file at Half (the approved light artboard).
    if (workflowFile >= 0) {
      const context = await browser.newContext({ ...PROFILES.find((p) => p.name === "phone"), colorScheme: "light" });
      const page = await context.newPage();
      await open(page, `?file=${encodeURIComponent(doc.F[workflowFile])}`);
      await setSheetDetent(page, "half");
      await page.waitForTimeout(500);
      await shot(page, "phone-file-selected-half-light");
      await context.close();
    }
  }

  // docs/UX.md §11 phase 3: search. Focused with results on the district
  // layer and the package layer, phone and desktop, dark (the approved
  // "Search active" artboard) and light; the empty and no-results states; a
  // result picked with the sheet at Peek and the target above it; the
  // desktop dropdown with a row highlighted from the keyboard.
  if (slugs.includes(DIFY_SLUG)) {
    const stem = `${out}/phase3`;
    const shot = async (page, name) => {
      await page.screenshot({ path: `${stem}-${name}.png` });
      console.log(`${stem}-${name}.png`);
    };
    const load = async (page, query = "") => {
      await page.goto(`${base}/${DIFY_SLUG}${query}`, { waitUntil: "domcontentloaded" });
      await page.locator("svg [data-k]").first().waitFor({ timeout: 30_000 });
      await page.waitForTimeout(700);
    };
    const type = async (page, text) => {
      await page.locator("input[data-search-input]").fill(text);
      await page.waitForTimeout(300);
    };
    const phones = [
      PROFILES.find((p) => p.name === "phone"),
      { name: "phone320", viewport: { width: 320, height: 568 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 },
    ];
    for (const colorScheme of ["dark", "light"]) {
      for (const profile of phones) {
        if (profile.name === "phone320" && colorScheme === "light") continue;
        const context = await browser.newContext({ ...profile, colorScheme });
        const page = await context.newPage();
        const tag = `${profile.name}-${colorScheme}`;
        const openSearch = async () => {
          await page.locator("[data-open-search]").click();
          await page.locator("[data-search-layer] input[data-search-input]").waitFor();
          await page.waitForTimeout(250);
        };
        await load(page);
        await openSearch();
        await shot(page, `${tag}-search-empty`);
        await type(page, "workflow");
        await shot(page, `${tag}-search-workflow-district-layer`);
        await type(page, "zzqqxxj");
        await shot(page, `${tag}-search-no-results`);
        await load(page, "?layer=p");
        await openSearch();
        await type(page, "workflow");
        await shot(page, `${tag}-search-workflow-package-layer`);
        // Picked: a file from the results, framed above the sheet at Peek.
        await load(page);
        await openSearch();
        await type(page, "workflow/types.ts");
        const doc = await (await fetch(`${base}/maps/${DIFY_SLUG}.json`)).json();
        const i = doc.F.indexOf("web/app/components/workflow/types.ts");
        const row = page.locator(`[data-search-key="f${i}"]`);
        if (i >= 0 && (await row.count())) {
          await row.click();
          await page.waitForTimeout(900);
          await shot(page, `${tag}-search-picked-file-peek`);
        }
        // Picked: a district.
        await openSearch();
        await type(page, "workflow");
        const dRow = page.locator('[data-search-option="district"]').first();
        if (await dRow.count()) {
          await dRow.click();
          await page.waitForTimeout(900);
          await shot(page, `${tag}-search-picked-district-peek`);
        }
        await context.close();
      }
      // Desktop: the dropdown on the district and package layers, a row
      // highlighted from the keyboard.
      const desktop = PROFILES.find((p) => p.name === "desktop");
      const context = await browser.newContext({ ...desktop, colorScheme });
      const page = await context.newPage();
      await load(page);
      await page.keyboard.press("/");
      await page.waitForTimeout(200);
      await shot(page, `desktop-${colorScheme}-search-empty`);
      await type(page, "workflow");
      await page.locator("input[data-search-input]").press("ArrowDown");
      await page.locator("input[data-search-input]").press("ArrowDown");
      await page.waitForTimeout(150);
      await shot(page, `desktop-${colorScheme}-search-workflow-district-layer`);
      await load(page, "?layer=p");
      await page.locator("input[data-search-input]").click();
      await type(page, "a");
      await shot(page, `desktop-${colorScheme}-search-a-package-layer`);
      await context.close();
    }
  }

  // Issue #97 (live job progress, ETA and cancel): the job progress page,
  // mid-build/queued/cancelled, in both themes -- against the same mock API
  // server (scripts/mock-api-server.mjs) check-view-stability.mjs's own
  // job-page checks use (viewer-check.yml starts it before Vite for both
  // scripts to share). Each slug is timestamped so a re-run never collides
  // with a job from a previous run still sitting in the mock's registry.
  {
    const postIndexJob = async (slug) => {
      const res = await fetch(`${base}/api/index`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ repo: slug }),
      });
      return res.json();
    };
    // The mock models the service's one-concurrent-job default -- a job left
    // running (or queued) after its own screenshot is taken would otherwise
    // sit ahead of the next theme's "mid-build" submission, so that frame
    // could capture "queued" instead of a running build (this is the same
    // fix check-view-stability.mjs's own job-page checks needed once several
    // of them ran back to back).
    const cancelJob = (jobId) => fetch(`${base}/api/jobs/${jobId}/cancel`, { method: "POST" }).catch(() => {});
    const jobStem = `${out}/job-progress`;
    const desktop = { viewport: { width: 1200, height: 800 }, isMobile: false, hasTouch: false, deviceScaleFactor: 1 };

    for (const colorScheme of ["light", "dark"]) {
      // Mid-build.
      {
        const slug = `shotorg/mid-build-${Date.now()}`;
        const accepted = await postIndexJob(slug);
        const context = await browser.newContext({ ...desktop, colorScheme });
        const page = await context.newPage();
        await page.goto(`${base}/new?job=${accepted.job_id}&slug=${encodeURIComponent(slug)}`, { waitUntil: "domcontentloaded" });
        await page.waitForSelector("[data-progress-fill][data-progress-pct]", { timeout: 20_000 }).catch(() => {});
        await page.waitForTimeout(300);
        await page.screenshot({ path: `${jobStem}-mid-build-${colorScheme}.png` });
        console.log(`${jobStem}-mid-build-${colorScheme}.png`);
        await context.close();
        await cancelJob(accepted.job_id);
      }

      // Queued: a second submission while the first still occupies the
      // mock's one concurrency slot (docs/API.md: TOLMAP_MAX_CONCURRENT_JOBS
      // defaults to 1).
      {
        const runningSlug = `shotorg/queue-running-${Date.now()}`;
        const queuedSlug = `shotorg/queue-behind-${Date.now()}`;
        const running = await postIndexJob(runningSlug);
        const queued = await postIndexJob(queuedSlug);
        const context = await browser.newContext({ ...desktop, colorScheme });
        const page = await context.newPage();
        await page.goto(`${base}/new?job=${queued.job_id}&slug=${encodeURIComponent(queuedSlug)}`, { waitUntil: "domcontentloaded" });
        await page.waitForSelector("[data-queued-text]", { timeout: 15_000 }).catch(() => {});
        await page.waitForTimeout(200);
        await page.screenshot({ path: `${jobStem}-queued-${colorScheme}.png` });
        console.log(`${jobStem}-queued-${colorScheme}.png`);
        await context.close();
        await cancelJob(running.job_id);
        await cancelJob(queued.job_id);
      }

      // Issue #162: a detection refusal, on a phone -- the plain-line
      // explanation above the evidence, and no "try again".
      {
        const slug = `shotorg/uncertain-${Date.now()}`;
        const accepted = await postIndexJob(slug);
        const phone = PROFILES.find((profile) => profile.name === "phone");
        const context = await browser.newContext({ ...phone, colorScheme });
        const page = await context.newPage();
        await page.goto(`${base}/new?job=${accepted.job_id}&slug=${encodeURIComponent(slug)}`, { waitUntil: "domcontentloaded" });
        await page.waitForSelector('[data-job-failure][data-job-failure-code="detection_uncertain"]', { timeout: 30_000 }).catch(() => {});
        await page.waitForTimeout(200);
        await page.screenshot({ path: `${jobStem}-detection-uncertain-phone-${colorScheme}.png` });
        console.log(`${jobStem}-detection-uncertain-phone-${colorScheme}.png`);
        await context.close();
      }

      // Cancelled.
      {
        const slug = `shotorg/cancel-${Date.now()}`;
        const accepted = await postIndexJob(slug);
        const context = await browser.newContext({ ...desktop, colorScheme });
        const page = await context.newPage();
        await page.goto(`${base}/new?job=${accepted.job_id}&slug=${encodeURIComponent(slug)}`, { waitUntil: "domcontentloaded" });
        await page.getByRole("button", { name: "cancel", exact: true }).waitFor({ timeout: 15_000 }).catch(() => {});
        await page.getByRole("button", { name: "cancel", exact: true }).click().catch(() => {});
        await page.getByRole("button", { name: "yes, cancel" }).click().catch(() => {});
        await page.waitForSelector("[data-job-failure]", { timeout: 15_000 }).catch(() => {});
        await page.waitForTimeout(200);
        await page.screenshot({ path: `${jobStem}-cancelled-${colorScheme}.png` });
        console.log(`${jobStem}-cancelled-${colorScheme}.png`);
        await context.close();
      }
    }
  }
  // Issue #110 P2: the "exact (SCIP) vs heuristic" reference-coverage
  // indicator, collapsed and expanded, both themes and both profiles.
  // No committed map has `coverage.references` (none was built with
  // `--refs scip`), so this drives the synthetic fixture
  // check-fixtures/scip-coverage__flask.json.gz (see its README.md entry
  // and web/scripts/make-scip-coverage-fixture.mjs) instead of a real slug.
  {
    const SLUG = "scip-coverage/flask";
    const stem = `${out}/${SLUG.replace("/", "__")}`;
    for (const profile of PROFILES) {
      for (const colorScheme of ["light", "dark"]) {
        const context = await browser.newContext({ ...profile, colorScheme });
        const page = await context.newPage();
        await page.goto(`${base}/${SLUG}`, { waitUntil: "domcontentloaded" });
        await page.locator("svg [data-k]").first().waitFor({ timeout: 30_000 });
        // Phone: docs/UX.md §4.2's map-quality row in the sheet opens the
        // same explanation as a Full sheet (the floating chip is gone).
        const trigger = profile.isMobile ? page.locator("[data-map-quality]") : page.locator("[data-reference-coverage] button");
        await trigger.waitFor({ timeout: 10_000 });
        await page.waitForTimeout(300);
        await page.screenshot({ path: `${stem}-${profile.name}-collapsed-${colorScheme}.png` });
        console.log(`${stem}-${profile.name}-collapsed-${colorScheme}.png`);

        // Tap on phone (no hover surface), click on desktop -- same
        // profile.isMobile split the other frames above use.
        if (profile.isMobile) await trigger.tap();
        else await trigger.click();
        await page.waitForTimeout(400);
        await page.screenshot({ path: `${stem}-${profile.name}-expanded-${colorScheme}.png` });
        console.log(`${stem}-${profile.name}-expanded-${colorScheme}.png`);

        await context.close();
      }
    }
  }
} finally {
  await browser.close();
}
