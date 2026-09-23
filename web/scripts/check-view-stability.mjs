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
// B4 (nested footprints, issue #82): both fixtures rebuilt WITH footprints
// (P, footprint_centroids, file_neighbourhoods, neighbourhoods) -- finding 29
// records the paired builds as identical to the pre-footprint maps on every
// OTHER key (F/N/E/L/S/U/districts/q/coverage), so this is an additive
// fixture swap, not a re-record of membership or geometry.
const MAP_FIXTURES = {
  "django/django": "24849f34f13f6c69b1587ed39fca65de05c868d3c0d2402d1fd7b5bb3b811621",
  "langgenius/dify": "2b7c2dcbda9f4b19a55862b32a58b5e4f167a696078e20af72f5468beed594f2",
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
    // B4: a footprint-mode file (or a district/neighbourhood outline) is a
    // `<path>` with neither `cx`/`cy` nor `x`/`y` -- its screen position
    // lives in `data-cx`/`data-cy` where the renderer set one (a footprint's
    // own anchor point; see MapRenderer's `footprint()`). Falls through to
    // `null` (not found on that element either) exactly like the pre-B4
    // `x`/`y` fallback already did for a treemap rect.
    let cx = parseFloat(el.getAttribute("cx") ?? el.getAttribute("x") ?? el.getAttribute("data-cx"));
    let cy = parseFloat(el.getAttribute("cy") ?? el.getAttribute("y") ?? el.getAttribute("data-cy"));
    // Perf follow-up: a district/neighbourhood outline (or a batched
    // footprint fill, though that one carries no data-k to look up by) has
    // none of the above -- fall back to its own first vertex, parsed off the
    // `d` attribute the same way fitFraming() already does a few functions
    // down. A district polygon's first point is a perfectly good fixed world
    // point for a k/pan measurement; it just isn't a "centre" of anything.
    if (Number.isNaN(cx) || Number.isNaN(cy)) {
      const m = el.getAttribute("d")?.match(/-?\d+(?:\.\d+)?/g);
      if (m && m.length >= 2) {
        cx = Number(m[0]);
        cy = Number(m[1]);
      }
    }
    return { cx, cy, r: el.hasAttribute("r") ? parseFloat(el.getAttribute("r")) : null };
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

/** A point that resolves (via elementFromPoint) to a district itself --
 * large by construction, unlike a file dot (see checkDragThresholdNoSelect's
 * own comment for why that distinction matters there). Same unobstructed-
 * point algorithm checkViewerCards uses for its own district tap, factored
 * out since a second check needs it.
 *
 * Perf follow-up (issue #82): always the district's NAME LABEL now, on both
 * profiles -- a footprint-mode district's own polygon fill is tiled edge to
 * edge by its files (batched or not), so a point that resolves to the BARE
 * polygon (not a label) is, semantically, either a rounding-gap artefact
 * between two adjacent file fills or a gutter stroke's paint-over -- both
 * genuinely belong to some file underneath, and MapRenderer's own
 * resolveKey() now deliberately refines exactly that case to the file (see
 * its own doc comment for the CI failure that fix addresses). A label tap is
 * the one gesture resolveKey() never second-guesses, and it's what this
 * function now finds on either profile -- desktop used to search the
 * polygon specifically; touch already searched the label, so only the
 * desktop path's target selector changes here. */
async function pickDistrictPoint(page, _hasTouch) {
  return page.evaluate(() => {
    const paths = [...document.querySelectorAll('svg.map-svg text.hit[data-k^="d:"]')];
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
  });
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
      const collect = (selector) => {
        const els = [...document.querySelectorAll(selector)];
        const out = [];
        for (const el of els) {
          const r = el.getBoundingClientRect();
          if (r.width <= 0 || r.height <= 0) continue;
          const cx = r.x + r.width / 2;
          const cy = r.y + r.height / 2;
          if (cx > vw * 0.15 && cx < vw * 0.85 && cy > vh * 0.15 && cy < vh * 0.85) {
            out.push({ dataK: el.getAttribute("data-k"), cx, cy });
          }
          if (out.length >= 40) break;
        }
        return out;
      };
      const findPair = (candidates) => {
        for (let a = 0; a < candidates.length; a++) {
          for (let b = a + 1; b < candidates.length; b++) {
            const d = Math.hypot(candidates[a].cx - candidates[b].cx, candidates[a].cy - candidates[b].cy);
            if (d >= minDist) return [candidates[a].dataK, candidates[b].dataK];
          }
        }
        return null;
      };
      // B4: `.hit` (not `circle.hit`) so this also matches a footprint-mode
      // file's own path element, not just a dot-mode circle -- see
      // visibleDots()'s own comment for why the same generalisation applies
      // there. Perf follow-up: most files in a footprint-mode district with
      // small on-screen footprints no longer have an individual element at
      // all (they're batched -- MapRenderer's own districtFootprintsLarge),
      // so file candidates alone can run out; fall back to district
      // polygons ("d:"), which stay one real element per district
      // regardless of footprint mode and serve exactly the same "two fixed
      // world points" purpose this function exists for.
      const filePair = findPair(collect('svg.map-svg .hit[data-k^="f:"]'));
      if (filePair) return filePair;
      return findPair(collect('svg.map-svg path.hit[data-k^="d:"]'));
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
    // B4: `.hit` matches a footprint-mode file's own path too (see
    // visibleDots()'s comment) -- footprint()'s fill-opacity reuses the
    // exact same 0.2/0.85 convention the dot loop's own baseOpacity does,
    // for exactly this: the "0.2 means dimmed" signal stays meaningful
    // whichever geometry drew the file.
    const dimmed = landmarks.filter((i) => {
      const c = document.querySelector(`svg.map-svg .hit[data-k="f:${i}"]`);
      return c && c.getAttribute("fill-opacity") === "0.2";
    }).length;
    return { file, dimmed, landmarksOnScreen: landmarks.filter((i) => document.querySelector(`svg.map-svg .hit[data-k="f:${i}"]`)).length };
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
// report, but dify's OWN build has ZERO multi-polygon districts -- every one
// of its districts is a single contiguous blob, so there is nothing there to
// exercise this on. B4 (nested footprints, issue #82) rebuilt django too
// (finding 29) and its own multi-polygon district went away along with it
// (0 of 12, measured against the current pinned fixture) -- so this now runs
// against encode/httpx instead, one of the committed data/ fixtures the
// catalogue already serves (also used by checkLegacyMapWithoutFootprints,
// scope item 9(f)), which reliably has four. The fix itself is generic
// (keyElements is built from every element carrying a data-k, not district-
// specific), so this is still a real exercise of the code path any repo's
// multi-polygon district would use.
async function checkMultiPolygonHover(browser, base) {
  const label = "multi-polygon district hover (encode/httpx, desktop)";
  console.log(`\n${label}`);
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  const page = await context.newPage();
  await page.goto(`${base}/encode/httpx`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);

  const districtId = await page.evaluate(async () => {
    const res = await fetch("/maps/encode/httpx.json");
    const doc = await res.json();
    for (const k in doc.districts) {
      if (doc.districts[k].blob.length > 1) return k;
    }
    return null;
  });
  if (districtId == null) {
    report(false, `${label}: no multi-polygon district found`, "fixture data may have changed");
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
    // B4: `.hit` (not `circle.hit`) so a footprint-mode file's own path
    // element is included -- see visibleDots()'s comment.
    const circles = [...document.querySelectorAll('svg.map-svg .hit[data-k^="f:"]')];
    const nonSelected = circles.filter((c) => c.getAttribute("data-k") !== `f:${i}`);
    // Exact "0.2" only (see the fresh-load check's own comment above for
    // why a threshold isn't safe on dify specifically: #49's UNRELATED
    // density-fade opacity can coincidentally sit under any threshold too).
    // Perf follow-up: a non-neighbour, non-landmark file in a small-on-
    // screen district has no individual element any more (batched --
    // MapRenderer's flushFootprintBatches) -- every batch is uniformly
    // faded or full for this whole paint (that method's own doc comment),
    // so a batched fill's OWN fill-opacity is exactly as valid a "some non-
    // neighbour file is dimmed" signal as an individual dimmed dot's used
    // to be, and is very likely the ONLY one on dify at fit zoom now.
    const dimmed = nonSelected.filter((c) => c.getAttribute("fill-opacity") === "0.2");
    const dimmedBatches = [...document.querySelectorAll("svg.map-svg path[data-footprint-batch]")]
      .filter((p) => p.getAttribute("fill-opacity") === "0.2");
    // A neighbour ring (MapRenderer.ring(), var(--hot) or var(--cold) stroke)
    // marks a file that's connected -- find one and check ITS OWN file's
    // opacity, which should read as full strength (alwaysDrawn), not dimmed.
    // B4: matched by `data-ring-for` (which file the ring belongs to,
    // MapRenderer.ring()'s own doc comment), not by cx/cy proximity -- a
    // position match stopped being reliable once footprint mode puts every
    // file's anchor on screen at once (thousands of them, some within 1px of
    // each other at a small fit-zoom k), where dot mode's #48 thinning only
    // ever left a sparse handful actually drawn at a time.
    const rings = [...document.querySelectorAll("svg.map-svg circle[data-ring-for]")];
    let neighbourFull = null;
    for (const ring of rings) {
      const key = ring.getAttribute("data-ring-for");
      if (key === `f:${i}`) continue;
      const dot = circles.find((c) => c.getAttribute("data-k") === key);
      if (dot) {
        neighbourFull = parseFloat(dot.getAttribute("fill-opacity"));
        break;
      }
    }
    return { totalCircles: circles.length, dimmedCount: dimmed.length, dimmedBatchCount: dimmedBatches.length, neighbourFull };
  }, target.i);
  report(result.dimmedCount > 0 || result.dimmedBatchCount > 0, `${label}: at least one non-neighbour file (or batch) is dimmed`, JSON.stringify(result));
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
    // B4: `.hit` so a footprint-mode file's own path element counts too.
    const circles = [...document.querySelectorAll('svg.map-svg .hit[data-k^="f:"]')];
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

  // Perf follow-up: always the district's NAME LABEL now -- see
  // pickDistrictPoint's own comment for why a bare-polygon point is no
  // longer a reliable "selects the district" target in footprint mode.
  const districtTarget = await page.evaluate(() => {
    const paths = [...document.querySelectorAll('svg.map-svg text.hit[data-k^="d:"]')];
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
    // B4: `.hit` so a footprint-mode file's own path element counts too.
    const circles = [...document.querySelectorAll('svg.map-svg .hit[data-k^="f:"]')];
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

// B4 (nested footprints, issue #82): a document with `P` draws a footprint
// PATH per file instead of a dot circle (MapRenderer's `footprint()`), which
// has no cx/cy of its own. That path carries `data-cx`/`data-cy` -- the same
// screen-space anchor point panTo/rings/labels use, rounded, for exactly
// this: a geometry-stability check can read it like a circle's cx/cy without
// reparsing the polygon's `d`. Matching BOTH shapes (circle OR path) under
// the same `[data-k^="f:"]` selector is what keeps this one helper usable
// for a footprint-mode fixture (django/dify, both now built with `P`) and a
// dot-mode one (any committed data/ fixture the catalogue still serves) --
// scope item 9(f).
async function visibleDots(page) {
  return page.locator('svg.map-svg .hit[data-k^="f:"]').evaluateAll((elements) =>
    Object.fromEntries(
      elements
        .filter((el) => el.hasAttribute("cx") || el.hasAttribute("data-cx"))
        .map((el) => [
          el.getAttribute("data-k"),
          [el.getAttribute("cx") ?? el.getAttribute("data-cx"), el.getAttribute("cy") ?? el.getAttribute("data-cy")],
        ]),
    ));
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
  //
  // B4: N's x,y is a LAYOUT input only once a document carries
  // footprint_centroids (map/geometry.ts's fileXY) -- moving only N[probe]
  // no longer moves what actually renders in footprint mode (dify has `P`).
  // Move the footprint's own anchor and polygon by the same delta so the
  // file's rendered position changes here too, whichever geometry drew it.
  const targetX = median(1);
  const targetY = median(2);
  doc.N[probe][1] = targetX;
  doc.N[probe][2] = targetY;
  if (doc.footprint_centroids) {
    const [ox, oy] = doc.footprint_centroids[probe];
    const dx = targetX - ox;
    const dy = targetY - oy;
    doc.footprint_centroids[probe] = [targetX, targetY];
    if (doc.P?.[String(probe)]) {
      doc.P[String(probe)] = doc.P[String(probe)].map(([x, y]) => [x + dx, y + dy]);
    }
  }
  // Perf follow-up: `probe` is an ordinary file, so in a small-on-screen
  // (batched) district it would have no individual element to assert
  // `[data-k="f:${probe}"]` against at all (MapRenderer's own
  // districtFootprintsLarge). Adding it to `doc.L` as a synthetic landmark
  // forces it into `alwaysDrawn` regardless of district size -- this test's
  // whole point is the folder-label/file collision at this exact spot, not
  // the batching threshold, so guaranteeing an individual element here is
  // the fixture tweak, not a workaround for a real bug. "capital" specifically
  // (not e.g. "entry"): pins.ts's isRetiredPinKind means a capital lands in
  // alwaysDrawn WITHOUT also placing a competing pin glyph that could itself
  // collide with the folder label this test is about.
  doc.L = [...doc.L, [probe, "capital", "test fixture", 99999]];
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
 * screen pans it into view without changing k. Zooms in on the viewport's
 * own centre first so plenty of the mainland ends up off screen, then picks
 * an off-screen file by data-k geometry (not by ID -- has to be genuinely
 * outside the safe content rect right now) AND a "companion" dot near it in
 * LOCAL (SVG) space, and checks the companion's own cx/cy shift (proves a
 * pan happened) and radius (proves k didn't, see readDot's own comment)
 * rather than trying to read the target's own geometry (which is exactly
 * what the pan is establishing, not a fixed reference to measure against).
 *
 * CI review finding #1: searching by bare basename picked
 * "django/conf/locale/<lang>/formats.py" as the off-screen target on one
 * run, then landed on "django/utils/formats.py" instead -- not an app bug.
 * django ships 86 files literally named `formats.py` (one per locale
 * directory), and map/search.ts's own ranking (an intentional, unrelated
 * design: "exact match wins over a substring match") ties ALL of them at
 * its best bucket for an exact-basename query, breaking the tie by import
 * fan-in -- so searching a NON-unique basename is inherently ambiguous
 * about which file Enter selects, independent of anything this PR touches.
 * Fixed by only ever choosing an off-screen TARGET whose basename is unique
 * across the whole document, so the search has exactly one right answer.
 *
 * CI review finding #2: the first fix still used an arbitrary CENTRAL dot
 * (pickTarget(), picked from the OLD view before the pan) as the fixed
 * reference point, and it went missing after the pan -- not because
 * nothing moved, but because it moved OFF screen. panTo() recentres the
 * target exactly (MapRenderer.panToPoint), which can be a large jump when
 * the target starts far from the viewport centre, and a dot that was
 * comfortably central in the OLD view is not guaranteed to survive an
 * arbitrarily large recentring translation even given paint()'s generous
 * cull margin. Fixed by picking the reference NEAR THE TARGET instead (a
 * "companion" dot within a small LOCAL-space radius of it): a pure
 * translation preserves relative positions, so anything close to the target
 * before the pan is equally close to it after -- and after, the target is
 * centred by definition, so its companion is guaranteed to still be on (or
 * very near) screen too. */
async function checkSearchPanOffscreen(browser, base) {
  const label = "search pick for an off-screen file pans without changing k (django)";
  console.log(`\n${label}`);
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  const page = await context.newPage();
  await page.goto(`${base}/django/django`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);
  // Zoom in centred on the viewport's own centre (not a corner): shrinks the
  // visible extent enough that plenty of files near the EDGES of the old
  // view end up genuinely off screen, which is what makes finding an
  // off-screen target below possible at all.
  await zoomIn(page, 600, 400);

  const doc = await (await fetch(`${base}/maps/django/django.json`)).json();
  const basenameCounts = new Map();
  for (const f of doc.F) {
    const base = f.split("/").pop();
    basenameCounts.set(base, (basenameCounts.get(base) ?? 0) + 1);
  }
  const uniqueBasenames = [...basenameCounts.entries()].filter(([, count]) => count === 1).map(([b]) => b);

  // cx/cy are read from the `cx`/`cy` ATTRIBUTES the renderer wrote
  // (`this.X(p[0]).toFixed(1)` et al), never getBoundingClientRect() -- that
  // is SVG-LOCAL space, exactly what fitViewport()'s rect and panTo()'s own
  // bounds check use (see MapRenderer.resize()/paint(): VW/VH come from the
  // wrap div's own box, and this.X()/this.Y() never add a page offset). The
  // <svg> element itself does not start at page (0,0) on desktop -- Sidebar
  // (250px) sits to its left, TopBar above it -- so a page-relative
  // getBoundingClientRect() cx/cy would be shifted from local space by
  // exactly that offset and misclassify things.
  const picked = await page.evaluate(
    ({ files, uniqueBasenames, companionRadius }) => {
      const svg = document.querySelector("svg.map-svg");
      const vw = svg.clientWidth;
      const vh = svg.clientHeight;
      const rect = vw <= 820 ? [16, 110, vw - 16, vh - 158] : [24, 12, vw - 24, vh - 38];
      const dots = [...document.querySelectorAll('svg.map-svg .hit[data-k^="f:"]')]
        .map((el) => ({
          el,
          key: el.getAttribute("data-k"),
          cx: parseFloat(el.getAttribute("cx") ?? el.getAttribute("data-cx")),
          cy: parseFloat(el.getAttribute("cy") ?? el.getAttribute("data-cy")),
        }))
        .filter((d) => !Number.isNaN(d.cx) && !Number.isNaN(d.cy));
      for (let i = 0; i < files.length; i++) {
        if (!uniqueBasenames.includes(files[i].split("/").pop())) continue;
        const key = `f:${i}`;
        const target = dots.find((d) => d.key === key);
        if (!target) continue; // not drawn near the viewport at all -- also off screen, but nothing to measure against the rect
        const offScreen = target.cx < rect[0] || target.cx > rect[2] || target.cy < rect[1] || target.cy > rect[3];
        if (!offScreen) continue;
        const companion = dots.find((d) => d.key !== key && Math.hypot(d.cx - target.cx, d.cy - target.cy) <= companionRadius);
        if (!companion) continue;
        return { index: i, file: files[i], companionKey: companion.key };
      }
      return null;
    },
    { files: doc.F, uniqueBasenames, companionRadius: 60 },
  );
  if (!picked) {
    report(false, `${label}: setup`, "no off-screen file with a unique basename and a nearby companion dot found");
    await context.close();
    return;
  }

  const before = await readDot(page, picked.companionKey);
  const beforeTarget = await readDot(page, `f:${picked.index}`);
  await page.locator('input[aria-label="Search files"]').fill(picked.file.split("/").pop());
  await page.waitForTimeout(150);
  await page.locator('input[aria-label="Search files"]').press("Enter");
  await page.waitForTimeout(650); // glide()/settle
  const after = await readDot(page, picked.companionKey);
  const afterTarget = await readDot(page, `f:${picked.index}`);

  report(new URL(page.url()).searchParams.get("file") === picked.file,
    `${label}: search selected the off-screen target`, page.url());

  // The positive complement to "the companion survived and didn't scale":
  // the TARGET itself -- off screen before -- is now actually inside the
  // safe content rect. Same rect formula, same attribute-based cx/cy as the
  // candidate search above (never getBoundingClientRect(), see this
  // function's own doc comment for why that matters).
  const targetInView = await page.evaluate((index) => {
    const svg = document.querySelector("svg.map-svg");
    const vw = svg.clientWidth;
    const vh = svg.clientHeight;
    const rect = vw <= 820 ? [16, 110, vw - 16, vh - 158] : [24, 12, vw - 24, vh - 38];
    const el = document.querySelector(`[data-k="f:${index}"]`);
    if (!el) return false;
    // B4: a footprint-mode file's own element is a `<path>` -- data-cx/
    // data-cy fallback, same as readDot()'s.
    const cx = parseFloat(el.getAttribute("cx") ?? el.getAttribute("data-cx"));
    const cy = parseFloat(el.getAttribute("cy") ?? el.getAttribute("data-cy"));
    if (Number.isNaN(cx) || Number.isNaN(cy)) return false;
    return cx >= rect[0] && cx <= rect[2] && cy >= rect[1] && cy <= rect[3];
  }, picked.index);
  report(targetInView, `${label}: the off-screen target is now inside the viewport`);

  if (!before || !after || !beforeTarget || !afterTarget) {
    report(false, `${label}: companion dot present before and after`, JSON.stringify({ before, after }));
  } else {
    const moved = Math.hypot(after.cx - before.cx, after.cy - before.cy) > 5;
    // B4: `r` is a dot-mode-only signal (a footprint `<path>` has no radius
    // to compare -- both before.r and after.r read null there, which used to
    // read as "not proven unchanged" rather than "unchanged"). The distance
    // between the TARGET and its COMPANION is a k-only invariant regardless
    // of which geometry drew either of them: pan doesn't change the distance
    // between two world points, zoom does, so an unchanged distance is
    // exactly the same positive evidence `r` used to provide.
    const distBefore = Math.hypot(beforeTarget.cx - before.cx, beforeTarget.cy - before.cy);
    const distAfter = Math.hypot(afterTarget.cx - after.cx, afterTarget.cy - after.cy);
    const sameScale = Math.abs(distBefore - distAfter) <= Math.max(1, distBefore * 0.02);
    report(moved, `${label}: the view actually panned`, JSON.stringify({ before, after }));
    report(sameScale, `${label}: k unchanged (target-companion distance identical)`, JSON.stringify({ distBefore, distAfter }));
  }
  await context.close();
}

// A2 (district colour, issue #82): screen-space proxy for "no two adjacent
// districts share a hue" -- the authoritative check is
// scripts/check-district-colours.ts (pure, world-space adjacency, run
// against the exact function -- map/colour.ts's assignDistrictHues -- the
// app itself calls); this is a real-DOM sanity check on top of it.
//
// FIRST VERSION of this check compared each district's whole-shape
// AXIS-ALIGNED BOUNDING BOX, which is a bad proxy for an elongated or
// diagonal blob: two real districts on dify (d:6 "dify-agent & dify_agent"
// and d:12 "agent-v2 & features") have bounding boxes that touch corner-to-
// corner (bboxA's [x0,y1] === bboxB's [x1,y0] exactly) while their actual
// polygons sit about 64px apart on screen at desktop fit zoom -- confirmed
// by measuring the true world-space point/segment distance (0.249 world
// units against colour.ts's own 0.186-unit threshold for this map) and
// multiplying by fitScale's k (~258). That is not a neighbour pair, and
// check-district-colours.ts agrees (0 collisions on every fixture); the bbox
// heuristic was simply wrong for shapes whose bounding box is much bigger
// than the shape itself. Fixed by comparing the districts' own RENDERED
// OUTLINE POINTS (parsed straight from each path's `d` attribute -- the
// exact geometry paint() drew, not a rectangle around it), grid-bucketed the
// same way map/colour.ts's own near() is (a district can have hundreds of
// points; all-pairs would be too slow), at a tight on-screen threshold
// (3px) that means what "touching" should mean here -- not a coincidental
// bounding-box overlap.
async function checkDistrictHueAdjacency(browser, base) {
  const label = "no two adjacent districts share a hue (dify, desktop)";
  console.log(`\n${label}`);
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  await page.goto(`${base}/langgenius/dify`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);
  const result = await page.evaluate(() => {
    const paths = [...document.querySelectorAll('svg.map-svg path.hit[data-k^="d:"]')];
    const byDistrict = new Map();
    for (const p of paths) {
      const key = p.getAttribute("data-k");
      const fill = p.getAttribute("fill");
      // paint() builds `d` as "M<x> <y>L<x> <y>L...Z" (MapRenderer.ts's
      // district-polygon block) -- strip the command letters, split on
      // whitespace, pair consecutive numbers back into [x, y] points.
      const raw = (p.getAttribute("d") || "").replace(/[MLZ]/g, " ");
      const nums = raw.trim().split(/\s+/).filter(Boolean).map(Number);
      const points = [];
      for (let i = 0; i + 1 < nums.length; i += 2) points.push([nums[i], nums[i + 1]]);
      if (!byDistrict.has(key)) byDistrict.set(key, { fill, points: [] });
      byDistrict.get(key).points.push(...points);
    }
    const entries = [...byDistrict.entries()];
    const THRESHOLD_PX = 3;
    const near = (ptsA, ptsB, threshold) => {
      const grid = new Map();
      for (const p of ptsA) {
        const cell = `${Math.floor(p[0] / threshold)},${Math.floor(p[1] / threshold)}`;
        if (!grid.has(cell)) grid.set(cell, []);
        grid.get(cell).push(p);
      }
      const t2 = threshold * threshold;
      for (const p of ptsB) {
        const cx = Math.floor(p[0] / threshold);
        const cy = Math.floor(p[1] / threshold);
        for (let dx = -1; dx <= 1; dx++) {
          for (let dy = -1; dy <= 1; dy++) {
            const bucket = grid.get(`${cx + dx},${cy + dy}`);
            if (!bucket) continue;
            for (const q of bucket) {
              const ddx = p[0] - q[0];
              const ddy = p[1] - q[1];
              if (ddx * ddx + ddy * ddy < t2) return true;
            }
          }
        }
      }
      return false;
    };
    const collisions = [];
    for (let i = 0; i < entries.length; i++) {
      for (let j = i + 1; j < entries.length; j++) {
        const [keyA, infoA] = entries[i];
        const [keyB, infoB] = entries[j];
        if (infoA.fill !== infoB.fill) continue;
        if (near(infoA.points, infoB.points, THRESHOLD_PX)) collisions.push([keyA, keyB]);
      }
    }
    return { districts: entries.length, collisions };
  });
  report(result.collisions.length === 0, `${label}: no two screen-adjacent districts render the same fill`, JSON.stringify(result));
  await context.close();
}

// A road's hit path is a quadratic bezier, often bowed and diagonal -- its
// AXIS-ALIGNED BOUNDING BOX centre is frequently not anywhere near the curve
// itself (the same class of bug the district-hue check above had). Find a
// point that is actually ON the path with the SVG platform API built for
// exactly this (getPointAtLength), then map it from the path's own local
// user space to viewport CSS pixels with getScreenCTM -- the same
// coordinate space page.mouse.click/page.touchscreen.tap expect.
// B4 (nested footprints, issue #82): footprints now tile most of a
// district's own area, so a road/street's exact MIDPOINT is more likely than
// before to sit visually under one of them (footprints paint AFTER
// drawRoads/drawStreets -- MapRenderer.paint()'s own z-order) -- a plain dot
// rarely covered a road's path pixel-for-pixel, but a district-wide footprint
// tiling regularly does. Several points along the path's length, not just
// the midpoint, so a caller can fall back toward the ribbon's own ends
// (nearer a district's edge, where fewer footprints extend) when the middle
// is covered.
async function pointOnPathAt(locator, fraction) {
  return locator.evaluate((el, fraction) => {
    const len = el.getTotalLength();
    const pt = el.getPointAtLength(len * fraction);
    const screenPt = pt.matrixTransform(el.getScreenCTM());
    return { x: screenPt.x, y: screenPt.y };
  }, fraction);
}
const PATH_SAMPLE_FRACTIONS = [0.5, 0.3, 0.7, 0.15, 0.85, 0.4, 0.6, 0.2, 0.8, 0.05, 0.95];

// B4: on the phone profile, dify's districts pack edge to edge at fit zoom
// (confirmed by looking at the CI screenshots -- the gap a road threads
// through is a couple of screen px at most there), and footprint mode's own
// tiling closes even more of what little gap dot mode left open. The same
// "zoom in and try again" fallback findEmptyPointWithZoomOut already uses
// for the inverse problem (finding empty space): re-reads the path list
// after each zoom (some roads leave the viewport, in-district streets don't
// exist at this scope but the same helper serves both callers below).
async function findTappablePathPoint(page, selector, vw, vh, maxZoomIns = 2) {
  for (let attempt = 0; attempt <= maxZoomIns; attempt++) {
    const paths = await page.locator(selector).all();
    for (const path of paths) {
      const dataK = await path.getAttribute("data-k");
      for (const fraction of PATH_SAMPLE_FRACTIONS) {
        const p = await pointOnPathAt(path, fraction);
        if (p.x < 0 || p.x > vw || p.y < 0 || p.y > vh) continue;
        if (await isPointClickable(page, p.x, p.y, dataK)) {
          return { point: p, dataK, checked: paths.length };
        }
      }
    }
    if (attempt < maxZoomIns) {
      await page.locator('button[aria-label="Zoom in"]').click();
      await page.waitForTimeout(500);
    }
  }
  const finalCount = await page.locator(selector).count();
  return { point: null, dataK: null, checked: finalCount };
}

// On-screen coordinates aren't enough on the PHONE profile specifically: the
// bottom drawer's peek strip, the zoom controls, the search box and the
// unconnected-files chip are all real DOM elements stacked on TOP of the
// map SVG, and a road/hub near the phone's edges can sit right under one of
// them. The desktop profile (fixed left rail, no floating bottom chrome)
// rarely has this problem, which is exactly the split CI saw: both new taps
// passed on desktop and failed on phone. `document.elementFromPoint` is the
// browser's own answer to "what would actually receive a tap here" --
// verifying with it (the same technique checkMultiPolygonHover's
// `polyCentre` already uses in this file for the same reason) catches this
// class of bug that a pure coordinate/geometry check cannot.
async function isPointClickable(page, x, y, expectedDataK) {
  return page.evaluate(
    ({ x, y, expectedDataK }) => {
      const el = document.elementFromPoint(x, y);
      if (!el) return false;
      const withKey = el.closest("[data-k]");
      return !!withKey && withKey.getAttribute("data-k") === expectedDataK;
    },
    { x, y, expectedDataK },
  );
}

// A3 (road tap, issue #82): a touch tap on a road shows its import-count
// explanation but must NOT change or clear the current selection, and must
// NOT fall through to the district underneath (the road's hit path is drawn
// after every district polygon -- MapRenderer.drawRoads -- so the browser's
// own hit-test already resolves the tap to the road, never the district;
// this asserts the OBSERVABLE consequence of that ordering, not the ordering
// itself). langgenius/dify's corpus has enough cross-district import edges
// that at least one road always draws (the top-12-by-flow selection alone
// guarantees it once there are >=12 cross-district pairs, which dify's
// 6347-file / dozens-of-districts graph clears easily). Roads only connect
// MAINLAND districts (see roads.ts's aggregateDistrictFlows), so at fit zoom
// -- which frames exactly the mainland extent -- a road's own midpoint
// should normally be on screen; this still tries every road in DOM order
// rather than assuming the first one is, since a bowed bezier's midpoint can
// stray slightly outside the frame even when both its endpoints are inside.
async function checkRoadTap(browser, base, profile) {
  const label = `road tap keeps selection (dify) / ${profile.name}`;
  console.log(`\n${label}`);
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1,
  });
  const page = await context.newPage();
  const doc = await (await context.request.get(`${base}/maps/langgenius/dify.json`)).json();
  const baseline = doc.F[doc.L[0][0]]; // a landmark file: guaranteed connected/on-map
  await page.goto(`${base}/langgenius/dify?file=${encodeURIComponent(baseline)}`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);
  const roads = await page.locator('svg.map-svg path.hit[data-k^="r:"]').all();
  if (roads.length === 0) {
    report(false, `${label}: at least one road is drawn`, "no road hit path found");
    await context.close();
    return;
  }
  const { width: vw, height: vh } = profile.viewport;
  const { point, checked } = await findTappablePathPoint(page, 'svg.map-svg path.hit[data-k^="r:"]', vw, vh);
  if (!point) {
    report(false, `${label}: at least one point along a road is on screen and not covered by chrome`, `checked ${checked} roads`);
    await context.close();
    return;
  }
  await tap(page, profile, point.x, point.y);
  report(
    new URL(page.url()).searchParams.get("file") === baseline,
    `${label}: tapping a road does not change or clear the current selection`,
  );
  const cardVisible = await page
    .locator(".tolmap-hover-card")
    .isVisible()
    .catch(() => false);
  const cardText = cardVisible ? await page.locator(".tolmap-hover-card").innerText() : "";
  report(cardVisible && /import/i.test(cardText), `${label}: tapping a road shows its import-count explanation`, cardText);
  await context.close();
}

