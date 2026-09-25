import { useEffect, useId, useRef, useState } from "react";
import type { ReferenceCoverageSummary } from "@/map/referenceCoverage";

/** issue #82's ZoomControls precedent (its "Fit map" button doc comment):
 * "Deliberately no `title` attribute -- it would show a second, redundant
 * tooltip." This indicator needs more than a one-line accessible name can
 * carry (per-language reason and recall), so it gets its own disclosure
 * panel instead of relying on `title` at all -- never both. */
function StatusGlyph({ status }: { status: ReferenceCoverageSummary["status"] }) {
  // Shape-coded, not colour-coded (LinkLegend's own precedent: "colour is
  // never used alone"), so the three states read the same on a colour-blind
  // screen and need no state-specific token in index.css's palette:
  // `currentColor` inherits `text-[var(--on)]`/`text-[var(--dim)]` from the
  // trigger button below, which already has both a light and a dark value.
  if (status === "exact") {
    return (
      <svg width="11" height="11" viewBox="0 0 12 12" aria-hidden="true" className="inline-block align-middle">
        <circle cx="6" cy="6" r="5" fill="none" stroke="currentColor" strokeWidth="1.3" />
        <path d="M3.3 6.2 L5.2 8 L8.7 4" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
    );
  }
  if (status === "partial") {
    return (
      <svg width="11" height="11" viewBox="0 0 12 12" aria-hidden="true" className="inline-block align-middle">
        <circle cx="6" cy="6" r="5" fill="none" stroke="currentColor" strokeWidth="1.3" />
        <path d="M6 1.2 A4.8 4.8 0 0 1 6 10.8 Z" fill="currentColor" />
      </svg>
    );
  }
  return (
    <svg width="11" height="11" viewBox="0 0 12 12" aria-hidden="true" className="inline-block align-middle">
      <circle cx="6" cy="6" r="5" fill="none" stroke="currentColor" strokeWidth="1.3" strokeDasharray="1.6 1.6" />
    </svg>
  );
}

/** One language's row in the expanded detail: "Python: exact (SCIP) ·
 * recall 98%" or "JavaScript: heuristic (hand) -- the indexer isn't
 * installed". Plain words throughout (docs/GLOSSARY.md: "reference" is a
 * code-object term here, never "road"). */
function LanguageRow({ row }: { row: ReferenceCoverageSummary["languages"][number] }) {
  return (
    <li className="py-0.5">
      <span className="text-[var(--on)]">{row.label}</span>
      {": "}
      {row.exact ? "exact (SCIP)" : "heuristic (hand)"}
      {row.reasonLabel && <> — {row.reasonLabel}</>}
      {row.recallPercent != null && <> · recall {row.recallPercent}%</>}
    </li>
  );
}

/** Issue #110 P2: the collapsed "Exact references"/"Partly exact"/
 * "Heuristic references" line plus its expanded per-language detail --
 * an icon-triggered disclosure, not a text button (owner standing feedback:
 * icons with proper tooltips, not text buttons, and no native `title`
 * duplicating a real one -- see ZoomControls' "Fit map" button above).
 * Click/tap toggles it -- deliberately not hover-to-open: the owner reviews
 * this viewer from a phone as often as a desktop, and a single click-toggle
 * contract behaves identically on both profiles instead of needing a
 * touch-only fallback path a hover-based one would still need. Escape, an
 * outside click/tap, or moving keyboard focus elsewhere all close it -- no
 * @radix-ui/react-tooltip or -popover is installed (docs/ARCHITECTURE.md),
 * and the ground rules for this change forbid adding a dependency to make
 * an instruction true; this is small enough not to need one. */
export function ReferenceCoverageIndicator({ summary }: { summary: ReferenceCoverageSummary }) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const detailId = useId();

  useEffect(() => {
    if (!open) return;
    const closeIfOutside = (target: EventTarget | null) => {
      if (!rootRef.current?.contains(target as Node)) setOpen(false);
    };
    const onPointerDown = (e: PointerEvent) => closeIfOutside(e.target);
    // Tab-ing focus away (no pointer event at all) must close it too, or a
    // keyboard user leaves it open over whatever's drawn underneath.
    const onFocusIn = (e: FocusEvent) => closeIfOutside(e.target);
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("focusin", onFocusIn);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("focusin", onFocusIn);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  return (
    // `block w-fit` (FooterStats' own "unconnected files" chip precedent,
    // its doc comment above explains why): each floating footer chip gets
    // its own line sized to its own content, so this and the unconnected
    // chip stack instead of colliding in one shared line box.
    <div ref={rootRef} className="pointer-events-auto relative mt-1 block w-fit" data-reference-coverage>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={detailId}
        aria-label={`${summary.label}. Activate for the per-language breakdown.`}
        onClick={() => setOpen((v) => !v)}
        className="flex items-center gap-1 rounded border border-[var(--rule)] bg-[var(--chrome)] px-1.5 py-0.5 text-[10px] text-[var(--on)] hover:text-[var(--hot)]"
      >
        <StatusGlyph status={summary.status} />
        {summary.label}
      </button>
      {open && (
        <div
          id={detailId}
          role="group"
          aria-label="Reference coverage by language"
          className="absolute bottom-full left-0 z-30 mb-1 w-[230px] max-w-[78vw] rounded-md border border-[var(--rule)] bg-[rgba(var(--chrome-float-rgb),0.98)] p-2 text-[9.5px] leading-relaxed text-[var(--dim)] shadow-lg"
        >
          {summary.languages.length === 0 ? (
            <p>
              Built without SCIP indexing (<code>--refs scip</code> not used); every language's references come from the
              hand-written resolver.
            </p>
          ) : (
            <ul>
              {summary.languages.map((row) => (
                <LanguageRow key={row.language} row={row} />
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
