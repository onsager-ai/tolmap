#!/usr/bin/env node
// Regression check for fix/keep-view-on-select: MapCanvas's ResizeObserver
// used to be recreated on every [doc, geo, layer, sel, selSym, selD,
// route] change, and ResizeObserver.observe() always delivers
// one synchronous "initial size" callback on subscribe -- size unchanged or
// not. That callback called MapRenderer.resize(), which always called
// fit(), so tapping a file dot, tapping it away, or switching the layer all
// snapped the view back to the opening fit. See MapCanvas.tsx's
// ResizeObserver effect and MapRenderer.resize()'s doc comment for the fix.
//
// This drives a real Vite dev server (own port, never 5173 -- that belongs
// to another worktree's live session) with Playwright, on both a map small
// enough to eyeball (django/django) and one with a denser, more irregular
// mainland silhouette still below the local browser ceiling
// (langgenius/dify), at a desktop size and a touch phone
// size, and asserts the view is byte-for-byte the same SVG geometry after
// each of: tapping a file dot, tapping empty map, and switching the layer.
// It also resizes the browser viewport and checks the view shifts by
// EXACTLY half the viewport delta (the "keep the world point under the
// centre at the centre" contract resize() now has), which is a strong
// positive check that a resize does NOT quietly re-fit: a re-fit would
// reframe from mainlandBounds() and essentially never produce that exact
// half-delta by coincidence. It checks the fit button and repo switching
// still do fit -- the two places fitting is supposed to remain -- and,
// since fixing the ResizeObserver bug surfaced a second, previously-papered-
// over bug in the repoKey effect (fit() ran before MapRenderer.state pointed
// at the new document, so it framed the PREVIOUS repo's bounds, or nothing
// at all on first mount), that the mainland's on-screen extent actually
// sits inside fit()'s own pad -- not overflowing the viewport -- on first
// load and after a repo switch, plus (readable-overview PR review finding)
// that NEITHER profile auto-selects anything or opens with a dimmed dot on
// a fresh load -- desktop used to auto-select the top landmark, which,
// once selection started dimming non-neighbour files, made a fresh
// django/django load open with most of the map already dimmed; removed.
//
// Usage:
//   pnpm exec vite --port 5176 --strictPort &
//   node scripts/check-view-stability.mjs [--base http://127.0.0.1:5176]

import { chromium } from "playwright";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";

// Required generated files, relative to web/. The dify fixture is the #79
// rebuilt main map (not the older terrain-off map). public/maps is gitignored;
// a fresh checkout must supply this artifact before running check:view.
const MAP_FIXTURES = {
  "django/django": "d37c1e72dfd6363d6da2368505fb375f0094fa50e750f084b45b099ec292843e",
  "langgenius/dify": "bdc82e2586be79f091d28bba0cc2b32138e8452efa7e87ad108e8e863aef2014",
};

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

async function preflight(base) {
  console.log("Required map files in web/public/maps/: " + Object.keys(MAP_FIXTURES).map((slug) => `${slug}.json`).join(", "));
  for (const [slug, expectedHash] of Object.entries(MAP_FIXTURES)) {
    const relative = `public/maps/${slug}.json`;
    const file = new URL(`../${relative}`, import.meta.url);
    let local;
    try {
      local = await readFile(file);
    } catch (error) {
      if (error.code === "ENOENT") throw new Error(`Required map missing: web/${relative}. Generate/copy the map before running check:view.`);
      throw error;
    }
    const actualHash = sha256(local);
    if (actualHash !== expectedHash) {
      throw new Error(`Wrong map fixture: web/${relative} has SHA-256 ${actualHash}; check:view requires ${expectedHash} (#79 rebuilt main artifact).`);
    }
    let response;
    try {
      response = await fetch(`${base}/maps/${slug}.json`);
    } catch (error) {
      throw new Error(`Cannot reach Vite at ${base}: ${error.message}`);
    }
    if (!response.ok) throw new Error(`Vite did not serve required map /maps/${slug}.json (HTTP ${response.status}).`);
    const servedHash = sha256(Buffer.from(await response.arrayBuffer()));
    if (servedHash !== actualHash) {
      throw new Error(`Vite at ${base} serves a different /maps/${slug}.json (SHA-256 ${servedHash}); start Vite from this worktree's web/ directory.`);
    }
  }
  const source = await fetch(`${base}/src/map/MapRenderer.ts`);
  if (!source.ok || !(await source.text()).includes("data-folder-label")) {
    throw new Error(`Vite at ${base} does not serve this branch's folder-label viewer; start Vite from this worktree's web/ directory.`);
  }
}

function parseArgs(argv) {
  const args = { base: "http://127.0.0.1:5176", beforeBase: null, featureOnly: false };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--base") args.base = argv[++i];
    else if (argv[i] === "--before-base") args.beforeBase = argv[++i];
    else if (argv[i] === "--feature-only") args.featureOnly = true;
    else throw new Error(`unknown arg: ${argv[i]}`);
  }
  return args;
}

const MAPS = ["django/django", "langgenius/dify"];
const PROFILES = [
  { name: "desktop", viewport: { width: 1200, height: 800 }, isMobile: false, hasTouch: false },
  { name: "phone", viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 },
];

let failures = 0;
let checks = 0;

function report(ok, label, detail) {
  checks++;
  if (ok) {
    console.log(`  ok    ${label}`);
  } else {
    failures++;
    console.log(`  FAIL  ${label}${detail ? " -- " + detail : ""}`);
  }
}

/** The first district polygon (`path.hit`, data-k="d:...") is a stable
 * on-screen reference: its geometry only depends on the current (k, tx, ty),
 * never on selection/layer highlighting (those only touch fill/stroke
 * attributes, not the path's `d`). Same element the repro script that found
 * this bug used. */
async function stableBox(page) {
  return page.evaluate(() => {
    const el = document.querySelector("svg.map-svg path.hit");
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, w: r.width, h: r.height };
  });
}

function boxesClose(a, b, eps = 1) {
  if (!a || !b) return false;
  return Math.abs(a.x - b.x) <= eps && Math.abs(a.y - b.y) <= eps && Math.abs(a.w - b.w) <= eps && Math.abs(a.h - b.h) <= eps;
}

/** The union of every district polygon's on-screen box -- the mainland
 * extent fit() actually frames (mainlandBounds(), see MapRenderer.ts), not
 * the full SVG viewport. Used to catch the repoKey-effect bug (fit() run
 * before MapRenderer.state pointed at the new document fit the PREVIOUS
 * repo's bounds, or the [0,0,1,1] sentinel on first mount, so the new
 * document's geometry painted far outside the pad, or even outside the
 * viewport entirely) on first load and on repo switch. */
async function districtUnionBox(page) {
  return page.evaluate(() => {
    const svg = document.querySelector("svg.map-svg");
    const origin = svg.getBoundingClientRect();
    const els = [...document.querySelectorAll('svg.map-svg path.hit[data-k^="d:"]')];
    if (els.length === 0) return null;
    let x0 = Infinity;
    let y0 = Infinity;
    let x1 = -Infinity;
    let y1 = -Infinity;
    for (const el of els) {
      const r = el.getBoundingClientRect();
      x0 = Math.min(x0, r.x);
      y0 = Math.min(y0, r.y);
      x1 = Math.max(x1, r.x + r.width);
      y1 = Math.max(y1, r.y + r.height);
    }
    return { x: x0 - origin.x, y: y0 - origin.y, w: x1 - x0, h: y1 - y0 };
  });
}

function fitsWithinPad(box, vw, vh, eps = 2) {
  const [left, top, right, bottom] = vw <= 820 ? [16, 110, vw - 16, vh - 158] : [24, 12, vw - 24, vh - 38];
  if (!box) return false;
  return box.x >= left - eps && box.y >= top - eps && box.x + box.w <= right + eps && box.y + box.h <= bottom + eps;
}

/** A file dot (or, at low zoom / dense districts, a symbol room) reasonably
 * central on screen -- central so it's never under the sidebar, the
 * selection panel, or the phone bottom drawer regardless of which corner
 * those occupy at this viewport size. Returns its data-k so later steps can
 * re-find the exact same element after a repaint. */
async function pickTarget(page, vw, vh) {
  return page.evaluate(
    ({ vw, vh }) => {
      const els = [...document.querySelectorAll('svg.map-svg [data-k^="f:"], svg.map-svg [data-k^="s:"]')];
      for (const el of els) {
        const r = el.getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0) continue;
        const cx = r.x + r.width / 2;
        const cy = r.y + r.height / 2;
        if (cx > vw * 0.25 && cx < vw * 0.75 && cy > vh * 0.3 && cy < vh * 0.65) {
          return { x: cx, y: cy, dataK: el.getAttribute("data-k") };
        }
      }
      return null;
    },
    { vw, vh },
  );
}

/** A point that resolves (via elementFromPoint) to something inside the map
 * SVG but with no data-k ancestor -- i.e. actually empty map, not a
 * district/file/symbol hit target, and not chrome (the sidebar, the
 * selection panel, search, footer, zoom controls) sitting on top of the SVG,
 * since none of those are descendants of svg.map-svg. Tries a spread of
 * candidate points because how much blank margin is on screen depends on
 * the map's silhouette and the current zoom. */
async function findEmptyPoint(page, vw, vh) {
  return page.evaluate(
    ({ vw, vh }) => {
      const fracs = [0.02, 0.06, 0.12, 0.5, 0.88, 0.94, 0.98];
      const yfracs = [0.03, 0.1, 0.5, 0.88, 0.96];
      for (const fy of yfracs) {
        for (const fx of fracs) {
          const x = Math.round(vw * fx);
          const y = Math.round(vh * fy);
          const el = document.elementFromPoint(x, y);
          if (!el) continue;
          if (!el.closest("svg.map-svg")) continue;
          if (el.closest("[data-k]")) continue;
          return [x, y];
        }
      }
      return null;
    },
    { vw, vh },
  );
}

async function readViewBox(page) {
  return page.evaluate(() => {
    const svg = document.querySelector("svg.map-svg");
    const [, , w, h] = (svg?.getAttribute("viewBox") || "0 0 0 0").split(" ").map(Number);
    return { w, h };
  });
}

async function readDot(page, dataK) {
  return page.evaluate((k) => {
    const el = document.querySelector(`[data-k="${CSS.escape(k)}"]`);
    if (!el) return null;
    // `r` is included so a caller can tell k apart from a pure pan: this
    // circle's radius is a monotonic function of k alone for a fixed file
    // (MapRenderer.paint()'s `r = ... * Math.sqrt(this.k / fitScale())`,
    // and fitScale() is constant for a fixed document/viewport) -- a pan
    // moves cx/cy but never touches r, so an unchanged r is direct evidence
    // k didn't move, independent of the cx/cy comparison already used for
    // "did the view move at all."
    return {
      cx: parseFloat(el.getAttribute("cx") ?? el.getAttribute("x")),
      cy: parseFloat(el.getAttribute("cy") ?? el.getAttribute("y")),
      r: el.hasAttribute("r") ? parseFloat(el.getAttribute("r")) : null,
    };
  }, dataK);
}

/** A wheel zoom, on every profile including the touch one -- not the
 * gesture a phone user would actually perform (that's a pinch or a
 * double-tap), but this script isn't testing gesture input, it's testing
 * that selection/layer changes and resizes leave an already-established
 * zoomed-in view alone, which doesn't care how the zoom got there. Two
 * touch-native alternatives were tried and dropped:
 *   - Double-tap zoom (MapRenderer.endPointer's TOUCH lastTap<300ms path):
 *     in this headless/CDP setup, the gap Chromium actually delivers the
 *     second tap's pointerdown at measured over 1s after the first tap's
 *     pointerup even with only an 80ms wait requested in between -- CDP
 *     touch-event dispatch latency here, not anything about the app -- so
 *     it blew the 300ms window every time and never zoomed at all.
 *   - A scripted two-finger pinch: MapRenderer's pinch math reads
 *     `this.tx`/`this.ty` (already updated by the previous frame) rather
 *     than the pinch's own anchor, and a single CDP touchmove event with
 *     both touch points moved together is delivered to the page as two
 *     separate PointerEvent dispatches (one per pointer id) -- so
 *     mid()/dist() briefly see one point already moved and one still stale,
 *     compounding into a large, unrealistic pan a real two-finger gesture
 *     never produces. Plausibly a real (if minor) bug in the pinch handler,
 *     but out of scope here -- a real finger's touchmove events aren't
 *     synthesized that way. */
async function zoomIn(page, cx, cy) {
  await page.mouse.move(cx, cy);
  for (let i = 0; i < 3; i++) {
    await page.mouse.wheel(0, -200);
    await page.waitForTimeout(50);
  }
  await page.waitForTimeout(650); // glide()/settle
}