// A4 (hub ring tap, issue #82): a hub's hit circle is `fill="transparent"`
// (its visible ring/dot are separate, non-interactive elements --
// MapRenderer.drawHubRings) with `data-k="f:i"`, which is what distinguishes
// it from the SAME file's own dot (always a real tint fill, never
// "transparent") when both carry the same data-k. Tapping it must select
// that file, same as tapping the dot would.
async function checkHubRingTap(browser, base, profile) {
  const label = `hub ring tap selects its file (dify) / ${profile.name}`;
  console.log(`\n${label}`);
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1,
  });
  const page = await context.newPage();
  const doc = await (await context.request.get(`${base}/maps/langgenius/dify.json`)).json();
  await page.goto(`${base}/langgenius/dify`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);
  // Every hub hit circle on screen, not just the first: on the phone
  // profile the first one found can sit under the bottom drawer's peek
  // strip, the zoom controls or the search box -- real chrome stacked on
  // top of the map SVG (see isPointClickable's own comment). The same
  // data-k also matches the file's own dot (circle.hit[data-k=...], a real
  // tint fill, never "transparent") -- disambiguate by fill, or Playwright's
  // strict mode throws on the 2-element match ("locator(...) resolved to 2
  // elements", found in CI) if a single-match locator is used instead.
  //
  // B4 (nested footprints, issue #82): a footprint-mode file's own dot is a
  // PATH, not a circle, so `circle.hit[data-k^="f:"]` no longer picks it up
  // at all -- but scope item 6's own small-footprint hit circles are ALSO
  // `fill="transparent"` `circle.hit[data-k^="f:"]` elements now, so
  // "transparent fill" alone no longer disambiguates a hub ring from a small
  // file's extra hit circle the way it used to. Filter to file indices whose
  // fan-in actually clears HUB_FANIN_THRESHOLD (map/hubs.ts) using the same
  // map JSON already fetched above, so this stays a check of hub rings
  // specifically, not of "some transparent circle."
  const HUB_FANIN_THRESHOLD = 30;
  const hubFileIndices = new Set(doc.N.flatMap((row, i) => (row[6] >= HUB_FANIN_THRESHOLD ? [i] : [])));
  const hubKeys = await page.evaluate((hubIndices) => {
    const els = [...document.querySelectorAll('svg.map-svg circle.hit[data-k^="f:"]')];
    return els
      .filter((el) => el.getAttribute("fill") === "transparent")
      .map((el) => el.getAttribute("data-k"))
      .filter((key) => hubIndices.includes(Number(key.slice(2))));
  }, [...hubFileIndices]);
  if (hubKeys.length === 0) {
    report(false, `${label}: at least one hub ring is on screen at fit zoom`, "no hub hit circle found");
    await context.close();
    return;
  }
  let chosenKey = null;
  let point = null;
  for (const key of hubKeys) {
    // B4: a hub file small enough ALSO to qualify for scope item 6's own
    // extra small-footprint hit circle now has TWO transparent circle.hit
    // elements sharing this data-k (the hub ring's own hit circle, and the
    // small-footprint one) -- `.first()` since either is a valid tap target
    // for the same file, and a strict-mode locator throws on the 2-element
    // match otherwise.
    const box = await page.locator(`svg.map-svg circle.hit[data-k="${key}"][fill="transparent"]`).first().boundingBox();
    if (!box) continue;
    const candidate = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
    if (await isPointClickable(page, candidate.x, candidate.y, key)) {
      chosenKey = key;
      point = candidate;
      break;
    }
  }
  if (!point) {
    report(false, `${label}: at least one hub ring is tappable (not covered by chrome)`, `checked ${hubKeys.length} rings`);
    await context.close();
    return;
  }
  const fileIndex = Number(chosenKey.slice(2));
  const expected = doc.F[fileIndex];
  await tap(page, profile, point.x, point.y);
  report(
    new URL(page.url()).searchParams.get("file") === expected,
    `${label}: tapping the ring selects the hub's file`,
    `expected=${expected}`,
  );
  await context.close();
}

