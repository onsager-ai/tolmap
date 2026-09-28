#!/usr/bin/env -S npx tsx
// Unit checks for docs/UX.md phase 5's pure parts:
//   - map/layoutProfile.ts: §9's profile table on its edge cases, and the
//     safe rectangles of the landscape side sheet and the desktop/tablet
//     chrome (the inspector, the quality strip, the controls, the fullscreen
//     fallback's search) with every safe-area inset;
//   - map/phoneShell.ts: the portrait safe rect with left/right insets (§9:
//     insets apply on every side in every profile);
//   - lib/repoOptions.ts: issue #171 (the repository select's file counts).
// No browser: the same standalone-script pattern as check-phone-shell.ts.
// CI runs it in the `web build and lint` job; the browser side (the layouts
// themselves, at each profile's sizes) is check-view-stability.mjs's.
//
// Run: npx tsx web/scripts/check-layout-profile.ts

import {
  DESKTOP_GAP_PX,
  DESKTOP_GUTTER_PX,
  FULLSCREEN_SEARCH_HEIGHT_PX,
  INSPECTOR_INSET_PX,
  NO_SAFE_AREA,
  SIDE_SHEET_WIDTH_PX,
  compactMap,
  desktopControlSize,
  desktopSafeInsets,
  inspectorWidth,
  isPhoneShell,
  isTouchProfile,
  landscapeSafeInsets,
  layoutProfile,
  qualityStripHeight,
  sideSheetWidth,
  type LayoutProfile,
  type SafeArea,
} from "../src/map/layoutProfile";
import { CONTROL_SIZE_PX, FLOAT_GUTTER_PX, PILL_HEIGHT_PX, PILL_TOP_PX, SAFE_GAP_PX, safeInsets, safeRect } from "../src/map/phoneShell";
import { fitViewport } from "../src/map/geometry";
import { repoOptions } from "../src/lib/repoOptions";
import type { CatalogueEntry, MapDocument } from "../src/types";

let failures = 0;
let checks = 0;

function report(ok: boolean, label: string, detail?: string) {
  checks++;
  if (ok) console.log(`  ok    ${label}`);
  else {
    failures++;
    console.log(`  FAIL  ${label}${detail ? " -- " + detail : ""}`);
  }
}
const eq = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

// ---------------------------------------------------------------- §9 profiles
console.log("\nlayout profile (§9)");
const table: Array<[number, number, LayoutProfile, string]> = [
  [390, 844, "phone", "a phone held upright"],
  [844, 390, "landscape", "the same phone sideways: never the desktop layout below 500 px of height"],
  [667, 375, "landscape", "a small phone sideways"],
  [768, 1024, "tablet", "a portrait tablet (phase 2 gave it the phone shell)"],
  [1024, 768, "tablet", "a landscape tablet"],
  [1100, 800, "tablet", "1100 wide is still a tablet (width <= 1100)"],
  [1101, 800, "desktop", "1101 wide is desktop"],
  [1440, 900, "desktop", "the desktop design size"],
  [360, 640, "phone", "a small Android phone"],
  [320, 568, "phone", "the narrowest phone"],
  [1200, 500, "landscape", "500 px tall is landscape even when wide (height <= 500)"],
  [1200, 501, "desktop", "501 px tall and wide is desktop"],
  [600, 900, "phone", "600 wide is still a phone (width <= 600)"],
  [601, 900, "tablet", "601 wide is a tablet"],
  [320, 480, "phone", "a box under 500 tall but taller than wide stays a portrait phone"],
  [1200, 800, "desktop", "check:view's desktop profile"],
  [1040, 710, "tablet", "check:view's desktop profile resized by runOne (-160, -90) stays in the same tree"],
];
for (const [w, h, want, why] of table) {
  const got = layoutProfile(w, h);
  report(got === want, `${w}x${h} -> ${want}: ${why}`, got);
}
console.log("\nmap box level of detail (#180)");
const compactBoxes: Array<[number, number, boolean, string]> = [
  [390, 844, true, "phone portrait uses compact detail"],
  [844, 390, true, "a wide but short landscape map uses compact detail"],
  [667, 375, true, "a small landscape map uses compact detail"],
  [768, 1024, false, "a portrait tablet map uses full detail"],
  [1024, 768, false, "a landscape tablet map uses full detail"],
  [1120, 844, false, "a desktop map area uses full detail"],
  [600, 900, true, "600 px is the compact width edge"],
  [601, 900, false, "601 px is above the compact width edge"],
  [900, 500, true, "500 px is the compact height edge"],
  [900, 501, false, "501 px is above the compact height edge"],
];
for (const [w, h, want, why] of compactBoxes) {
  const got = compactMap(w, h);
  report(got === want, `${w}x${h} map box -> ${want}: ${why}`, String(got));
}
report(isPhoneShell("phone") && isPhoneShell("landscape") && !isPhoneShell("tablet") && !isPhoneShell("desktop"), "both phone profiles use the phone shell; tablet and desktop the desktop layout");
report(isTouchProfile("phone") && isTouchProfile("landscape") && isTouchProfile("tablet") && !isTouchProfile("desktop"), "44 px targets on phone, landscape and tablet (§8.2, §9); desktop may be 32-40");

