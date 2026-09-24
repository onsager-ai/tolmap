#!/usr/bin/env node
// Issue #51: a repeatable browser benchmark for pan/zoom performance on the
// corpus maps, so the "transform during gestures" work in MapRenderer.ts is
// judged by numbers instead of a feeling that it's smoother. Runs against a
// Vite dev server for THIS worktree (never the one on 5173 -- that belongs
// to another worktree's live session) and drives a real Chromium tab with
// Playwright: real PointerEvents for drag/pinch, real wheel events for
// desktop zoom, so the numbers reflect what MapRenderer.ts's own pointer
// handlers actually do, not a synthetic shortcut around them.
//
// Usage:
//   pnpm exec vite --port 5174 --strictPort &   # or let --start-server handle it
//   node scripts/perf-bench.mjs --out <path>.json [--base http://localhost:5174]
//     [--maps owner/repo,owner/repo] [--runs 3] [--start-server]
//     [--big-files 5000] [--gesture-cap-ms 60000]
//
// Metrics per map x profile:
//   - firstMapMs: navigation -> first frame with the map <svg> populated
//   - drawsOnLoad: issue #51 -- performance.getEntriesByName("tolmap:draw")
//     count from navigation through the 700ms settle wait below (the initial
//     fit()/auto-select glide's own final paint included). Reads straight off
//     the same Performance timeline the draw-ms stats do (window.__TOLMAP_PERF__
//     is set by installRecorder() below, via addInitScript, before any app
//     code runs, so no draw before the app's own first one is missed). Target
//     is 1: the redundant fit()+render() double-paint and the desktop
//     landmark auto-select's own extra paint both used to push this to 3
//     (5 in dev, under React StrictMode's double-invoked effects -- see the
//     PR description). --check-single-paint below turns this into a hard
//     assertion for a fixed map set; every run still reports the number.
//   - draw ms (p50/p95/max): performance.getEntriesByName("tolmap:draw")
//     durations, read straight off the Performance timeline (see the
//     mark/measure pair MapRenderer.draw() leaves always-on)
//   - drag frame ms (p50/p95) + frames>50ms: a scripted 40-step drag across
//     half the viewport
//   - zoom frame ms (p50/p95) + frames>50ms: 20 wheel-zoom steps in then out
//     (desktop), or a scripted two-finger pinch via CDP touch events, with a
//     double-tap zoom fallback, on phone
//
// Each configuration (map x profile x phase) runs `--runs` times; the
// reported numbers are the median run's summary statistics, not an average
// pooled across runs -- a single slow run (GC pause, the low-priority
// `tolmap build` corpus job sharing the machine) would otherwise drag every
// number toward it instead of being the outlier it is.
//
// Cost control for the big maps (n8n, aws-sdk-go-v2, and any map over
// --big-files, default 5000): unmodified MapRenderer.draw() takes seconds
// per frame there (measured: dify at 6347 files was already ~1.3-1.9s/draw
// on desktop, ~5s/draw on the throttled phone profile), and each scripted
// gesture step is one synchronous pointer/wheel event dispatch -- Playwright
// awaits it, which means the event's handler (draw() included) runs to
// completion before the next step fires. A fixed 40-80 step gesture at that
// per-step cost is minutes, and three of those per config is the run going
// from a coffee break to an afternoon. So above the file threshold: ONE run
// (not three -- there is nothing to take a median of if only one completes
// in reasonable time), and each gesture phase runs against a wall-clock
// budget (--gesture-cap-ms, default 60000) instead of a fixed step count --
// it takes as many steps as fit in the budget and reports how many that was,
// rather than blocking however long the fixed count happens to take. Small
// maps (flask, django) are unaffected: unlimited time budget, the original
// fixed step count, still median-of-3. The threshold and cap are read the
// same way for a "before" and an "after" run of this same script, so the
// two are comparable.

import { chromium } from "playwright";
import { spawn } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";

const DEFAULT_MAPS = [
  "pallets/flask",
  "django/django",
  "langgenius/dify",
  "n8n-io/n8n",
  "aws/aws-sdk-go-v2",
];

const DEFAULT_BIG_MAP_FILES = 5000;
const DEFAULT_GESTURE_CAP_MS = 60_000;