// ---------------------------------------------------------------------------
// B4 (nested footprints, issue #82): file footprints by default, neighbourhood
// gutters/shades, streets. Scope item 9(a)-(f).
// ---------------------------------------------------------------------------

// 9(a): footprint mode draws a polygon (not a dot) per visible file, each
// carrying data-k="f:i" -- and, since a `<path>` has no cx/cy of its own, the
// data-cx/data-cy anchor every geometry-stability read in this file relies on
// (visibleDots(), readDot(), etc.).
async function checkFootprintModeDrawsPolygons(browser, base, profile) {
  const label = `footprint mode draws per-file polygons (dify) / ${profile.name}`;
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
  const info = await page.evaluate(() => {
    const paths = [...document.querySelectorAll('svg.map-svg path.hit[data-k^="f:"]')];
    const dots = document.querySelectorAll('svg.map-svg circle.hit[data-k^="f:"][fill]:not([fill="transparent"])').length;
    return {
      count: paths.length,
      allHaveAnchor: paths.length > 0 && paths.every((p) => p.hasAttribute("data-cx") && p.hasAttribute("data-cy")),
      dotsInstead: dots,
    };
  });
  report(info.count > 0, `${label}: at least one footprint polygon on screen`, JSON.stringify(info));
  report(info.allHaveAnchor, `${label}: every footprint polygon carries a data-cx/data-cy anchor`, JSON.stringify(info));
  report(info.dotsInstead === 0, `${label}: no dot-mode circles draw a file when P is present`, JSON.stringify(info));
  await context.close();
}