/** Finds a point that's actually empty map to tap, zooming out one notch at
 * a time (via the real zoom-out button, not by resetting state) if the
 * current zoom leaves none. n8n's mainland is dense enough (thousands of
 * file dots -- see constants.ts's DOT_DENSITY_FLOOR comment) that the
 * zoomed-in view zoomIn() establishes can leave no blank pixel anywhere on a
 * 390px phone viewport; a real person in that situation zooms out a little
 * before they find water to tap, so this does the same rather than special-
 * casing the assertion away for dense maps. */
async function findEmptyPointWithZoomOut(page, vw, vh, maxAttempts = 3) {
  for (let i = 0; i <= maxAttempts; i++) {
    const pt = await findEmptyPoint(page, vw, vh);
    if (pt) return pt;
    if (i === maxAttempts) return null;
    await page.locator('button[aria-label="Zoom out"]').click();
    await page.waitForTimeout(300);
  }
  return null;
}

/** A point that resolves (via elementFromPoint) to a district polygon
 * itself -- large by construction, unlike a file dot (see
 * checkDragThresholdNoSelect's own comment for why that distinction
 * matters there). Same unobstructed-point algorithm checkViewerCards uses
 * for its own district tap, factored out since a second check needs it. */
async function pickDistrictPoint(page, hasTouch) {
  return page.evaluate((touch) => {
    const paths = [...document.querySelectorAll(touch ? 'svg.map-svg text.hit[data-k^="d:"]' : 'svg.map-svg path.hit[data-k^="d:"]')];
    for (const path of paths) {
      const rect = path.getBoundingClientRect();
      for (let yi = 1; yi < 6; yi++) {
        for (let xi = 1; xi < 6; xi++) {
          const x = rect.left + (rect.width * xi) / 6;
          const y = rect.top + (rect.height * yi) / 6;
          const hit = document.elementFromPoint(x, y)?.closest?.('[data-k^="d:"]');
          if (hit?.getAttribute("data-k") === path.getAttribute("data-k")) {
            return { x, y, key: path.getAttribute("data-k") };
          }
        }
      }
    }
    return null;
  }, hasTouch);
}

/** Two on-screen file dots, at least `minDist` CSS px apart, for measuring k
 * directly (CI review finding on the fullscreen check, see
 * checkFullscreenPreservesView's own comment): the on-screen distance
 * between two fixed world points is exactly `worldDistance * k`, independent
 * of tx/ty (pan) AND of fitScale() -- unlike a single dot's own radius
 * (`Math.sqrt(this.k / this.fitScale())` in MapRenderer.paint()), which
 * moves whenever fitScale() does, and fitScale() DOES change across a
 * fullscreen transition (removing/adding chrome changes the available
 * fitting box) even when k itself does not. `minDist` keeps the distance
 * comparison meaningful against the renderer's own `toFixed(1)` rounding on
 * cx/cy -- too close together and that rounding alone could swing the
 * measured distance by a percent or more. */
async function pickTwoOnScreenDots(page, vw, vh, minDist = 80) {
  return page.evaluate(
    ({ vw, vh, minDist }) => {
      const els = [...document.querySelectorAll('svg.map-svg circle.hit[data-k^="f:"]')];
      const candidates = [];
      for (const el of els) {
        const r = el.getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0) continue;
        const cx = r.x + r.width / 2;
        const cy = r.y + r.height / 2;
        if (cx > vw * 0.15 && cx < vw * 0.85 && cy > vh * 0.15 && cy < vh * 0.85) {
          candidates.push({ dataK: el.getAttribute("data-k"), cx, cy });
        }
        if (candidates.length >= 40) break;
      }
      for (let a = 0; a < candidates.length; a++) {
        for (let b = a + 1; b < candidates.length; b++) {
          const d = Math.hypot(candidates[a].cx - candidates[b].cx, candidates[a].cy - candidates[b].cy);
          if (d >= minDist) return [candidates[a].dataK, candidates[b].dataK];
        }
      }
      return null;
    },
    { vw, vh, minDist },
  );
}

async function tap(page, profile, x, y) {
  if (profile.hasTouch) await page.touchscreen.tap(x, y);
  else await page.mouse.click(x, y);
  await page.waitForTimeout(400);
}

async function runOne({ browser, base, slug, profile }) {
  const label = `${slug} / ${profile.name}`;
  console.log(`\n${label}`);
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1,
  });
  const page = await context.newPage();
  const { width: vw, height: vh } = profile.viewport;

  await page.goto(`${base}/${slug}`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(800); // initial fit()/auto-select glide

  // First load must fit the mainland inside the pad, not overflow the
  // viewport -- the repoKey-effect bug (fit() run against whatever document
  // MapRenderer.state still held, which on first mount is nothing at all).
  report(fitsWithinPad(await districtUnionBox(page), vw, vh), `${label}: first load fits within the pad`);

  // Desktop used to auto-select the top landmark on load (MapView.tsx).
  // Removed as a readable-overview PR review finding: once selection
  // started dimming non-neighbour files (that PR's own change), the
  // auto-select made a FRESH desktop load of django/django open with 685 of
  // 851 dots already dimmed -- the overview auto-obstructing itself before
  // a reader had asked for anything. This is the opposite of what this
  // check used to assert (desktop DID auto-select): now BOTH profiles must
  // open with no file selected and nothing dimmed -- phone always did,
  // desktop now matches it, and there is no longer a selection to "clear"
  // before establishing the zoomed-in baseline below (the old clear-it tap
  // that used to sit here is gone with it).
  // "Dimmed" here means the SELECTION dim/undim path specifically
  // (MapRenderer.paint()'s `baseOpacity = dim && !dim.has(i) ? 0.2 : 0.85`),
  // not #49's UNRELATED density-fade opacity -- a large, not-yet-fully-
  // revealed district (dify at fit zoom is exactly this: it never clears
  // DOT_DENSITY_FLOOR's fast path the way every acceptance fixture does)
  // legitimately draws dots at all kinds of partial opacity with no
  // selection active at all, via `factor` in that same expression.
  //
  // Two things were tried and failed here before landing on the check
  // below, kept as the record of why it looks like this:
  //   1. `fill-opacity < some threshold` -- 28 false positives on dify
  //      desktop, 570 on phone (the fade band is wide at fit zoom).
  //   2. `fill-opacity === "0.2"` exactly, over every file dot -- narrower,
  //      but the fade path computes `Math.round(0.85 * factor * 1000) /
  //      1000`, and SOME `factor` in a large district's continuous fade
  //      band rounds to exactly 0.2 too (0.85 * 0.235294... = 0.2 before
  //      rounding) -- 1 false positive on dify phone, not zero.
  // A landmark file's dot is exempt from the fade multiplier entirely:
  // `alwaysDrawn.has(i) ? 1 : this.dotFactor(i)` forces `factor = 1` for
  // every landmark unconditionally, so its fill-opacity is ALWAYS exactly
  // `baseOpacity` with no multiplication -- 0.2 there can only mean `dim`
  // was real. Scoping the exact-match check to landmark dots (fetched from
  // the map's own JSON, the same pattern checkSelectionDim below uses)
  // keeps the check meaningful while removing the collision entirely.
  const landmarkFiles = await page.evaluate(async (s) => {
    const res = await fetch(`/maps/${s}.json`);
    const doc = await res.json();
    return doc.L.map((l) => l[0]);
  }, slug);
  const openState = await page.evaluate((landmarks) => {
    const file = new URL(location.href).searchParams.get("file");
    const dimmed = landmarks.filter((i) => {
      const c = document.querySelector(`svg.map-svg circle.hit[data-k="f:${i}"]`);
      return c && c.getAttribute("fill-opacity") === "0.2";
    }).length;
    return { file, dimmed, landmarksOnScreen: landmarks.filter((i) => document.querySelector(`svg.map-svg circle.hit[data-k="f:${i}"]`)).length };
  }, landmarkFiles);
  report(!openState.file, `${label}: no auto-selected file on a fresh load`, JSON.stringify(openState));
  report(openState.dimmed === 0, `${label}: no landmark dot is selection-dimmed on a fresh load`, JSON.stringify(openState));

  const centre = [vw / 2, vh / 2];
  await zoomIn(page, centre[0], centre[1]);
  const baseline = await stableBox(page);
  report(!!baseline, `${label}: map rendered after zoom-in`);
  if (!baseline) {
    await context.close();
    return;
  }
  // The view this run currently expects to see unchanged. Starts as the
  // zoomed-in baseline; findEmptyPointWithZoomOut below may deliberately
  // zoom out to find blank map on a dense repo, which is itself a real view
  // change this script isn't testing -- so every check after that compares
  // against the view as of the last deliberate change, not the original
  // baseline.
  let current = baseline;

  // Step 1: tap a file dot.
  const target = await pickTarget(page, vw, vh);
  if (!target) {
    report(false, `${label}: tap a file dot`, "no dot found in the safe central region");
  } else {
    await tap(page, profile, target.x, target.y);
    report(boxesClose(current, await stableBox(page)), `${label}: view unchanged after tapping a file dot`);
  }

  // Step 2: tap empty map (deselect). May zoom out first to find blank map
  // (see findEmptyPointWithZoomOut).
  const empty2 = await findEmptyPointWithZoomOut(page, vw, vh);
  if (!empty2) {
    report(false, `${label}: tap empty map`, "no empty point found even after zooming out");
  } else {
    current = await stableBox(page);
    await tap(page, profile, empty2[0], empty2[1]);
    report(boxesClose(current, await stableBox(page)), `${label}: view unchanged after tapping empty map`);
  }

  // Step 3: change the layer via the UI (not the URL, the real control).
  if (profile.isMobile) {
    await page.locator('button[aria-label="Cycle layer"]').click();
  } else {
    await page.locator('[aria-label="Layer"] button', { hasText: "churn" }).click();
  }
  await page.waitForTimeout(200);
  report(boxesClose(current, await stableBox(page)), `${label}: view unchanged after changing layer`);

  // Step 4: resize the viewport. Assert the view shifts by EXACTLY half the
  // viewport delta -- the signature of "kept the centre point centred,
  // didn't re-fit" -- not just "assert nothing crashed."
  if (target?.dataK) {
    const before = await readDot(page, target.dataK);
    const vbBefore = await readViewBox(page);
    const newVw = vw - 160;
    const newVh = vh - 90;
    await page.setViewportSize({ width: newVw, height: newVh });
    await page.waitForTimeout(300);
    const vbAfter = await readViewBox(page);
    const after = await readDot(page, target.dataK);
    if (!before || !after) {
      report(false, `${label}: resize preserves centre`, "target dot missing after resize");
    } else {
      const expectDx = (vbAfter.w - vbBefore.w) / 2;
      const expectDy = (vbAfter.h - vbBefore.h) / 2;
      const actualDx = after.cx - before.cx;
      const actualDy = after.cy - before.cy;
      const ok = Math.abs(actualDx - expectDx) <= 1.5 && Math.abs(actualDy - expectDy) <= 1.5;
      report(
        ok,
        `${label}: resize preserves the centre point and does not re-fit`,
        ok ? undefined : `expected shift (${expectDx.toFixed(1)}, ${expectDy.toFixed(1)}), got (${actualDx.toFixed(1)}, ${actualDy.toFixed(1)})`,
      );
    }
    // restore the viewport for the remaining steps
    await page.setViewportSize({ width: vw, height: vh });
    await page.waitForTimeout(300);
  } else {
    report(false, `${label}: resize preserves centre`, "no target dot to track");
  }

  // Step 5: the fit button still fits (view actually changes back).
  const beforeFit = await stableBox(page);
  await page.locator('button[aria-label="Fit map"]').click();
  await page.waitForTimeout(650);
  const afterFit = await stableBox(page);
  report(!boxesClose(beforeFit, afterFit, 3), `${label}: fit button still re-fits the view`);

  await context.close();
}

