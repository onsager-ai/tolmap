// The map's pointer-gesture rules (docs/UX.md §7.1 rules 1-3), pulled out of
// MapRenderer's pointerdown/pointermove/pointerup handlers into one pure
// state machine, in the same spirit as pinch.ts: the renderer feeds it
// pointer events and applies the intents it returns; it never infers a tap,
// a pan or a pinch itself. No DOM here, so web/scripts/check-gestures.ts can
// drive every rule with scripted event sequences instead of CDP touch
// input, which proved unreliable for exactly this (check-view-stability.mjs's
// zoomIn() comment: a scripted double-tap never landed inside 300 ms, and a
// scripted pinch arrived as two half-updated pointer dispatches).
//
// What the old inline handlers got wrong, each now a rule with a test:
//
// - Tap slop was 4 px of CUMULATIVE path length (sum of per-move hypot), so
//   ordinary finger jitter -- 1-2 px back and forth -- crossed it and turned
//   taps into drags. A tap is now judged by straight-line distance from the
//   pointerdown point: <= 10 px on touch (and pen), <= 4 px on a mouse, and
//   under 500 ms.
// - Double-tap zoomed about the viewport centre, had no distance check, and
//   counted the end of a pinch as a first tap (the pinch's own fingers never
//   ran the drag-distance accumulator, so the sequence looked motionless).
//   It is now two single-pointer taps within 300 ms (first tap's pointerup to
//   the second's pointerdown) and 24 px of each other, and never a pinch end
//   or a cancelled sequence. The renderer zooms 2x about the tapped point.
// - A lost pointerup (the browser dropped it, or a repaint removed the
//   element a touch pointer was implicitly captured to) left a ghost entry in
//   the pointer map, so the next one-finger drag became a two-pointer pinch.
//   An isPrimary pointerdown means the browser has no other pointer of that
//   kind active, so any pointer still tracked then is a ghost: the state is
//   reset. pointercancel and lostpointercapture (for a pointer still
//   tracked, i.e. no pointerup came first) reset it too.
// - A third finger was added to the pointer map, and mid()/dist() read
//   whichever two entries came first, so the view jumped. A third pointer is
//   now ignored entirely: its down, moves, up and cancel change nothing.
//
// Coordinates are whatever the caller passes, used only for distances and
// deltas; the renderer passes CSS pixels (clientX/clientY) so the slop is a
// real on-screen distance, and converts intents to SVG units itself.
//
// Double-tap is only recognised when both taps come from a non-mouse
// pointer. The inline handler it replaces gated double-tap on a coarse
// pointer (MapRenderer.TOUCH); a desktop double-click never zoomed, and this
// keeps it that way.

export const TAP_SLOP_TOUCH_PX = 10;
export const TAP_SLOP_MOUSE_PX = 4;
export const TAP_MAX_MS = 500;
export const DOUBLE_TAP_MS = 300;
export const DOUBLE_TAP_PX = 24;
/** docs/UX.md §7.1 rule 4: React-driven render() calls stay queued for this
 * long after the last pointerup. Lives here, next to the other gesture
 * timings; the queue itself is renderGate.ts, driven by MapRenderer. */
export const QUIET_AFTER_GESTURE_MS = 350;
/** docs/UX.md §3.5: panning the map with the phone's sheet raised returns
 * the sheet to Peek once the pan has travelled this far (CSS px, summed
 * along the pan -- here the question is "is the person moving the map",
 * not "was this a tap", so path length is the right measure). */
export const PAN_DISMISS_PX = 24;

export type GestureEventType = "down" | "move" | "up" | "cancel" | "lostcapture";

export interface GestureEvent {
  type: GestureEventType;
  id: number;
  x: number;
  y: number;
  /** Milliseconds, any monotonic origin (the renderer passes e.timeStamp). */
  t: number;
  /** PointerEvent.pointerType: "mouse", "touch", "pen" (or "" if unknown,
   * treated like touch). */
  pointerType: string;
  isPrimary: boolean;
}

export type GestureIntent =
  /** A single pointer left its tap slop: the map should follow it. `start`
   * is true on the first pan intent of a sequence, whose delta is the whole
   * displacement since pointerdown (so the point grabbed stays under the
   * finger); later deltas are since the previous move. The renderer takes
   * pointer capture for `id` on `start` (never at pointerdown -- reference
   * bug fix #1, capture retargets the click). */
  | { kind: "pan"; id: number; dx: number; dy: number; start: boolean }
  /** A second pointer went down. The renderer captures BOTH ids on the map
   * element, so a repaint that replaces child nodes cannot orphan either. */
  | { kind: "pinch-start"; ids: [number, number]; mid: [number, number] }
  /** `scale` is the current finger spread over the spread at pinch-start;
   * `mid` is the current midpoint. Absolute, not incremental, so the
   * renderer can anchor to the pinch's own start state (pinch.ts). */
  | { kind: "pinch"; mid: [number, number]; scale: number }
  /** A single-pointer tap ended. The browser's click follows; the renderer
   * lets exactly that click through to selection. */
  | { kind: "tap"; x: number; y: number; pointerType: string }
  /** The second tap of a double-tap ended: zoom 2x about (x, y), and swallow
   * this tap's click. The first tap already selected and that stands. */
  | { kind: "double-tap"; x: number; y: number }
  /** Gesture state was reset without a tap (pointercancel,
   * lostpointercapture, or a ghost pointer found on an isPrimary down). */
  | { kind: "cancel" }
  /** The last tracked pointer went up. Always after any tap/double-tap
   * intent of the same event. */
  | { kind: "end" };

interface Tracked {
  x0: number;
  y0: number;
  x: number;
  y: number;
  t0: number;
  pointerType: string;
}

type Mode = "idle" | "press" | "pan" | "pinch" | "residual";

