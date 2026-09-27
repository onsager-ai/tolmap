// docs/UX.md §9: which layout the map page uses, and the safe rectangles of
// the landscape side sheet and the desktop/tablet chrome -- pure, so
// web/scripts/check-layout-profile.ts can test them without a browser (the
// same reason map/phoneShell.ts and map/gestures.ts are pure).
//
// The layout is chosen by the available box, not by `vh` or the window width
// alone. The old single breakpoint (width <= 820 px, hooks/useIsNarrow.ts)
// gave an 844 x 390 landscape phone the desktop layout with a 250 px rail and
// 28 px buttons, and a 768 px portrait tablet the phone shell.

import type { Insets } from "./geometry";
import { CONTROL_SIZE_PX, FLOAT_GUTTER_PX, PILL_HEIGHT_PX, PILL_TOP_PX, SAFE_GAP_PX } from "./phoneShell";

export type LayoutProfile = "phone" | "landscape" | "tablet" | "desktop";

/** §9's thresholds, CSS px. */
export const LANDSCAPE_MAX_HEIGHT_PX = 500;
export const PHONE_MAX_WIDTH_PX = 600;
export const TABLET_MAX_WIDTH_PX = 1100;

/** §9's table, in the order its rules have to be read:
 *
 *  1. height <= 500 (and wider than tall): phone landscape, whatever the
 *     width. "The desktop layout is never used below 500 px of height" --
 *     an 844 x 390 phone and a 1200 x 480 window both get the side sheet. A
 *     box under 500 px tall that is taller than wide (a 320 x 480 portrait
 *     phone) is at most 500 px wide, too narrow for a 360 px side sheet
 *     beside the map, so it falls through to rule 2.
 *  2. width <= 600: phone portrait. The table's own rule also says
 *     "height > width"; a box that is <= 600 wide, taller than 500 and not
 *     taller than it is wide (a 580 x 540 window) matches no row of the
 *     table, and the phone shell is the only layout that fits 580 px.
 *  3. width <= 1100: tablet (the desktop layout, rail collapsible, 44 px
 *     touch targets).
 *  4. otherwise desktop.
 */
export function layoutProfile(width: number, height: number): LayoutProfile {
  if (height <= LANDSCAPE_MAX_HEIGHT_PX && width > height) return "landscape";
  if (width <= PHONE_MAX_WIDTH_PX) return "phone";
  if (width <= TABLET_MAX_WIDTH_PX) return "tablet";
  return "desktop";
}

/** The phone shell (pill, control column, one sheet) serves both phone
 * profiles; the desktop layout (top bar, rail, inspector) serves the other
 * two. */
export function isPhoneShell(p: LayoutProfile): boolean {
  return p === "phone" || p === "landscape";
}

/** Touch-first profiles: 44 px targets (§8.2; tablets "are touch", §9). */
export function isTouchProfile(p: LayoutProfile): boolean {
  return p !== "desktop";
}

// ---------------------------------------------------------------- safe areas

/** env(safe-area-inset-*), measured (hooks/usePhoneMetrics.ts). */
export interface SafeArea {
  top: number;
  right: number;
  bottom: number;
  left: number;
}
export const NO_SAFE_AREA: SafeArea = { top: 0, right: 0, bottom: 0, left: 0 };

// ---------------------------------------------------------------- landscape

/** §9: the landscape side sheet is 360 px wide (plus the left safe inset,
 * which it pads rather than covers), full height. */
export const SIDE_SHEET_WIDTH_PX = 360;

/** The side sheet's outer width: 0 when collapsed, else 360 + the left inset. */
export function sideSheetWidth(open: boolean, safe: SafeArea): number {
  return open ? SIDE_SHEET_WIDTH_PX + safe.left : 0;
}

/** §9 "map safe rectangle is the rest": right of the side sheet, below the
 * pill (which sits over the map, right of the sheet), left of the control
 * column, above the bottom inset. Insets from the page edges (the map fills
 * the page in the phone shell). */