async function checkRepoSwitch(browser, base, profile) {
  const label = `repo switching still fits / ${profile.name}`;
  console.log(`\n${label}`);
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1,
  });
  const page = await context.newPage();
  const { width: vw, height: vh } = profile.viewport;
  await page.goto(`${base}/django/django`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(800);
  const before = await stableBox(page);

  // TopBar's <option value> is the bare "owner/repo" slug regardless of how
  // the visible label is formatted (it appends " · N files" on desktop).
  // A plain CSS locator, not getByLabel: the map SVG's own long aria-label
  // ("Pannable, zoomable map...") confused Playwright's fuzzy accessible-name
  // matching into treating getByLabel("Repository") as ambiguous.
  const available = await page.locator('select[aria-label="Repository"] option').evaluateAll((options) =>
    options.map((option) => option.value),
  );
  await page.locator('select[aria-label="Repository"]').selectOption(
    available.includes("langgenius/dify") ? "langgenius/dify" : "prometheus/prometheus",
  );
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(1000);
  const after = await stableBox(page);
  report(!!after, `${label}: new repo rendered`);
  report(!boxesClose(before, after, 3), `${label}: new repo's view differs from the old repo's (it fit its own bounds, not a stale one)`);
  // The bug this reviews: fit() ran against whatever document
  // MapRenderer.state still held (the OLD repo, one commit behind), so n8n's
  // geometry painted at django's transform -- massively overflowing the
  // viewport rather than sitting inside the pad.
  report(fitsWithinPad(await districtUnionBox(page), vw, vh), `${label}: new repo fits within the pad after switching`);
  await context.close();
}

// readable-overview PR: "when hovering an isolated area of a district, only
// that area is highlighted, not the whole district" -- a district whose
// `blob` has several polygons draws one <path> per polygon (plus its label)
// all sharing data-k="d:N"; the fix highlights every element for that key,
// not just the one the pointer resolved to. Wants dify per the user's
// report, but dify's OWN build (every worktree's copy of it checked, none
// built with tolmap build for this task) happens to have ZERO multi-polygon
// districts -- every one of its districts is a single contiguous blob, so
// there is nothing there to exercise this on. django DOES have one
// (district "sessions", 3 polygons, found by exactly the districts[d].blob
// .length > 1 scan the task asked for) and is well inside the browser-size
// limit, so this runs there instead; the fix itself is generic (keyElements
// is built from every element carrying a data-k, not district-specific), so
// this is still a real exercise of the code path dify would use too.
async function checkMultiPolygonHover(browser, base) {
  const label = "multi-polygon district hover (django, desktop -- dify has none, see comment)";
  console.log(`\n${label}`);
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  const page = await context.newPage();
  await page.goto(`${base}/django/django`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);

  const districtId = await page.evaluate(async () => {
    const res = await fetch("/maps/django/django.json");
    const doc = await res.json();
    for (const k in doc.districts) {
      if (doc.districts[k].blob.length > 1) return k;
    }
    return null;
  });
  if (districtId == null) {
    report(false, `${label}: no multi-polygon district found in django either`, "fixture data may have changed");
    await context.close();
    return;
  }

  const before = await page.evaluate((d) => {
    const els = [...document.querySelectorAll(`svg.map-svg [data-k="d:${d}"]`)];
    return els.length;
  }, districtId);
  report(before > 1, `${label}: district d:${districtId} has multiple elements sharing its key`, `found ${before}`);

  const polyCentre = await page.evaluate((d) => {
    for (const el of document.querySelectorAll(`svg.map-svg path.hit[data-k="d:${d}"]`)) {
      const r = el.getBoundingClientRect();
      for (let yi = 1; yi < 8; yi++) for (let xi = 1; xi < 8; xi++) {
        const x = r.x + r.width * xi / 8, y = r.y + r.height * yi / 8;
        if (document.elementFromPoint(x, y)?.getAttribute("data-k") === `d:${d}`) return { x, y };
      }
    }
    return null;
  }, districtId);
  if (!polyCentre) {
    report(false, `${label}: no polygon path found for d:${districtId}`);
    await context.close();
    return;
  }
  await page.mouse.move(polyCentre.x, polyCentre.y, { steps: 4 });
  await page.waitForTimeout(150);
  const after = await page.evaluate((d) => {
    const els = [...document.querySelectorAll(`svg.map-svg [data-k="d:${d}"]`)];
    return { total: els.length, hovered: els.filter((e) => e.classList.contains("hovered")).length };
  }, districtId);
  report(after.total === before && after.hovered === after.total, `${label}: hovering one polygon highlights every polygon (and the label)`, JSON.stringify(after));
  await context.close();
}

// readable-overview PR: selecting a file (no symbol, no route) dims files
// it has no direct import edge to and keeps its neighbours at full opacity
// -- the same dim/highlight treatment hovering already had, extended to a
// persistent selection (tap-select included, so this runs on phone too).
async function checkSelectionDim(browser, base, profile) {
  const label = `selection dim/highlight (dify) / ${profile.name}`;
  console.log(`\n${label}`);
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1,
  });
  const page = await context.newPage();
  await page.goto(`${base}/langgenius/dify`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);

  // A file with both in- and out-edges, so the dim/highlight split is
  // actually exercised in both directions, picked from the map's own data
  // rather than hardcoded (this build's file indices aren't guaranteed
  // stable across a corpus refresh).
  const target = await page.evaluate(async () => {
    const res = await fetch("/maps/langgenius/dify.json");
    const doc = await res.json();
    const outDeg = new Map();
    const inDeg = new Map();
    for (const [a, b] of doc.E) {
      outDeg.set(a, (outDeg.get(a) || 0) + 1);
      inDeg.set(b, (inDeg.get(b) || 0) + 1);
    }
    for (const [i, od] of outDeg) {
      if (od > 2 && (inDeg.get(i) || 0) > 2) return { i, file: doc.F[i], neighbour: [...outDeg.keys()][0] === i ? null : i };
    }
    return null;
  });
  if (!target) {
    report(false, `${label}: no file with in+out edges found`);
    await context.close();
    return;
  }

  await page.goto(`${base}/langgenius/dify?geo=r&layer=d&file=${encodeURIComponent(target.file)}`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(900);

  const result = await page.evaluate((i) => {
    const circles = [...document.querySelectorAll('svg.map-svg circle.hit[data-k^="f:"]')];
    const nonSelected = circles.filter((c) => c.getAttribute("data-k") !== `f:${i}`);
    // Exact "0.2" only (see the fresh-load check's own comment above for
    // why a threshold isn't safe on dify specifically: #49's UNRELATED
    // density-fade opacity can coincidentally sit under any threshold too).
    const dimmed = nonSelected.filter((c) => c.getAttribute("fill-opacity") === "0.2");
    // A neighbour ring (MapRenderer.ring(), var(--hot) or var(--cold) stroke)
    // marks a file that's connected -- find one and check ITS dot opacity,
    // which should read as full strength (alwaysDrawn), not dimmed.
    const rings = [...document.querySelectorAll("svg.map-svg circle[stroke]:not(.hit)")];
    let neighbourFull = null;
    for (const ring of rings) {
      const cx = parseFloat(ring.getAttribute("cx"));
      const cy = parseFloat(ring.getAttribute("cy"));
      const dot = circles.find((c) => Math.abs(parseFloat(c.getAttribute("cx")) - cx) < 1 && Math.abs(parseFloat(c.getAttribute("cy")) - cy) < 1);
      if (dot && dot.getAttribute("data-k") !== `f:${i}`) {
        neighbourFull = parseFloat(dot.getAttribute("fill-opacity"));
        break;
      }
    }
    return { totalCircles: circles.length, dimmedCount: dimmed.length, neighbourFull };
  }, target.i);
  report(result.dimmedCount > 0, `${label}: at least one non-neighbour dot is dimmed`, JSON.stringify(result));
  report(result.neighbourFull != null && result.neighbourFull >= 0.8, `${label}: a ringed neighbour's dot stays at full opacity`, JSON.stringify(result));
  await context.close();
}

