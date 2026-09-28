#!/usr/bin/env -S npx tsx
// Unit checks for docs/UX.md phase 7a's pure parts:
//   - map/layoutProfile.ts: §9's profile table on its edge cases, and the
//     safe rectangles of the landscape side sheet and the desktop/tablet
//     floating desktop/tablet chrome, with the panel open/hidden and safe-area
//     insets;
//   - map/phoneShell.ts: the portrait safe rect with left/right insets (§9:
//     insets apply on every side in every profile);
//   - lib/repoOptions.ts: issue #171 (the repository select's file counts).
// No browser: the same standalone-script pattern as check-phone-shell.ts.
// CI runs it in the `web build and lint` job; the browser side (the layouts
// themselves, at each profile's sizes) is check-view-stability.mjs's.
//
// Run: npx tsx web/scripts/check-layout-profile.ts

import {
  DESKTOP_COMMAND_SAFE_GAP_PX,
  DESKTOP_GAP_PX,
  DESKTOP_GUTTER_PX,
  DESKTOP_HIDDEN_LEFT_INSET_PX,
  DESKTOP_PANEL_BOTTOM_PX,
  DESKTOP_PANEL_LEFT_PX,
  DESKTOP_PANEL_SAFE_GAP_PX,
  DESKTOP_PANEL_TOP_PX,
  DESKTOP_PANEL_WIDTH_PX,
  NO_SAFE_AREA,
  SIDE_SHEET_WIDTH_PX,
  compactMap,
  desktopControlSize,
  desktopSafeInsets,
  isPhoneShell,
  isTouchProfile,
  landscapeSafeInsets,
  layoutProfile,
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
  const cases: Array<[number, number, "desktop" | "tablet", boolean, SafeArea, Record<string, number>, number[]]> = [
    [1440, 900, "desktop", true, NO_SAFE_AREA, { left: 400, top: 76, right: 80, bottom: 72 }, [400, 76, 1360, 828]],
    [1440, 900, "desktop", false, NO_SAFE_AREA, { left: 24, top: 76, right: 80, bottom: 72 }, [24, 76, 1360, 828]],
    [1100, 800, "tablet", true, NO_SAFE_AREA, { left: 400, top: 76, right: 84, bottom: 72 }, [400, 76, 1016, 728]],
    [1100, 800, "tablet", false, NO_SAFE_AREA, { left: 24, top: 76, right: 84, bottom: 72 }, [24, 76, 1016, 728]],
    [1024, 768, "tablet", true, NO_SAFE_AREA, { left: 400, top: 76, right: 84, bottom: 72 }, [400, 76, 940, 696]],
    [1024, 768, "tablet", false, NO_SAFE_AREA, { left: 24, top: 76, right: 84, bottom: 72 }, [24, 76, 940, 696]],
    [768, 1024, "tablet", false, NO_SAFE_AREA, { left: 24, top: 76, right: 84, bottom: 72 }, [24, 76, 684, 952]],
    [1024, 768, "tablet", true, { top: 47, right: 20, bottom: 21, left: 30 }, { left: 430, top: 123, right: 104, bottom: 93 }, [430, 123, 920, 675]],
    [768, 1024, "tablet", false, { top: 47, right: 20, bottom: 21, left: 30 }, { left: 54, top: 123, right: 104, bottom: 93 }, [54, 123, 664, 931]],
  ];
  for (const [width, height, profile, panelOpen, safe, expectedInsets, expectedRect] of cases) {
    const insets = desktopSafeInsets({ profile, panelOpen, safe });
    const rect = fitViewport(width, height, insets);
    report(eq(insets, expectedInsets) && eq(rect, expectedRect),
      `${width}x${height} ${profile}, panel ${panelOpen ? "open" : "hidden"}: floating-chrome fit rectangle`, JSON.stringify({ insets, rect }));
  }
  report(DESKTOP_PANEL_LEFT_PX + DESKTOP_PANEL_WIDTH_PX + DESKTOP_PANEL_SAFE_GAP_PX === 400 &&
    DESKTOP_HIDDEN_LEFT_INSET_PX === 24 && DESKTOP_PANEL_TOP_PX + DESKTOP_PANEL_BOTTOM_PX === 144 &&
    DESKTOP_GUTTER_PX + desktopControlSize("desktop") + DESKTOP_GAP_PX === 80 &&
    desktopControlSize("tablet") === 44 && DESKTOP_COMMAND_SAFE_GAP_PX === 16,
  "safe rectangles share panel, command-bar and control measurements with the floating chrome");
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
