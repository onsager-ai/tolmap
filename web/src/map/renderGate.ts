// docs/UX.md §7.1 rule 4, "no repaint mid-gesture": while any pointer is down
// on the map, and for QUIET_AFTER_GESTURE_MS (gestures.ts) after the last
// pointerup, a React-driven MapRenderer.render(state) is held here instead of
// painting, and only the latest held state is applied, once, when the hold is
// released. The renderer's own transform-only gesture frames and its settle
// paint are not React-driven and never pass through this.
//
// Why: paint() replaces every child of the map <svg>. A paint that lands
// between a tap's pointerup and its click deletes the element the click is
// about to be dispatched to, and the click then resolves to empty map and
// clears the selection -- reference bug fix #2, which the port kept for its
// OWN paints but not for React's. A symbols fetch resolving at that moment
// was enough to trigger it.
//
// The timing (when to hold, when to release) is MapRenderer's, because it
// needs timers and the DOM's click; this class is only the pure part, so
// web/scripts/check-gestures.ts can test it directly.
export class RenderGate<S> {
  private holding = false;
  private pending: S | null = null;

  /** Starts (or extends) a hold: offers from now on are queued. */
  hold(): void {
    this.holding = true;
  }

  /** True while offers are being queued. */
  get held(): boolean {
    return this.holding;
  }

  /** The newest queued state, or null. Readers that act on "the state React
   * last asked for" (the two-step file tap reads the current selection)
   * prefer this over what was last painted. */
  get latest(): S | null {
    return this.pending;
  }

  /** Returns true when the caller should apply `s` now. While held, `s`
   * replaces anything queued before it and false is returned. */
  offer(s: S): boolean {
    if (!this.holding) return true;
    this.pending = s;
    return false;
  }

  /** Ends the hold and hands back the newest queued state (null if nothing
   * was queued) for the caller to apply exactly once. */
  release(): S | null {
    this.holding = false;
    const s = this.pending;
    this.pending = null;
    return s;
  }

  /** Forgets a queued state without applying it: a newer state reached the
   * renderer by another path (a new document's fit) and supersedes it. The
   * hold itself is unchanged. */
  drop(): void {
    this.pending = null;
  }
}