// Issue #51: the fixed map set --check-single-paint asserts drawsOnLoad<=1
// against, matching what the issue asked for specifically (django, dify) --
// not the full DEFAULT_MAPS list, since n8n and aws-sdk-go-v2 are "ultra"-band
// maps this machine must not open a browser against (see CLAUDE.md's machine
// constraint), and flask is too tiny to be worth a dedicated assertion.
const SINGLE_PAINT_CHECK_MAPS = ["django/django", "langgenius/dify"];

function parseArgs(argv) {
  const args = {
    base: "http://localhost:5174",
    maps: DEFAULT_MAPS,
    runs: 3,
    startServer: false,
    bigMapFiles: DEFAULT_BIG_MAP_FILES,
    gestureCapMs: DEFAULT_GESTURE_CAP_MS,
    checkSinglePaint: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--out") args.out = argv[++i];
    else if (a === "--base") args.base = argv[++i];
    else if (a === "--maps") args.maps = argv[++i].split(",").map((s) => s.trim()).filter(Boolean);
    else if (a === "--runs") args.runs = Number(argv[++i]);
    else if (a === "--start-server") args.startServer = true;
    else if (a === "--big-files") args.bigMapFiles = Number(argv[++i]);
    else if (a === "--gesture-cap-ms") args.gestureCapMs = Number(argv[++i]);
    // Issue #51: opt-in, not on by default -- this script is shared with
    // other in-flight work on MapRenderer.ts (paint() internals, pins,
    // badges, hover), and a hard assertion baked into every run here would
    // fail THEIR runs over something unrelated to what they're touching.
    else if (a === "--check-single-paint") args.checkSinglePaint = true;
    else throw new Error(`unknown arg: ${a}`);
  }
  if (!args.out) throw new Error("--out <path> is required");
  return args;
}

/** File counts per map slug, read from the same public/maps/index.json the
 * app itself serves the catalogue from -- so "is this a big map" uses the
 * one place that number is already recorded, rather than a second copy of
 * it hardcoded into this script that could drift from the actual fixtures.
 * Missing/unreadable index.json (a fresh clone that hasn't run collect-maps
 * yet) falls back to treating every map as small: the original fixed-step,
 * three-run behaviour, which is always correct, just potentially slow. */
async function loadMapFileCounts() {
  const indexPath = path.resolve(new URL(".", import.meta.url).pathname, "../public/maps/index.json");
  try {
    const raw = await readFile(indexPath, "utf8");
    const entries = JSON.parse(raw);
    const sizes = new Map();
    for (const m of entries) sizes.set(m.slug, m.files);
    return sizes;
  } catch (err) {
    console.warn(`perf-bench: could not read ${indexPath} (${err.message}); treating every map as small`);
    return new Map();
  }
}

function quantile(sorted, q) {
  if (sorted.length === 0) return 0;
  const idx = Math.min(sorted.length - 1, Math.max(0, Math.round(q * (sorted.length - 1))));
  return sorted[idx];
}

function summarize(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return {
    p50: round2(quantile(sorted, 0.5)),
    p95: round2(quantile(sorted, 0.95)),
    max: round2(sorted.length ? sorted[sorted.length - 1] : 0),
    n: sorted.length,
  };
}
function round2(v) {
  return Math.round(v * 100) / 100;
}
// The median RUN, by its p50 draw time -- not an average pooled across runs.
// A slow run (GC, the low-priority corpus build job) is an outlier to
// discard, not a data point to blend in.
function medianRun(runs, keyFn) {
  const withKey = runs.map((r) => ({ r, k: keyFn(r) })).sort((a, b) => a.k - b.k);
  return withKey[Math.floor((withKey.length - 1) / 2)].r;
}

const PROFILES = {
  desktop: { viewport: { width: 1440, height: 900 }, isMobile: false, hasTouch: false, cpuThrottle: 1 },
  phone: { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, cpuThrottle: 4 },
};

