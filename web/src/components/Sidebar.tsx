import { useMemo, useState } from "react";
import type { MapDocument } from "@/types";
import { buildDistrictIndex, type DistrictIndexRow } from "@/map/districtIndex";
import type { PackageLayout } from "@/map/packageLayout";
import { FolderBody } from "@/components/SelectionPanel";

/** A plain district row: name (tap = select, no view move) and file count.
 * Used for islands, which keep today's bare-name-and-count treatment --
 * only mainland rows get the enriched "mostly"/key-file body (see
 * RailDistrictRow below). No colour chip on either: once every
 * district shares one of six hues, a chip repeats too often to identify
 * anything (owner feedback, issue #82 "district index"). */
function PlainDistrictRow({ doc, d, onSelectDistrict, large = false, current = false }: { doc: MapDocument; d: string; onSelectDistrict(d: number): void; large?: boolean; current?: boolean }) {
  if (large) {
    return (
      <button type="button" onClick={() => onSelectDistrict(+d)} className="flex min-h-[44px] w-full items-center gap-1.5 px-5 text-left text-small">
        <span className="min-w-0 flex-1 truncate">{doc.names[d]}</span>
        <span className="font-mono text-meta text-[var(--dim)]">{doc.districts[d].size}</span>
      </button>
    );
  }
  return (
    <button
      type="button"
      aria-current={current ? "true" : undefined}
      onClick={() => onSelectDistrict(+d)}
      className={`flex min-h-[32px] w-full items-center gap-1.5 px-4 text-left text-small hover:bg-subtle touch:min-h-[44px] ${current ? "shadow-[inset_3px_0_0_var(--accent)]" : ""}`}
    >
      <span className="min-w-0 flex-1 truncate">{doc.names[d]}</span>
      <span className="font-mono text-meta text-[var(--dim)]">{doc.districts[d].size}</span>
    </button>
  );
}

/** A mainland district's full row in the desktop/tablet rail (docs/UX.md
 * §5; the "Desktop map" artboard): the district is one real button (name,
 * "mostly <folder>", file count), and each key file another -- the single
 * replacement for the old rail's three stacked parts (Landmarks, Hubs,
 * Districts). Every landmark kind the old rail showed (bar capital, dropped
 * from the map entirely) is reachable from some row's key files -- see
 * map/districtIndex.ts's own doc comment for how "most imported"/"entry"/
 * "links" cover hub/entry/bridge, and where a hazard file gets a look-in.
 * The selected district (or the selected file's) carries §5's accent bar
 * and `aria-current`. */
