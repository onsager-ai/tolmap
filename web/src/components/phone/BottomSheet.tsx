import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { SIDE_SHEET_WIDTH_PX } from "@/map/layoutProfile";
import {
  DRAG_DECIDE_PX,
  SHEET_DRAG_SLOP_PX,
  dragHeight,
  dragOwner,
  nextDetent,
  releaseVelocity,
  snapDetent,
  type Detent,
  type DetentHeights,
  type DragOwner,
} from "@/map/phoneShell";

interface Props {
  detent: Detent;
  heights: DetentHeights;
  onDetent(d: Detent): void;
  children: ReactNode;
}

/** One pointer's drag on the sheet, from pointerdown until it ends. */
interface Gesture {
  id: number;
  y0: number;
  h0: number;
  zone: "handle" | "content";
  scrollTop: number;
  canScroll: boolean;
  /** Decided once the finger has moved DRAG_DECIDE_PX vertically
   * (map/phoneShell.ts's dragOwner); null until then. */
  owner: DragOwner | null;
  /** The sheet itself has started moving (past SHEET_DRAG_SLOP_PX). */
  moved: boolean;
  samples: Array<[number, number]>;
}

/** The scroll state under the finger: every scroller from the touched
 * element up to the sheet body. At Peek and Half the body is clipped
 * (overflow hidden), so nothing there counts as a scroller. */
function scrollStateAt(target: Element, body: HTMLElement | null): { scrollTop: number; canScroll: boolean } {
  let scrollTop = 0;
  let canScroll = false;
  if (!body || !body.contains(target)) return { scrollTop, canScroll };
  for (let el: Element | null = target; el; el = el.parentElement) {
    const h = el as HTMLElement;
    const oy = getComputedStyle(h).overflowY;
    if ((oy === "auto" || oy === "scroll") && h.scrollHeight > h.clientHeight + 1) {
      canScroll = true;
      // iOS reports a negative offset while the content bounces past its
      // top: that is "at the top".
      scrollTop += Math.max(0, h.scrollTop);
    }
    if (el === body) break;
  }
  return { scrollTop, canScroll };
}

/** docs/UX.md §3.1: the phone's one bottom sheet, three detents.
 *
 * The sheet is always laid out at its Full height and translated down so
 * only the current detent's height shows -- so dragging only changes a
 * transform, and the content never re-lays out mid-drag. Heights come from
 * the caller (map/phoneShell.ts's detentHeights, measured from
 * visualViewport), never from `vh`.
 *
 * Drag, flick and grabber tap follow the approved prototype: a drag starts
 * after SHEET_DRAG_SLOP_PX of travel (so a tap on a row is still a tap, and
 * the click that ends a drag is swallowed); release snaps by position and
 * velocity (phoneShell.ts's snapDetent); the grabber is a real button that
 * cycles peek -> half -> full -> peek. At Peek and Half the body is clipped
 * and every vertical drag moves the sheet. At Full the body scrolls, and
 * who owns a drag is phoneShell.ts's dragOwner (the standard bottom-sheet
 * rule; owner, 2026-09-28): the grabber and the card header (any element
 * marked `data-sheet-dragzone`) always move the sheet; a downward drag that
 * starts with the content at its top moves the sheet; one that starts
 * mid-scroll scrolls; an upward drag scrolls. Dragging below Peek springs
 * back (§3.5).
 *
 * How the sheet takes a drag from a native scroller: the body keeps
 * `touch-action: pan-y` at Full, so scrolling stays native (momentum, the
 * edge bounce) and `overscroll-behavior: contain` keeps it from chaining to
 * the page. Pointer events alone cannot stop a native scroll -- the browser
 * decides at the first touchmove, and once it scrolls it sends
 * pointercancel -- and iOS Safari has no directional `touch-action`
 * (pan-up / pan-down). So a non-passive touchmove listener cancels the
 * touch whenever dragOwner gave the drag to the sheet; the pointer events
 * that follow then drive the sheet as at Peek and Half. The grabber and the
 * headers are `touch-action: none` besides. */