/** Installed once per page via addInitScript, so it is present before any
 * of the app's own scripts run (including the very first draw()). Records
 * every requestAnimationFrame callback-to-callback delta into a flat array
 * the bench script can slice by index before/after a gesture phase, and
 * exposes a helper to snapshot performance "tolmap:draw" measure durations
 * the same way -- both read out via page.evaluate() rather than pushed over
 * the wire per-frame, so instrumentation never perturbs the thing it's
 * timing. Also sets window.__TOLMAP_PERF__ = true, the flag MapRenderer's
 * draw() gates its performance.mark/measure pair behind (see its comment):
 * the User Timing buffer has no eviction, so a real session leaves it off
 * by default, and this is the one place -- before any app code runs --
 * that can turn it on for a bench run specifically. */
function installRecorder() {
  window.__TOLMAP_PERF__ = true;
  window.__bench = { frameTimes: [] };
  let last = null;
  function loop(t) {
    if (last != null) window.__bench.frameTimes.push(t - last);
    last = t;
    requestAnimationFrame(loop);
  }
  requestAnimationFrame(loop);
}

async function markStart(page) {
  return page.evaluate(() => ({
    frameIdx: window.__bench.frameTimes.length,
    drawIdx: performance.getEntriesByName("tolmap:draw", "measure").length,
  }));
}
async function collectSince(page, start) {
  return page.evaluate(
    ({ frameIdx, drawIdx }) => {
      const frames = window.__bench.frameTimes.slice(frameIdx);
      const draws = performance
        .getEntriesByName("tolmap:draw", "measure")
        .slice(drawIdx)
        .map((e) => e.duration);
      return { frames, draws };
    },
    start,
  );
}

async function waitForFirstMap(page, url) {
  const t0 = Date.now();
  await page.goto(url, { waitUntil: "domcontentloaded" });
  // Playwright's waitForFunction signature is (pageFunction, arg, options) --
  // the options object has to be the THIRD argument, not the second (a
  // previous version of this script passed { timeout } as `arg`, where
  // Playwright ignores it and silently falls back to its 30s default; that
  // is exactly what timed out on aws-sdk-go-v2/desktop, a map big enough for
  // parse + first paint to occasionally run past 30s even unthrottled).
  //
  // This wait is deliberately generous (5 minutes) and NOT the same budget
  // as the gesture time cap above: time-to-first-map is itself one of the
  // things this script measures, so truncating it would throw away the
  // number rather than bound an incidental cost. aws-sdk-go-v2 on the
  // throttled phone profile measured past 120s here -- a real result (the
  // whole premise of issue #51), not a script bug to paper over.
  await page.waitForFunction(
    () => {
      const svg = document.querySelector("svg.map-svg");
      return !!svg && svg.childElementCount > 0 && svg.querySelector("g")?.childElementCount > 0;
    },
    undefined,
    { timeout: 300_000 },
  );
  return Date.now() - t0;
}

// Every scripted gesture below takes { steps, timeCapMs } and returns
// { steps, completed, capped }: `steps` is the nominal (fixed) step count,
// `completed` is how many actually fit inside timeCapMs (Infinity for the
// small maps, so completed === steps there always), and `capped` says
// whether the wall-clock budget cut the gesture short. Checked once per
// step rather than mid-step: a step is one synchronous event dispatch
// (Playwright awaits its handler, draw() included), so that's the finest
// granularity available anyway.

/** Real PointerEvents via Playwright's mouse API (desktop) -- this exercises
 * MapRenderer's own pointerdown/pointermove/pointerup handlers exactly as a
 * real drag would, including the moved<5 tap threshold and pointer capture
 * (bug fix #1/#2 in MapRenderer.ts), rather than calling a private method
 * directly. */
async function scriptedDragDesktop(page, vw, vh, { steps = 40, timeCapMs = Infinity } = {}) {
  const y = vh / 2;
  const x0 = vw * 0.25;
  const x1 = vw * 0.75;
  await page.mouse.move(x0, y);
  await page.mouse.down();
  const deadline = Date.now() + timeCapMs;
  let completed = 0;
  for (let i = 1; i <= steps; i++) {
    if (Date.now() > deadline) break;
    const x = x0 + ((x1 - x0) * i) / steps;
    await page.mouse.move(x, y, { steps: 1 });
    completed = i;
  }
  await page.mouse.up();
  return { steps, completed, capped: completed < steps };
}

