import { useMemo, useState } from "react";
import type { MapDocument } from "@/types";
import { buildDistrictIndex, type DistrictIndexRow } from "@/map/districtIndex";
import type { PackageLayout } from "@/map/packageLayout";

/** A plain district row: name (tap = select, no view move) and file count.
 * Used for islands, which keep today's bare-name-and-count treatment --
 * only mainland rows get the enriched "mostly"/key-file body (see
 * SheetDistrictRow below). The desktop panel owns its own 50 px rows. The
 * phone has no colour chip: once every district shares one of six hues, a
 * chip repeats too often to identify
 * anything (owner feedback, issue #82 "district index"). */
function PlainDistrictRow({ doc, d, onSelectDistrict }: { doc: MapDocument; d: string; onSelectDistrict(d: number): void }) {
  return (
    <button
      type="button"
      onClick={() => onSelectDistrict(+d)}
      className="flex min-h-[44px] w-full items-center gap-1.5 px-5 text-left text-small"
    >
      <span className="min-w-0 flex-1 truncate">{doc.names[d]}</span>
      <span className="font-mono text-meta text-[var(--dim)]">{doc.districts[d].size}</span>
    </button>
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
        aria-expanded={open}
        className="flex min-h-[44px] w-full items-center gap-1.5 px-5 text-label uppercase text-[var(--dim)]"
      >
        <span className={`inline-block text-meta transition-transform ${open ? "rotate-90" : ""}`}>›</span>
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

/** The phone sheet's Districts tab (§4.3: rows of at least 64 px, 16 px
 * names, every key file a 44 px target). */
export function DistrictIndexList({
  doc,
  packageLayout,
  onPickKeyFile,
  onSelectDistrict,
}: {
  doc: MapDocument;
  packageLayout: PackageLayout;
  onPickKeyFile(fileIndex: number): void;
  onSelectDistrict(d: number): void;
}) {
  const index = useMemo(() => buildDistrictIndex(doc, packageLayout), [doc, packageLayout]);
  return (
    <div data-district-index>
      {index.mainland.map((row) => (
        <SheetDistrictRow key={row.d} row={row} onPickKeyFile={onPickKeyFile} onSelectDistrict={onSelectDistrict} />
      ))}
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
          {/* A span, not the button's own text: text-overflow does not
              reach a flex container's text, so a long "links A ↔ B" line
              overran the row instead of ending in an ellipsis. */}
          <span className="min-w-0 truncate">{kf.text}</span>
        </button>
      ))}
    </div>
  );
}