// Issue #74: all path-derived UI and the folder dim route, exercised through
// the real URL and controls at both supported interaction profiles. Django is
// deliberately used here: every dot is above #49's density floor at fit, so
// exact 0.2 opacity means the directory dim set and cannot collide with a
// density-fade value the way it can on the medium fixture.
async function checkPackageLayout(browser, base, profile) {
  const label = `package layout overlays (django) / ${profile.name}`;
  console.log(`\n${label}`);
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1,
  });
  const page = await context.newPage();
  await page.goto(`${base}/django/django?geo=r&layer=p`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForSelector("[data-package-legend]");
  await page.waitForTimeout(700);

  const initialLegend = await page.locator("[data-package-legend]").evaluate((element) => {
    const box = element.getBoundingClientRect();
    const zoomBox = document.querySelector('button[aria-label="Zoom in"]')?.parentElement?.getBoundingClientRect();
    const root = document.documentElement;
    const previousTheme = root.getAttribute("data-theme");
    const readPalette = (theme) => {
      root.setAttribute("data-theme", theme);
      const css = getComputedStyle(root);
      return Array.from({ length: 10 }, (_, i) => css.getPropertyValue(`--p${i}`).trim());
    };
    const light = readPalette("light");
    const dark = readPalette("dark");
    const rgb = (hex) => [1, 3, 5].map((offset) => Number.parseInt(hex.slice(offset, offset + 2), 16));
    const distance = (a, b) => Math.hypot(...rgb(a).map((channel, i) => channel - rgb(b)[i]));
    const firstPairStrongest = (palette) => {
      const adjacent = palette.slice(0, -1).map((color, i) => distance(color, palette[i + 1]));
      return adjacent[0] === Math.max(...adjacent);
    };
    if (previousTheme == null) root.removeAttribute("data-theme");
    else root.setAttribute("data-theme", previousTheme);
    const overlapsZoom =
      !!zoomBox && box.left < zoomBox.right && box.right > zoomBox.left && box.top < zoomBox.bottom && box.bottom > zoomBox.top;
    return {
      expanded: element.getAttribute("data-package-expanded") === "true",
      rows: element.querySelectorAll("[data-package-groups] [style*='background']").length,
      text: element.textContent ?? "",
      height: box.height,
      visible: box.width > 0,
      overlapsZoom,
      light,
      dark,
      firstPairStrongest: firstPairStrongest(light) && firstPairStrongest(dark),
    };
  });
  report(initialLegend.visible && !initialLegend.overlapsZoom, `${label}: package legend does not overlap zoom controls`, JSON.stringify(initialLegend));
  report(
    new Set(initialLegend.light).size === 10 &&
      new Set(initialLegend.dark).size === 10 &&
      initialLegend.light.every((color, i) => color !== initialLegend.dark[i]) &&
      initialLegend.firstPairStrongest,
    `${label}: package palette has ten distinct light and dark colours`,
    JSON.stringify({ light: initialLegend.light, dark: initialLegend.dark }),
  );
  if (profile.isMobile) {
    report(
      !initialLegend.expanded && initialLegend.rows === 0 && initialLegend.height < 40 && /packages\s*·\s*depth\s+\d/.test(initialLegend.text),
      `${label}: phone package legend starts as a one-line chip`,
      JSON.stringify(initialLegend),
    );
    await page.getByRole("button", { name: "Expand package legend" }).click();
    await page.waitForSelector('[data-package-legend][data-package-expanded="true"]');
  } else {
    report(initialLegend.expanded, `${label}: desktop package legend starts expanded`, JSON.stringify(initialLegend));
  }

  const legend = await page.locator("[data-package-legend]").evaluate((element) => ({
    rows: element.querySelectorAll("[data-package-groups] [style*='background']").length,
    depth: element.querySelector("[data-package-depth]")?.textContent ?? "",
    visible: element.getBoundingClientRect().width > 0,
  }));
  report(legend.visible && legend.rows >= 3, `${label}: layer p renders a package legend`, JSON.stringify(legend));
  report(new URL(page.url()).searchParams.get("depth") == null, `${label}: automatic depth is omitted from the URL`);

  const plus = page.locator('button[aria-label="Increase package depth"]');
  if (await plus.isEnabled()) {
    const beforeDepth = await stableBox(page);
    await plus.click();
    await page.waitForTimeout(250);
    report(new URL(page.url()).searchParams.has("depth"), `${label}: an overridden package depth is in the URL`);
    report(boxesClose(beforeDepth, await stableBox(page)), `${label}: changing package depth does not re-fit the view`);
  } else {
    report(false, `${label}: package depth has an available override`);
  }
  if (profile.isMobile) {
    await page.getByRole("button", { name: "Collapse package legend" }).click();
    const collapsed = await page.locator("[data-package-legend]").getAttribute("data-package-expanded");
    report(collapsed === "false", `${label}: expanded phone package legend collapses again`);
  }

  const target = await page.evaluate(async () => {
    const response = await fetch("/maps/django/django.json");
    const doc = await response.json();
    const counts = new Map();
    for (let i = 0; i < doc.F.length; i++) {
      const parts = doc.F[i].split("/").slice(0, -1);
      for (let depth = 1; depth <= parts.length; depth++) {
        const path = parts.slice(0, depth).join("/");
        const row = counts.get(path) || { path, files: [] };
        row.files.push(i);
        counts.set(path, row);
      }
    }
    const folder = [...counts.values()]
      .filter((row) => row.files.length >= 3 && row.files.length <= doc.F.length / 2)
      .sort((a, b) => b.files.length - a.files.length || a.path.localeCompare(b.path))[0];
    const district = Object.keys(doc.districts)
      .map(Number)
      .sort((a, b) => doc.districts[String(b)].size - doc.districts[String(a)].size)[0];
    return { dir: folder?.path ?? null, district };
  });
  if (!target.dir) {
    report(false, `${label}: fixture has a usable directory`);
    await context.close();
    return;
  }

  if (profile.isMobile) {
    const header = page.locator("[data-selection-panel] > div").first();
    const box = await header.boundingBox();
    if (box) await tap(page, profile, box.x + box.width / 2, box.y + box.height / 2);
  }
  const treeRow = page.locator('[data-folder-browser] button[data-folder-path]').first();
  const rootPath = await treeRow.getAttribute("data-folder-path");
  const rootCount = await page.evaluate(async (path) => {
    const doc = await (await fetch("/maps/django/django.json")).json();
    return { total: doc.F.length, count: doc.F.filter((file) => file.startsWith(`${path}/`)).length };
  }, rootPath);
  const shareLabel = (count, total) => {
    const percent = (count / total) * 100;
    return `${percent < 10 ? percent.toFixed(1) : Math.round(percent)}%`;
  };
  report(
    (await treeRow.locator("xpath=..").innerText()).includes(shareLabel(rootCount.count, rootCount.total)),
    `${label}: folder tree row shows its share of repository files`,
  );
  const rootExpand = page.getByRole("button", { name: `Expand ${rootPath}` });
  if (await rootExpand.count()) {
    await rootExpand.click();
    const child = page.locator('[data-folder-browser] button[data-folder-path]').nth(1);
    const childPath = await child.getAttribute("data-folder-path");
    const childCount = await page.evaluate(async (path) => {
      const doc = await (await fetch("/maps/django/django.json")).json();
      return doc.F.filter((file) => file.startsWith(`${path}/`)).length;
    }, childPath);
    report(
      (await child.locator("xpath=..").innerText()).includes(shareLabel(childCount, rootCount.total)),
      `${label}: child folder share uses repository total`,
    );
  }
  const beforeFolder = await stableBox(page);
  const input = page.locator('input[aria-label="Filter folder paths"]');
  await input.fill(target.dir);
  const filteredRow = page.locator(`[data-folder-browser] button[data-folder-path="${target.dir}"]`);
  const filteredCount = await page.evaluate(async (path) => {
    const doc = await (await fetch("/maps/django/django.json")).json();
    return doc.F.filter((file) => file.startsWith(`${path}/`)).length;
  }, target.dir);
  report(
    (await filteredRow.innerText()).includes(shareLabel(filteredCount, rootCount.total)),
    `${label}: folder filter result shows repository share`,
  );
  await input.press("Enter");
  await page.waitForFunction((dir) => new URL(location.href).searchParams.get("dir") === dir, target.dir);
  await page.waitForTimeout(250);
  report(boxesClose(beforeFolder, await stableBox(page)), `${label}: picking a folder does not re-fit the view`);

  const dim = await page.evaluate((dir) => {
    const circles = [...document.querySelectorAll('svg.map-svg circle.hit[data-k^="f:"]')];
    let insideFull = 0;
    let outsideDim = 0;
    for (const circle of circles) {
      const index = Number(circle.getAttribute("data-k").slice(2));
      const title = circle.querySelector("title")?.textContent?.split("\n")[0] ?? "";
      const inside = title.startsWith(`${dir}/`);
      const opacity = circle.getAttribute("fill-opacity");
      if (inside && opacity !== "0.2") insideFull++;
      if (!inside && opacity === "0.2") outsideDim++;
      if (!Number.isInteger(index)) return { insideFull: 0, outsideDim: 0 };
    }
    const outlined = [...document.querySelectorAll("svg.map-svg [data-folder-highlight]")];
    const outlinesInside = outlined.every((ring) => {
      const index = Number(ring.getAttribute("data-folder-highlight"));
      return Number.isInteger(index) && circles.some((circle) => circle.getAttribute("data-k") === `f:${index}` && (circle.querySelector("title")?.textContent ?? "").startsWith(`${dir}/`));
    });
    return { insideFull, outsideDim, outlined: outlined.length, outlinesInside };
  }, target.dir);
  report(dim.insideFull > 0 && dim.outsideDim > 0, `${label}: ?dir= keeps folder files bright and dims files outside`, JSON.stringify(dim));
  report(dim.outlined > 0 && dim.outlinesInside, `${label}: highlighted folder files have a contrasting outline`, JSON.stringify(dim));

  const empty = await findEmptyPoint(page, profile.viewport.width, profile.viewport.height);
  if (empty) {
    await tap(page, profile, empty[0], empty[1]);
    report(!new URL(page.url()).searchParams.has("dir"), `${label}: an empty-map tap clears the folder highlight`);
  } else {
    report(false, `${label}: an empty-map tap clears the folder highlight`, "no empty map point found");
  }

  await page.goto(`${base}/django/django?geo=r&layer=d&d=${target.district}`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(600);
  if (profile.isMobile) {
    const header = page.locator("[data-selection-panel] > div").first();
    const box = await header.boundingBox();
    if (box) await tap(page, profile, box.x + box.width / 2, box.y + box.height / 2);
  }
  const panel = page.locator("[data-selection-panel]");
  const folderToggle = panel.locator("[data-district-folders-toggle]");
  const fileToggle = panel.locator("[data-district-files-toggle]");
  report(
    (await folderToggle.getAttribute("aria-expanded")) === "false" &&
      (await fileToggle.getAttribute("aria-expanded")) === "false" &&
      (await panel.locator("[data-district-path]").count()) === 0,
    `${label}: district folders and key files start collapsed`,
  );
  if (profile.isMobile) {
    const height = await panel.evaluate((element) => element.getBoundingClientRect().height);
    report(height <= 844 * 0.22, `${label}: collapsed phone district card fits 22% of viewport`, `height=${height}`);
  }
  const beforeZoom = await stableBox(page);
  await panel.getByRole("button", { name: "Zoom to district" }).click();
  await page.waitForTimeout(650);
  report(!boxesClose(beforeZoom, await stableBox(page)), `${label}: district zoom icon changes map view`);
  await fileToggle.click();
  const fileRows = panel.locator("[data-district-key-file]");
  report(
    (await fileRows.count()) > 0 && (await fileRows.first().innerText()).includes("lines"),
    `${label}: expanded key files label line counts`,
  );
  await fileToggle.click();
  await folderToggle.click();
  const pathRows = page.locator("[data-district-path]");
  const rowCount = await pathRows.count();
  report(rowCount > 0 && (await folderToggle.getAttribute("aria-expanded")) === "true", `${label}: expanded folders show path breakdown`, `rows=${rowCount}`);
  const neighbours = panel.locator("[data-neighbour-district]");
  if (await neighbours.count()) {
    const neighbourId = await neighbours.first().getAttribute("data-neighbour-district");
    await neighbours.first().click();
    report(
      new URL(page.url()).searchParams.get("d") === neighbourId && (await folderToggle.getAttribute("aria-expanded")) === "true",
      `${label}: neighbour tap selects district and preserves folder expansion`,
    );
  } else {
    report(false, `${label}: selected district has a tappable neighbour`);
  }
  const selectedPathRows = page.locator("[data-district-path]");
  if (await selectedPathRows.count()) {
    const first = selectedPathRows.first();
    const path = await first.getAttribute("data-district-path");
    const box = await first.boundingBox();
    if (box) await tap(page, profile, box.x + box.width / 2, box.y + box.height / 2);
    report(
      !!path && new URL(page.url()).searchParams.get("dir") === path,
      `${label}: tapping a district path applies the folder highlight`,
      `expected=${path} url=${page.url()}`,
    );
  }
  await context.close();
}

// Review regression for district refinement. A single docker file makes the
// district-wide common prefix empty; the 886-file workflow branch still has
// to refine past web/ and its single-child chain before reaching five rows.
// A wide workflow fan-out must instead keep its parent: showing only nodes/
// would hide about half of the district in `other`.
// Intercepting one small synthetic map keeps this an end-to-end check of the
// actual memoised TypeScript derivation and district-card rendering without
// adding a second implementation of the algorithm to this script.
async function checkDistrictRefinement(browser, base) {
  const label = "district path iterative refinement / synthetic";
  console.log(`\n${label}`);
  const seed = await (await fetch(`${base}/maps/django/django.json`)).json();
  const balancedPaths = Array.from(
    { length: 886 },
    (_, i) => `web/app/components/workflow/${["a", "b", "c"][i % 3]}/file-${i}.ts`,
  ).concat("web/types/index.ts", "docker/compose.yml");
  const widePaths = Array.from({ length: 434 }, (_, i) => `web/app/components/workflow/nodes/file-${i}.ts`);
  for (let child = 0; child < 20; child++) {
    for (let i = 0; i < (child < 10 ? 23 : 22); i++) {
      widePaths.push(`web/app/components/workflow/sub${String(child).padStart(2, "0")}/file-${i}.ts`);
    }
  }
  widePaths.push("web/app/(commonLayout)/index.ts", "web/app/(humanInputLayout)/form/[token]/index.ts", "docker/compose.yml");
  const context = await browser.newContext({ viewport: { width: 1000, height: 700 } });
  const page = await context.newPage();
  let paths = balancedPaths;
  await page.route("**/maps/synthetic/refinement.json", (route) => route.fulfill({
    json: {
      ...seed,
      repo: "synthetic/refinement",
      names: { 0: "workflow" },
      districts: { 0: { ...seed.districts["0"], size: paths.length } },
      F: paths,
      N: paths.map((_, i) => {
        const row = [...seed.N[i % seed.N.length]];
        row[0] = 0;
        return row;
      }),
      E: [], L: [], S: {}, U: {}, roads: [],
    },
  }));
  const open = () => page.goto(`${base}/synthetic/refinement?geo=r&layer=d&d=0`, { waitUntil: "domcontentloaded" });
  await open();
  await page.waitForSelector("[data-district-path-breakdown]");
  report((await page.locator("[data-district-path]").count()) === 0, `${label}: breakdown is collapsed by default`);
  report(!(await page.locator("[data-district-summary]").innerText()).includes("mostly"), `${label}: no mostly line below 40%`);
  await page.locator("[data-district-folders-toggle]").click();
  const rows = await page.locator("[data-district-path]").evaluateAll((elements) =>
    elements.map((element) => ({ path: element.getAttribute("data-district-path"), text: element.textContent ?? "" })),
  );
  const rowPaths = rows.map((row) => row.path);
  report(
    rows.length === 5 &&
      ["a", "b", "c"].every((branch) => rowPaths.includes(`web/app/components/workflow/${branch}`)) &&
      rowPaths.includes("web/types") &&
      rowPaths.includes("docker"),
    `${label}: dominant web branch refines to workflow children`,
    JSON.stringify(rows),
  );
  report(
    rows.filter((row) => /\(1 file\)/.test(row.text)).length === 2 && rows.every((row) => !/\(1 files\)/.test(row.text)),
    `${label}: singular file counts use “file”`,
    JSON.stringify(rows),
  );

  paths = widePaths;
  await open();
  await page.waitForSelector("[data-district-path-breakdown]");
  report(
    (await page.locator("[data-district-summary]").innerText()).includes("mostly web/app/components/workflow/"),
    `${label}: wide workflow fan-out keeps the parent as mostly`,
  );
  await page.locator("[data-district-folders-toggle]").click();
  const wideRows = await page.locator("[data-district-path-breakdown] > :not([data-district-folders-toggle])").evaluateAll((elements) =>
    elements.map((element) => ({
      path: element.getAttribute("data-district-path"),
      count: Number(element.textContent?.match(/\((\d+) files?\)/)?.[1]),
      text: element.textContent ?? "",
    })),
  );
  const largestNamed = Math.max(...wideRows.filter((row) => row.path != null).map((row) => row.count));
  const other = wideRows.find((row) => row.path == null);
  report(
    wideRows[0]?.path === "web/app/components/workflow" && wideRows[0].count === 884 &&
      wideRows.every((row, i) => i === 0 || wideRows[i - 1].count >= row.count) &&
      (!other || other.count <= largestNamed),
    `${label}: wide split preserves the 884-file parent and ranks all rows`,
    JSON.stringify(wideRows),
  );
  await context.close();
}

async function checkDifyDistrictSummary(browser, base, profile) {
  const label = `workflow district summary (dify) / ${profile.name}`;
  console.log(`\n${label}`);
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1,
  });
  const page = await context.newPage();
  const doc = await (await (await context.request.get(`${base}/maps/langgenius/dify.json`)).json());
  const d = doc.N[doc.F.indexOf("web/app/components/workflow/types.ts")][0];
  const workflowCount = doc.F.filter((path, i) => doc.N[i][0] === d && path.startsWith("web/app/components/workflow/")).length;
  await page.goto(`${base}/langgenius/dify?geo=r&layer=d&d=${d}`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("[data-selection-panel] h3");
  if (profile.isMobile) await page.locator("[data-selection-panel] > div").first().tap();
  await page.waitForSelector("[data-district-summary]");
  const panel = page.locator("[data-selection-panel]");
  const text = await panel.innerText();
  report(
    text.includes(`${doc.districts[String(d)].size} files`) &&
      text.includes("mostly web/app/components/workflow/") &&
      /folders \(\d+\)/.test(text) &&
      text.includes("key files (6)"),
    `${label}: compact summary and folder count match the fixture`,
    text,
  );
  report(
    (await panel.locator("[data-neighbour-district]").count()) === 2 &&
      !text.includes("landmarks") &&
      (await panel.getByRole("button", { name: "Zoom to district" }).count()) === 1,
    `${label}: two tappable neighbours and header zoom replace the old rows`,
  );
  await panel.locator("[data-district-folders-toggle]").click();
  const rows = await panel.locator("[data-district-path-breakdown] > :not([data-district-folders-toggle])").evaluateAll((elements) =>
    elements.map((element) => ({
      path: element.getAttribute("data-district-path"),
      count: Number(element.textContent?.match(/\((\d+) files?\)/)?.[1]),
    })),
  );
  const named = rows.filter((row) => row.path != null);
  const largestNamed = Math.max(...named.map((row) => row.count));
  report(
    rows[0]?.path === "web/app/components/workflow" && rows[0].count === workflowCount &&
      named.every((row, i) => i === 0 || named[i - 1].count >= row.count) &&
      rows.at(-1)?.path == null && rows.at(-1).count <= largestNamed,
    `${label}: folder rows are ranked, other is last and below the largest folder`,
    JSON.stringify(rows),
  );
  await context.close();
}

