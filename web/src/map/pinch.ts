// Pinch-to-transform math, pulled out of MapRenderer's pointerMove into its
// own pure function so web/scripts/check-pinch-math.ts can exercise it
// directly with a scripted sequence of midpoints/spreads, without driving an
// actual two-finger gesture -- CDP's synthesized touch events proved
// unreliable for that in this environment (see
// web/scripts/check-view-stability.mjs's zoomIn() comment: a scripted pinch
// there compounded into a large, unrealistic pan for the same underlying
// reason this file exists).
//
// Deliberately differs from viewer/template.html's pointermove handler
// (frozen, not touched -- CLAUDE.md): `tx=m[0]-(pinch.m[0]-tx)*(nk/pinch.k)`
// reapplies the ratio since the pinch START to the CURRENT tx on every call.
// That's fine for exactly one call, but a two-finger pinch delivers a
// pointermove per animation frame even once the fingers stop moving-ish
// (both real touch input and, worse, scripted CDP touch events can deliver
// near-duplicate frames) -- and every one of those calls rewrites tx
// relative to whatever the PREVIOUS call already rewrote it to. Solving the
// fixed point: tx_{i+1} - tx* = (tx_i - tx*) * (nk/pinch.k), so held-still
// input doesn't stay put, it walks toward (or, once nk/pinch.k > 1, away
// from) a fixed point geometrically every frame -- "the view jumps" during
// a pinch, the same complaint fix/keep-view-on-select's other two fixes
// address for the ResizeObserver and repoKey-effect cases.
//
// Anchoring to the pinch's own start state (tx0/ty0, captured once when the
// second finger goes down -- NOT the live this.tx/this.ty) removes the
// compounding: the world point that was under the pinch's start midpoint
// stays under the CURRENT midpoint, full stop, computed fresh from the
// anchor every time rather than incrementally from the last frame's output.

export interface PinchAnchor {
  /** Midpoint (SVG-space) when the second finger went down. */
  readonly m: readonly [number, number];
  /** k when the second finger went down. */
  readonly k: number;
  /** tx/ty when the second finger went down -- the anchor's own start
   * state, not the live this.tx/this.ty a later frame may have already
   * moved. Reading the live value instead of these two fields is exactly
   * the bug this module exists to avoid reintroducing. */
  readonly tx0: number;
  readonly ty0: number;
}

/** Returns the (tx, ty) that keeps the world point under the pinch's start
 * midpoint (anchor.m, mapped through anchor.tx0/ty0/anchor.k) fixed under
 * the CURRENT midpoint, at the CURRENT k. `nk` is passed in already clamped
 * -- clamping needs the live document's fitScale()/fullScale(), which is
 * MapRenderer state this pure function has no business depending on. */
export function pinchTransform(
  anchor: PinchAnchor,
  midpoint: readonly [number, number],
  nk: number,
): { tx: number; ty: number } {
  const r = nk / anchor.k;
  return {
    tx: midpoint[0] - (anchor.m[0] - anchor.tx0) * r,
    ty: midpoint[1] - (anchor.m[1] - anchor.ty0) * r,
  };
}