/** Phone drag via CDP Input.dispatchTouchEvent -- Chromium synthesizes
 * PointerEvents from these the same way it does for a real finger, so this
 * exercises the identical MapRenderer pointer path as scriptedDragDesktop,
 * just through the touch input pipeline instead of the mouse one. */
async function scriptedDragTouch(cdp, vw, vh, { steps = 40, timeCapMs = Infinity } = {}) {
  const y = vh / 2;
  const x0 = vw * 0.25;
  const x1 = vw * 0.75;
  const touchPoint = (x) => ({ x, y, id: 1 });
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [touchPoint(x0)] });
  const deadline = Date.now() + timeCapMs;
  let completed = 0;
  for (let i = 1; i <= steps; i++) {
    if (Date.now() > deadline) break;
    const x = x0 + ((x1 - x0) * i) / steps;
    await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [touchPoint(x)] });
    completed = i;
  }
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  return { steps, completed, capped: completed < steps };
}

async function scriptedWheelZoom(page, vw, vh, { steps = 20, timeCapMs = Infinity } = {}) {
  const cx = vw / 2;
  const cy = vh / 2;
  await page.mouse.move(cx, cy);
  const total = steps * 2;
  const deadline = Date.now() + timeCapMs;
  let completed = 0;
  for (let i = 0; i < steps; i++) {
    if (Date.now() > deadline) break;
    await page.mouse.wheel(0, -120); // zoom in
    completed++;
  }
  for (let i = 0; i < steps; i++) {
    if (Date.now() > deadline) break;
    await page.mouse.wheel(0, 120); // zoom out
    completed++;
  }
  return { steps: total, completed, capped: completed < total };
}

/** Two-finger pinch via CDP Input.dispatchTouchEvent with two touch points
 * moving apart then together -- the same synthesis path MapRenderer's own
 * pinch handling (pts.size === 2) is built to receive from a real device. */
async function scriptedPinchTouch(cdp, vw, vh, { steps = 20, timeCapMs = Infinity } = {}) {
  const cx = vw / 2;
  const cy = vh / 2;
  const pointsAt = (halfSpread) => [
    { x: cx - halfSpread, y: cy, id: 1 },
    { x: cx + halfSpread, y: cy, id: 2 },
  ];
  const start = 30;
  const end = Math.min(vw, vh) * 0.4;
  const total = steps * 2;
  const deadline = Date.now() + timeCapMs;
  let completed = 0;
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: pointsAt(start) });
  for (let i = 1; i <= steps; i++) {
    if (Date.now() > deadline) break;
    const spread = start + ((end - start) * i) / steps;
    await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: pointsAt(spread) });
    completed++;
  }
  for (let i = 1; i <= steps; i++) {
    if (Date.now() > deadline) break;
    const spread = end - ((end - start) * i) / steps;
    await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: pointsAt(spread) });
    completed++;
  }
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  return { steps: total, completed, capped: completed < total };
}

async function doubleTapZoom(page, vw, vh) {
  const cx = vw / 2;
  const cy = vh / 2;
  await page.touchscreen.tap(cx, cy);
  await page.waitForTimeout(80);
  await page.touchscreen.tap(cx, cy);
  return { steps: 2, completed: 2, capped: false };
}