// 9(b), rewritten for the perf follow-up: scope item 6's dedicated small-
// footprint hit CIRCLES are retired (MapRenderer's own resolveKey() /
// footprints.ts's hitTestFootprint test the file's REAL polygon instead, off
// a world-space index, for every batched file at once -- see paint()'s own
// comment on why the circles became redundant). This proves that JS hit-
// test path directly: pick the file with the smallest on-screen footprint
// (excluding landmarks, which stay individually drawn and would exercise the
// OLD per-element DOM path instead), compute where its footprint_centroid
// lands on screen by reproducing the SAME fit-view transform
// checkFitFraming/fitFraming already validate against the live DOM
// elsewhere in this file, and tap exactly there with no DOM element lookup
// at all -- if a batched (pointer-events:none, no data-k) fill is what's
// actually under that point, only resolveKey()'s hit-test can be what
// selects the right file.
async function checkFootprintCoordinateHitTest(browser, base, profile) {
  const label = `tapping a footprint by coordinate selects it (dify) / ${profile.name}`;
  console.log(`\n${label}`);
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1,
  });
  const page = await context.newPage();
  const doc = await (await context.request.get(`${base}/maps/langgenius/dify.json`)).json();
  const landmarks = new Set(doc.L.map((l) => l[0]));
  await page.goto(`${base}/langgenius/dify`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);

  const batchCount = await page.locator("svg.map-svg path[data-footprint-batch]").count();
  report(batchCount > 0, `${label}: at least one batched footprint fill exists at fit zoom`, `${batchCount} batches`);

  const result = await page.evaluate(({ doc, landmarkArr }) => {
    const landmarkSet = new Set(landmarkArr);
    const svg = document.querySelector("svg.map-svg");
    const { width: vw, height: vh } = svg.getBoundingClientRect();
    const rect = vw <= 820 ? [16, 110, vw - 16, vh - 158] : [24, 12, vw - 24, vh - 38];
    // Same fit-view transform derivation fitFraming() already validates
    // against the live DOM elsewhere in this file (mainlandBounds + fit
    // scale/tx/ty, ported from map/geometry.ts's own formulas) -- computed
    // independently here rather than read off the app, since there's no API
    // to read MapRenderer's private k/tx/ty from outside it.
    const b = [Infinity, Infinity, -Infinity, -Infinity];
    const add = ([x, y]) => { b[0] = Math.min(b[0], x); b[1] = Math.min(b[1], y); b[2] = Math.max(b[2], x); b[3] = Math.max(b[3], y); };
    const mainland = (d) => !["island", "unconnected"].includes(doc.districts[String(d)].class);
    doc.N.forEach((row) => { if (mainland(row[0])) add([row[1], row[2]]); });
    for (const [d, district] of Object.entries(doc.districts)) {
      if (mainland(d)) district.blob.forEach((poly) => poly.forEach(add));
    }
    const scale = Math.min((rect[2] - rect[0]) / (b[2] - b[0]), (rect[3] - rect[1]) / (b[3] - b[1]));
    const tx = rect[0] + (rect[2] - rect[0] - (b[2] - b[0]) * scale) / 2 - b[0] * scale;
    const middle = (rect[1] + rect[3]) / 2;
    const centreY = vw <= 820 ? Math.max(rect[1] + (b[3] - b[1]) * scale / 2, Math.min(middle, 315)) : middle;
    const ty = centreY - (b[1] + b[3]) * scale / 2;

    const area = (poly) => {
      let a = 0;
      for (let i = 0; i < poly.length; i++) {
        const [x0, y0] = poly[i];
        const [x1, y1] = poly[(i + 1) % poly.length];
        a += x0 * y1 - x1 * y0;
      }
      return Math.abs(a) / 2;
    };
    // Even-odd ray cast, ported from map/roads.ts's inRings -- verifies the
    // reported footprint_centroid actually lands inside its OWN polygon
    // before trusting it as a tap target: footprints.ts's own hitTestFootprint
    // doc comment records that ~15% of a fixture's files (measured on
    // django) do NOT satisfy this, almost always the smallest, most
    // degenerate cells -- exactly the population this test is drawn from.
    // Skipping those (rather than picking the single smallest file
    // unconditionally) keeps this a test of the hit-testing CODE, not of
    // that separate, out-of-scope layout precision question.
    const inPoly = (p, poly) => {
      let c = false;
      for (let a = 0, bI = poly.length - 1; a < poly.length; bI = a++) {
        const [xi, yi] = poly[a];
        const [xj, yj] = poly[bI];
        if (yi > p[1] !== yj > p[1] && p[0] < ((xj - xi) * (p[1] - yi)) / (yj - yi) + xi) c = !c;
      }
      return c;
    };

    // Screen-space equivalent side (sqrt(world area) * scale), the same
    // quantity MapRenderer's own districtFootprintsLarge compares against
    // its ~24px threshold. Banded to [10, 22]px rather than "smallest
    // possible": the ABSOLUTE smallest footprints are exactly where two
    // INDEPENDENTLY computed transforms (this script's own scale/tx/ty vs
    // the live app's k/tx/ty) can disagree by more than the polygon's own
    // size, tipping a world-correct point into a neighbouring file's (or, if
    // it's a bigger, individually-drawn landmark sitting nearby, THAT file's)
    // territory on screen even though `inPoly` below holds exactly in world
    // space -- a real precision hazard of testing pixel-perfect taps on the
    // tiniest targets, not a hit-testing bug. checkFitFraming's own
    // pointError assertion elsewhere in this file accepts UP TO 2px of
    // exactly this kind of discrepancy between an independently-computed
    // transform and the live one; the margin check just below is sized
    // against that same accepted tolerance, and 10px of on-screen size
    // comfortably clears it while staying well under the batching threshold.
    const candidates = [];
    for (let i = 0; i < doc.F.length; i++) {
      if (landmarkSet.has(i)) continue;
      if (!mainland(doc.N[i][0])) continue;
      const poly = doc.P?.[String(i)];
      const c = doc.footprint_centroids?.[i];
      if (!poly || poly.length < 3 || !c) continue;
      const screenSide = Math.sqrt(area(poly)) * scale;
      if (screenSide < 10 || screenSide > 22) continue;
      candidates.push({ i, screenSide, poly, c });
    }
    candidates.sort((x, y) => x.screenSide - y.screenSide);
    const marginWorld = 2.5 / scale;
    for (const cand of candidates) {
      // Robust to the SAME ~2px transform tolerance checkFitFraming already
      // accepts: the centroid AND every point up to that margin away (in
      // world units) must all still resolve inside this file's own polygon.
      const probes = [
        cand.c,
        [cand.c[0] + marginWorld, cand.c[1]],
        [cand.c[0] - marginWorld, cand.c[1]],
        [cand.c[0], cand.c[1] + marginWorld],
        [cand.c[0], cand.c[1] - marginWorld],
      ];
      if (!probes.every((p) => inPoly(p, cand.poly))) continue;
      const sx = cand.c[0] * scale + tx;
      const sy = cand.c[1] * scale + ty;
      if (sx < rect[0] || sx > rect[2] || sy < rect[1] || sy > rect[3]) continue;
      return { i: cand.i, sx, sy, wx: cand.c[0], wy: cand.c[1] };
    }
    return null;
  }, { doc, landmarkArr: [...landmarks] });

  if (!result) {
    report(false, `${label}: setup`, "no small, on-screen, self-containing footprint found");
    await context.close();
    return;
  }
  await tap(page, profile, result.sx, result.sy);
  const selectedFile = new URL(page.url()).searchParams.get("file");
  // Assert the OUTCOME is geometrically justified rather than predicting
  // which exact file wins -- multiple tiny, adjacent, reserved-minimum-pixel
  // cells (finding 29) can genuinely all contain the same point at this
  // scale (footprints.ts's hitTestFootprint ties to the lowest file index in
  // its bucket, deterministically, but this script doesn't replicate that
  // bucket's exact contents/order independently). Verifying the selected
  // file's OWN polygon contains the exact world point that was tapped is a
  // direct proof the JS hit-test worked, without depending on tie-breaking.
  const selectedContains = selectedFile
    ? await page.evaluate(
        ({ file, wx, wy }) => {
          // Same even-odd ray cast as the candidate search above.
          return fetch("/maps/langgenius/dify.json")
            .then((r) => r.json())
            .then((doc) => {
              const i = doc.F.indexOf(file);
              const poly = doc.P?.[String(i)];
              if (!poly) return false;
              let c = false;
              for (let a = 0, b = poly.length - 1; a < poly.length; b = a++) {
                const [xi, yi] = poly[a];
                const [xj, yj] = poly[b];
                if (yi > wy !== yj > wy && wx < ((xj - xi) * (wy - yi)) / (yj - yi) + xi) c = !c;
              }
              return c;
            });
        },
        { file: selectedFile, wx: result.wx, wy: result.wy },
      )
    : false;
  report(
    !!selectedFile && selectedContains,
    `${label}: tapping a small footprint by coordinate selects a file whose polygon actually contains that point`,
    `guessed=${doc.F[result.i]} selected=${selectedFile}`,
  );
  await context.close();
}

