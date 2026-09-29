// docs/UX.md §9: which layout the map page uses, and the safe rectangles of
// the landscape side sheet and the desktop/tablet chrome -- pure, so
// web/scripts/check-layout-profile.ts can test them without a browser (the
// same reason map/phoneShell.ts and map/gestures.ts are pure).
//
// The layout is chosen by the available box, not by `vh` or the window width
// alone. The old single breakpoint (width <= 820 px, hooks/useIsNarrow.ts)
// gave an 844 x 390 landscape phone the desktop layout and a 768 px portrait
// tablet the phone shell.

import type { Insets } from "./geometry";
import { CONTROL_SIZE_PX, FLOAT_GUTTER_PX, PILL_HEIGHT_PX, PILL_TOP_PX, SAFE_GAP_PX } from "./phoneShell";

export type LayoutProfile = "phone" | "landscape" | "tablet" | "desktop";

/** §9's thresholds, CSS px. */
export const LANDSCAPE_MAX_HEIGHT_PX = 500;
export const PHONE_MAX_WIDTH_PX = 600;
export const TABLET_MAX_WIDTH_PX = 1100;

/** Issue #180: renderer density follows the map element's available CSS-px
 * box, using the same width and height edges as §9's layout profiles. A
 * landscape phone can be wider than 820 px while its short map box still
 * needs compact labels and hub rings. */
export function compactMap(width: number, height: number): boolean {
  return width <= PHONE_MAX_WIDTH_PX || height <= LANDSCAPE_MAX_HEIGHT_PX;
}

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
 *  3. width <= 1100: tablet (the desktop layout, panel hidden below 900 px,
 *     44 px touch targets).
 *  4. otherwise desktop.
 */
export function layoutProfile(width: number, height: number): LayoutProfile {
  if (height <= LANDSCAPE_MAX_HEIGHT_PX && width > height) return "landscape";
  if (width <= PHONE_MAX_WIDTH_PX) return "phone";
  if (width <= TABLET_MAX_WIDTH_PX) return "tablet";
  return "desktop";
}

/** docs/UX.md §5: desktop label styling applies to the desktop shell and
 * tablet rail; the shared 7d placement rules apply to every profile. */
export function hasDesktopMapLabels(profile: LayoutProfile): boolean {
  return profile === "tablet" || profile === "desktop";
}

/** The phone shell (pill, control column, one sheet) serves both phone
 * profiles; the floating command bar and panel serve the other two. */
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

/** §5 floating desktop/tablet chrome. Keep these with desktopSafeInsets():
 * the map frame and the objects over it use one set of measurements. */
export const DESKTOP_GUTTER_PX = 16;
export const COMMAND_BAR_TOP_PX = 16;
export const COMMAND_BAR_HEIGHT_PX = 44;
export const DESKTOP_PANEL_LEFT_PX = 16;
export const DESKTOP_PANEL_TOP_PX = 72;
export const DESKTOP_PANEL_BOTTOM_PX = 72;
export const DESKTOP_PANEL_WIDTH_PX = 360;
export const DESKTOP_LEGEND_BOTTOM_PX = 64;
export const DESKTOP_CONTROL_BOTTOM_PX = 16;
/** Extra space after the panel before the safe rectangle starts, matching
 * the desktop prototype's fit rectangle. */
export const DESKTOP_PANEL_SAFE_GAP_PX = 24;
/** The prototype's unoccluded map margin when the panel is hidden. */
export const DESKTOP_HIDDEN_LEFT_INSET_PX = 24;
/** Gap after the command row, control group and fit rectangle. */
export const DESKTOP_GAP_PX = 24;
export const DESKTOP_COMMAND_SAFE_GAP_PX = 16;
/** §5: 40 px controls on desktop (a pointer), 44 on a tablet (touch, §8.2). */
export function desktopControlSize(p: LayoutProfile): number {
  return p === "tablet" ? 44 : 40;
}
export interface DesktopChrome {
  profile: LayoutProfile;
  /** The left panel is a real inset while it is visible. */
  panelOpen: boolean;
  safe: SafeArea;
}

/** §5's fit rectangle, measured from the full-bleed map. Each edge uses the
 * same constants as the command bar, panel and controls. */
export function desktopSafeInsets(c: DesktopChrome): Insets {
  const controls = desktopControlSize(c.profile);
  return {
    left: c.safe.left + (c.panelOpen
      ? DESKTOP_PANEL_LEFT_PX + DESKTOP_PANEL_WIDTH_PX + DESKTOP_PANEL_SAFE_GAP_PX
      : DESKTOP_HIDDEN_LEFT_INSET_PX),
    top: c.safe.top + COMMAND_BAR_TOP_PX + COMMAND_BAR_HEIGHT_PX + DESKTOP_COMMAND_SAFE_GAP_PX,
    right: c.safe.right + DESKTOP_GUTTER_PX + controls + DESKTOP_GAP_PX,
    // The prototype leaves 72 px below the fitted map for the floating
    // controls and legend row, while the controls themselves stay at 16 px.
    bottom: c.safe.bottom + DESKTOP_PANEL_BOTTOM_PX,
  };
}
