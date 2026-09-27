import { useMemo, useState } from "react";
import type { MapDocument } from "@/types";
import { buildDistrictIndex, type DistrictIndexRow } from "@/map/districtIndex";
import type { PackageLayout } from "@/map/packageLayout";

interface Props {
  doc: MapDocument;
  packageLayout: PackageLayout;
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
function PlainDistrictRow({ doc, d, onSelectDistrict, large = false }: { doc: MapDocument; d: string; onSelectDistrict(d: number): void; large?: boolean }) {
  if (large) {
    return (
      <button type="button" onClick={() => onSelectDistrict(+d)} className="flex min-h-[44px] w-full items-center gap-1.5 px-5 text-left text-small">
        <span className="min-w-0 flex-1 truncate">{doc.names[d]}</span>
        <span className="font-mono text-meta text-[var(--dim)]">{doc.districts[d].size}</span>
      </button>
    );
  }
  return (
    <div
      onClick={() => onSelectDistrict(+d)}
      className="flex cursor-pointer items-baseline gap-1.5 px-3 py-1 text-small hover:bg-[var(--chrome2)]"
    >
      <span className="min-w-0 flex-1 truncate">{doc.names[d]}</span>
      <span className="font-mono text-meta text-[var(--dim)]">{doc.districts[d].size}</span>
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
        className="flex cursor-pointer items-baseline gap-1.5 px-3 hover:text-[var(--accent)]"
      >
        <span className="min-w-0 flex-1 truncate text-small font-semibold">{row.name}</span>
        <span className="flex-none font-mono text-meta text-[var(--dim)]">{row.size}</span>
      </div>
      {row.mostly && (
        <p className="truncate px-3 text-meta text-[var(--dim)]" title={`mostly ${row.mostly}`}>
          mostly <span className="font-mono text-[var(--on)]">{row.mostly}</span>
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
          className="block w-full truncate px-3 py-0.5 text-left text-meta text-[var(--dim)] hover:bg-[var(--chrome2)] hover:text-[var(--on)]"
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
  large = false,
}: {
  label: string;
  ids: readonly string[];
  doc: MapDocument;
  onSelectDistrict(d: number): void;
  /** Phone sheet: 44 px rows (docs/UX.md §8.2). */
  large?: boolean;
}) {
  const [open, setOpen] = useState(false);
  if (ids.length === 0) return null; // a section with zero members renders nothing, not an empty header
  return (
    <>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className={`flex w-full items-center gap-1.5 text-label uppercase text-[var(--dim)] ${large ? "min-h-[44px] px-5" : "mb-1.5 mt-3 px-3"}`}
      >
        <span className={`inline-block text-meta transition-transform ${open ? "rotate-90" : ""}`}>›</span>
        {ids.length} {label}
      </button>
      {open && (
        <div>
          {ids.map((d) => (
            <PlainDistrictRow key={d} doc={doc} d={d} onSelectDistrict={onSelectDistrict} large={large} />
          ))}
        </div>
      )}
    </>
  );
}

/** The district index's rows, shared by the desktop rail (`variant="rail"`)
 * and the phone sheet's Districts tab (`variant="sheet"`, docs/UX.md §4.3:
 * rows of at least 64 px, 16 px names, every key file a 44 px target).
 * Same data (map/districtIndex.ts) and the same data attributes in both, so
 * the viewer checks read one contract. The sheet keeps every key-file line
 * rather than §4.3's single "key file" mention: those lines are how every
 * landmark kind stays listed (CLAUDE.md's viewer acceptance bar). */
export function DistrictIndexList({
  doc,
  packageLayout,
  onPickKeyFile,
  onSelectDistrict,
  variant,
}: {
  doc: MapDocument;
  packageLayout: PackageLayout;
  onPickKeyFile(fileIndex: number): void;
  onSelectDistrict(d: number): void;
  variant: "rail" | "sheet";
}) {
  const index = useMemo(() => buildDistrictIndex(doc, packageLayout), [doc, packageLayout]);
  if (variant === "sheet") {
    return (
      <div data-district-index>
        {index.mainland.map((row) => (
          <SheetDistrictRow key={row.d} row={row} onPickKeyFile={onPickKeyFile} onSelectDistrict={onSelectDistrict} />
        ))}
        <CollapsibleSection label="islands" ids={index.islandIds} doc={doc} onSelectDistrict={onSelectDistrict} large />
      </div>
    );
  }
  return (
    <div data-district-index>
      <div>
        {index.mainland.map((row) => (
          <DistrictIndexRowView key={row.d} row={row} onPickKeyFile={onPickKeyFile} onSelectDistrict={onSelectDistrict} />
        ))}
      </div>
      <CollapsibleSection label="islands" ids={index.islandIds} doc={doc} onSelectDistrict={onSelectDistrict} />
    </div>
  );
}

/** docs/UX.md §4.3's phone row: the district is one real button (name,
 * count, "mostly" line), each key file another. */
function SheetDistrictRow({
  row,
  onPickKeyFile,
  onSelectDistrict,
}: {
  row: DistrictIndexRow;
  onPickKeyFile(fileIndex: number): void;
  onSelectDistrict(d: number): void;
}) {
  return (
    <div data-district-index-row={row.d} className="border-b border-[var(--rule)]">
      <button
        type="button"
        onClick={() => onSelectDistrict(row.d)}
        className="flex min-h-[64px] w-full items-center gap-3 px-5 py-2.5 text-left"
      >
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="truncate text-row">{row.name}</span>
          {row.mostly && (
            <span className="truncate text-meta text-[var(--dim)]">
              mostly <span className="font-mono">{row.mostly}</span>
            </span>
          )}
        </span>
        <span className="flex-none font-mono text-small text-[var(--dim)]">{row.size}</span>
      </button>
      {row.keyFiles.map((kf) => (
        <button
          key={kf.kind}
          type="button"
          data-district-index-key-file={kf.file}
          onClick={() => onPickKeyFile(kf.file)}
          className="-mt-1 flex min-h-[44px] w-full items-center truncate px-5 text-left text-meta text-[var(--dim)]"
        >
          {kf.text}
        </button>
      ))}
    </div>
  );
}

/** The ONE "Districts" list (issue #82 "district index", owner decision
 * AskUserQuestion 2026-09-24): replaces the old rail's three stacked parts
 * (a jargon-heavy Landmarks list, a separate Hubs list, and district rows
 * whose colour chips no longer identified anything once six shared hues
 * started repeating). The desktop rail. On a phone the same list is the
 * bottom sheet's Districts tab (docs/UX.md §4.3, DistrictIndexList above);
 * the old phone drawer is gone (§3).
 *
 * Districts split into mainland and islands. Unconnected files are listed
 * off the map (the footer list, the phone's map-quality row). */
export function Sidebar({ doc, packageLayout, onPickKeyFile, onSelectDistrict }: Props) {
  const totalFiles = useMemo(() => buildDistrictIndex(doc, packageLayout).totalFiles, [doc, packageLayout]);
  const title = `Districts · ${totalFiles.toLocaleString("en-US")} files`;
  return (
    <aside className="w-[250px] flex-none overflow-y-auto border-r border-[var(--rule)] bg-[var(--chrome)] pb-4 text-[var(--on)]">
      <h2 className="mb-1.5 mt-3 px-3 text-label uppercase text-[var(--dim)]">{title}</h2>
      <DistrictIndexList
        doc={doc}
        packageLayout={packageLayout}
        onPickKeyFile={onPickKeyFile}
        onSelectDistrict={onSelectDistrict}
        variant="rail"
      />
    </aside>
  );
}