// 9(c): a selected file's persistent import lines (MapRenderer.paint()'s
// selNeighbours block) start at the SAME anchor its own footprint polygon
// reports via data-cx/data-cy -- not at N's layout site (map/geometry.ts's
// `fileXY`, scope item 3).
async function checkImportLinesAnchorOnFootprintCentroids(browser, base) {
  const label = "selection import lines anchor on footprint centroids (dify) / desktop";
  console.log(`\n${label}`);
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  const page = await context.newPage();
  const doc = await (await context.request.get(`${base}/maps/langgenius/dify.json`)).json();
  const outCount = new Map();
  for (const [a] of doc.E) outCount.set(a, (outCount.get(a) ?? 0) + 1);
  const target = [...outCount.entries()].sort((a, b) => b[1] - a[1])[0]?.[0];
  if (target == null) {
    report(false, `${label}: setup`, "no file with out edges found");
    await context.close();
    return;
  }
  await page.goto(`${base}/langgenius/dify?geo=r&layer=d&file=${encodeURIComponent(doc.F[target])}`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);
  const result = await page.evaluate((key) => {
    const own = document.querySelector(`svg.map-svg [data-k="${key}"]`);
    if (!own) return null;
    const ocx = own.getAttribute("data-cx") ?? own.getAttribute("cx");
    const ocy = own.getAttribute("data-cy") ?? own.getAttribute("cy");
    const lines = [...document.querySelectorAll("svg.map-svg g line")].filter((l) => {
      const stroke = l.getAttribute("stroke");
      return stroke === "var(--hot)" || stroke === "var(--cold)";
    });
    const matching = lines.filter((l) => l.getAttribute("x1") === ocx && l.getAttribute("y1") === ocy);
    return { total: lines.length, matching: matching.length, ocx, ocy };
  }, `f:${target}`);
  report(
    !!result && result.total > 0 && result.matching === result.total,
    `${label}: every persistent import line starts at the selected file's footprint anchor`,
    JSON.stringify(result),
  );
  await context.close();
}

