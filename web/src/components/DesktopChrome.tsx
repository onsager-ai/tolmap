import type { ReactNode } from "react";
import type { MapDocument } from "@/types";
import type { Layer } from "@/map/constants";
import { RAMP_STOPS, districtClass } from "@/map/geometry";
import { summarizeReferenceCoverage } from "@/map/referenceCoverage";
import {
  DESKTOP_GAP_PX,
  DESKTOP_GUTTER_PX,
  INSPECTOR_INSET_PX,
  desktopControlSize,
  desktopLeftEdge,
  inspectorWidth,
  type LayoutProfile,
} from "@/map/layoutProfile";
import { StatusGlyph } from "@/components/ReferenceCoverageIndicator";

// docs/UX.md §5: the desktop (and, §9, tablet) chrome that floats over the
// map -- the inspector and the map-quality strip. Positions come from the
// constants map/layoutProfile.ts computes the fit safe rectangle from, so
// the rectangle and the chrome cannot drift apart.

/** §5: the inspector, 380 px (360 on a tablet, §9), floating top-right over
 * the map with 20 px insets. Its content is the phone sheet's Half content
 * -- the same card components (phone/SheetCards.tsx's SelectionCard,
 * principle 10). It stops above the controls in the bottom-right corner
 * and scrolls inside itself. Esc closes it (MapView). */
export function Inspector({ profile, fullscreen, children }: { profile: LayoutProfile; fullscreen: boolean; children: ReactNode }) {
  const top = fullscreen ? `calc(${INSPECTOR_INSET_PX}px + env(safe-area-inset-top, 0px))` : `${INSPECTOR_INSET_PX}px`;
  const controls = 4 * desktopControlSize(profile) + 2;
  return (
    <section
      aria-label="Inspector"
      data-inspector
      data-selection-panel
      className="absolute z-30 flex flex-col overflow-hidden rounded-[16px] border border-[var(--rule)] bg-[var(--chrome)] text-[var(--on)] shadow-[0_12px_32px_rgba(0,0,0,.28)]"
      style={{
        top,
        right: `calc(${INSPECTOR_INSET_PX}px + env(safe-area-inset-right, 0px))`,
        width: inspectorWidth(profile),
        maxHeight: `calc(100% - ${top} - ${DESKTOP_GUTTER_PX + controls + DESKTOP_GAP_PX}px - env(safe-area-inset-bottom, 0px))`,
      }}
    >
      <div data-sheet-body className="min-h-0 overflow-y-auto px-5 pb-5 pt-4" style={{ overscrollBehavior: "contain" }}>
        {children}
      </div>
    </section>
  );
}

/** §5's map-quality strip, bottom-left of the map: files, districts,
 * modularity, the reference kind and the unconnected files -- the same
 * content as the phone's map-quality row (§4.2), and like it, one button
 * that opens the Map quality card (in the inspector here). Numbers are
 * mono (§8.1). */
export function QualityStrip({ doc, unconnected, onOpen, touch }: { doc: MapDocument; unconnected: number; onOpen(): void; touch: boolean }) {
  // Issue #34: "districts" is the mainland count; islands are not districts
  // anyone navigates by (see the phone overview's own title).
  let mainland = 0;
  for (const d of Object.values(doc.districts)) if (districtClass(d) === "mainland") mainland++;
  const summary = summarizeReferenceCoverage(doc.coverage);
  const num = (n: number | string) => <span className="font-mono text-[var(--on)]">{typeof n === "number" ? n.toLocaleString("en-US") : n}</span>;
  return (
    <button
      type="button"
      data-map-quality
      data-quality-strip
      onClick={onOpen}
      className={`flex max-w-full flex-wrap items-center gap-x-2.5 gap-y-0.5 rounded-[12px] border border-[var(--rule)] bg-[rgba(var(--chrome-float-rgb),0.94)] px-3.5 py-1 text-left text-meta text-[var(--dim)] shadow-[0_6px_18px_rgba(0,0,0,.18)] hover:text-[var(--on)] ${touch ? "min-h-[44px]" : "min-h-[40px]"}`}
    >
      <span>
        {num(doc.F.length)} files · {num(mainland)} districts · modularity {num(doc.q)}
      </span>
      <span aria-hidden="true" className="h-[18px] w-px bg-[var(--rule)]" />
      <span className="flex items-center gap-1.5">
        <StatusGlyph status={summary?.status ?? "heuristic"} />
        {summary ? summary.label : "References"}
        {unconnected > 0 && (
          <>
            {" · "}
            <span data-unconnected-count>{unconnected.toLocaleString("en-US")} unconnected files</span>
          </>
        )}
      </span>
    </button>
  );
}

/** The churn and complexity layers' colour ramp with its range -- on the
 * phone it is in the Layers sheet (§4.6); the desktop's layer control is
 * the top bar's segmented group, so the ramp sits over the map with the
 * other bottom-left legends. */
export function RampLegend({ layer, maxCh, maxCx }: { layer: Layer; maxCh: number; maxCx: number }) {
  if (layer !== "c" && layer !== "x") return null;
  return (
    <div
      data-ramp-legend={layer}
      className="flex w-fit items-center gap-2 rounded-[12px] border border-[var(--rule)] bg-[rgba(var(--chrome-float-rgb),0.94)] px-3.5 py-2 text-meta text-[var(--dim)] shadow-[0_6px_18px_rgba(0,0,0,.18)]"
    >
      {layer === "c" ? "Commits touching each file" : "Branch points in each file"}
      <span className="h-2 w-24 shrink-0 rounded" style={{ background: `linear-gradient(90deg,${RAMP_STOPS.join(",")})` }} />
      <span className="font-mono text-[var(--on)]">{layer === "c" ? `1 → ${maxCh}` : `0 → ${maxCx}`}</span>
    </div>
  );
}

/** The bottom-left stack: the layer's legend over the map-quality strip. */
export function BottomLeftStack({ atScreenEdge, children }: { atScreenEdge: boolean; children: ReactNode }) {
  return (
    <div
      data-bottom-left
      className="pointer-events-none absolute z-20 flex flex-col items-start gap-2 [&>*]:pointer-events-auto"
      style={{
        left: desktopLeftEdge(atScreenEdge),
        bottom: `calc(${DESKTOP_GUTTER_PX}px + env(safe-area-inset-bottom, 0px))`,
        // Clear of the controls column on the right.
        maxWidth: `calc(100% - ${2 * DESKTOP_GUTTER_PX + 44 + DESKTOP_GAP_PX}px)`,
      }}
    >
      {children}
    </div>
  );
}