// A mixed mainland/island folder must not become an island-fade exception.
// Compare the opening pin ranks before and after applying api/: the folder
// outline and dim are allowed to change, but no previously hidden pin may
// appear. (An all-island folder is intentionally exempted in MapRenderer so
// a legitimate highlight cannot produce an empty view.)
async function checkFolderIslandFade(browser, base, profile) {
  const label = `folder highlight preserves island fade (dify) / ${profile.name}`;
  console.log(`\n${label}`);
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1,
  });
  const page = await context.newPage();
  const visiblePinRanks = () =>
    page.locator('svg.map-svg g.hit[data-k^="f:"] > text').evaluateAll((elements) => elements.map((element) => element.textContent));

  await page.goto(`${base}/langgenius/dify?geo=r&layer=p`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(2200);
  if (profile.isMobile) await page.getByRole("button", { name: "Expand package legend" }).click();
  const legendText = await page.locator("[data-package-legend]").innerText();
  report(
    legendText.includes("(repo root)") && !legendText.includes("(root)/"),
    `${label}: repository-root package uses the explicit legend label`,
    legendText,
  );
  const before = await visiblePinRanks();
  await page.goto(`${base}/langgenius/dify?geo=r&layer=p&dir=api`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg [data-folder-highlight]");
  await page.waitForTimeout(2200);
  const after = await visiblePinRanks();
  const extra = after.filter((rank) => !before.includes(rank));
  const folderClasses = await page.evaluate(async () => {
    const doc = await (await fetch("/maps/langgenius/dify.json")).json();
    return [...new Set(doc.F.map((path, i) => (path.startsWith("api/") ? doc.districts[String(doc.N[i][0])].class : null)).filter(Boolean))];
  });
  report(
    folderClasses.includes("mainland") && folderClasses.includes("island"),
    `${label}: api/ exercises a mixed mainland/island folder`,
    JSON.stringify(folderClasses),
  );
  report(extra.length === 0, `${label}: folder highlight reveals no extra opening-zoom pins`, JSON.stringify({ before, after, extra }));
  await context.close();
}

// CLAUDE.md's standing viewer contract: the navigation lists remain
// populated, and the three tap paths that open detail (district polygon,
// file dot, symbol row) work on both a mouse viewport and a coarse-pointer
// phone. Kept here beside #74's additions because the always-present empty
// Folders panel changes the chrome around those same taps.
async function checkViewerCards(browser, base, profile) {
  const label = `viewer cards and directory lists (django) / ${profile.name}`;
  console.log(`\n${label}`);
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1,
  });
  const page = await context.newPage();
  await page.goto(`${base}/django/django`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);

  const fixture = await page.evaluate(async () => {
    const response = await fetch("/maps/django/django.json");
    const doc = await response.json();
    return {
      doc,
      firstDistrictName: doc.names[Object.keys(doc.districts)[0]],
      firstLandmarkPath: doc.F[doc.L[0][0]],
    };
  });
  const sidebarText = await page.locator("aside").first().textContent();
  report(
    sidebarText?.includes(fixture.firstDistrictName) && sidebarText?.includes(fixture.firstLandmarkPath.split("/").pop()),
    `${label}: districts are named and landmarks are listed`,
  );

  const districtTarget = await page.evaluate((touch) => {
    const paths = [...document.querySelectorAll(touch ? 'svg.map-svg text.hit[data-k^="d:"]' : 'svg.map-svg path.hit[data-k^="d:"]')];
    for (const path of paths) {
      const rect = path.getBoundingClientRect();
      for (let yi = 1; yi < 6; yi++) {
        for (let xi = 1; xi < 6; xi++) {
          const x = rect.left + (rect.width * xi) / 6;
          const y = rect.top + (rect.height * yi) / 6;
          const hit = document.elementFromPoint(x, y)?.closest?.('[data-k^="d:"]');
          if (hit?.getAttribute("data-k") === path.getAttribute("data-k")) {
            return { x, y, key: path.getAttribute("data-k") };
          }
        }
      }
    }
    return null;
  }, profile.hasTouch);
  if (districtTarget) {
    await tap(page, profile, districtTarget.x, districtTarget.y);
    const districtId = districtTarget.key.split(":")[1];
    report(
      new URL(page.url()).searchParams.get("d") === districtId && (await page.locator("[data-selection-panel] h3").count()) > 0,
      `${label}: tapping a district produces its card`,
    );
  } else {
    report(false, `${label}: tapping a district produces its card`, "no unobstructed district point found");
  }

  if (profile.isMobile && districtTarget) {
    const header = page.locator("[data-selection-panel] > div").first();
    const box = await header.boundingBox();
    if (box) await tap(page, profile, box.x + box.width / 2, box.y + box.height / 2);
  }
  const fileTarget = await page.evaluate((doc) => {
    const circles = [...document.querySelectorAll('svg.map-svg circle.hit[data-k^="f:"]')];
    for (const circle of circles) {
      const index = Number(circle.getAttribute("data-k").slice(2));
      if (!doc.S?.[String(index)]?.length) continue;
      const rect = circle.getBoundingClientRect();
      const x = rect.left + rect.width / 2;
      const y = rect.top + rect.height / 2;
      const hitKey = document.elementFromPoint(x, y)?.closest?.("[data-k]")?.getAttribute("data-k");
      if (hitKey !== `f:${index}`) continue;
      if (x > innerWidth * 0.22 && x < innerWidth * 0.78 && y > innerHeight * 0.18 && y < innerHeight * 0.72) {
        return { x, y, index, file: doc.F[index] };
      }
    }
    return null;
  }, fixture.doc);
  if (fileTarget) {
    await tap(page, profile, fileTarget.x, fileTarget.y);
    report(
      new URL(page.url()).searchParams.get("file") === fileTarget.file && (await page.locator("[data-selection-panel]").count()) === 1,
      `${label}: tapping a file produces its card`,
      `expected=${fileTarget.file} url=${page.url()}`,
    );
    const symbol = page.locator("button[data-symbol-row]").first();
    if ((await symbol.count()) > 0) {
      const key = await symbol.getAttribute("data-symbol-row");
      if (profile.hasTouch) await symbol.tap();
      else await symbol.click();
      await page.waitForTimeout(400);
      report(
        !!key && new URL(page.url()).searchParams.get("sym") === key.split(":")[1],
        `${label}: tapping a symbol produces its card`,
      );
    } else {
      report(false, `${label}: tapping a symbol produces its card`, "selected file has no visible symbol row");
    }
  } else {
    report(false, `${label}: tapping a file produces its card`, "no on-screen file with symbols found");
    report(false, `${label}: tapping a symbol produces its card`, "no file card to tap from");
  }
  await context.close();
}

async function visibleDots(page) {
  return page.locator('svg.map-svg circle.hit[data-k^="f:"]').evaluateAll((elements) =>
    Object.fromEntries(elements.map((el) => [el.getAttribute("data-k"), [el.getAttribute("cx"), el.getAttribute("cy")]])));
}

async function fitFraming(page, doc) {
  return page.evaluate((map) => {
    const svg = document.querySelector("svg.map-svg");
    const { width: vw, height: vh } = svg.getBoundingClientRect();
    const rect = vw <= 820 ? [16, 110, vw - 16, vh - 158] : [24, 12, vw - 24, vh - 38];
    const b = [Infinity, Infinity, -Infinity, -Infinity];
    const add = ([x, y]) => { b[0] = Math.min(b[0], x); b[1] = Math.min(b[1], y); b[2] = Math.max(b[2], x); b[3] = Math.max(b[3], y); };
    const mainland = (d) => !["island", "unconnected"].includes(map.districts[String(d)].class);
    map.N.forEach((row) => { if (mainland(row[0])) add([row[1], row[2]]); });
    for (const [d, district] of Object.entries(map.districts)) {
      if (mainland(d)) district.blob.forEach((poly) => poly.forEach(add));
    }
    const scale = Math.min((rect[2] - rect[0]) / (b[2] - b[0]), (rect[3] - rect[1]) / (b[3] - b[1]));
    const tx = rect[0] + (rect[2] - rect[0] - (b[2] - b[0]) * scale) / 2 - b[0] * scale;
    const middle = (rect[1] + rect[3]) / 2;
    const centreY = vw <= 820 ? Math.max(rect[1] + (b[3] - b[1]) * scale / 2, Math.min(middle, 315)) : middle;
    const ty = centreY - (b[1] + b[3]) * scale / 2;
    const d = Object.keys(map.districts).find(mainland);
    const actual = document.querySelector(`svg.map-svg path.hit[data-k="d:${d}"]`).getAttribute("d").match(/-?\d+(?:\.\d+)?/g).map(Number);
    const first = map.districts[d].blob[0][0];
    const sx = first[0] * scale + tx, sy = first[1] * scale + ty;
    const islandsAtFit = [...document.querySelectorAll('svg.map-svg path.hit[data-k^="d:"]')]
      .filter((el) => map.districts[el.getAttribute("data-k").slice(2)].class === "island").length;
    return { expectedWidth: (b[2] - b[0]) * scale, scale, pointError: Math.hypot(actual[0] - sx, actual[1] - sy), islandsAtFit,
      viewport: [vw, vh], rect };
  }, doc);
}

