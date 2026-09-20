import { useState } from "react";
import type { MapDocument } from "@/types";
import { districtClass, districtColor } from "@/map/geometry";
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

/** One district row, shared by every section below. */
function DistrictRow({ doc, d, onFlyDistrict }: { doc: MapDocument; d: string; onFlyDistrict(d: number): void }) {
  return (
    <div
      onClick={() => onFlyDistrict(+d)}
      className="flex cursor-pointer items-center gap-1.5 px-3 py-1 text-[10.5px] hover:bg-[var(--chrome2)]"
    >
      <i className="block h-2.5 w-2.5 flex-none rounded-sm" style={{ background: districtColor(+d) }} />
      {doc.names[d]}
      <span className="ml-auto text-[9.5px] text-[var(--dim)]">{doc.districts[d].size}</span>
    </div>
  );
}

/** Islands/unconnected section: a single-line, tappable header ("269
 * islands") that expands into the full list on click. Collapsed by
 * default — n8n alone puts 269 districts through this path, and a rail
 * that dumps all of them flat would be worse than the 365-district list
 * this feature exists to replace. A real `<button>`, not a div with an
 * onClick like the district/landmark rows below it: this element's whole
 * job is the toggle itself (no secondary click target competing for the
 * same tap the way a row's "select" and "fly to" affordances share one),
 * so it gets the element that is a toggle by default — focusable, and
 * reachable by touch or keyboard without any of it being hand-rolled. */
function CollapsibleSection({
  label,
  ids,
  doc,
  onFlyDistrict,
}: {
  label: string;
  ids: string[];
  doc: MapDocument;
  onFlyDistrict(d: number): void;
}) {
  const [open, setOpen] = useState(false);
  if (ids.length === 0) return null; // a section with zero members renders nothing, not an empty header
  return (
    <>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="mb-1.5 mt-3 flex w-full items-center gap-1.5 px-3 font-sans text-[9.5px] font-semibold uppercase tracking-[0.14em] text-[var(--dim)]"
      >
        <span className={`inline-block text-[10px] transition-transform ${open ? "rotate-90" : ""}`}>›</span>
        {ids.length} {label}
      </button>
      {open && (
        <div>
          {ids.map((d) => (
            <DistrictRow key={d} doc={doc} d={d} onFlyDistrict={onFlyDistrict} />
          ))}
        </div>
      )}
    </>
  );
}

/** Landmarks and districts list. Desktop: a fixed left rail. Phone: a
 * bottom drawer that peeks a grab handle and opens on tap or drag — the
 * `.side`/`.grab`/`.open` pattern from the reference's CSS, reimplemented
 * as a translateY transition driven by the `open` prop instead of a class
 * toggled directly on the DOM node.
 *
 * Districts split into up to three sections by class (issue #34): mainland
 * districts under "Districts" as before, and islands/unconnected each
 * collapsed behind their own count — see `CollapsibleSection`. A pre-#34
 * document (no `class` anywhere) has every district read as mainland
 * (`districtClass`'s default), so `islandIds`/`unconnectedIds` are both
 * empty and this collapses to exactly the flat list that shipped before. */
export function Sidebar({ doc, open, onToggleOpen, onPickLandmark, onFlyDistrict }: Props) {
  const narrow = useIsNarrow();
  const byClass = (cls: "mainland" | "island" | "unconnected") =>
    Object.keys(doc.districts)
      .filter((d) => districtClass(doc.districts[d]) === cls)
      .sort((a, b) => doc.districts[b].size - doc.districts[a].size);
  const mainlandIds = byClass("mainland");
  const islandIds = byClass("island");
  // Named "Unfiled", not "Districts" or "Towns": these are files the map
  // couldn't place anywhere meaningful (no import edge to the rest of the
  // repo), and the name has to say that rather than read as a place — spec
  // requirement, and the same honesty SelectionPanel's note line applies.
  const unconnectedIds = byClass("unconnected");

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
        {mainlandIds.map((d) => (
          <DistrictRow key={d} doc={doc} d={d} onFlyDistrict={onFlyDistrict} />
        ))}
      </div>
      <CollapsibleSection label="islands" ids={islandIds} doc={doc} onFlyDistrict={onFlyDistrict} />
      <CollapsibleSection label="unfiled" ids={unconnectedIds} doc={doc} onFlyDistrict={onFlyDistrict} />
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
