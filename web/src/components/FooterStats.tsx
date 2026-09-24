import type { MapDocument } from "@/types";
import type { Layer } from "@/map/constants";
import { districtClass } from "@/map/geometry";

interface Props {
  doc: MapDocument;
  layer: Layer;
  maxCh: number;
  maxCx: number;
  unconnectedCount: number;
  onOpenUnconnected(): void;
  mobileHidden: boolean;
}

/** Desktop keeps the compact stats footer. Phones show only the
 * unconnected-file chip above the collapsed selection panel. */
export function FooterStats({ doc, layer, maxCh, maxCx, unconnectedCount, onOpenUnconnected, mobileHidden }: Props) {
  // `Object.keys(doc.districts).length` alone is the number issue #34
  // exists to fix — it reports 365 for n8n, which is not a map. Report the
  // mainland count as "districts" and name islands separately. The
  // island clause is omitted when its count is zero, as on pre-#34 fixtures (a missing `class`
  // defaults to mainland — `districtClass`) and collapses this back to the
  // plain "N districts" it says today.
  let mainlandCount = 0;
  let islandCount = 0;
  for (const d of Object.values(doc.districts)) {
    const cls = districtClass(d);
    if (cls === "mainland") mainlandCount++;
    else if (cls === "island") islandCount++;
  }
  return (
    <div className={`pointer-events-none absolute bottom-2.5 left-2.5 max-w-[min(440px,calc(100%-22px))] rounded-md border border-[var(--rule)] bg-[rgba(var(--chrome-float-rgb),0.94)] px-2.5 py-2 text-[9.5px] leading-relaxed text-[var(--dim)] max-[820px]:bottom-[calc(112px+env(safe-area-inset-bottom,0px))] max-[820px]:z-20 max-[820px]:border-0 max-[820px]:bg-transparent max-[820px]:p-0 ${layer === "p" ? "min-[821px]:left-[225px]" : ""}`}>
      {layer !== "p" && <span className="max-[820px]:hidden">
      {layer === "d" ? (
        <>
          <b className="font-medium text-[var(--on)]">{doc.F.length} files</b> ·{" "}
          <b className="font-medium text-[var(--on)]">{mainlandCount} districts</b>
          {islandCount > 0 ? ` · ${islandCount} islands` : ""}
          {" · modularity "}
          <b className="font-medium text-[var(--on)]">{doc.q}</b> · {doc.E.length} import edges. Scroll to zoom, drag to
          pan, search a file <i className="not-italic">or a symbol</i> to jump to it without changing the zoom.{" "}
          {doc.P
            ? "Footprint area is proportional to code lines and comparable only within one district."
            : "A file's classes and functions are listed in its card."}
        </>
      ) : (
        <>
          {layer === "c" ? "Commits touching each file" : "Branch points per file"}
          <span
            className="mx-1.5 inline-block h-[7px] w-24 rounded-sm align-middle"
            style={{ background: "linear-gradient(90deg,#3E6E88,#B8B06A,#C0472F)" }}
          />
          <b className="font-medium text-[var(--on)]">{layer === "c" ? `1 → ${maxCh}` : `0 → ${maxCx}`}</b>
        </>
      )}
      </span>}
      {/* B4 perf follow-up: the footprint-area sentence added to the "d"
          layer's text made this paragraph long enough that its wrapped last
          line and this chip -- both inline-level, sharing the same line box
          via nothing but this chip's own `mt-1` -- started overlapping
          (desktop-langgenius__dify-zoom0.png). `mt-1` alone only adds space
          ABOVE an element already on its own line; it doesn't put it there.
          `block w-fit` does: always its own line under the paragraph, sized
          to its own content rather than the full row. */}
      {unconnectedCount > 0 && <button type="button" data-unconnected-chip
        className={`pointer-events-auto mt-1 block w-fit rounded border border-[var(--rule)] bg-[var(--chrome)] px-2 py-1 text-[10px] text-[var(--on)] hover:text-[var(--hot)] max-[820px]:mt-0 ${mobileHidden ? "max-[820px]:hidden" : ""}`}
        onClick={onOpenUnconnected}>{unconnectedCount} unconnected files</button>}
    </div>
  );
}