async function checkFitFraming(browser, base, slug, profile) {
  const label = `mainland fit / ${slug} / ${profile.name}`;
  const context = await browser.newContext({ viewport: profile.viewport, isMobile: profile.isMobile,
    hasTouch: profile.hasTouch, deviceScaleFactor: profile.deviceScaleFactor ?? 1 });
  const page = await context.newPage();
  await page.goto(`${base}/${slug}`);
  await page.waitForSelector("svg.map-svg path.hit");
  const doc = await page.evaluate(async (name) => (await fetch(`/maps/${name}.json`)).json(), slug);
  const frame = await fitFraming(page, doc);
  report(frame.pointError < 2, `${label}: fit projects mainland bounds into the content rectangle`, JSON.stringify(frame));
  report(frame.islandsAtFit === 0, `${label}: invisible islands cannot set the opening frame`);
  const box = await districtUnionBox(page);
  report(fitsWithinPad(box, frame.viewport[0], frame.viewport[1]), `${label}: mainland clears fit inset`, JSON.stringify(box));
  if (profile.isMobile) {
    const chip = await page.locator("[data-unconnected-chip]").count()
      ? await page.locator("[data-unconnected-chip]").boundingBox() : null;
    const sheet = await page.locator("[data-selection-panel]").boundingBox();
    const svg = await page.locator("svg.map-svg").boundingBox();
    report(box.y + box.h + svg.y < Math.min(chip?.y ?? Infinity, sheet?.y ?? Infinity),
      `${label}: mainland clears chip and bottom sheet`);
  }
  if (slug === "langgenius/dify" && Object.values(doc.districts).some((d) => d.class === "island")) {
    const islandStroke = () => page.locator('svg.map-svg path.hit[data-k^="d:"]').evaluateAll((els, map) =>
      els.filter((el) => map[el.getAttribute("data-k").slice(2)].class === "island")
        .map((el) => Number(el.getAttribute("stroke-opacity"))), doc.districts);
    await page.locator('button[aria-label="Zoom in"]').click();
    await page.waitForTimeout(600);
    const inward = await islandStroke();
    report(inward.length > 0 && inward.some((opacity) => opacity > 0 && opacity < 0.4),
      `${label}: islands fade in when zooming inward`);
    await page.locator('button[aria-label="Fit map"]').click();
    await page.waitForTimeout(650);
    await page.locator('button[aria-label="Zoom out"]').click();
    await page.waitForTimeout(600);
    const outward = await islandStroke();
    report(outward.length > 0 && outward.every((opacity) => opacity > 0 && opacity <= 0.4),
      `${label}: islands reappear when zooming outward`);
  }
  console.log(`  fit width ${slug} ${profile.name}: ${frame.expectedWidth.toFixed(1)} CSS px`);
  await context.close();
}

async function checkFolderWinsFileCollision(browser, base) {
  const label = "folder wins file collision (dify) / desktop";
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  const page = await context.newPage();
  const doc = await (await context.request.get(`${base}/maps/langgenius/dify.json`)).json();
  const probe = doc.F.indexOf("web/app/components/workflow/hooks/use-nodes-interactions.ts");
  const district = doc.N[probe][0];
  const members = doc.F.flatMap((path, i) => path.startsWith("web/app/components/workflow/") && doc.N[i][0] === district ? [i] : []);
  const median = (column) => {
    const values = members.map((i) => doc.N[i][column]).sort((a, b) => a - b);
    return (values[(values.length - 1) >> 1] + values[values.length >> 1]) / 2;
  };
  // Move one uniquely named, ranked file to the folder's measured median.
  // This changes only the test response. The control below hides that folder
  // label without moving the view, proving the file has label budget.
  doc.N[probe][1] = median(1);
  doc.N[probe][2] = median(2);
  await page.route("**/maps/langgenius/dify.json", (route) => route.fulfill({ json: doc }));
  await page.goto(`${base}/langgenius/dify?d=${district}`);
  await page.waitForSelector('button[aria-label="Zoom to district"]');
  await page.locator('button[aria-label="Zoom to district"]').click();
  await page.waitForTimeout(750);
  await page.locator('button[aria-label="Zoom in"]').click();
  await page.waitForTimeout(750);
  const folder = page.locator('[data-folder-label="web/app/components/workflow"]');
  const file = page.locator(`[data-file-label="${probe}"]`);
  report(await folder.count() === 1 && await page.locator(`[data-k="f:${probe}"]`).count() > 0 && await file.count() === 0,
    `${label}: folder occupies the file's median while its dot remains visible`);
  await page.locator("[data-district-folders-toggle]").click();
  await page.locator('[data-district-path="web/app/components/rag-pipeline"]').click();
  report(await folder.count() === 0 && await file.count() === 1,
    `${label}: file label returns at the same zoom when the folder label yields`);
  await context.close();
}

async function checkFolderLabelsAndUnconnected(browser, base, beforeBase, profile) {
  const label = `folder labels and unconnected files / ${profile.name}`;
  console.log(`\n${label}`);
  const options = { viewport: profile.viewport, isMobile: profile.isMobile, hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1 };
  const context = await browser.newContext(options);
  const page = await context.newPage();
  const slug = "langgenius/dify";
  await page.goto(`${base}/${slug}`);
  await page.waitForSelector("svg.map-svg path.hit");
  const doc = await page.evaluate(async () => (await fetch("/maps/langgenius/dify.json")).json());
  const workflowFile = doc.F.findIndex((path) => path === "web/app/components/workflow/types.ts");
  const workflowDistrict = doc.N[workflowFile][0];
  const unconnected = doc.N.flatMap((row, i) => doc.districts[String(row[0])].class === "unconnected" ? [i] : []);
  report((await page.locator("[data-folder-label]").count()) === 0, `${label}: no folder labels at fit`);
  const chip = page.locator("[data-unconnected-chip]");
  report((await chip.textContent())?.trim() === `${unconnected.length} unconnected files`, `${label}: chip counts files, not districts`);
  if (doc.coverage) {
    report(await page.locator('[data-coverage-detail]').count() === 0,
      `${label}: coverage detail stays off the opening footer`);
  }
  const noRing = async () => page.locator('svg.map-svg [data-k^="f:"]').evaluateAll((els, indices) =>
    els.every((el) => !indices.includes(Number(el.getAttribute("data-k").slice(2)))), unconnected);
  report(await noRing(), `${label}: no unconnected file dots at fit`);
  for (let i = 0; i < 3; i++) await page.locator('button[aria-label="Zoom out"]').click();
  await page.waitForTimeout(500);
  report(await noRing(), `${label}: no ring dots at full extent`);
  await page.locator('button[aria-label="Fit map"]').click();
  await page.waitForTimeout(500);
  await chip.click();
  report((await page.locator("[data-unconnected-list] details").count()) > 0, `${label}: chip opens grouped folders`);
  if (doc.coverage) report((await page.locator('[data-coverage-detail]').innerText()).includes("py:") &&
    (await page.locator('[data-coverage-detail]').innerText()).includes("ts:"),
    `${label}: coverage detail appears inside the list header`);
  const first = page.locator("[data-unconnected-list] details").first();
  await first.locator("summary").click();
  const file = first.locator("[data-unconnected-file]").first();
  const index = Number(await file.getAttribute("data-unconnected-file"));
  const beforePick = await stableBox(page);
  await file.click();
  report(new URL(page.url()).searchParams.get("file") === doc.F[index] &&
    (await page.locator("[data-selection-panel]").innerText()).includes("not connected to anything, so it isn't placed on the map"),
  `${label}: listed file opens its card with the off-map note`);
  report(boxesClose(beforePick, await stableBox(page)), `${label}: listed file leaves the map view in place`);
  await page.goto(`${base}/${slug}?sel=${encodeURIComponent(doc.F[index])}`);
  await page.waitForSelector("svg.map-svg path.hit");
  if (profile.hasTouch) await page.locator("[data-selection-panel] > div").first().click();
  report((await page.locator("[data-selection-panel]").innerText()).includes("not connected to anything, so it isn't placed on the map"),
    `${label}: ?sel= unconnected file deep link opens its card`);
  await page.goto(`${base}/${slug}`);
  await page.waitForSelector("svg.map-svg path.hit");
  await page.locator('input[aria-label="Search files"]').fill(doc.F[index]);
  await page.locator('input[aria-label="Search files"]').press("Enter");
  report(new URL(page.url()).searchParams.get("file") === doc.F[index],
    `${label}: search result opens the unconnected file card`);

  // #79's rebuilt map moves workflow from d:0 to d:1; target the actual
  // district containing its top-ranked types.ts landmark in either map.
  await page.goto(`${base}/${slug}?d=${workflowDistrict}`);
  await page.waitForSelector('button[aria-label="Zoom to district"]');
  await page.locator('button[aria-label="Zoom to district"]').click({ force: true });
  await page.waitForTimeout(850);
  // A pin occupies the workflow median in #79's phone map at the district
  // jump. The label correctly yields there and appears one zoom step later.
  if (!(await page.locator('[data-folder-label="web/app/components/workflow"]').count())) {
    await page.locator('button[aria-label="Zoom in"]').click();
    await page.waitForTimeout(850);
  }
  const labels = await page.locator("[data-folder-label]").evaluateAll((els) =>
    els.map((el) => [el.getAttribute("data-folder-district"), el.getAttribute("data-folder-label")]));
  report(labels.some(([district, path]) => district === String(workflowDistrict) && path === "web/app/components/workflow"),
    `${label}: workflow folder label appears after zooming in`, JSON.stringify(labels));
  const counts = new Map();
  for (const [district] of labels) counts.set(district, (counts.get(district) ?? 0) + 1);
  report([...counts.values()].every((count) => count <= 4), `${label}: at most four labels per district`);
  const zoomDots = await visibleDots(page);
  const fileLabels = await page.locator("svg.map-svg [data-file-label]").evaluateAll((els) =>
    els.map((el) => Number(el.getAttribute("data-file-label"))));
  const repeated = new Map();
  for (let i = 0; i < doc.F.length; i++) {
    const key = `${doc.N[i][0]}:${doc.F[i].split("/").pop()}`;
    repeated.set(key, (repeated.get(key) ?? 0) + 1);
  }
  const shown = new Map();
  for (const i of fileLabels) {
    const key = `${doc.N[i][0]}:${doc.F[i].split("/").pop()}`;
    shown.set(key, (shown.get(key) ?? 0) + 1);
  }
  report(repeated.get(`${workflowDistrict}:types.ts`) >= 3 &&
    [...shown].every(([key, count]) => (repeated.get(key) ?? 0) < 3 || count <= 1),
  `${label}: repeated basenames show at most one label per district`, JSON.stringify([...shown].filter(([key]) => key.endsWith(":types.ts"))));

  for (const layer of ["c", "x", "p"]) {
    if (profile.isMobile) await page.locator('button[aria-label="Cycle layer"]').click();
    else await page.locator('[aria-label="Layer"] button', { hasText: layer === "c" ? "churn" : layer === "x" ? "complexity" : "package" }).click();
    report((await page.locator('[data-folder-label="web/app/components/workflow"]').count()) > 0,
      `${label}: workflow folder label remains on ${layer} layer`);
    if (layer === "p" && !profile.isMobile) {
      const legend = await page.locator("[data-package-legend]").boundingBox();
      const footerChip = await chip.boundingBox();
      report(!!legend && !!footerChip && footerChip.x >= legend.x + legend.width,
        `${label}: package legend leaves room for the unconnected chip`);
    }
  }
  if (profile.isMobile) await page.locator('button[aria-label="Cycle layer"]').click();
  else await page.locator('[aria-label="Layer"] button', { hasText: "district" }).click();
  const workflowLabel = page.locator('[data-folder-label="web/app/components/workflow"]');
  if (await workflowLabel.count()) {
    for (const theme of ["light", "dark"]) {
      const ratio = await page.evaluate(({ theme, district }) => {
        document.documentElement.dataset.theme = theme;
        const label = document.querySelector('[data-folder-label="web/app/components/workflow"]');
        const shape = document.querySelector(`svg.map-svg path.hit[data-k="d:${district}"]`);
        const root = getComputedStyle(document.documentElement);
        const rgb = (value) => { const match = value.match(/#[0-9a-f]{6}|\d+/gi); const hex = match[0].startsWith("#") ? match[0].slice(1) : null;
          return hex ? [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16)) : match.slice(0, 3).map(Number); };
        const blend = (a, b, opacity) => a.map((v, i) => v * (1 - opacity) + b[i] * opacity);
        const districtFill = blend(rgb(root.getPropertyValue("--canvas").trim()), rgb(getComputedStyle(shape).fill), Number(shape.getAttribute("fill-opacity")));
        const textFill = blend(districtFill, rgb(root.getPropertyValue("--ink").trim()), Number(label.getAttribute("fill-opacity")));
        const lum = (color) => color.map((x) => x / 255).map((x) => x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4)
          .reduce((sum, x, i) => sum + x * [0.2126, 0.7152, 0.0722][i], 0);
        const a = lum(districtFill), b = lum(textFill);
        return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
      }, { theme, district: workflowDistrict });
      report(ratio >= 3, `${label}: folder contrast >= 3:1 in ${theme}`, ratio.toFixed(2));
    }
  }
  if (!profile.hasTouch && (await workflowLabel.count())) {
    await workflowLabel.hover({ force: true });
    report((await page.locator("[data-folder-highlight]").count()) > 0 && !new URL(page.url()).searchParams.has("dir"),
      `${label}: hovering previews folder highlight without selecting it`);
  }
  if (await workflowLabel.count()) {
    await workflowLabel.click({ force: true });
    report(new URL(page.url()).searchParams.get("dir") === "web/app/components/workflow" &&
      (await page.locator("[data-folder-label]").evaluateAll((els) =>
        els.every((el) => el.getAttribute("data-folder-label") === "web/app/components/workflow"))),
      `${label}: tapping applies folder highlight and shows only its label`);
  }

  if (beforeBase) {
    const prior = await browser.newContext(options);
    const oldPage = await prior.newPage();
    await oldPage.goto(`${beforeBase}/${slug}?d=${workflowDistrict}`);
    await oldPage.waitForSelector('button[aria-label="Zoom to district"]');
    await oldPage.locator('button[aria-label="Zoom to district"]').click({ force: true });
    await oldPage.waitForTimeout(850);
    const oldDots = await visibleDots(oldPage);
    const common = Object.keys(zoomDots).filter((key) => oldDots[key]);
    report(common.length > 100 && common.every((key) => JSON.stringify(zoomDots[key]) === JSON.stringify(oldDots[key])),
      `${label}: existing dot cx/cy match origin/main at the same map and zoom`, `${common.length} shared dots`);
    await prior.close();
  }
  await context.close();
}