async function runOneConfig({ browser, base, slug, profile, timeCapMs }) {
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.isMobile ? 2 : 1,
  });
  const page = await context.newPage();
  await page.addInitScript(installRecorder);
  const cdp = await context.newCDPSession(page);
  if (profile.cpuThrottle !== 1) {
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: profile.cpuThrottle });
  }

  const url = `${base}/${slug}`;
  const firstMapMs = await waitForFirstMap(page, url);
  // Let the fit() glide (issue #51's own settle path included) finish and
  // the frame recorder accumulate a quiet baseline before timing gestures.
  await page.waitForTimeout(700);
  // Issue #51: every tolmap:draw() so far is a paint on load -- nothing
  // between installRecorder()'s addInitScript (runs before any app code) and
  // this point is a gesture, so the whole count belongs to load/fit/
  // auto-select, not to drag/zoom (those are measured separately, starting
  // from their own markStart() below).
  const drawsOnLoad = await page.evaluate(() => performance.getEntriesByName("tolmap:draw", "measure").length);

  const { width: vw, height: vh } = profile.viewport;
  let pinchMode = "cdp-pinch";

  const dragStart = await markStart(page);
  const dragStats = profile.hasTouch
    ? await scriptedDragTouch(cdp, vw, vh, { timeCapMs })
    : await scriptedDragDesktop(page, vw, vh, { timeCapMs });
  await page.waitForTimeout(200); // let the settle-redraw (issue #51 part 2) land
  const drag = { ...(await collectSince(page, dragStart)), ...dragStats };

  const zoomStart = await markStart(page);
  let zoomStats;
  if (profile.hasTouch) {
    try {
      zoomStats = await scriptedPinchTouch(cdp, vw, vh, { timeCapMs });
    } catch (err) {
      pinchMode = "double-tap (pinch via CDP failed: " + String(err?.message || err) + ")";
      zoomStats = await doubleTapZoom(page, vw, vh);
    }
  } else {
    zoomStats = await scriptedWheelZoom(page, vw, vh, { timeCapMs });
  }
  await page.waitForTimeout(200);
  const zoom = { ...(await collectSince(page, zoomStart)), ...zoomStats };

  await context.close();

  return {
    firstMapMs,
    drawsOnLoad,
    pinchMode: profile.hasTouch ? pinchMode : undefined,
    drag,
    zoom,
  };
}

function frameStats(frames) {
  const s = summarize(frames);
  return { p50: s.p50, p95: s.p95, over50: frames.filter((f) => f > 50).length, n: frames.length };
}

// Issue #82 C2 scope item 7: "Add to CI's perf step a dify deep-zoom paint
// measurement with cards on." Picks the file with the most symbols in the
// bundled district-0 symbols fixture (web/check-fixtures/langgenius__
// dify.symbols.tar.gz) -- the same file the CI workflow's other symbol
// checks/screenshots use, a large real-world class-and-function file that
// exercises the card pass under real load. Reads it off the SERVED fixtures
// (never hardcodes a path or index) so a fixture regeneration can't silently
// desync this from what's actually being measured. Returns null (not a
// thrown error) for anything short of a clean answer -- a map/symbols
// fixture that isn't being served (a plain perf-bench run against some OTHER
// map set, or a fixture temporarily missing) skips this one measurement
// rather than failing the whole bench.
async function pickDeepZoomCardsTarget(base) {
  try {
    const [mapDoc, symbols] = await Promise.all([
      fetch(`${base}/maps/langgenius/dify.json`).then((r) => (r.ok ? r.json() : null)),
      fetch(`${base}/maps/langgenius/dify.symbols/0.json`).then((r) => (r.ok ? r.json() : null)),
    ]);
    if (!mapDoc || !symbols) return null;
    const counts = new Map();
    for (const row of symbols.symbols) counts.set(row[0], (counts.get(row[0]) ?? 0) + 1);
    let bestFile = null;
    let bestCount = -1;
    for (const [file, count] of counts) {
      if (count > bestCount) {
        bestFile = file;
        bestCount = count;
      }
    }
    if (bestFile == null) return null;
    return { path: mapDoc.F[bestFile], symbolCount: bestCount };
  } catch (err) {
    console.warn(`perf-bench: could not pick a deep-zoom-cards target (${err.message}); skipping that measurement`);
    return null;
  }
}

/** Reuses runOneConfig unmodified: navigating with `?file=<path>` selects
 * and (via MapCanvas's own panTo-on-load) CENTRES that file, so symbol cards
 * are already drawn at load (a selected file is always symbol-gate-eligible,
 * regardless of on-screen size -- symbolCards.ts's fileCrossesSymbolGate).
 * The desktop zoom phase's 20 wheel-steps-in then centre on that already-
 * centred file, growing its (and its neighbours') footprints past the 40px
 * card gate -- exactly the "deep zoom, cards on" scenario, with zero new
 * gesture-scripting code. Target: desktop draw.zoom.p50 <= 150ms (spec);
 * reported plainly either way, never asserted (this script has no failing
 * exit code for a perf number, only --check-single-paint does, and that's a
 * different check). */
