import { useMemo, useState } from "react";
import type { MapDocument } from "@/types";
import { buildDistrictIndex, type DistrictIndexRow } from "@/map/districtIndex";
import type { PackageLayout } from "@/map/packageLayout";
import { useIsNarrow } from "@/hooks/useIsNarrow";

interface Props {
  doc: MapDocument;
  packageLayout: PackageLayout;
  open: boolean;
  onToggleOpen(): void;
  /** A district's own key-file line (most imported / entry / links a bridge)
   * -- selects that file, no view move, the same pan-free contract
   * onPickHub/onPickLandmark had before this list replaced them (issue #82
   * "district index", see MapView.tsx's wiring). */
  onPickKeyFile(fileIndex: number): void;
  /** Issue #82 A1: renamed from onFlyDistrict now that a row SELECTS the
   * district (mainland and island alike) and pans to it only if it's off
   * screen, instead of always zooming in -- see MapView.tsx's wiring. */
  onSelectDistrict(d: number): void;
}

/** A plain district row: name (tap = select, no view move) and file count.
 * Used for islands, which keep today's bare-name-and-count treatment --
 * only mainland rows get the enriched "mostly"/key-file body (see
 * DistrictIndexRowView below). No colour chip on either: once every
 * district shares one of six hues, a chip repeats too often to identify
 * anything (owner feedback, issue #82 "district index"). */
function PlainDistrictRow({ doc, d, onSelectDistrict }: { doc: MapDocument; d: string; onSelectDistrict(d: number): void }) {
  return (
    <div
      onClick={() => onSelectDistrict(+d)}
      className="flex cursor-pointer items-baseline gap-1.5 px-3 py-1 text-[10.5px] hover:bg-[var(--chrome2)]"
    >
      <span className="min-w-0 flex-1 truncate">{doc.names[d]}</span>
      <span className="text-[9.5px] text-[var(--dim)]">{doc.districts[d].size}</span>
    </div>
  );
}

/** A mainland district's full row: header (name, tap = select; file count),
 * "mostly <folder>" when one folder dominates, and up to a few tappable key
 * files in plain words -- the single replacement for the old rail's three
 * stacked parts (Landmarks, Hubs, Districts). Every landmark kind the old
 * rail showed (bar capital, dropped from the map entirely) is reachable
 * from some row's key files -- see map/districtIndex.ts's own doc comment
 * for exactly how "most imported"/"entry"/"links" cover hub/entry/bridge,
 * and where a hazard file gets a look-in. */
function DistrictIndexRowView({
  row,
  onPickKeyFile,
  onSelectDistrict,
}: {
  row: DistrictIndexRow;
  onPickKeyFile(fileIndex: number): void;
  onSelectDistrict(d: number): void;
}) {
  return (
    <div data-district-index-row={row.d} className="border-t border-[var(--rule)] py-1.5 first:border-t-0">
      <div
        onClick={() => onSelectDistrict(row.d)}
        className="flex cursor-pointer items-baseline gap-1.5 px-3 hover:text-[var(--hot)]"
      >
        <span className="min-w-0 flex-1 truncate text-[10.5px]">{row.name}</span>
        <span className="flex-none text-[9.5px] text-[var(--dim)]">{row.size}</span>
      </div>
      {row.mostly && (
        <p className="truncate px-3 text-[9px] text-[var(--dim)]" title={`mostly ${row.mostly}`}>
          mostly <span className="text-[var(--on)]">{row.mostly}</span>
        </p>
      )}
      {row.keyFiles.map((kf) => (
        <button
          key={kf.kind}
          type="button"
          data-district-index-key-file={kf.file}
          onClick={(event) => {
            event.stopPropagation();
            onPickKeyFile(kf.file);
          }}
          className="block w-full truncate px-3 py-0.5 text-left text-[9.5px] text-[var(--dim)] hover:bg-[var(--chrome2)] hover:text-[var(--on)]"
        >
          {kf.text}
        </button>
      ))}
    </div>
  );
}

/** Island section: a single-line, tappable header that expands into the
 * full list on click. A real `<button>`, not a div with an
 * onClick like the district rows above it, so it gets the element
 * that is a toggle by default — focusable, and reachable by touch or
 * keyboard without any of it being hand-rolled. */
function CollapsibleSection({
  label,
  ids,
  doc,
  onSelectDistrict,
}: {
  label: string;
  ids: readonly string[];
  doc: MapDocument;
  onSelectDistrict(d: number): void;
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
            <PlainDistrictRow key={d} doc={doc} d={d} onSelectDistrict={onSelectDistrict} />
          ))}
        </div>
      )}
    </>
  );
}

/** The ONE "Districts" list (issue #82 "district index", owner decision
 * AskUserQuestion 2026-09-24): replaces the old rail's three stacked parts
 * (a jargon-heavy Landmarks list, a separate Hubs list, and district rows
 * whose colour chips no longer identified anything once six shared hues
 * started repeating). Desktop: a fixed left rail. Phone: a bottom drawer
 * that peeks a grab handle and opens on tap or drag — the `.side`/`.grab`/
 * `.open` pattern from the reference's CSS, reimplemented as a translateY
 * transition driven by the `open` prop instead of a class toggled directly
 * on the DOM node.
 *
 * Districts split into mainland and islands. Unconnected files still live
 * in the footer list, with no district row to select from here. */
export function Sidebar({ doc, packageLayout, open, onToggleOpen, onPickKeyFile, onSelectDistrict }: Props) {
  const narrow = useIsNarrow();
  const index = useMemo(() => buildDistrictIndex(doc, packageLayout), [doc, packageLayout]);

  const body = (
    <>
      <h2 className="mb-1.5 mt-3 px-3 font-sans text-[9.5px] font-semibold uppercase tracking-[0.14em] text-[var(--dim)]">
        Districts · {index.totalFiles.toLocaleString("en-US")} files
      </h2>
      <div>
        {index.mainland.map((row) => (
          <DistrictIndexRowView key={row.d} row={row} onPickKeyFile={onPickKeyFile} onSelectDistrict={onSelectDistrict} />
        ))}
      </div>
      <CollapsibleSection label="islands" ids={index.islandIds} doc={doc} onSelectDistrict={onSelectDistrict} />
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
        districts
      </button>
      {body}
    </aside>
  );
}
