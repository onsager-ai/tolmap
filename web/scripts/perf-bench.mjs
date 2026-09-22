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
//
// Metrics per map x profile:
//   - firstMapMs: navigation -> first frame with the map <svg> populated
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

import { chromium } from "playwright";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

const DEFAULT_MAPS = [
  "pallets/flask",
  "django/django",
  "langgenius/dify",
  "n8n-io/n8n",
  "aws/aws-sdk-go-v2",
];

function parseArgs(argv) {
  const args = { base: "http://localhost:5174", maps: DEFAULT_MAPS, runs: 3, startServer: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--out") args.out = argv[++i];
    else if (a === "--base") args.base = argv[++i];
    else if (a === "--maps") args.maps = argv[++i].split(",").map((s) => s.trim()).filter(Boolean);
    else if (a === "--runs") args.runs = Number(argv[++i]);
    else if (a === "--start-server") args.startServer = true;
    else throw new Error(`unknown arg: ${a}`);
  }
  if (!args.out) throw new Error("--out <path> is required");
  return args;
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
 * timing. */
function installRecorder() {
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
  await page.waitForFunction(
    () => {
      const svg = document.querySelector("svg.map-svg");
      return !!svg && svg.childElementCount > 0 && svg.querySelector("g")?.childElementCount > 0;
    },
    { timeout: 60_000 },
  );
  return Date.now() - t0;
}

/** Real PointerEvents via Playwright's mouse API (desktop) -- this exercises
 * MapRenderer's own pointerdown/pointermove/pointerup handlers exactly as a
 * real drag would, including the moved<5 tap threshold and pointer capture
 * (bug fix #1/#2 in MapRenderer.ts), rather than calling a private method
 * directly. */
async function scriptedDragDesktop(page, vw, vh, steps = 40) {
  const y = vh / 2;
  const x0 = vw * 0.25;
  const x1 = vw * 0.75;
  await page.mouse.move(x0, y);
  await page.mouse.down();
  for (let i = 1; i <= steps; i++) {
    const x = x0 + ((x1 - x0) * i) / steps;
    await page.mouse.move(x, y, { steps: 1 });
  }
  await page.mouse.up();
}

/** Phone drag via CDP Input.dispatchTouchEvent -- Chromium synthesizes
 * PointerEvents from these the same way it does for a real finger, so this
 * exercises the identical MapRenderer pointer path as scriptedDragDesktop,
 * just through the touch input pipeline instead of the mouse one. */
async function scriptedDragTouch(cdp, vw, vh, steps = 40) {
  const y = vh / 2;
  const x0 = vw * 0.25;
  const x1 = vw * 0.75;
  const touchPoint = (x) => ({ x, y, id: 1 });
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [touchPoint(x0)] });
  for (let i = 1; i <= steps; i++) {
    const x = x0 + ((x1 - x0) * i) / steps;
    await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [touchPoint(x)] });
  }
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
}

async function scriptedWheelZoom(page, vw, vh, steps = 20) {
  const cx = vw / 2;
  const cy = vh / 2;
  await page.mouse.move(cx, cy);
  for (let i = 0; i < steps; i++) await page.mouse.wheel(0, -120); // zoom in
  for (let i = 0; i < steps; i++) await page.mouse.wheel(0, 120); // zoom out
}

/** Two-finger pinch via CDP Input.dispatchTouchEvent with two touch points
 * moving apart then together -- the same synthesis path MapRenderer's own
 * pinch handling (pts.size === 2) is built to receive from a real device. */
async function scriptedPinchTouch(cdp, vw, vh, steps = 20) {
  const cx = vw / 2;
  const cy = vh / 2;
  const pointsAt = (halfSpread) => [
    { x: cx - halfSpread, y: cy, id: 1 },
    { x: cx + halfSpread, y: cy, id: 2 },
  ];
  const start = 30;
  const end = Math.min(vw, vh) * 0.4;
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: pointsAt(start) });
  for (let i = 1; i <= steps; i++) {
    const spread = start + ((end - start) * i) / steps;
    await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: pointsAt(spread) });
  }
  for (let i = 1; i <= steps; i++) {
    const spread = end - ((end - start) * i) / steps;
    await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: pointsAt(spread) });
  }
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
}