async function benchDeepZoomCards(browser, base, args) {
  const target = await pickDeepZoomCardsTarget(base);
  if (!target) return null;
  const slug = `langgenius/dify?file=${encodeURIComponent(target.path)}`;
  const out = await runOneConfig({ browser, base, slug, profile: PROFILES.desktop, timeCapMs: args.gestureCapMs });
  const result = {
    map: "langgenius/dify (deep zoom, cards on)",
    files: null,
    profile: "desktop",
    runs: 1,
    capped: true,
    firstMapMs: round2(out.firstMapMs),
    drawsOnLoad: out.drawsOnLoad,
    pinchMode: undefined,
    draw: { drag: summarize(out.drag.draws), zoom: summarize(out.zoom.draws) },
    frames: { drag: frameStats(out.drag.frames), zoom: frameStats(out.zoom.frames) },
    steps: {
      drag: { completed: out.drag.completed, of: out.drag.steps, capped: out.drag.capped },
      zoom: { completed: out.zoom.completed, of: out.zoom.steps, capped: out.zoom.capped },
    },
    note: `target: ${target.path} (${target.symbolCount} symbols in district 0)`,
  };
  const meets = result.draw.zoom.p50 <= 150;
  console.log(
    `${result.map}: draw zoom p50/p95 ${result.draw.zoom.p50}/${result.draw.zoom.p95}ms (target <=150ms desktop p50: ${meets ? "MEETS" : "MISSES"}) -- ${result.note}`,
  );
  return result;
}

async function bench(args) {
  const browser = await chromium.launch();
  const fileCounts = await loadMapFileCounts();
  const results = [];
  for (const slug of args.maps) {
    const files = fileCounts.get(slug);
    // Unknown file count (index.json missing/stale for this slug) is treated
    // as small -- the safe default is "take longer, not silently truncate a
    // gesture no one asked to cap."
    const big = (files ?? 0) > args.bigMapFiles;
    const runCount = big ? 1 : args.runs;
    const timeCapMs = big ? args.gestureCapMs : Infinity;
    for (const [profileName, profile] of Object.entries(PROFILES)) {
      const runs = [];
      for (let r = 0; r < runCount; r++) {
        const out = await runOneConfig({ browser, base: args.base, slug, profile, timeCapMs });
        runs.push(out);
      }
      const median = medianRun(runs, (r) => summarize(r.drag.draws).p50 || 0);
      results.push({
        map: slug,
        files: files ?? null,
        profile: profileName,
        runs: runCount,
        capped: big,
        firstMapMs: round2(medianRun(runs, (r) => r.firstMapMs).firstMapMs),
        // Issue #51: drawsOnLoad is a code-path count, not a timing that
        // should vary run to run (unlike firstMapMs) -- read off the same
        // median run as everything else below for consistency, not
        // re-medianed on its own.
        drawsOnLoad: median.drawsOnLoad,
        pinchMode: median.pinchMode,
        draw: {
          drag: summarize(median.drag.draws),
          zoom: summarize(median.zoom.draws),
        },
        frames: {
          drag: frameStats(median.drag.frames),
          zoom: frameStats(median.zoom.frames),
        },
        steps: {
          drag: { completed: median.drag.completed, of: median.drag.steps, capped: median.drag.capped },
          zoom: { completed: median.zoom.completed, of: median.zoom.steps, capped: median.zoom.capped },
        },
      });
      const p = results[results.length - 1];
      const capNote = big
        ? ` [1 run, capped at ${(timeCapMs / 1000).toFixed(0)}s: drag ${p.steps.drag.completed}/${p.steps.drag.of} steps, zoom ${p.steps.zoom.completed}/${p.steps.zoom.of} steps]`
        : "";
      console.log(
        `${slug} / ${profileName}: firstMap ${p.firstMapMs}ms, drawsOnLoad ${p.drawsOnLoad}, draw drag p50/p95 ${p.draw.drag.p50}/${p.draw.drag.p95}ms, draw zoom p50/p95 ${p.draw.zoom.p50}/${p.draw.zoom.p95}ms, drag frames>50ms ${p.frames.drag.over50}/${p.frames.drag.n}, zoom frames>50ms ${p.frames.zoom.over50}/${p.frames.zoom.n}${capNote}`,
      );
    }
  }
  if (args.maps.includes("langgenius/dify")) {
    const deepZoomCards = await benchDeepZoomCards(browser, args.base, args);
    if (deepZoomCards) results.push(deepZoomCards);
  }
  await browser.close();
  return results;
}

