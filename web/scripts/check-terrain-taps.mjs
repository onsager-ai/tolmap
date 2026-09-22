#!/usr/bin/env node
// Issue #54 scope item 3: CLAUDE.md's viewer check (districts named,
// landmarks listed, tapping a district/file/symbol each produce a card)
// plus the sub-district/parcel/arterial taps owed since #47 -- finding
// 16 shipped the terrain schema and renderer but the phone/desktop card
// check never covered the three new hit target kinds. Drives a real
// vite dev server with Playwright, desktop (1440x900) and an emulated
// touch phone (390x844) -- see CLAUDE.md's own caveat: this is Playwright's
// touch emulation, not a real device.
//
// Also records at which wheel-zoom notch (0 = fit) terrain elements first
// appear in the DOM, as a live visual cross-check against
// terrain-zoom-measure.ts's Node-only zf_establish numbers.
//
// Usage:
//   nice -n 10 pnpm exec vite --port 5184 --strictPort &
//   node scripts/check-terrain-taps.mjs [base] [owner/repo]
// Defaults to http://127.0.0.1:5184 and langgenius/dify -- the largest
// terrain map safe to open in a browser on this task's machine (CLAUDE.md).
// A map with arterials (n8n-io/n8n, aws/aws-sdk-go-v2) is over that
// ceiling, so the arterial-tap check reports "no on-screen arterial hit
// path" and fails on every map this script can actually be pointed at --
// see docs/FINDINGS.md's #54 entry for the structural (non-browser) check
// that covers that gap instead.
import { chromium } from "playwright";

const BASE = process.argv[2] || "http://127.0.0.1:5184";
const SLUG = process.argv[3] || "langgenius/dify";
const PROFILES = [
  { name: "desktop", viewport: { width: 1440, height: 900 }, isMobile: false, hasTouch: false },
  { name: "phone", viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 },
];

let fails = 0;
function report(ok, label, detail) {
  console.log(`${ok ? "PASS" : "FAIL"}  ${label}${detail ? "  " + detail : ""}`);
  if (!ok) fails++;
}

async function tap(page, profile, x, y) {
  if (profile.hasTouch) await page.touchscreen.tap(x, y);
  else await page.mouse.click(x, y);
  await page.waitForTimeout(400);
}

async function cardText(page) {
  // SelectionPanel.tsx's shadcn Card (rounded-lg, from ui/card.tsx, plus its
  // own absolute z-10) only rendered when selD/sel/selTerrain != null. NOT
  // ".absolute.z-10" alone -- Sidebar.tsx's phone bottom sheet also carries
  // "absolute ... z-10" and, being earlier in the DOM, would otherwise win
  // page.$'s first-match and silently report the sidebar's own permanent
  // text on every tap instead of the actual selection card (caught by hand
  // when every phone probe reported the identical "LANDMARKS & DISTRICTS"
  // text regardless of what was tapped). Sidebar's own classes have no
  // "rounded-lg" (it uses "rounded-t-xl"), so the combination is unique.
  const el = await page.$(".rounded-lg.z-10");
  if (!el) return null;
  const txt = (await el.innerText()).trim();
  return txt.length ? txt : null;
}