// ---------------------------------------------------------------- landscape side sheet
console.log("\nlandscape side sheet safe rectangle (§9)");
const notchLeft: SafeArea = { top: 0, right: 0, bottom: 21, left: 47 };
{
  const open = landscapeSafeInsets(true, NO_SAFE_AREA);
  report(eq(open, { left: 360 + 12, top: 12 + 48 + 8, right: 12 + 44 + 8, bottom: 12 }),
    "open: right of the 360 px sheet, below the pill, left of the control column", JSON.stringify(open));
  const shut = landscapeSafeInsets(false, NO_SAFE_AREA);
  report(eq(shut, { left: 12, top: 68, right: 64, bottom: 12 }), "collapsed: the sheet's 360 px go back to the map", JSON.stringify(shut));
  const [l, t, r, b] = fitViewport(844, 390, open);
  report(r - l === 844 - 372 - 64 && b - t === 390 - 68 - 12, "844x390, open: the map rect is 408 x 310", JSON.stringify([l, t, r, b]));
  const [l2, , r2] = fitViewport(667, 375, open);
  report(r2 - l2 > 200, "667x375, open: still over 200 px of map beside the sheet", String(r2 - l2));
  const n = landscapeSafeInsets(true, notchLeft);
  report(n.left === SIDE_SHEET_WIDTH_PX + 47 + FLOAT_GUTTER_PX && sideSheetWidth(true, notchLeft) === 407,
    "the sheet pads the left (notch) inset inside itself, and the map starts after it", JSON.stringify(n));
  report(landscapeSafeInsets(false, notchLeft).left === 47 + 12, "collapsed, the map clears the notch itself", JSON.stringify(landscapeSafeInsets(false, notchLeft)));
  report(n.bottom === 21 + 12, "the home indicator (bottom inset) is cleared", JSON.stringify(n));
  const notchRight: SafeArea = { top: 0, right: 47, bottom: 21, left: 0 };
  report(landscapeSafeInsets(true, notchRight).right === 47 + 12 + 44 + 8, "turned the other way, the notch is right: the control column and the rect clear it", JSON.stringify(landscapeSafeInsets(true, notchRight)));
  report(sideSheetWidth(false, notchLeft) === 0, "collapsed is 0 wide (§9: \"collapsed 0, peek 360\")");
}

// ---------------------------------------------------------------- portrait with side insets
console.log("\nportrait safe rectangle with every inset (§3.3, §9)");
{
  const plain = safeInsets({ width: 390, height: 844, safeTop: 0, safeBottom: 0 }, 156);
  report(eq(plain, { left: 12, top: 68, right: 64, bottom: 164 }), "no side insets: phase 2's rect, unchanged", JSON.stringify(plain));
  const sides = safeInsets({ width: 390, height: 844, safeTop: 47, safeBottom: 34, safeLeft: 10, safeRight: 6 }, 190);
  report(eq(sides, { left: 22, top: 47 + 68, right: 70, bottom: 198 }), "left and right insets widen the gutters", JSON.stringify(sides));
  const rect = safeRect({ width: 390, height: 844, safeTop: 0, safeBottom: 0, safeLeft: 10, safeRight: 6 }, 156);
  report(rect.left === 22 && rect.right === 390 - 70, "safeRect follows", JSON.stringify(rect));
  report(PILL_TOP_PX + PILL_HEIGHT_PX + SAFE_GAP_PX === 68 && CONTROL_SIZE_PX === 44, "(the shared pill and control constants)");
}

