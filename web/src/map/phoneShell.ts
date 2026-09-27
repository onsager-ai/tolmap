// docs/UX.md §3.1-3.3: the phone shell's geometry -- sheet detents, the
// drag-release snap, and the map's safe rectangle -- as pure functions, so
// web/scripts/check-phone-shell.ts can test them without a browser (the same
// reason map/gestures.ts is pure).
//
// Everything is in CSS pixels. Heights come from the VISIBLE viewport
// (window.visualViewport.height, measured by the caller) and the measured
// safe-area insets, never from `vh`: iOS `vh` is the large viewport, which is
// what put the old 58vh sheet over the lower zoom buttons (§3.1).

import type { Insets } from "./geometry";

export type Detent = "peek" | "half" | "full";
export const DETENT_ORDER: readonly Detent[] = ["peek", "half", "full"];

/** §3: the search pill sits 12 px below the safe-area top, 48 px tall. */
export const PILL_TOP_PX = 12;
export const PILL_HEIGHT_PX = 48;
/** §8.2: 12 px gutter for floating controls. */
export const FLOAT_GUTTER_PX = 12;
/** §3 control column: 44 x 44 buttons on the right edge. */
export const CONTROL_SIZE_PX = 44;
/** §3.3's 8 px breathing room between the safe rectangle and any chrome. */
export const SAFE_GAP_PX = 8;
/** §3.1 Peek: 156 px plus the bottom safe inset. */
export const PEEK_BASE_PX = 156;
/** §3.1 Half: 480 px, or 57% of the visible height if that is smaller. */
export const HALF_MAX_PX = 480;
export const HALF_SHARE = 0.57;
/** A sheet drag starts once the finger has moved this far (the prototype's
 * value), so a tap on a row inside the sheet stays a tap. */
export const SHEET_DRAG_SLOP_PX = 8;
/** Release velocity (px/ms) above which a drag is a flick: it goes one
 * detent further in the flick's direction instead of to the nearest. */
export const SHEET_FLICK_PX_PER_MS = 0.45;
/** How far below Peek the sheet may be dragged before it springs back
 * (§3.5: "dragging the sheet below Peek is not a state"). */
export const SHEET_UNDERSHOOT_PX = 40;

export interface PhoneMetrics {
  /** Visible viewport, CSS px (visualViewport, falling back to inner*). */
  width: number;
  height: number;
  /** env(safe-area-inset-top/bottom), measured. */
  safeTop: number;
  safeBottom: number;
}

/** Bottom edge of the search pill, from the top of the page. */
export function pillBottom(m: PhoneMetrics): number {
  return m.safeTop + PILL_TOP_PX + PILL_HEIGHT_PX;
}

export type DetentHeights = Record<Detent, number>;

/** §3.1's three heights. Half is clamped between Peek and Full so a very
 * short viewport (landscape is phase 5) never inverts the order. */
export function detentHeights(m: PhoneMetrics): DetentHeights {
  const peek = PEEK_BASE_PX + m.safeBottom;
  const full = Math.max(peek, Math.round(m.height - (pillBottom(m) + SAFE_GAP_PX)));
  const half = Math.min(full, Math.max(peek, Math.min(HALF_MAX_PX, Math.round(m.height * HALF_SHARE))));
  return { peek, half, full };
}

/** §3.3: `[left 12, top pillBottom + 8, right W - 12 - 44 - 8, bottom H -
 * sheetHeight - 8]`, expressed as insets from the map box's edges (the map
 * fills the page on a phone, so the page edges are the map's). */
export function safeInsets(m: PhoneMetrics, sheetHeight: number): Insets {
  return {
    left: FLOAT_GUTTER_PX,
    top: pillBottom(m) + SAFE_GAP_PX,
    right: FLOAT_GUTTER_PX + CONTROL_SIZE_PX + SAFE_GAP_PX,
    bottom: sheetHeight + SAFE_GAP_PX,
  };
}

/** The same rectangle in page coordinates, for tests and for "is the
 * selection covered" questions. */
export function safeRect(m: PhoneMetrics, sheetHeight: number): { left: number; top: number; right: number; bottom: number } {
  const i = safeInsets(m, sheetHeight);
  return { left: i.left, top: i.top, right: m.width - i.right, bottom: m.height - i.bottom };
}

/** Where a released drag settles, from the sheet's visible height at release
 * and the release velocity (px/ms, positive = finger moving DOWN, i.e. the
 * sheet shrinking). The prototype's rule: a flick goes to the next detent in
 * its direction from where the sheet is; a slow release goes to the nearest
 * detent. Below Peek always springs back to Peek. */
export function snapDetent(height: number, velocity: number, heights: DetentHeights): Detent {
  const hs = DETENT_ORDER.map((d) => heights[d]);
  let nearest = 0;
  for (let j = 1; j < hs.length; j++) if (Math.abs(hs[j] - height) < Math.abs(hs[nearest] - height)) nearest = j;
  if (Math.abs(velocity) > SHEET_FLICK_PX_PER_MS) {
    if (velocity < 0) {
      // Flicked up: the nearest detent if it is above us, else the next one up.
      return DETENT_ORDER[Math.min(hs.length - 1, hs[nearest] > height ? nearest : nearest + 1)];
    }
    return DETENT_ORDER[Math.max(0, hs[nearest] < height ? nearest : nearest - 1)];
  }
  return DETENT_ORDER[nearest];
}

/** §3.1: the grabber is a real button; tapping it cycles peek -> half ->
 * full -> peek. */
export function nextDetent(d: Detent): Detent {
  return d === "peek" ? "half" : d === "half" ? "full" : "peek";
}

/** The sheet height a live drag shows: the start height minus the finger's
 * travel, clamped to [peek - undershoot, full]. */
export function dragHeight(startHeight: number, dy: number, heights: DetentHeights): number {
  return Math.max(heights.peek - SHEET_UNDERSHOOT_PX, Math.min(heights.full, startHeight - dy));
}

/** Release velocity over the last few samples, px/ms (+ = down). */
export function releaseVelocity(samples: ReadonlyArray<readonly [number, number]>): number {
  if (samples.length < 2) return 0;
  const [t0, y0] = samples[0];
  const [t1, y1] = samples[samples.length - 1];
  return (y1 - y0) / Math.max(1, t1 - t0);
}