export function tapSlop(pointerType: string): number {
  return pointerType === "mouse" ? TAP_SLOP_MOUSE_PX : TAP_SLOP_TOUCH_PX;
}

export class GestureRecognizer {
  private pointers = new Map<number, Tracked>();
  private mode: Mode = "idle";
  // Last move position of the panning pointer, for incremental deltas.
  private panLast: [number, number] = [0, 0];
  private pinchIds: [number, number] | null = null;
  private pinchStartDist = 1;
  // The previous completed tap, a candidate first half of a double-tap.
  // Cleared by anything that is not a plain tap: a pan, a pinch, a cancel,
  // a too-long press, a double-tap itself.
  private lastTap: { x: number; y: number; tUp: number; pointerType: string } | null = null;

  /** True while any tracked pointer is down. */
  get active(): boolean {
    return this.pointers.size > 0;
  }

  /** How many pointers are tracked (never more than 2). */
  get pointerCount(): number {
    return this.pointers.size;
  }

  handle(e: GestureEvent): GestureIntent[] {
    switch (e.type) {
      case "down":
        return this.down(e);
      case "move":
        return this.move(e);
      case "up":
        return this.up(e);
      case "cancel":
      case "lostcapture":
        // Only for a pointer still tracked: a third, ignored pointer's cancel
        // changes nothing, and the lostpointercapture every captured pointer
        // gets right after its own pointerup arrives once it is already gone.
        return this.pointers.has(e.id) ? this.reset() : [];
    }
  }

  /** Drops every tracked pointer and the double-tap candidate. */
  private reset(): GestureIntent[] {
    this.pointers.clear();
    this.mode = "idle";
    this.pinchIds = null;
    this.lastTap = null;
    return [{ kind: "cancel" }];
  }

  private down(e: GestureEvent): GestureIntent[] {
    const out: GestureIntent[] = [];
    if (e.isPrimary && this.pointers.size > 0) out.push(...this.reset());
    if (this.pointers.has(e.id)) return out;
    if (this.pointers.size >= 2) return out; // a third pointer is ignored
    this.pointers.set(e.id, { x0: e.x, y0: e.y, x: e.x, y: e.y, t0: e.t, pointerType: e.pointerType });
    if (this.pointers.size === 1) {
      this.mode = "press";
      return out;
    }
    // Second pointer: a pinch, whatever the first one was doing.
    const ids = [...this.pointers.keys()] as [number, number];
    this.pinchIds = ids;
    this.pinchStartDist = this.spread(ids);
    this.mode = "pinch";
    this.lastTap = null;
    out.push({ kind: "pinch-start", ids, mid: this.midpoint(ids) });
    return out;
  }

  private move(e: GestureEvent): GestureIntent[] {
    const p = this.pointers.get(e.id);
    if (!p) return [];
    p.x = e.x;
    p.y = e.y;
    switch (this.mode) {
      case "press": {
        if (Math.hypot(p.x - p.x0, p.y - p.y0) <= tapSlop(p.pointerType)) return [];
        this.mode = "pan";
        this.lastTap = null;
        this.panLast = [p.x, p.y];
        return [{ kind: "pan", id: e.id, dx: p.x - p.x0, dy: p.y - p.y0, start: true }];
      }
      case "pan": {
        const dx = p.x - this.panLast[0];
        const dy = p.y - this.panLast[1];
        this.panLast = [p.x, p.y];
        return [{ kind: "pan", id: e.id, dx, dy, start: false }];
      }
      case "pinch": {
        const ids = this.pinchIds!;
        return [{ kind: "pinch", mid: this.midpoint(ids), scale: this.spread(ids) / this.pinchStartDist }];
      }
      default:
        // "residual": one finger left over from a pinch does nothing until
        // it lifts or a second finger joins it again.
        return [];
    }
  }

  private up(e: GestureEvent): GestureIntent[] {
    const p = this.pointers.get(e.id);
    if (!p) return [];
    p.x = e.x;
    p.y = e.y;
    this.pointers.delete(e.id);
    if (this.mode === "pinch") {
      this.mode = "residual";
      this.pinchIds = null;
    }
    if (this.pointers.size > 0) return [];
    const out: GestureIntent[] = [];
    const isTap =
      this.mode === "press" &&
      Math.hypot(p.x - p.x0, p.y - p.y0) <= tapSlop(p.pointerType) &&
      e.t - p.t0 < TAP_MAX_MS;
    if (isTap) {
      const prev = this.lastTap;
      const isDouble =
        prev != null &&
        p.pointerType !== "mouse" &&
        prev.pointerType !== "mouse" &&
        p.t0 - prev.tUp <= DOUBLE_TAP_MS &&
        Math.hypot(p.x0 - prev.x, p.y0 - prev.y) <= DOUBLE_TAP_PX;
      if (isDouble) {
        this.lastTap = null;
        out.push({ kind: "double-tap", x: p.x0, y: p.y0 });
      } else {
        this.lastTap = { x: p.x0, y: p.y0, tUp: e.t, pointerType: p.pointerType };
        out.push({ kind: "tap", x: p.x0, y: p.y0, pointerType: p.pointerType });
      }
    } else {
      this.lastTap = null;
    }
    this.mode = "idle";
    out.push({ kind: "end" });
    return out;
  }

  private midpoint([a, b]: [number, number]): [number, number] {
    const pa = this.pointers.get(a)!;
    const pb = this.pointers.get(b)!;
    return [(pa.x + pb.x) / 2, (pa.y + pb.y) / 2];
  }

  private spread([a, b]: [number, number]): number {
    const pa = this.pointers.get(a)!;
    const pb = this.pointers.get(b)!;
    return Math.hypot(pa.x - pb.x, pa.y - pb.y) || 1;
  }
}