// 9(d): focusing a district draws streets (scope item 4), and tapping one
// shows its explanation WITHOUT clearing the district focus -- the exact
// "keeps the selection" contract checkRoadTap already proves for a road one
// level up.
async function checkStreetsAndTap(browser, base, profile) {
  const label = `district focus draws streets and a street tap keeps it (dify) / ${profile.name}`;
  console.log(`\n${label}`);
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1,
  });
  const page = await context.newPage();
  const doc = await (await context.request.get(`${base}/maps/langgenius/dify.json`)).json();
  const workflowFile = doc.F.findIndex((path) => path === "web/app/components/workflow/types.ts");
  const workflowDistrict = doc.N[workflowFile][0];
  await page.goto(`${base}/langgenius/dify?d=${workflowDistrict}`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector('button[aria-label="Zoom to district"]');
  await page.locator('button[aria-label="Zoom to district"]').click({ force: true });
  await page.waitForTimeout(850);
  const streets = await page.locator('svg.map-svg path.hit[data-k^="st:"]').all();
  report(streets.length > 0, `${label}: at least one street is drawn once the district is focused`, `${streets.length} streets`);
  if (streets.length === 0) {
    await context.close();
    return;
  }
  const { width: vw, height: vh } = profile.viewport;
  const { point, dataK, checked } = await findTappablePathPoint(page, 'svg.map-svg path.hit[data-k^="st:"]', vw, vh);
  if (!point) {
    report(false, `${label}: at least one point along a street is on screen and not covered by chrome`, `checked ${checked}`);
    await context.close();
    return;
  }
  await tap(page, profile, point.x, point.y);
  report(
    new URL(page.url()).searchParams.get("d") === String(workflowDistrict) && !new URL(page.url()).searchParams.has("file"),
    `${label}: tapping a street does not change or clear the district focus`,
    `data-k=${dataK} url=${page.url()}`,
  );
  const cardVisible = await page
    .locator(".tolmap-hover-card")
    .isVisible()
    .catch(() => false);
  const cardText = cardVisible ? await page.locator(".tolmap-hover-card").innerText() : "";
  report(cardVisible && /import/i.test(cardText), `${label}: tapping a street shows its import-count explanation`, cardText);
  await context.close();
}

