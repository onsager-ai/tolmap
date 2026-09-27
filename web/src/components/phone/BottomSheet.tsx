import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import {
  SHEET_DRAG_SLOP_PX,
  dragHeight,
  nextDetent,
  releaseVelocity,
  snapDetent,
  type Detent,
  type DetentHeights,
} from "@/map/phoneShell";

interface Props {
  detent: Detent;
  heights: DetentHeights;
  onDetent(d: Detent): void;
  children: ReactNode;
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
 * cycles peek -> half -> full -> peek. At Peek and Half a vertical drag
 * anywhere on the sheet moves the sheet and the content does not scroll; at
 * Full the content scrolls, and only the grabber and the card header (any
 * element marked `data-sheet-dragzone`) drag the sheet -- no scroll inside
 * scroll (§3.1). Dragging below Peek springs back (§3.5). */
export function BottomSheet({ detent, heights, onDetent, children }: Props) {
  const [dragH, setDragH] = useState<number | null>(null);
  const drag = useRef<{ id: number; y0: number; h0: number; moved: boolean; samples: Array<[number, number]> } | null>(null);
  const suppressClick = useRef(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  const sheetRef = useRef<HTMLElement>(null);
  const full = heights.full;
  const shown = dragH ?? heights[detent];

  // Back at Peek the content starts from its top again.
  useEffect(() => {
    if (detent !== "full" && bodyRef.current) bodyRef.current.scrollTop = 0;
  }, [detent]);

  function onPointerDown(e: ReactPointerEvent) {
    const target = e.target as Element;
    if (detent === "full" && bodyRef.current?.contains(target) && !target.closest("[data-sheet-dragzone]")) return;
    drag.current = { id: e.pointerId, y0: e.clientY, h0: shown, moved: false, samples: [[e.timeStamp, e.clientY]] };
  }
  function onPointerMove(e: ReactPointerEvent) {
    const d = drag.current;
    if (!d || e.pointerId !== d.id) return;
    const dy = e.clientY - d.y0;
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