// ---------------------------------------------------------------- desktop and tablet
console.log("\ndesktop and tablet safe rectangle (§5)");
{
  const base = { rail: true, fullscreen: false, safe: NO_SAFE_AREA };
  const closed = desktopSafeInsets({ ...base, profile: "desktop", inspector: false });
  report(eq(closed, { left: 20, top: 20, right: 20 + 40 + 12, bottom: 20 + 40 + 12 }),
    "desktop, nothing selected: clear of the controls (right) and the quality strip (bottom)", JSON.stringify(closed));
  const open = desktopSafeInsets({ ...base, profile: "desktop", inspector: true });
  report(open.right === INSPECTOR_INSET_PX + 380 + DESKTOP_GAP_PX && open.left === closed.left && open.top === closed.top && open.bottom === closed.bottom,
    "desktop, inspector open: the map box minus the 380 px inspector and its 20 px inset", JSON.stringify(open));
  // 1440 x 900: the map box is 1120 x 844 (rail 320, top bar 56).
  const [l, , r] = fitViewport(1440 - 320, 900 - 56, open);
  report(r - l === 1120 - 20 - 412 && r < 1120 - 20 - 380, "1440x900 with the inspector: the rect ends left of the inspector", JSON.stringify([l, r]));
  const tab = desktopSafeInsets({ ...base, profile: "tablet", inspector: true });
  report(tab.right === 20 + 360 + 12 && tab.bottom === 20 + 44 + 12 && inspectorWidth("tablet") === 360 && desktopControlSize("tablet") === 44 && qualityStripHeight("tablet") === 44,
    "tablet: a 360 px inspector, 44 px controls and strip", JSON.stringify(tab));
  // 768 x 1024 with the rail collapsed and the inspector open: still a map.
  const [l2, , r2] = fitViewport(768, 1024 - 56, desktopSafeInsets({ ...base, rail: false, profile: "tablet", inspector: true }));
  report(r2 - l2 >= 340, "768x1024, rail collapsed, inspector open: over 340 px of map", String(r2 - l2));
  const inset: SafeArea = { top: 47, right: 20, bottom: 21, left: 30 };
  const railed = desktopSafeInsets({ profile: "tablet", inspector: false, rail: true, fullscreen: false, safe: inset });
  report(railed.left === DESKTOP_GUTTER_PX && railed.top === DESKTOP_GUTTER_PX, "with the rail on screen, the rail pads the left inset and the top bar the top one", JSON.stringify(railed));
  report(railed.right === 20 + 20 + 44 + 12 && railed.bottom === 21 + 20 + 44 + 12, "right and bottom insets are cleared by the map's own chrome", JSON.stringify(railed));
  const bare = desktopSafeInsets({ profile: "tablet", inspector: false, rail: false, fullscreen: false, safe: inset });
  report(bare.left === 30 + 20, "rail collapsed: the map reaches the screen's left edge and clears the inset itself", JSON.stringify(bare));
  const fs = desktopSafeInsets({ profile: "desktop", inspector: false, rail: true, fullscreen: true, safe: inset });
  report(fs.top === 47 + 20 + FULLSCREEN_SEARCH_HEIGHT_PX + 12 && fs.left === 30 + 20,
    "the CSS fullscreen fallback keeps the top inset: the rect starts below the notch and the floating search", JSON.stringify(fs));
  const fsOpen = desktopSafeInsets({ profile: "desktop", inspector: true, rail: true, fullscreen: true, safe: inset });
  report(fsOpen.right === 20 + 20 + 380 + 12, "fullscreen with the inspector open clears it and the right inset", JSON.stringify(fsOpen));
}

// ---------------------------------------------------------------- issue #171
console.log("\nrepository options (issue #171)");
{
  const doc = { F: Array.from({ length: 6347 }, (_, i) => `f${i}`) } as unknown as MapDocument;
  const entry = (slug: string, files: number): CatalogueEntry => {
    const [owner, repo] = slug.split("/");
    return { slug, owner, repo, file: "", files, districts: 1, modularity: 0, lang: "py", source: "static" };
  };
  const notListed = repoOptions([entry("django/django", 2804)], doc, "langgenius", "dify");
  report(notListed[0].label === "langgenius/dify · 6,347 files", "a map the catalogue does not list: its option counts the map's own files, not 0", JSON.stringify(notListed));
  report(notListed.every((o) => !/ 0 files/.test(o.label)), "no option says 0 files", JSON.stringify(notListed));
  report(notListed[1].label === "django/django · 2,804 files", "other repositories keep the catalogue's count", JSON.stringify(notListed));
  const stale = repoOptions([entry("langgenius/dify", 12)], doc, "langgenius", "dify");
  report(stale.length === 1 && stale[0].label === "langgenius/dify · 6,347 files", "a listed map shows the count of the map on screen", JSON.stringify(stale));
  const none = repoOptions(undefined, doc, "langgenius", "dify");
  report(none.length === 1 && none[0].slug === "langgenius/dify", "no catalogue yet: the current map still has its option", JSON.stringify(none));
}

console.log(`\n${checks - failures}/${checks} checks passed`);
if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exitCode = 1;
}