// 9(e): neighbourhood labels (scope item 5) need MORE screen real estate per
// neighbourhood than the opening fit view gives dify's 512 of them -- they
// should appear once a district is focused (deeper zoom), essentially absent
// at the opening overview.
async function checkNeighbourhoodLabelsAtDeeperZoom(browser, base, profile) {
  const label = `neighbourhood labels appear at a deeper zoom (dify) / ${profile.name}`;
  console.log(`\n${label}`);
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1,
  });
  const page = await context.newPage();
  const doc = await (await context.request.get(`${base}/maps/langgenius/dify.json`)).json();
  const workflowFile = doc.F.findIndex((path) => path === "web/app/components/workflow/types.ts");
  const workflowDistrict = doc.N[workflowFile][0];

  await page.goto(`${base}/langgenius/dify`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);
  const atFit = await page.locator("[data-neighbourhood-label]").count();

  await page.goto(`${base}/langgenius/dify?d=${workflowDistrict}`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector('button[aria-label="Zoom to district"]');
  await page.locator('button[aria-label="Zoom to district"]').click({ force: true });
  await page.waitForTimeout(850);
  const focused = await page.locator("[data-neighbourhood-label]").count();

  report(focused > atFit, `${label}: focusing a district reveals more neighbourhood labels than the opening fit`, `fit=${atFit} focused=${focused}`);
  await context.close();
}