function RailDistrictRow({
  row,
  current,
  onPickKeyFile,
  onSelectDistrict,
}: {
  row: DistrictIndexRow;
  current: boolean;
  onPickKeyFile(fileIndex: number): void;
  onSelectDistrict(d: number): void;
}) {
  return (
    <div
      data-district-index-row={row.d}
      data-current={current || undefined}
      // The accent bar and wash as classes, not an inline background: the
      // index has no colour chips, and check:view asserts none by looking
      // for inline backgrounds (issue #82 "district index").
      className={`border-b border-[var(--rule)] ${current ? "bg-[color:color-mix(in_srgb,var(--accent)_10%,transparent)] shadow-[inset_3px_0_0_var(--accent)]" : ""}`}
    >
      <button
        type="button"
        aria-current={current ? "true" : undefined}
        onClick={() => onSelectDistrict(row.d)}
        className="flex min-h-[56px] w-full items-center gap-2.5 px-4 py-2 text-left hover:bg-subtle"
      >
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="truncate text-[15px] font-semibold leading-5">{row.name}</span>
          {row.mostly && (
            <span className="truncate text-meta text-[var(--dim)]">
              mostly <span className="font-mono">{row.mostly}</span>
            </span>
          )}
        </span>
        <span className="flex-none font-mono text-meta text-[var(--dim)]">{row.size}</span>
      </button>
      {row.keyFiles.map((kf) => (
        <button
          key={kf.kind}
          type="button"
          data-district-index-key-file={kf.file}
          onClick={() => onPickKeyFile(kf.file)}
          className="-mt-1.5 flex min-h-[28px] w-full items-center truncate px-4 pb-1 text-left text-meta text-[var(--dim)] hover:text-[var(--on)] touch:mt-0 touch:min-h-[44px] touch:pb-0"
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
  selected = null,
}: {
  label: string;
  ids: readonly string[];
  doc: MapDocument;
  onSelectDistrict(d: number): void;
  /** Phone sheet: 44 px rows (docs/UX.md §8.2). */
  large?: boolean;
  selected?: number | null;
}) {
  const [open, setOpen] = useState(selected != null && ids.includes(String(selected)));
  if (ids.length === 0) return null; // a section with zero members renders nothing, not an empty header
  return (
    <>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className={`flex w-full items-center gap-1.5 text-label uppercase text-[var(--dim)] ${large ? "min-h-[44px] px-5" : "min-h-[40px] px-4 touch:min-h-[44px]"}`}
      >
        <span className={`inline-block text-meta transition-transform ${open ? "rotate-90" : ""}`}>›</span>
        {ids.length} {label}
      </button>
      {open && (
        <div>
          {ids.map((d) => (
            <PlainDistrictRow key={d} doc={doc} d={d} onSelectDistrict={onSelectDistrict} large={large} current={selected === +d} />
          ))}
        </div>
      )}
    </>
  );
}

/** The district index's rows, shared by the desktop/tablet rail
 * (`variant="rail"`) and the phone sheet's Districts tab (`variant="sheet"`,
 * docs/UX.md §4.3: rows of at least 64 px, 16 px names, every key file a
 * 44 px target). Same data (map/districtIndex.ts) and the same data
 * attributes in both, so the viewer checks read one contract. Both keep
 * every key-file line rather than §4.3's single "key file" mention: those
 * lines are how every landmark kind stays listed (CLAUDE.md's viewer
 * acceptance bar). */
export function DistrictIndexList({
  doc,
  packageLayout,
  onPickKeyFile,
  onSelectDistrict,
  variant,
  selected = null,
}: {
  doc: MapDocument;
  packageLayout: PackageLayout;
  onPickKeyFile(fileIndex: number): void;
  onSelectDistrict(d: number): void;
  variant: "rail" | "sheet";
  /** The rail marks this district as current (§5). */
  selected?: number | null;
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
      {index.mainland.map((row) => (
        <RailDistrictRow key={row.d} row={row} current={selected === row.d} onPickKeyFile={onPickKeyFile} onSelectDistrict={onSelectDistrict} />
      ))}
      <CollapsibleSection label="islands" ids={index.islandIds} doc={doc} onSelectDistrict={onSelectDistrict} selected={selected} />
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

/** docs/UX.md §5: the 320 px left rail -- navigation. The District index
 * with a Districts / Folders tab (the Folders tree, with percentages, owner
 * 2026-09-23, is the second tab rather than a separate panel, as on the
 * phone, §4.3). The ONE "Districts" list (issue #82 "district index", owner
 * decision AskUserQuestion 2026-09-24) replaced the old rail's three stacked
 * parts; islands collapse under it; unconnected files are listed off the map
 * (the map-quality strip). On a tablet the rail is collapsible from the top
 * bar (§9). The left and bottom safe-area insets are padded inside it. */
export function DistrictRail({
  doc,
  packageLayout,
  tab,
  onTab,
  selected,
  activeDirectory,
  onPickKeyFile,
  onSelectDistrict,
  onSelectDirectory,
}: {
  doc: MapDocument;
  packageLayout: PackageLayout;
  tab: "districts" | "folders";
  onTab(t: "districts" | "folders"): void;
  selected: number | null;
  activeDirectory?: string;
  /** A district row's key-file line -- selects that file, no view move
   * (issue #82 "district index"). */
  onPickKeyFile(fileIndex: number): void;
  /** A district row: selects it, and pans it into view only if it is off
   * screen (issue #82 A1; MapView's wiring). */
  onSelectDistrict(d: number): void;
  onSelectDirectory(path?: string): void;
}) {
  const index = useMemo(() => buildDistrictIndex(doc, packageLayout), [doc, packageLayout]);
  return (
    <aside
      id="district-rail"
      aria-label="Districts"
      data-rail
      className="flex w-[320px] flex-none flex-col border-r border-[var(--rule)] bg-[var(--chrome)] text-[var(--on)]"
      style={{ width: "calc(320px + env(safe-area-inset-left, 0px))", paddingLeft: "env(safe-area-inset-left, 0px)", paddingBottom: "env(safe-area-inset-bottom, 0px)" }}
    >
      <div className="flex items-center justify-between gap-2 px-4 pb-1 pt-3.5">
        <h2 className="text-[17px] font-semibold">
          {tab === "districts" ? "Districts" : "Folders"}{" "}
          <span className="font-medium text-[var(--dim)]">{tab === "districts" ? index.mainland.length : packageLayout.directories.length}</span>
        </h2>
        <div role="tablist" aria-label="Browse by" className="flex rounded-[9px] border border-[var(--rule)] bg-[var(--canvas)] p-0.5">
          {(["districts", "folders"] as const).map((t) => (
            <button
              key={t}
              type="button"
              role="tab"
              aria-selected={tab === t}
              data-index-tab={t}
              onClick={() => onTab(t)}
              className={`h-7 rounded-[7px] px-2.5 text-meta touch:h-11 ${tab === t ? "bg-[var(--rule)] font-semibold text-[var(--on)]" : "text-[var(--dim)]"}`}
            >
              {t === "districts" ? "Districts" : "Folders"}
            </button>
          ))}
        </div>
      </div>
      <p className="px-4 pb-2.5 text-meta text-[var(--dim)]" data-rail-files>
        <span className="font-mono">{index.totalFiles.toLocaleString("en-US")}</span> files
      </p>
      <div className="min-h-0 flex-1 overflow-y-auto border-t border-[var(--rule)]" style={{ overscrollBehavior: "contain" }}>
        {tab === "districts" ? (
          <DistrictIndexList
            doc={doc}
            packageLayout={packageLayout}
            onPickKeyFile={onPickKeyFile}
            onSelectDistrict={onSelectDistrict}
            variant="rail"
            selected={selected}
          />
        ) : (
          <div className="px-4 pb-4">
            <FolderBody layout={packageLayout} activeDirectory={activeDirectory} onSelectDirectory={onSelectDirectory} nested={false} />
          </div>
        )}
      </div>
    </aside>
  );
}