// Issue #51: asserts drawsOnLoad<=1 for SINGLE_PAINT_CHECK_MAPS -- opt-in via
// --check-single-paint (see parseArgs). Returns the failure count so main()
// can set the process exit code the way check-view-stability.mjs does.
function checkSinglePaint(results) {
  let failures = 0;
  console.log("\nsingle-paint-on-load check:");
  for (const r of results) {
    if (!SINGLE_PAINT_CHECK_MAPS.includes(r.map)) continue;
    const ok = r.drawsOnLoad <= 1;
    if (!ok) failures++;
    console.log(`  ${ok ? "ok  " : "FAIL"}  ${r.map} / ${r.profile}: drawsOnLoad=${r.drawsOnLoad} (want <=1)`);
  }
  console.log(`${failures === 0 ? "passed" : `${failures} FAILED`}`);
  return failures;
}

function toMarkdown(results) {
  const rows = [
    "| map | profile | runs | first map (ms) | draws on load | draw drag p50/p95/max (ms) | draw zoom p50/p95/max (ms) | drag frame p50/p95 (ms) | drag frames>50ms | zoom frame p50/p95 (ms) | zoom frames>50ms | gesture steps completed (drag/zoom) |",
    "|---|---|---|---|---|---|---|---|---|---|---|---|",
  ];
  for (const r of results) {
    const runsCell = r.capped ? "1 (capped)" : `${r.runs} (median)`;
    const stepsCell = `${r.steps.drag.completed}/${r.steps.drag.of}${r.steps.drag.capped ? "*" : ""} / ${r.steps.zoom.completed}/${r.steps.zoom.of}${r.steps.zoom.capped ? "*" : ""}`;
    rows.push(
      `| ${r.map} | ${r.profile} | ${runsCell} | ${r.firstMapMs} | ${r.drawsOnLoad} | ${r.draw.drag.p50}/${r.draw.drag.p95}/${r.draw.drag.max} | ${r.draw.zoom.p50}/${r.draw.zoom.p95}/${r.draw.zoom.max} | ${r.frames.drag.p50}/${r.frames.drag.p95} | ${r.frames.drag.over50}/${r.frames.drag.n} | ${r.frames.zoom.p50}/${r.frames.zoom.p95} | ${r.frames.zoom.over50}/${r.frames.zoom.n} | ${stepsCell} |`,
    );
  }
  rows.push("");
  rows.push("`*` = the wall-clock gesture cap cut that phase short; frame/draw stats above are still over exactly the steps completed.");
  return rows.join("\n");
}

async function waitForServer(base, timeoutMs = 30_000) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeoutMs) {
    try {
      const res = await fetch(base);
      if (res.ok || res.status === 404) return;
    } catch {
      /* not up yet */
    }
    await new Promise((r) => setTimeout(r, 300));
  }
  throw new Error(`server at ${base} did not come up in time`);
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  let serverProc = null;
  if (args.startServer) {
    const port = new URL(args.base).port || "5174";
    serverProc = spawn("pnpm", ["exec", "vite", "--port", port, "--strictPort"], {
      cwd: path.resolve(new URL(".", import.meta.url).pathname, ".."),
      stdio: "inherit",
    });
    await waitForServer(args.base);
  }
  try {
    const results = await bench(args);
    await mkdir(path.dirname(args.out), { recursive: true });
    await writeFile(args.out, JSON.stringify(results, null, 2));
    const md = toMarkdown(results);
    const mdPath = args.out.replace(/\.json$/, ".md");
    await writeFile(mdPath, md);
    console.log("\n" + md);
    console.log(`\nwrote ${args.out} and ${mdPath}`);
    if (args.checkSinglePaint) {
      const failures = checkSinglePaint(results);
      if (failures > 0) process.exitCode = 1;
    }
  } finally {
    if (serverProc) serverProc.kill();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