// ---------------------------------------------------------------------------
// Issue #82 A1 ("selecting never moves the map"; step-back; breadcrumb;
// fullscreen; drag-vs-click threshold; hit-area data-k fixes). No existing
// check above asserted the OLD fly-on-select behaviour (a sidebar pick or a
// search result changing k) -- runOne()'s own "tap a file dot" step only
// ever exercised a MAP-originated selection, which never flew before this
// PR either, and nothing else in this file drove Sidebar or SearchBox and
// then compared the view. So there was nothing here to correct for the new
// pan-only behaviour; these are all new checks instead.
// ---------------------------------------------------------------------------

/** Issue #82 A1 scope item 2: an empty map tap steps back one level at a
 * time -- symbol -> its file -> the file's district -> nothing -- instead
 * of clearing everything in one step. Drives it from a `?file=&sym=` deep
 * link on a file/symbol known to be in a normal (non-unconnected) district,
 * so the district level is actually reachable. */
async function checkEmptyTapStepBack(browser, base) {
  const label = "empty tap steps back one level at a time (django)";
  console.log(`\n${label}`);
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  const page = await context.newPage();
  const doc = await (await fetch(`${base}/maps/django/django.json`)).json();
  let fileIndex = null;
  let districtId = null;
  for (let i = 0; i < doc.F.length; i++) {
    const syms = doc.S?.[String(i)];
    if (!syms || syms.length === 0) continue;
    const d = doc.N[i][0];
    if (doc.districts[String(d)].class === "unconnected") continue;
    fileIndex = i;
    districtId = d;
    break;
  }
  if (fileIndex == null) {
    report(false, `${label}: setup`, "no file with a symbol in a normal district found");
    await context.close();
    return;
  }
  await page.goto(`${base}/django/django?geo=r&layer=d&file=${encodeURIComponent(doc.F[fileIndex])}&sym=0`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);

  const step = async (assertFn, stepLabel) => {
    const pt = await findEmptyPointWithZoomOut(page, 1200, 800);
    if (!pt) {
      report(false, `${label}: ${stepLabel}`, "no empty point found even after zooming out");
      return;
    }
    const before = await stableBox(page);
    await tap(page, PROFILES[0], pt[0], pt[1]);
    await page.waitForTimeout(300);
    const url = new URL(page.url());
    assertFn(url, stepLabel);
    report(boxesClose(before, await stableBox(page)), `${label}: ${stepLabel} (view unchanged)`);
  };

  await step((url, stepLabel) => report(
    url.searchParams.get("file") === doc.F[fileIndex] && !url.searchParams.has("sym"),
    `${label}: ${stepLabel}`,
    url.toString(),
  ), "symbol -> file (drops the symbol, keeps the file)");

  await step((url, stepLabel) => report(
    !url.searchParams.has("file") && url.searchParams.get("d") === String(districtId),
    `${label}: ${stepLabel}`,
    url.toString(),
  ), "file -> district (its own district)");

  await step((url, stepLabel) => report(
    !url.searchParams.has("file") && !url.searchParams.has("d"),
    `${label}: ${stepLabel}`,
    url.toString(),
  ), "district -> nothing");

  // One more empty tap once nothing is selected: no-op, not an error --
  // matches the prototype's own `else return;` (arch20.body.html).
  const finalPt = await findEmptyPointWithZoomOut(page, 1200, 800);
  if (finalPt) {
    const beforeUrl = page.url();
    await tap(page, PROFILES[0], finalPt[0], finalPt[1]);
    await page.waitForTimeout(300);
    report(page.url() === beforeUrl, `${label}: an empty tap with nothing selected is a no-op`);
  }
  await context.close();
}

/** Issue #82 A1 scope item 3: breadcrumb segments select their level
 * without moving the view. Same deep link setup as the step-back check
 * above (a file+symbol in a normal district), clicking from the deepest
 * segment (file, since the symbol itself renders as plain text, not a
 * button -- see Breadcrumb.tsx) back up through district to repo. */
async function checkBreadcrumbNoMove(browser, base, profile) {
  const label = `breadcrumb selects without moving the view (django) / ${profile.name}`;
  console.log(`\n${label}`);
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1,
  });
  const page = await context.newPage();
  const doc = await (await fetch(`${base}/maps/django/django.json`)).json();
  let fileIndex = null;
  let districtId = null;
  for (let i = 0; i < doc.F.length; i++) {
    const syms = doc.S?.[String(i)];
    if (!syms || syms.length === 0) continue;
    const d = doc.N[i][0];
    if (doc.districts[String(d)].class === "unconnected") continue;
    fileIndex = i;
    districtId = d;
    break;
  }
  if (fileIndex == null) {
    report(false, `${label}: setup`, "no file with a symbol in a normal district found");
    await context.close();
    return;
  }
  await page.goto(`${base}/django/django?geo=r&layer=d&file=${encodeURIComponent(doc.F[fileIndex])}&sym=0`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("[data-breadcrumb]");
  await page.waitForTimeout(600);
  if (profile.isMobile) {
    const header = page.locator("[data-selection-panel] > div").first();
    const box = await header.boundingBox();
    if (box) await tap(page, profile, box.x + box.width / 2, box.y + box.height / 2);
    await page.waitForTimeout(300);
  }

  const buttons = page.locator("[data-breadcrumb] button");
  report((await buttons.count()) === 3, `${label}: repo/district/file are all clickable with a symbol selected`, `count=${await buttons.count()}`);

  let before = await stableBox(page);
  await buttons.nth(2).click(); // file segment: drops the symbol
  await page.waitForTimeout(300);
  let url = new URL(page.url());
  report(url.searchParams.get("file") === doc.F[fileIndex] && !url.searchParams.has("sym"),
    `${label}: file segment drops the symbol, keeps the file`, url.toString());
  report(boxesClose(before, await stableBox(page)), `${label}: view unchanged after the file segment`);

  before = await stableBox(page);
  await buttons.nth(1).click(); // district segment
  await page.waitForTimeout(300);
  url = new URL(page.url());
  report(!url.searchParams.has("file") && url.searchParams.get("d") === String(districtId),
    `${label}: district segment selects the district`, url.toString());
  report(boxesClose(before, await stableBox(page)), `${label}: view unchanged after the district segment`);

  before = await stableBox(page);
  await buttons.nth(0).click(); // repo segment
  await page.waitForTimeout(300);
  url = new URL(page.url());
  report(!url.searchParams.has("file") && !url.searchParams.has("d"),
    `${label}: repo segment clears the selection`, url.toString());
  report(boxesClose(before, await stableBox(page)), `${label}: view unchanged after the repo segment`);
  await context.close();
}

/** Issue #82 A1 scope item 5: entering and leaving fullscreen keeps k and
 * the centre world point, changing only the aspect -- the same contract
 * MapRenderer.resize() already gives a plain viewport resize (see runOne's
 * own "resize preserves centre" step, whose centre-point formula this
 * reuses almost verbatim). Whether the browser actually grants the native
 * Fullscreen API or MapView falls back to its CSS-only mode is not asserted
 * either way -- both paths go through the exact same resize(), never fit()
 * (see MapView.tsx's isFullscreen wiring), so the invariant holds
 * regardless of which one engaged.
 *
 * CI review finding: this used to compare a single dot's own radius before
 * and after, on the theory that radius is a pure function of k for a fixed
 * file. It isn't -- MapRenderer.paint() computes it as
 * `... * Math.sqrt(this.k / this.fitScale())`, and fitScale() itself
 * changes across this exact transition (removing/adding chrome changes the
 * available fitting box), so the radius moves even when k does not. Fixed
 * by comparing the on-screen DISTANCE between two fixed dots instead
 * (pickTwoOnScreenDots) -- distance = worldDistance * k is independent of
 * both tx/ty and fitScale(), so it isolates k cleanly. */
async function checkFullscreenPreservesView(browser, base, profile) {
  const label = `fullscreen keeps k and the centre point (django) / ${profile.name}`;
  console.log(`\n${label}`);
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1,
  });
  const page = await context.newPage();
  await page.goto(`${base}/django/django`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);
  // Zoom in one notch first, off the exact fit-scale view: clampK's floor
  // is fitScale()*0.5, and fitScale() itself moves across this transition
  // (see above) -- starting EXACTLY at fit, where k already sits right at
  // that floor, risks resize()'s existing, correct, and unrelated re-clamp
  // bumping k by a hair as the floor moves under it, a false failure of
  // THIS check rather than a bug in the fullscreen feature. One notch of
  // headroom above fit is enough that the floor can't reach k in a normal
  // chrome size change.
  await zoomIn(page, profile.viewport.width / 2, profile.viewport.height / 2);

  const keys = await pickTwoOnScreenDots(page, profile.viewport.width, profile.viewport.height);
  if (!keys) {
    report(false, `${label}: setup`, "could not find two on-screen dots far enough apart");
    await context.close();
    return;
  }
  const [keyA, keyB] = keys;
  const vbBefore = await readViewBox(page);
  const beforeA = await readDot(page, keyA);
  const beforeB = await readDot(page, keyB);
  const distBefore = beforeA && beforeB ? Math.hypot(beforeA.cx - beforeB.cx, beforeA.cy - beforeB.cy) : null;

  await page.locator('button[aria-label="Enter fullscreen"]').click();
  await page.waitForTimeout(500);
  const vbAfter = await readViewBox(page);
  const afterA = await readDot(page, keyA);
  const afterB = await readDot(page, keyB);
  const distAfter = afterA && afterB ? Math.hypot(afterA.cx - afterB.cx, afterA.cy - afterB.cy) : null;

  if (!beforeA || !afterA) {
    report(false, `${label}: entering fullscreen`, "reference dot A missing after entering fullscreen");
  } else {
    // Same formula as runOne's "resize preserves centre" step: resize()
    // keeps the world point under the OLD viewport centre under the NEW
    // one, which is exactly a shift of half the viewBox delta. Dot A is an
    // arbitrary fixed world point for this purpose; either would do.
    const expectDx = (vbAfter.w - vbBefore.w) / 2;
    const expectDy = (vbAfter.h - vbBefore.h) / 2;
    const actualDx = afterA.cx - beforeA.cx;
    const actualDy = afterA.cy - beforeA.cy;
    const centreOk = Math.abs(actualDx - expectDx) <= 1.5 && Math.abs(actualDy - expectDy) <= 1.5;
    report(centreOk, `${label}: entering fullscreen keeps the centre world point`,
      centreOk ? undefined : `expected shift (${expectDx.toFixed(1)}, ${expectDy.toFixed(1)}), got (${actualDx.toFixed(1)}, ${actualDy.toFixed(1)})`);
  }
  if (distBefore == null || distAfter == null) {
    report(false, `${label}: entering fullscreen keeps k`, "one of the two reference dots went missing");
  } else {
    const kOk = Math.abs(distBefore - distAfter) <= 1.5;
    report(kOk, `${label}: entering fullscreen keeps k (screen distance between two fixed dots unchanged)`,
      kOk ? undefined : `distance before=${distBefore.toFixed(2)}px after=${distAfter.toFixed(2)}px`);
  }
  report(await page.locator('button[aria-label="Exit fullscreen"]').count() === 1,
    `${label}: the zoom controls now show an exit-fullscreen button`);

  await page.locator('button[aria-label="Exit fullscreen"]').click();
  await page.waitForTimeout(500);
  const vbFinal = await readViewBox(page);
  const finalA = await readDot(page, keyA);
  const finalB = await readDot(page, keyB);
  const distFinal = finalA && finalB ? Math.hypot(finalA.cx - finalB.cx, finalA.cy - finalB.cy) : null;
  if (finalA && distFinal != null) {
    const backOk = Math.abs(finalA.cx - beforeA.cx) <= 1.5 && Math.abs(finalA.cy - beforeA.cy) <= 1.5 &&
      Math.abs(vbFinal.w - vbBefore.w) <= 1.5 && Math.abs(vbFinal.h - vbBefore.h) <= 1.5;
    report(backOk, `${label}: exiting fullscreen returns to the original view`,
      backOk ? undefined : JSON.stringify({ vbBefore, vbFinal, beforeA, finalA }));
    const kBackOk = distBefore != null && Math.abs(distBefore - distFinal) <= 1.5;
    report(kBackOk, `${label}: exiting fullscreen restores k`,
      kBackOk ? undefined : `distance before=${distBefore?.toFixed(2)}px final=${distFinal.toFixed(2)}px`);
  } else {
    report(false, `${label}: exiting fullscreen`, "a reference dot went missing after exiting fullscreen");
  }
  await context.close();
}