// 9(f): an old map with no `P` at all (any committed data/ fixture the
// catalogue still serves) renders dots, unchanged -- footprint mode is opt-in
// per document, never forced.
async function checkLegacyMapWithoutFootprints(browser, base) {
  const label = "map without P still renders dots (encode/httpx) / desktop";
  console.log(`\n${label}`);
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 } });
  const page = await context.newPage();
  const doc = await (await context.request.get(`${base}/maps/encode/httpx.json`)).json();
  report(!doc.P, `${label}: fixture has no P (setup)`, JSON.stringify(Object.keys(doc)));
  await page.goto(`${base}/encode/httpx`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(700);
  const info = await page.evaluate(() => ({
    dots: document.querySelectorAll('svg.map-svg circle.hit[data-k^="f:"]').length,
    footprintPaths: document.querySelectorAll('svg.map-svg path.hit[data-k^="f:"]').length,
  }));
  report(info.dots > 0 && info.footprintPaths === 0, `${label}: files render as dot circles, not footprint polygons`, JSON.stringify(info));
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
    // issue #82 (district hues, import roads, hub rings, always-on district names)
    await checkDistrictHueAdjacency(browser, args.base);
    for (const profile of PROFILES) await checkRoadTap(browser, args.base, profile);
    for (const profile of PROFILES) await checkHubRingTap(browser, args.base, profile);
    // B4 (nested footprints, issue #82): scope item 9(a)-(f)
    for (const profile of PROFILES) await checkFootprintModeDrawsPolygons(browser, args.base, profile);
    for (const profile of PROFILES) await checkFootprintCoordinateHitTest(browser, args.base, profile);
    await checkImportLinesAnchorOnFootprintCentroids(browser, args.base);
    for (const profile of PROFILES) await checkStreetsAndTap(browser, args.base, profile);
    for (const profile of PROFILES) await checkNeighbourhoodLabelsAtDeeperZoom(browser, args.base, profile);
    await checkLegacyMapWithoutFootprints(browser, args.base);
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
