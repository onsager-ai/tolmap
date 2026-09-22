#!/usr/bin/env node
// Regression check for fix/keep-view-on-select: MapCanvas's ResizeObserver
// used to be recreated on every [doc, geo, layer, sel, selSym, selD,
// selTerrain, route] change, and ResizeObserver.observe() always delivers
// one synchronous "initial size" callback on subscribe -- size unchanged or
// not. That callback called MapRenderer.resize(), which always called
// fit(), so tapping a file dot, tapping it away, or switching the layer all
// snapped the view back to the opening fit. See MapCanvas.tsx's
// ResizeObserver effect and MapRenderer.resize()'s doc comment for the fix.
//
// This drives a real Vite dev server (own port, never 5173 -- that belongs
// to another worktree's live session) with Playwright, on both a map small
// enough to eyeball (django/django) and one with a denser, more irregular
// mainland silhouette (n8n-io/n8n), at a desktop size and a touch phone
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
// load and after a repo switch, plus that desktop's auto-select of the top
// landmark on load still fires.
//
// Usage:
//   pnpm exec vite --port 5176 --strictPort &
//   node scripts/check-view-stability.mjs [--base http://127.0.0.1:5176]

import { chromium } from "playwright";

function parseArgs(argv) {
  const args = { base: "http://127.0.0.1:5176" };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--base") args.base = argv[++i];
    else throw new Error(`unknown arg: ${argv[i]}`);
  }
  return args;
}

const MAPS = ["django/django", "n8n-io/n8n"];
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
    return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
  });
}

// fit()'s own padding constant (MapRenderer.ts's fit()); at least one axis
// of the fitted mainland touches it exactly, the other has equal or more
// margin (fit() picks the smaller scale of the two axes and centres on the
// other) -- so "fits inside the pad" is the general, always-true shape of a
// correctly fitted view, not something specific to one map's aspect ratio.
const FIT_PAD = 46;

function fitsWithinPad(box, vw, vh, eps = 2) {
  if (!box) return false;
  return box.x >= FIT_PAD - eps && box.y >= FIT_PAD - eps && box.x + box.w <= vw - FIT_PAD + eps && box.y + box.h <= vh - FIT_PAD + eps;
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
 * district/file/symbol/terrain hit target, and not chrome (the sidebar, the
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
    return { cx: parseFloat(el.getAttribute("cx") ?? el.getAttribute("x")), cy: parseFloat(el.getAttribute("cy") ?? el.getAttribute("y")) };
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

  // Desktop auto-selects the top landmark on load (MapView.tsx); check that
  // before clearing it -- this fix touched the same repoKey effect that
  // precedes it, so it's worth confirming that behaviour is still intact
  // rather than assuming it because nothing here looks like it should touch
  // it.
  if (!profile.isMobile) {
    const autoSelectedFile = await page.evaluate(() => new URL(location.href).searchParams.get("file"));
    report(!!autoSelectedFile, `${label}: desktop still auto-selects the top landmark on load`);
  }

  // Desktop auto-selects the top landmark on load (MapView.tsx); clear it at
  // fit zoom, where there's reliably blank margin along the fit's letterboxed
  // axis, before establishing the zoomed-in baseline below.
  if (!profile.isMobile) {
    const empty = await findEmptyPoint(page, vw, vh);
    if (empty) await tap(page, profile, empty[0], empty[1]);
  }

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
  await page.locator('select[aria-label="Repository"]').selectOption("n8n-io/n8n");
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

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const browser = await chromium.launch();
  for (const slug of MAPS) {
    for (const profile of PROFILES) {
      await runOne({ browser, base: args.base, slug, profile });
    }
  }
  for (const profile of PROFILES) {
    await checkRepoSwitch(browser, args.base, profile);
  }
  await browser.close();

  console.log(`\n${checks - failures}/${checks} checks passed`);
  if (failures > 0) {
    console.error(`${failures} check(s) failed`);
    process.exit(1);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