async function doubleTapZoom(page, vw, vh) {
  const cx = vw / 2;
  const cy = vh / 2;
  await page.touchscreen.tap(cx, cy);
  await page.waitForTimeout(80);
  await page.touchscreen.tap(cx, cy);
}

async function runOneConfig({ browser, base, slug, profile }) {
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

  const { width: vw, height: vh } = profile.viewport;
  let pinchMode = "cdp-pinch";

  const dragStart = await markStart(page);
  if (profile.hasTouch) await scriptedDragTouch(cdp, vw, vh);
  else await scriptedDragDesktop(page, vw, vh);
  await page.waitForTimeout(200); // let the settle-redraw (issue #51 part 2) land
  const drag = await collectSince(page, dragStart);

  const zoomStart = await markStart(page);
  if (profile.hasTouch) {
    try {
      await scriptedPinchTouch(cdp, vw, vh);
    } catch (err) {
      pinchMode = "double-tap (pinch via CDP failed: " + String(err?.message || err) + ")";
      await doubleTapZoom(page, vw, vh);
    }
  } else {
    await scriptedWheelZoom(page, vw, vh);
  }
  await page.waitForTimeout(200);
  const zoom = await collectSince(page, zoomStart);

  await context.close();

  return {
    firstMapMs,
    pinchMode: profile.hasTouch ? pinchMode : undefined,
    drag,
    zoom,
  };
}

function frameStats(frames) {
  const s = summarize(frames);
  return { p50: s.p50, p95: s.p95, over50: frames.filter((f) => f > 50).length, n: frames.length };
}

async function bench(args) {
  const browser = await chromium.launch();
  const results = [];
  for (const slug of args.maps) {
    for (const [profileName, profile] of Object.entries(PROFILES)) {
      const runs = [];
      for (let r = 0; r < args.runs; r++) {
        const out = await runOneConfig({ browser, base: args.base, slug, profile });
        runs.push(out);
      }
      const median = medianRun(runs, (r) => summarize(r.drag.draws).p50 || 0);
      results.push({
        map: slug,
        profile: profileName,
        runs: args.runs,
        firstMapMs: round2(medianRun(runs, (r) => r.firstMapMs).firstMapMs),
        pinchMode: median.pinchMode,
        draw: {
          drag: summarize(median.drag.draws),
          zoom: summarize(median.zoom.draws),
        },
        frames: {
          drag: frameStats(median.drag.frames),
          zoom: frameStats(median.zoom.frames),
        },
      });
      const p = results[results.length - 1];
      console.log(
        `${slug} / ${profileName}: firstMap ${p.firstMapMs}ms, draw drag p50/p95 ${p.draw.drag.p50}/${p.draw.drag.p95}ms, draw zoom p50/p95 ${p.draw.zoom.p50}/${p.draw.zoom.p95}ms, drag frames>50ms ${p.frames.drag.over50}/${p.frames.drag.n}, zoom frames>50ms ${p.frames.zoom.over50}/${p.frames.zoom.n}`,
      );
    }
  }
  await browser.close();
  return results;
}

function toMarkdown(results) {
  const rows = [
    "| map | profile | first map (ms) | draw drag p50/p95/max (ms) | draw zoom p50/p95/max (ms) | drag frame p50/p95 (ms) | drag frames>50ms | zoom frame p50/p95 (ms) | zoom frames>50ms |",
    "|---|---|---|---|---|---|---|---|---|",
  ];
  for (const r of results) {
    rows.push(
      `| ${r.map} | ${r.profile} | ${r.firstMapMs} | ${r.draw.drag.p50}/${r.draw.drag.p95}/${r.draw.drag.max} | ${r.draw.zoom.p50}/${r.draw.zoom.p95}/${r.draw.zoom.max} | ${r.frames.drag.p50}/${r.frames.drag.p95} | ${r.frames.drag.over50}/${r.frames.drag.n} | ${r.frames.zoom.p50}/${r.frames.zoom.p95} | ${r.frames.zoom.over50}/${r.frames.zoom.n} |`,
    );
  }
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
  } finally {
    if (serverProc) serverProc.kill();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