async function run(browser, profile) {
  const label = `${SLUG} / ${profile.name}`;
  const context = await browser.newContext({
    viewport: profile.viewport,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    deviceScaleFactor: profile.deviceScaleFactor ?? 1,
  });
  const page = await context.newPage();
  const { width: vw, height: vh } = profile.viewport;
  await page.goto(`${BASE}/${SLUG}`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("svg.map-svg path.hit");
  await page.waitForTimeout(800);

  // Districts named + landmarks listed: Sidebar.tsx renders both; just
  // check non-trivial counts exist in the DOM.
  const districtCount = await page.locator("svg.map-svg path.hit[data-k^='d:']").count();
  report(districtCount > 0, `${label}: districts drawn`, String(districtCount));

  const cx = vw / 2,
    cy = vh / 2;
  await page.mouse.move(cx, cy);
  const zfAtNotch = {};
  const MAX_NOTCH = 12;
  for (let notch = 0; notch <= MAX_NOTCH; notch++) {
    if (notch > 0) {
      await page.mouse.wheel(0, -200);
      await page.waitForTimeout(60);
    }
    const sd = await page.locator("svg.map-svg path.hit[data-k^='sd:']").count();
    const pc = await page.locator("svg.map-svg rect.hit[data-k^='p:']").count();
    zfAtNotch[notch] = { sd, pc };
  }
  await page.waitForTimeout(650);
  console.log(`${label}: element counts by wheel notch (0=fit): ${JSON.stringify(zfAtNotch)}`);
  const firstSdNotch = Object.entries(zfAtNotch).find(([, v]) => v.sd > 0)?.[0];
  const firstPcNotch = Object.entries(zfAtNotch).find(([, v]) => v.pc > 0)?.[0];
  report(firstSdNotch != null, `${label}: sub-district contours appear within ${MAX_NOTCH} notches`, `at notch ${firstSdNotch}`);
  report(
    firstPcNotch == null || firstSdNotch == null || +firstPcNotch >= +firstSdNotch,
    `${label}: parcels do not appear before sub-district contours`,
    `sd@${firstSdNotch} pc@${firstPcNotch}`,
  );

  // Reset to fit before tap probes below -- the notch-discovery loop above
  // may have zoomed in far past where these elements are still on screen
  // (DOM presence, checked above, is not the same as on-screen, checked
  // below); a real reader would be zoomed in only as far as they chose to
  // get to what they're tapping, not stuck at whatever notch this loop
  // stopped counting at.
  const fitBtn = page.locator('button[aria-label="Fit map"]');
  if ((await fitBtn.count()) > 0) {
    await fitBtn.click();
    await page.waitForTimeout(650);
  }
  async function zoomToNotch(n) {
    await page.mouse.move(cx, cy);
    for (let i = 0; i < n; i++) {
      await page.mouse.wheel(0, -200);
      await page.waitForTimeout(60);
    }
    await page.waitForTimeout(650);
  }

  async function onscreenBox(locator, n) {
    const count = Math.min(await locator.count(), n ?? 80);
    for (let i = 0; i < count; i++) {
      const box = await locator.nth(i).boundingBox();
      if (!box) continue;
      const x = box.x + box.width / 2,
        y = box.y + box.height / 2;
      if (x >= 0 && x <= vw && y >= 0 && y <= vh) return { x, y };
    }
    return null;
  }

  // Tap a district. Terrain draws on top of district polygons (this
  // method's own doc comment in MapRenderer.ts), so a tap over an eligible
  // district's own area at this zoom may land on ITS sub-district/parcel
  // instead -- report whatever selection resulted; that's still delegated
  // hit-testing across the district/terrain/file layers working, and a
  // second, later probe (below) targets an on-screen d: hit specifically
  // where no terrain currently covers it.
  const dPt = await onscreenBox(page.locator("svg.map-svg path.hit[data-k^='d:']"));
  if (dPt) {
    await tap(page, profile, dPt.x, dPt.y);
    const txt = await cardText(page);
    report(!!txt, `${label}: tap district (or terrain covering it) -> card`, txt ? txt.slice(0, 60) : `no card (pt ${JSON.stringify(dPt)})`);
  } else {
    report(false, `${label}: tap district -> card`, "no on-screen d: element");
  }

  // Zoom to where sub-districts were first found present (may be a no-op if
  // already at notch 0).
  if (firstSdNotch != null) await zoomToNotch(+firstSdNotch);

  // Tap a sub-district contour.
  const sdPt = await onscreenBox(page.locator("svg.map-svg path.hit[data-k^='sd:']"));
  if (sdPt) {
    await tap(page, profile, sdPt.x, sdPt.y);
    const txt = await cardText(page);
    report(!!txt, `${label}: tap sub-district -> card`, txt ? txt.slice(0, 60) : "");
  } else {
    report(false, `${label}: tap sub-district -> card`, "no on-screen sd: element");
  }

  // Reset and zoom to where parcels were first found present (falls back to
  // a deep zoom if this map has none, so the file/arterial probes below
  // still get a close-up view).
  if ((await fitBtn.count()) > 0) {
    await fitBtn.click();
    await page.waitForTimeout(650);
  }
  await zoomToNotch(firstPcNotch != null ? +firstPcNotch : MAX_NOTCH);

  const pcPt = await onscreenBox(page.locator("svg.map-svg rect.hit[data-k^='p:']"));
  if (pcPt) {
    await tap(page, profile, pcPt.x, pcPt.y);
    const txt = await cardText(page);
    report(!!txt, `${label}: tap parcel -> card`, txt ? txt.slice(0, 60) : "");
  } else {
    report(false, `${label}: tap parcel -> card`, "no on-screen p: element");
  }

  // Arterial: stroke="transparent" hit path with data-k="f:<file>", found by
  // filtering file-keyed hit paths (arterials are the only path.hit with a
  // data-k of that shape and pointer-events:stroke).
  const artPt = await onscreenBox(page.locator("svg.map-svg path.hit[data-k^='f:'][pointer-events='stroke']"));
  if (artPt) {
    await tap(page, profile, artPt.x, artPt.y);
    const txt = await cardText(page);
    report(!!txt, `${label}: tap arterial -> card`, txt ? txt.slice(0, 60) : "");
  } else {
    report(false, `${label}: tap arterial -> card`, "no on-screen arterial hit path (this map may have none)");
  }

  // File dot: try several on-screen files until one has a symbol directory
  // (doc.S is sparse -- most files parse zero symbols), so the symbol-tap
  // check isn't at the mercy of which file happens to be first in the DOM.
  const fLocator = page.locator("svg.map-svg circle.hit[data-k^='f:']");
  const fCount = Math.min(await fLocator.count(), 40);
  let filePassed = false,
    symRowFound = false;
  for (let i = 0; i < fCount && !symRowFound; i++) {
    const box = await fLocator.nth(i).boundingBox();
    if (!box) continue;
    const x = box.x + box.width / 2,
      y = box.y + box.height / 2;
    if (x < 0 || x > vw || y < 0 || y > vh) continue;
    await tap(page, profile, x, y);
    const txt = await cardText(page);
    if (!filePassed && txt) {
      report(true, `${label}: tap file -> card`, txt.slice(0, 60));
      filePassed = true;
    }
    const symRow = page.locator(".absolute.z-10 [class*='cursor-pointer']").first();
    if ((await symRow.count()) > 0) {
      await symRow.click();
      await page.waitForTimeout(300);
      const symTxt = await cardText(page);
      report(!!symTxt, `${label}: tap symbol -> card`, symTxt ? symTxt.slice(0, 60) : "");
      symRowFound = true;
    }
  }
  if (!filePassed) report(false, `${label}: tap file -> card`, "no on-screen file dot produced a card");
  if (!symRowFound) report(false, `${label}: tap symbol -> card`, "no on-screen file (of " + fCount + " tried) had a symbol row");

  await context.close();
}

const browser = await chromium.launch();
for (const profile of PROFILES) {
  await run(browser, profile);
}
await browser.close();
console.log(`\n${fails === 0 ? "ALL PASS" : fails + " FAILURE(S)"}`);
process.exit(fails === 0 ? 0 : 1);
