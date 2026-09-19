import type { MapDocument } from "@/types";
import { districtColor } from "@/map/geometry";
import { useIsNarrow } from "@/hooks/useIsNarrow";

const WHY_COLOR: Record<string, string> = {
  entry: "#6FB39F",
  bridge: "#D79A4A",
  hub: "#79A7D4",
  capital: "#9FA8B0",
  hazard: "#E0705A",
};

interface Props {
  doc: MapDocument;
  open: boolean;
  onToggleOpen(): void;
  onPickLandmark(fileIndex: number): void;
  onFlyDistrict(d: number): void;
}

/** Landmarks and districts list. Desktop: a fixed left rail. Phone: a
 * bottom drawer that peeks a grab handle and opens on tap or drag — the
 * `.side`/`.grab`/`.open` pattern from the reference's CSS, reimplemented
 * as a translateY transition driven by the `open` prop instead of a class
 * toggled directly on the DOM node. */
export function Sidebar({ doc, open, onToggleOpen, onPickLandmark, onFlyDistrict }: Props) {
  const narrow = useIsNarrow();
  const districtIds = Object.keys(doc.districts).sort((a, b) => doc.districts[b].size - doc.districts[a].size);

  const body = (
    <>
      <h2 className="mb-1.5 mt-3 px-3 font-sans text-[9.5px] font-semibold uppercase tracking-[0.14em] text-[var(--dim)]">
        Landmarks
      </h2>
      <div>
        {doc.L.map(([i, why, detail, rank]) => (
          <div
            key={i}
            onClick={() => onPickLandmark(i)}
            className="grid cursor-pointer grid-cols-[18px_1fr] items-baseline gap-1.5 border-l-2 border-transparent px-3 py-1.5 hover:border-[var(--hot)] hover:bg-[var(--chrome2)]"
          >
            <span className="text-[10px] text-[var(--dim)]">{rank}</span>
            <span>
              <span className="block break-all text-[10.5px] leading-snug">{doc.F[i].split("/").slice(1).join("/") || doc.F[i]}</span>
              <span className="block text-[9px] uppercase tracking-wide" style={{ color: WHY_COLOR[why] }}>
                {why} · {detail}
              </span>
            </span>
          </div>
        ))}
        {doc.L.length === 0 && <p className="px-3 py-2 text-[10.5px] text-[var(--dim)]">no landmarks surfaced</p>}
      </div>
      <h2 className="mb-1.5 mt-3 px-3 font-sans text-[9.5px] font-semibold uppercase tracking-[0.14em] text-[var(--dim)]">
        Districts
      </h2>
      <div>
        {districtIds.map((d) => (
          <div
            key={d}
            onClick={() => onFlyDistrict(+d)}
            className="flex cursor-pointer items-center gap-1.5 px-3 py-1 text-[10.5px] hover:bg-[var(--chrome2)]"
          >
            <i className="block h-2.5 w-2.5 flex-none rounded-sm" style={{ background: districtColor(+d) }} />
            {doc.names[d]}
            <span className="ml-auto text-[9.5px] text-[var(--dim)]">{doc.districts[d].size}</span>
          </div>
        ))}
      </div>
    </>
  );

  if (!narrow) {
    return (
      <aside className="w-[250px] flex-none overflow-y-auto border-r border-[var(--rule)] bg-[var(--chrome)] pb-4 text-[var(--on)]">
        {body}
      </aside>
    );
  }

  return (
    <aside
      className="absolute inset-x-0 bottom-0 z-10 max-h-[68%] overflow-y-auto rounded-t-xl border-t border-[var(--rule)] bg-[var(--chrome)] text-[var(--on)] shadow-[0_-8px_26px_rgba(0,0,0,.34)] transition-transform duration-[260ms] ease-out"
      style={{
        transform: open ? "translateY(0)" : "translateY(calc(100% - 46px))",
        paddingBottom: "calc(16px + env(safe-area-inset-bottom, 0px))",
      }}
    >
      <button
        onClick={onToggleOpen}
        className="sticky top-0 block h-[46px] w-full bg-[var(--chrome)] text-center text-[11px] uppercase tracking-[0.1em] text-[var(--dim)]"
      >
        landmarks &amp; districts
      </button>
      {body}
    </aside>
  );
}