export function landscapeSafeInsets(open: boolean, safe: SafeArea): Insets {
  return {
    left: sideSheetWidth(open, safe) + (open ? 0 : safe.left) + FLOAT_GUTTER_PX,
    top: safe.top + PILL_TOP_PX + PILL_HEIGHT_PX + SAFE_GAP_PX,
    right: safe.right + FLOAT_GUTTER_PX + CONTROL_SIZE_PX + SAFE_GAP_PX,
    bottom: safe.bottom + FLOAT_GUTTER_PX,
  };
}

// ---------------------------------------------------------------- desktop and tablet

/** §5: a 56 px top bar and a 320 px rail. */
export const TOP_BAR_HEIGHT_PX = 56;
export const RAIL_WIDTH_PX = 320;
/** §5: the inspector floats 20 px in from the map's top and right edges;
 * 380 px on desktop, 360 on a tablet (§9). */
export const INSPECTOR_INSET_PX = 20;
export function inspectorWidth(p: LayoutProfile): number {
  return p === "tablet" ? 360 : 380;
}
/** §5: 40 px controls on desktop (a pointer), 44 on a tablet (touch, §8.2). */
export function desktopControlSize(p: LayoutProfile): number {
  return p === "tablet" ? 44 : 40;
}
/** The map-quality strip's height at the bottom-left (a 40 px control, 44
 * on touch). */
export function qualityStripHeight(p: LayoutProfile): number {
  return p === "tablet" ? 44 : 40;
}
/** The fullscreen fallback's floating search box (the top bar is gone). */
export const FULLSCREEN_SEARCH_HEIGHT_PX = 44;
/** Floating chrome's inset from the map box's edges. */
export const DESKTOP_GUTTER_PX = 20;
/** Clear space kept between the fit rectangle and chrome. */
export const DESKTOP_GAP_PX = 12;

/** The left edge of floating chrome, as CSS: the gutter, plus the left
 * safe inset where the map box reaches the screen's edge (no rail, or
 * fullscreen). */
export function desktopLeftEdge(atScreenEdge: boolean): string {
  return atScreenEdge ? `calc(${DESKTOP_GUTTER_PX}px + env(safe-area-inset-left, 0px))` : `${DESKTOP_GUTTER_PX}px`;
}

export interface DesktopChrome {
  profile: LayoutProfile;
  /** The inspector is open (something is selected). */
  inspector: boolean;
  /** The rail is on screen (always on desktop; toggled on a tablet). It
   * sits outside the map box, so it only decides whether the map box
   * reaches the left edge of the screen and so has to clear the left safe
   * inset itself. */
  rail: boolean;
  /** Fullscreen (native or the CSS fallback): no top bar and no rail, and
   * search floats in the map's top-left corner. */
  fullscreen: boolean;
  safe: SafeArea;
}

/** §5 "the fit safe rectangle is the map element's box minus the inspector
 * when it is open, computed from the real chrome": insets from the MAP
 * BOX's edges (not the window's), built from the same constants the chrome
 * is laid out with. The quality strip (bottom-left) and the controls
 * (bottom-right) are cleared along the bottom edge; the inspector, when
 * open, along the right; the safe-area insets wherever the map box reaches
 * the screen's edge (every side in fullscreen; never the top otherwise --
 * the top bar carries it). */
export function desktopSafeInsets(c: DesktopChrome): Insets {
  const edgeLeft = c.fullscreen || !c.rail ? c.safe.left : 0;
  const edgeTop = c.fullscreen ? c.safe.top : 0;
  const controls = desktopControlSize(c.profile);
  const top = c.fullscreen
    ? edgeTop + DESKTOP_GUTTER_PX + FULLSCREEN_SEARCH_HEIGHT_PX + DESKTOP_GAP_PX
    : DESKTOP_GUTTER_PX;
  const right = c.inspector
    ? c.safe.right + INSPECTOR_INSET_PX + inspectorWidth(c.profile) + DESKTOP_GAP_PX
    : c.safe.right + DESKTOP_GUTTER_PX + controls + DESKTOP_GAP_PX;
  return {
    left: edgeLeft + DESKTOP_GUTTER_PX,
    top,
    right,
    bottom: c.safe.bottom + DESKTOP_GUTTER_PX + qualityStripHeight(c.profile) + DESKTOP_GAP_PX,
  };
}