/** Issue #82 A1 scope item 6: a file basename label carries `data-k`, so a
 * tap on the LABEL ITSELF (not the dot it names) selects that file instead
 * of falling through to nothing. */
async function checkFileLabelTapSelects(browser, base) {
  const label = "tapping a file label selects that file (django)";
  console.log(`\n${label}`);
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  const page = await context.newPage();
  await page.goto(`${base}/django/django`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);
  await zoomIn(page, 600, 400); // file labels only appear once zf > 1.5 (drawLabels)

  const target = await page.evaluate(() => {
    const els = [...document.querySelectorAll("svg.map-svg text[data-file-label]")];
    for (const el of els) {
      const r = el.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) continue;
      return { x: r.x + r.width / 2, y: r.y + r.height / 2, index: Number(el.getAttribute("data-file-label")) };
    }
    return null;
  });
  if (!target) {
    report(false, `${label}: setup`, "no file label found after zooming in");
    await context.close();
    return;
  }
  const doc = await (await fetch(`${base}/maps/django/django.json`)).json();
  await page.mouse.click(target.x, target.y);
  await page.waitForTimeout(300);
  report(new URL(page.url()).searchParams.get("file") === doc.F[target.index],
    `${label}: tapping the label selects the file it names`,
    `expected=${doc.F[target.index]} url=${page.url()}`);
  await context.close();
}

/** Issue #82 A1 scope item 4: a single named 4px (Euclidean) threshold for
 * both "still a tap" and "swallow the click" -- a drag that crosses it never
 * selects whatever was under the pointer when it lifted, and a sub-threshold
 * jitter (real touch/mouse input always has some) still counts as a tap. */
async function checkDragThresholdNoSelect(browser, base) {
  const label = "drag vs. click threshold (django) / desktop";
  console.log(`\n${label}`);
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  const page = await context.newPage();
  await page.goto(`${base}/django/django`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);

  // A district polygon, not a file dot: on desktop a dot's hit area is only
  // its own painted (LOC-scaled, sometimes just a couple of screen pixels)
  // radius -- no touch padding, unlike the circle's stroke-width:TOUCH?16:0
  // -- so a several-pixel drag can exit a SMALL dot's geometry by simple
  // accident, passing this check even if the THRESHOLD logic under test
  // were wrong. A district polygon spans hundreds of pixels, so a 2px or
  // 6px move is guaranteed to land inside it either way; what differs is
  // purely whether pointerMove/click's shared threshold swallows the click,
  // which is the one thing actually being tested here.
  const dragPoint = await pickDistrictPoint(page, false);
  if (!dragPoint) {
    report(false, `${label}: setup`, "no unobstructed district point found for the drag case");
  } else {
    await page.mouse.move(dragPoint.x, dragPoint.y);
    await page.mouse.down();
    await page.mouse.move(dragPoint.x + 6, dragPoint.y + 6, { steps: 4 }); // >= DRAG_THRESHOLD_PX
    await page.mouse.up();
    await page.waitForTimeout(300);
    report(!new URL(page.url()).searchParams.has("d"),
      `${label}: a 6px drag starting on a district does not select it`, page.url());
  }

  const tapPoint = await pickDistrictPoint(page, false);
  if (!tapPoint) {
    report(false, `${label}: setup`, "no unobstructed district point found for the jitter case");
  } else {
    await page.mouse.move(tapPoint.x, tapPoint.y);
    await page.mouse.down();
    await page.mouse.move(tapPoint.x + 2, tapPoint.y + 1, { steps: 2 }); // < DRAG_THRESHOLD_PX
    await page.mouse.up();
    await page.waitForTimeout(300);
    report(new URL(page.url()).searchParams.get("d") === tapPoint.key.split(":")[1],
      `${label}: a 2px jitter still counts as a tap`, `expected d=${tapPoint.key.split(":")[1]} url=${page.url()}`);
  }
  await context.close();
}

/** Issue #82 A1 scope item 1: a search pick for a file that's currently off
 * screen pans it into view without changing k. Zooms in on a corner first
 * so plenty of the mainland ends up off screen, picks an off-screen file by
 * data-k geometry (not by ID -- has to be genuinely outside the safe content
 * rect right now), and checks an UNRELATED on-screen reference dot's own
 * cx/cy shift (proves a pan happened) and radius (proves k didn't, see
 * readDot's own comment) rather than trying to read the target's geometry,
 * which may not even be in the DOM yet (paint() culls far-off content).
 *
 * CI review finding: searching by bare basename picked
 * "django/conf/locale/<lang>/formats.py" as the off-screen target on one
 * run, then landed on "django/utils/formats.py" instead -- not an app bug.
 * django ships 86 files literally named `formats.py` (one per locale
 * directory), and map/search.ts's own ranking (an intentional, unrelated
 * design: "exact match wins over a substring match") ties ALL of them at
 * its best bucket for an exact-basename query, breaking the tie by import
 * fan-in -- so searching a NON-unique basename is inherently ambiguous
 * about which file Enter selects, independent of anything this PR touches.
 * Fixed by only ever choosing an off-screen candidate whose basename is
 * unique across the whole document, so the search has exactly one right
 * answer. */
async function checkSearchPanOffscreen(browser, base) {
  const label = "search pick for an off-screen file pans without changing k (django)";
  console.log(`\n${label}`);
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  const page = await context.newPage();
  await page.goto(`${base}/django/django`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);
  // Zoom in centred on the viewport's own centre (not a corner): anchoring
  // there keeps pickTarget()'s central safe region populated (it needs a
  // reference dot to still be on screen afterwards) while still shrinking
  // the visible extent enough that plenty of files near the EDGES of the
  // old view end up genuinely off screen -- which is what makes finding an
  // off-screen target below possible at all.
  await zoomIn(page, 600, 400);

  const reference = await pickTarget(page, 1200, 800);
  if (!reference?.dataK) {
    report(false, `${label}: setup`, "no on-screen reference dot found");
    await context.close();
    return;
  }
  const doc = await (await fetch(`${base}/maps/django/django.json`)).json();
  const basenameCounts = new Map();
  for (const f of doc.F) {
    const base = f.split("/").pop();
    basenameCounts.set(base, (basenameCounts.get(base) ?? 0) + 1);
  }
  // CI review finding: this used getBoundingClientRect() (PAGE coordinates)
  // for cx/cy, compared against fitViewport()'s rect -- which is in SVG-
  // LOCAL units (0..VW, 0..VH; see MapRenderer.resize()/paint(), where VW/VH
  // come from the wrap div's own box, and this.X()/this.Y() bake tx/ty and k
  // in but never a page offset). The SVG element itself does not start at
  // page (0,0) on desktop -- Sidebar (250px) sits to its left and TopBar
  // above it -- so a page-relative cx/cy is shifted from local space by
  // exactly that offset, and comparing the two directly misclassified a
  // genuinely ON-screen dot (in local space, which is what panTo() itself
  // checks) as off screen. Fixed by reading the `cx`/`cy` ATTRIBUTES the
  // renderer actually wrote (`this.X(p[0]).toFixed(1)` et al) instead of a
  // derived bounding rect -- exactly the coordinate space panTo()'s own
  // bounds check runs in, so there is no origin to get wrong. (readDot()
  // elsewhere in this file already reads attributes for the same reason;
  // this is the one spot that had reintroduced getBoundingClientRect().)
  const offscreen = await page.evaluate(
    ({ files, refKey, uniqueBasenames }) => {
      const svg = document.querySelector("svg.map-svg");
      const vw = svg.clientWidth;
      const vh = svg.clientHeight;
      const rect = vw <= 820 ? [16, 110, vw - 16, vh - 158] : [24, 12, vw - 24, vh - 38];
      for (let i = 0; i < files.length; i++) {
        if (`f:${i}` === refKey) continue;
        if (!uniqueBasenames.includes(files[i].split("/").pop())) continue;
        const el = document.querySelector(`[data-k="f:${i}"]`);
        if (!el) continue; // not drawn near the viewport at all -- also off screen, but nothing to measure against the rect
        const cx = parseFloat(el.getAttribute("cx"));
        const cy = parseFloat(el.getAttribute("cy"));
        if (Number.isNaN(cx) || Number.isNaN(cy)) continue;
        if (cx < rect[0] || cx > rect[2] || cy < rect[1] || cy > rect[3]) return { index: i, file: files[i] };
      }
      return null;
    },
    {
      files: doc.F,
      refKey: reference.dataK,
      uniqueBasenames: [...basenameCounts.entries()].filter(([, count]) => count === 1).map(([base]) => base),
    },
  );
  if (!offscreen) {
    report(false, `${label}: setup`, "no off-screen file with a unique basename found after zooming in");
    await context.close();
    return;
  }

  const before = await readDot(page, reference.dataK);
  await page.locator('input[aria-label="Search files"]').fill(offscreen.file.split("/").pop());
  await page.waitForTimeout(150);
  await page.locator('input[aria-label="Search files"]').press("Enter");
  await page.waitForTimeout(650); // glide()/settle
  const after = await readDot(page, reference.dataK);

  report(new URL(page.url()).searchParams.get("file") === offscreen.file,
    `${label}: search selected the off-screen target`, page.url());
  if (!before || !after) {
    report(false, `${label}: reference dot present before and after`, JSON.stringify({ before, after }));
  } else {
    const moved = Math.hypot(after.cx - before.cx, after.cy - before.cy) > 5;
    const sameRadius = before.r != null && after.r != null && Math.abs(before.r - after.r) <= 0.05;
    report(moved, `${label}: the view actually panned`, JSON.stringify({ before, after }));
    report(sameRadius, `${label}: k unchanged (reference dot radius identical)`, JSON.stringify({ before, after }));
  }
  await context.close();
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  await preflight(args.base);
  const browser = await chromium.launch();
  try {
    if (args.featureOnly) {
      for (const slug of MAPS) for (const profile of PROFILES) await checkFitFraming(browser, args.base, slug, profile);
      await checkFolderWinsFileCollision(browser, args.base);
      for (const profile of PROFILES) await checkFolderLabelsAndUnconnected(browser, args.base, args.beforeBase, profile);
      return;
    }
    for (const slug of MAPS) {
      for (const profile of PROFILES) {
        await checkFitFraming(browser, args.base, slug, profile);
        await runOne({ browser, base: args.base, slug, profile });
      }
    }
    await checkFolderWinsFileCollision(browser, args.base);
    for (const profile of PROFILES) await checkRepoSwitch(browser, args.base, profile);
    await checkMultiPolygonHover(browser, args.base);
    for (const profile of PROFILES) await checkSelectionDim(browser, args.base, profile);
    for (const profile of PROFILES) await checkPackageLayout(browser, args.base, profile);
    await checkDistrictRefinement(browser, args.base);
    for (const profile of PROFILES) await checkDifyDistrictSummary(browser, args.base, profile);
    for (const profile of PROFILES) await checkFolderIslandFade(browser, args.base, profile);
    for (const profile of PROFILES) await checkViewerCards(browser, args.base, profile);
    for (const profile of PROFILES) await checkFolderLabelsAndUnconnected(browser, args.base, args.beforeBase, profile);
    // Issue #82 A1
    await checkEmptyTapStepBack(browser, args.base);
    for (const profile of PROFILES) await checkBreadcrumbNoMove(browser, args.base, profile);
    for (const profile of PROFILES) await checkFullscreenPreservesView(browser, args.base, profile);
    await checkFileLabelTapSelects(browser, args.base);
    await checkDragThresholdNoSelect(browser, args.base);
    await checkSearchPanOffscreen(browser, args.base);
  } finally {
    await browser.close();
  }
}

let runError = false;
try {
  await main();
} catch (err) {
  console.error(err);
  runError = true;
  process.exitCode = 1;
} finally {
  console.log(`\n${checks - failures}/${checks} checks passed${runError ? " (run aborted)" : ""}`);
  if (failures) {
    console.error(`${failures} check(s) failed`);
    process.exitCode = 1;
  }
}