export function BottomSheet({ detent, heights, onDetent, children }: Props) {
  const [dragH, setDragH] = useState<number | null>(null);
  const drag = useRef<Gesture | null>(null);
  const suppressClick = useRef(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  const sheetRef = useRef<HTMLElement>(null);
  const detentRef = useRef(detent);
  detentRef.current = detent;
  const full = heights.full;
  const shown = dragH ?? heights[detent];

  // Back at Peek the content starts from its top again.
  useEffect(() => {
    if (detent !== "full" && bodyRef.current) bodyRef.current.scrollTop = 0;
  }, [detent]);

  /** Decides the drag's owner once it has moved far enough vertically;
   * returns it (null while undecided). */
  function decide(g: Gesture, dy: number): DragOwner | null {
    if (g.owner) return g.owner;
    if (Math.abs(dy) < DRAG_DECIDE_PX) return null;
    g.owner = dragOwner({
      detent: detentRef.current,
      scrollTop: g.scrollTop,
      canScroll: g.canScroll,
      direction: dy > 0 ? "down" : "up",
      zone: g.zone,
    });
    return g.owner;
  }

  // Non-passive, so it can cancel the native scroll a drag that belongs to
  // the sheet would otherwise start (React's own touchmove is passive).
  useEffect(() => {
    const el = sheetRef.current;
    if (!el) return;
    const onTouchMove = (e: TouchEvent) => {
      const g = drag.current;
      if (!g || e.touches.length !== 1) return;
      if (decide(g, e.touches[0].clientY - g.y0) === "sheet" && e.cancelable) e.preventDefault();
    };
    el.addEventListener("touchmove", onTouchMove, { passive: false });
    return () => el.removeEventListener("touchmove", onTouchMove);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function onPointerDown(e: ReactPointerEvent) {
    // A second finger is not a new drag (the first one keeps it). A stale
    // gesture (a mouse released outside the sheet before it moved, so
    // never captured) is simply replaced.
    if (!e.isPrimary) return;
    const target = e.target as Element;
    const zone = target.closest("[data-sheet-grabber], [data-sheet-dragzone]") ? "handle" : "content";
    const { scrollTop, canScroll } = scrollStateAt(target, bodyRef.current);
    drag.current = { id: e.pointerId, y0: e.clientY, h0: shown, zone, scrollTop, canScroll, owner: null, moved: false, samples: [[e.timeStamp, e.clientY]] };
  }
  function onPointerMove(e: ReactPointerEvent) {
    const d = drag.current;
    if (!d || e.pointerId !== d.id) return;
    const dy = e.clientY - d.y0;
    if (decide(d, dy) !== "sheet") return;
    if (!d.moved) {
      if (Math.abs(dy) < SHEET_DRAG_SLOP_PX) return;
      d.moved = true;
      try {
        sheetRef.current?.setPointerCapture(e.pointerId);
      } catch {
        /* the pointer may already be gone; the drag still works uncaptured */
      }
    }
    setDragH(dragHeight(d.h0, dy, heights));
    d.samples.push([e.timeStamp, e.clientY]);
    if (d.samples.length > 6) d.samples.shift();
  }
  function onPointerEnd(e: ReactPointerEvent) {
    const d = drag.current;
    if (!d || e.pointerId !== d.id) return;
    drag.current = null;
    if (!d.moved) return;
    suppressClick.current = true;
    setTimeout(() => {
      suppressClick.current = false;
    }, 0);
    const h = dragHeight(d.h0, e.clientY - d.y0, heights);
    setDragH(null);
    onDetent(snapDetent(h, releaseVelocity(d.samples), heights));
  }

  return (
    <section
      ref={sheetRef}
      aria-label="Map details"
      data-phone-sheet
      data-selection-panel
      data-detent={detent}
      data-sheet-height={Math.round(shown)}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerEnd}
      onPointerCancel={onPointerEnd}
      onClickCapture={(e) => {
        if (suppressClick.current) {
          e.stopPropagation();
          e.preventDefault();
        }
      }}
      className={`absolute inset-x-0 bottom-0 z-30 flex flex-col rounded-t-[20px] border-t border-[var(--rule)] bg-[var(--chrome)] text-[var(--on)] shadow-[0_-8px_24px_rgba(0,0,0,.3)] ${dragH == null ? "transition-transform duration-300 ease-[cubic-bezier(.2,.8,.2,1)] motion-reduce:transition-none" : ""}`}
      style={{ height: full, transform: `translateY(${full - shown}px)`, touchAction: detent === "full" ? "auto" : "none" }}
    >
      <button
        type="button"
        data-sheet-grabber
        aria-expanded={detent !== "peek"}
        aria-label={`Sheet at ${detent}; tap to ${detent === "full" ? "collapse" : "expand"}`}
        onClick={() => {
          if (suppressClick.current) return;
          onDetent(nextDetent(detent));
        }}
        // 44 px tall (docs/UX.md §8.2), laid over the top of the body so the
        // sheet only spends the 28 px the design gives the grabber; the
        // extra 16 px overlap only the middle 88 px of the header.
        className="absolute left-1/2 top-0 z-10 flex h-11 w-[88px] -translate-x-1/2 items-start justify-center pt-3"
        style={{ touchAction: "none" }}
      >
        <span className="block h-[5px] w-9 rounded-[3px] bg-[var(--grabber)]" />
      </button>
      <div
        ref={bodyRef}
        data-sheet-body
        className={`min-h-0 flex-1 px-5 pt-7 ${detent === "full" ? "overflow-y-auto" : "overflow-hidden"}`}
        style={{
          paddingBottom: "calc(20px + env(safe-area-inset-bottom, 0px))",
          touchAction: detent === "full" ? "pan-y" : "none",
          overscrollBehavior: "contain",
        }}
      >
        {children}
      </div>
    </section>
  );
}

/** docs/UX.md §9: a phone held sideways (height <= 500) gets the sheet as a
 * side sheet on the left -- 360 px wide, full height -- with the detents
 * mapped to widths: collapsed 0, open 360 (the control column's panel
 * button toggles it; a new selection opens it). The detent still says how
 * much of a card to show (a file's key symbols, or all of them at Full), but
 * the sheet's box no longer changes with it: the whole height is there at
 * every detent, so the content always scrolls and there is nothing to drag.
 * Same data attributes as the bottom sheet, so the checks and the back
 * stack read one contract. The left safe inset is padded inside it (the
 * notch side of a landscape phone), the top and bottom ones too. */
export function SideSheet({ open, detent, children }: { open: boolean; detent: Detent; children: ReactNode }) {
  return (
    <section
      aria-label="Map details"
      data-phone-sheet
      data-side-sheet
      data-selection-panel
      data-detent={detent}
      data-side-open={open}
      className={`absolute inset-y-0 left-0 z-30 flex flex-col border-r border-[var(--rule)] bg-[var(--chrome)] text-[var(--on)] shadow-[8px_0_24px_rgba(0,0,0,.3)] transition-[transform,visibility] duration-300 ease-[cubic-bezier(.2,.8,.2,1)] motion-reduce:transition-none ${open ? "" : "invisible"}`}
      style={{
        width: `calc(${SIDE_SHEET_WIDTH_PX}px + env(safe-area-inset-left, 0px))`,
        paddingLeft: "env(safe-area-inset-left, 0px)",
        transform: open ? "none" : "translateX(-100%)",
      }}
    >
      <div
        data-sheet-body
        className="min-h-0 flex-1 overflow-y-auto px-5"
        style={{
          paddingTop: "calc(16px + env(safe-area-inset-top, 0px))",
          paddingBottom: "calc(20px + env(safe-area-inset-bottom, 0px))",
          touchAction: "pan-y",
          overscrollBehavior: "contain",
        }}
      >
        {children}
      </div>
    </section>
  );
}
