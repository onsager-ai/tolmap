import { useMemo, useState, type ReactNode } from "react";
import type { DistrictSymbols, MapDocument } from "@/types";
import { CH, CODE_LINES, D_, LOC, districtClass, symbolsOf } from "@/map/geometry";
import { KIND } from "@/map/constants";
import { computeBlast, type AdjMap, type Route } from "@/map/graph";
import {
  countOutlineSymbols,
  decodeDistrictSymbols,
  externalReferences,
  fileOutline,
  KIND_NAMES,
  rowEnd,
  rowKind,
  rowName,
  rowStart,
  type OutlineRow,
} from "@/map/symbolCards";
import type { PackageLayout } from "@/map/packageLayout";
import { formatDirectory } from "@/map/packageLayout";
import { neighbourhoodOf } from "@/map/neighbourhoods";
import { summarizeReferenceCoverage } from "@/map/referenceCoverage";
import type { StructureDetail } from "@/map/structureCard";
import type { Detent } from "@/map/phoneShell";
import { Breadcrumb } from "@/components/Breadcrumb";
import { DistrictIndexList } from "@/components/Sidebar";
import { PATH_KIND_TEXT as KIND_TEXT, WHY_COLOR, compactCount } from "@/lib/cardText";
import { LanguageRow, StatusGlyph } from "@/components/ReferenceCoverageIndicator";
import {
  BlastLine,
  DistrictBody,
  FolderBody,
  HierOutline,
  SymbolDirectory,
  SymbolRelationsCard,
  UnconnectedList,
} from "@/components/SelectionPanel";
import { ChevronIcon, CloseIcon, FitIcon } from "./icons";

// docs/UX.md §3.2 and §4: what the phone's one bottom sheet shows, by map
// state. Each card leads with its Peek part (the first ~110 px under the
// grabber: a summary and at most one row of actions) and continues with its
// Half/Full part. The detail blocks are the desktop panel's own components
// (SelectionPanel.tsx, Sidebar.tsx, Breadcrumb.tsx) -- principle 10, one
// component, two containers -- hosted with 44 px targets and without inner
// scrollers, since the sheet itself scrolls at Full.

// ---------- shared pieces ----------

export function Eyebrow({ children }: { children: ReactNode }) {
  return <div className="text-meta text-[var(--dim)]">{children}</div>;
}

export function SheetTitle({ children, mono = false }: { children: ReactNode; mono?: boolean }) {
  return (
    <h3 className={`mt-0.5 truncate text-sheet-title tabular-nums ${mono ? "font-mono text-[21px] font-medium" : ""}`}>{children}</h3>
  );
}

export function CloseButton({ label, onClick }: { label: string; onClick(): void }) {
  return (
    <button
      type="button"
      aria-label={label}
      data-sheet-close
      onClick={onClick}
      className="-mr-2.5 -mt-1.5 flex h-11 w-11 shrink-0 items-center justify-center rounded-full bg-[var(--chrome2)] text-[var(--on)]"
    >
      <CloseIcon />
    </button>
  );
}

function SheetButton({
  children,
  onClick,
  primary = false,
  ...rest
}: { children: ReactNode; onClick(): void; primary?: boolean; "aria-label"?: string; [data: `data-${string}`]: string | undefined }) {
  return (
    <button
      type="button"
      onClick={onClick}
      {...rest}
      className={`flex min-h-[44px] flex-1 items-center justify-center gap-2 rounded-[12px] px-2.5 text-[15px] font-medium ${
        primary ? "bg-[var(--accent)] font-semibold text-[var(--on-accent)]" : "border border-[var(--rule)] bg-[var(--chrome2)] text-[var(--on)]"
      }`}
    >
      {children}
    </button>
  );
}

function Section({ title, aside }: { title: ReactNode; aside?: ReactNode }) {
  return (
    <div className="mt-5 flex min-h-[44px] items-center justify-between">
      <h4 className="text-[15px] font-semibold">{title}</h4>
      {aside}
    </div>
  );
}

function fileRowLabel(doc: MapDocument, i: number) {
  const path = doc.F[i];
  const cut = path.lastIndexOf("/");
  return { name: path.slice(cut + 1), dir: cut >= 0 ? path.slice(0, cut + 1) : "" };
}

/** A list of files as 44 px rows: name, then its folder in mono. */
function FileRows({ doc, files, onSelectFile, dataKey }: { doc: MapDocument; files: readonly number[]; onSelectFile(i: number): void; dataKey?: string }) {
  return (
    <div className="-mx-5 mt-1 border-t border-[var(--rule)]">
      {files.map((i) => {
        const { name, dir } = fileRowLabel(doc, i);
        return (
          <button
            key={i}
            type="button"
            data-sheet-file={i}
            data-list={dataKey}
            onClick={() => onSelectFile(i)}
            className="flex min-h-[52px] w-full flex-col justify-center gap-0.5 border-b border-[var(--rule)] px-5 py-1.5 text-left"
          >
            <span className="truncate font-mono text-small">{name}</span>
            {dir && <span className="truncate font-mono text-meta text-[var(--dim)]">{dir}</span>}
          </button>
        );
      })}
    </div>
  );
}

// ---------- nothing selected: the repository, the map-quality row, the index ----------

/** docs/UX.md §4.2: one row, once, in a fixed place: how references were
 * resolved and how many files are off the map. Replaces the floating
 * chips. */
function MapQualityRow({ doc, unconnected, onOpen }: { doc: MapDocument; unconnected: number; onOpen(): void }) {
  const summary = summarizeReferenceCoverage(doc.coverage);
  if (!summary && unconnected === 0) return null;
  return (
    <button
      type="button"
      data-map-quality
      onClick={onOpen}
      className="mt-3.5 flex min-h-[48px] w-full items-center gap-2.5 rounded-[12px] border border-[var(--rule)] bg-[var(--chrome2)] px-3 text-left text-small"
    >
      <span className="text-[var(--dim)]">
        <StatusGlyph status={summary?.status ?? "heuristic"} />
      </span>
      <span className="min-w-0 flex-1">
        {summary ? summary.label : "References"}
        {unconnected > 0 && (
          <>
            <span className="text-[var(--dim)]"> · </span>
            <span data-unconnected-count>{unconnected.toLocaleString("en-US")} unconnected files</span>
          </>
        )}
      </span>
      <span className="text-[var(--dim)]">
        <ChevronIcon />
      </span>
    </button>
  );
}

export function OverviewCard({
  doc,
  packageLayout,
  activeDirectory,
  tab,
  onTab,
  onOpenQuality,
  onSelectDistrict,
  onPickKeyFile,
  onSelectDirectory,
}: {
  doc: MapDocument;
  packageLayout: PackageLayout;
  activeDirectory?: string;
  tab: "districts" | "folders";
  onTab(t: "districts" | "folders"): void;
  onOpenQuality(): void;
  onSelectDistrict(d: number): void;
  onPickKeyFile(i: number): void;
  onSelectDirectory(path?: string): void;
}) {
  let mainland = 0;
  for (const d of Object.values(doc.districts)) if (districtClass(d) === "mainland") mainland++;
  return (
    <div data-sheet-card="overview">
      <div data-sheet-dragzone>
        <div className="font-mono text-meta text-[var(--dim)]">{doc.repo}</div>
        <SheetTitle>
          {mainland} districts · {doc.F.length.toLocaleString("en-US")} files
        </SheetTitle>
        {activeDirectory ? (
          <div className="mt-3.5 flex min-h-[48px] items-center gap-2 rounded-[12px] border border-[var(--rule)] bg-[var(--chrome2)] pl-3 text-small">
            <span className="min-w-0 flex-1 truncate">
              Folder <span className="font-mono">{formatDirectory(activeDirectory)}</span>
            </span>
            <button type="button" aria-label="Clear folder" onClick={() => onSelectDirectory(undefined)} className="flex h-11 w-11 items-center justify-center text-[var(--dim)]">
              <CloseIcon />
            </button>
          </div>
        ) : (
          <MapQualityRow doc={doc} unconnected={packageLayout.unconnectedFiles.length} onOpen={onOpenQuality} />
        )}
        <div className="mt-5 flex items-center justify-between">
          <h4 className="text-[15px] font-semibold">
            {tab === "districts" ? "Districts" : "Folders"}{" "}
            <span className="font-medium text-[var(--dim)]">{tab === "districts" ? mainland : packageLayout.directories.length}</span>
          </h4>
          <div role="tablist" aria-label="Browse by" className="flex rounded-[10px] border border-[var(--rule)] bg-[var(--canvas)] p-0.5">
            {(["districts", "folders"] as const).map((t) => (
              <button
                key={t}
                type="button"
                role="tab"
                aria-selected={tab === t}
                data-index-tab={t}
                onClick={() => onTab(t)}
                className={`h-11 rounded-[8px] px-3.5 text-small ${tab === t ? "bg-[var(--rule)] font-semibold text-[var(--on)]" : "text-[var(--dim)]"}`}
              >
                {t === "districts" ? "Districts" : "Folders"}
              </button>
            ))}
          </div>
        </div>
      </div>
      <div className="-mx-5 mt-2.5 border-t border-[var(--rule)]">
        {tab === "districts" ? (
          <DistrictIndexList doc={doc} packageLayout={packageLayout} onPickKeyFile={onPickKeyFile} onSelectDistrict={onSelectDistrict} variant="sheet" />
        ) : (
          <div className="px-5">
            <FolderBody layout={packageLayout} activeDirectory={activeDirectory} onSelectDirectory={onSelectDirectory} nested={false} />
          </div>
        )}
      </div>
    </div>
  );
}

// ---------- map quality (§4.2's Full sheet) ----------

export function QualityCard({ doc, packageLayout, onClose, onSelectFile }: { doc: MapDocument; packageLayout: PackageLayout; onClose(): void; onSelectFile(i: number): void }) {
  const summary = summarizeReferenceCoverage(doc.coverage);
  const unconnected = packageLayout.unconnectedFiles.length;
  return (
    <div data-sheet-card="quality">
      <div data-sheet-dragzone className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <Eyebrow>Map quality</Eyebrow>
          <SheetTitle>{summary?.label ?? "References"}</SheetTitle>
        </div>
        <CloseButton label="Close map quality" onClick={onClose} />
      </div>
      <p className="mt-3.5 text-body text-[var(--dim)]">
        {summary?.status === "exact"
          ? "Imports and references come from each language's own indexer."
          : "Imports are resolved by tolmap's own rules, so some links are missing."}{" "}
        Every count is a lower bound: the map never guesses a link it cannot see.
      </p>
      {summary && summary.languages.length > 0 && (
        <ul className="mt-2 text-small text-[var(--dim)]" data-reference-languages>
          {summary.languages.map((row) => (
            <LanguageRow key={row.language} row={row} />
          ))}
        </ul>
      )}
      {unconnected > 0 && (
        <>
          <Section title={`${unconnected.toLocaleString("en-US")} unconnected files`} />
          {doc.coverage && (
            <p className="text-small text-[var(--dim)]" data-coverage-detail>
              {/* Same two counts, and the same wording, as the desktop list
                  header (SelectionPanel.tsx): the list is files not placed on
                  the map; coverage counts every file with no kept edge. */}
              {unconnected.toLocaleString("en-US")} not placed on the map. {doc.coverage.zero_edge_files.toLocaleString("en-US")} files have no detected link in all (
              {Object.entries(doc.coverage.by_language)
                .map(([lang, row]) => `${lang}: ${row.zero_edge_files.toLocaleString("en-US")}/${row.total_files.toLocaleString("en-US")}`)
                .join(" · ")}
              ); the rest sit in districts by folder.
            </p>
          )}
          <UnconnectedList layout={packageLayout} doc={doc} onSelectFile={onSelectFile} nested={false} />
        </>
      )}
    </div>
  );
}

// ---------- a district ----------

export function DistrictSheetCard({
  doc,
  d,
  packageLayout,
  onClose,
  onZoomDistrict,
  onDetails,
  onSelectFile,
  onSelectDistrict,
  onSelectDirectory,
}: {
  doc: MapDocument;
  d: number;
  packageLayout: PackageLayout;
  onClose(): void;
  onZoomDistrict(d: number): void;
  onDetails(): void;
  onSelectFile(i: number): void;
  onSelectDistrict(d: number): void;
  onSelectDirectory(path: string): void;
}) {
  const [foldersExpanded, setFoldersExpanded] = useState(false);
  const [filesExpanded, setFilesExpanded] = useState(false);
  const paths = packageLayout.districtPaths.get(d) ?? [];
  const largest = paths.find((p) => !p.other);
  const size = doc.districts[String(d)]?.size ?? 0;
  return (
    <div data-sheet-card="district">
      <div data-sheet-dragzone>
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1">
            <Eyebrow>District</Eyebrow>
            <SheetTitle>{doc.names[d] ?? `district ${d}`}</SheetTitle>
            <p className="mt-1 truncate text-small text-[var(--dim)]">
              <span className="text-[var(--on)]">{compactCount(size)}</span> files
              {largest && largest.share >= 40 && (
                <>
                  {" "}· mostly <span className="font-mono text-meta">{formatDirectory(largest.path!)}</span>
                </>
              )}
            </p>
          </div>
          <CloseButton label="Clear selection" onClick={onClose} />
        </div>
        <div className="mt-3 flex gap-2">
          <SheetButton aria-label="Zoom to district" onClick={() => onZoomDistrict(d)}>
            <FitIcon size={18} />
            Zoom to district
          </SheetButton>
          <SheetButton primary onClick={onDetails} data-sheet-details="">
            Details
          </SheetButton>
        </div>
      </div>
      <div className="mt-4 text-small">
        <DistrictBody
          doc={doc}
          d={d}
          paths={paths}
          onSelectFile={onSelectFile}
          onSelectDistrict={onSelectDistrict}
          onSelectDirectory={onSelectDirectory}
          foldersExpanded={foldersExpanded}
          filesExpanded={filesExpanded}
          onToggleFolders={() => setFoldersExpanded((v) => !v)}
          onToggleFiles={() => setFilesExpanded((v) => !v)}
        />
      </div>
    </div>
  );
}

// ---------- a file, or a symbol in it ----------

/** docs/UX.md §4.5: "the two counts are the legend". Each count carries the
 * same stroke the map draws for that direction -- a solid ring in
 * --link-out for what the file imports, a dashed ring in --link-in for what
 * imports it -- so direction is never colour alone. Tapping a count lists
 * those files. */
function RingGlyph({ dir }: { dir: "out" | "in" }) {
  const colour = dir === "out" ? "var(--link-out)" : "var(--link-in)";
  return (
    <svg viewBox="0 0 24 24" width="22" height="22" fill="none" aria-hidden="true" data-ring-glyph={dir}>
      <circle cx="12" cy="12" r="7" stroke={colour} strokeWidth="2.5" strokeDasharray={dir === "in" ? "4 3" : undefined} />
    </svg>
  );
}

function LinkCounts({ outDeg, inDeg, open, onOpen }: { outDeg: number; inDeg: number; open: "out" | "in" | null; onOpen(dir: "out" | "in"): void }) {
  const cell = (dir: "out" | "in", label: string, value: number) => (
    <button
      type="button"
      data-link-count={dir}
      aria-pressed={open === dir}
      onClick={() => onOpen(dir)}
      className="flex min-h-[56px] min-w-0 items-center gap-2.5 rounded-[12px] border border-[var(--rule)] bg-[var(--chrome2)] px-3 text-left"
      style={{ borderColor: open === dir ? (dir === "out" ? "var(--link-out)" : "var(--link-in)") : undefined }}
    >
      <RingGlyph dir={dir} />
      <span className="min-w-0">
        <span className="block text-meta text-[var(--dim)]">{label}</span>
        <span className="block font-mono text-[17px] font-medium">{value.toLocaleString("en-US")}</span>
      </span>
    </button>
  );
  return (
    <div data-link-legend data-link-counts className="mt-3 grid grid-cols-2 gap-2">
      {cell("out", "Imports", outDeg)}
      {cell("in", "Imported by", inDeg)}
    </div>
  );
}

const KEY_SYMBOLS_AT_HALF = 3;

export function FileSheetCard({
  doc,
  i,
  selSym,
  selHSym,
  symbolsDoc,
  symbolsLoading,
  adj,
  radj,
  detent,
  onClose,
  onDetent,
  onSelectFile,
  onSelectSymbol,
  onSelectHierSymbol,
  onSelectDistrict,
  onBreadcrumbRepo,
  onBreadcrumbFile,
  onPath,
}: {
  doc: MapDocument;
  i: number;
  selSym: number | null;
  selHSym: number | null;
  symbolsDoc?: DistrictSymbols;
  symbolsLoading: boolean;
  adj: AdjMap;
  radj: AdjMap;
  detent: Detent;
  onClose(): void;
  onDetent(d: Detent): void;
  onSelectFile(i: number): void;
  onSelectSymbol(i: number, s: number): void;
  onSelectHierSymbol(global: number): void;
  onSelectDistrict(d: number): void;
  onBreadcrumbRepo(): void;
  onBreadcrumbFile(i: number): void;
  onPath(i: number, dir: "from" | "to"): void;
}) {
  const [listDir, setListDir] = useState<"out" | "in" | null>(null);
  const sy = symbolsOf(doc, i);
  const sm = selSym != null ? sy[selSym] : null;
  const decoded = useMemo(() => (symbolsDoc ? decodeDistrictSymbols(symbolsDoc) : null), [symbolsDoc]);
  const outline = useMemo(() => (decoded ? fileOutline(decoded, i) : []), [decoded, i]);
  const external = useMemo(() => (decoded ? externalReferences(decoded, doc, i) : []), [decoded, doc, i]);
  const symbolCount = decoded ? countOutlineSymbols(outline) : sy.length;
  const outFiles = adj.get(i) ?? [];
  const inFiles = radj.get(i) ?? [];
  const lm = doc.L.find((l) => l[0] === i);
  const blast = computeBlast(doc, i, selSym);
  const unconnected = districtClass(doc.districts[String(D_(doc, i))]) === "unconnected";
  const hood = neighbourhoodOf(doc, i);

  // The selected hierarchical symbol, for the symbol card's own header.
  const hsymRow = decoded && selHSym != null ? (() => {
    const local = decoded.globalToLocal.get(selHSym);
    return local != null ? decoded.raw.symbols[local] : null;
  })() : null;
  const symbolHeader = hsymRow
    ? { kind: KIND_NAMES[rowKind(hsymRow)] ?? "symbol", name: rowName(hsymRow), lines: `${rowStart(hsymRow)}–${rowEnd(hsymRow)}` }
    : sm
      ? { kind: KIND[sm[1]] ?? "symbol", name: sm[0], lines: `${sm[2]}–${sm[3]}` }
      : null;

  // Key symbols: most referenced first, then largest (source order breaks
  // ties, so the pick is deterministic). All of them, as the shared outline
  // tree, at Full.
  const keyRows = useMemo(() => {
    const flat: OutlineRow[] = [];
    const walk = (rows: OutlineRow[]) => rows.forEach((r) => {
      flat.push(r);
      if (r.children.length) walk(r.children);
    });
    walk(outline);
    return flat
      .map((r, order) => ({ r, order }))
      .sort((a, b) => b.r.refsIn - a.r.refsIn || (rowEnd(b.r.row) - rowStart(b.r.row)) - (rowEnd(a.r.row) - rowStart(a.r.row)) || a.order - b.order)
      .slice(0, KEY_SYMBOLS_AT_HALF)
      .map((x) => x.r);
  }, [outline]);
  const flatKey = sy
    .map((s, n) => ({ s, n, span: s[3] - s[2] + 1 }))
    .sort((a, b) => b.span - a.span || a.n - b.n)
    .slice(0, KEY_SYMBOLS_AT_HALF);

  const kindName = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
  return (
    <div data-sheet-card={symbolHeader ? "symbol" : "file"}>
      <div data-sheet-dragzone>
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1">
            <Breadcrumb
              doc={doc}
              sel={i}
              selSym={selSym}
              selD={null}
              selHSym={selHSym}
              symbolsDoc={symbolsDoc}
              onSelectRepo={onBreadcrumbRepo}
              onSelectDistrict={onSelectDistrict}
              onSelectFile={onBreadcrumbFile}
              onSelectHierSymbol={onSelectHierSymbol}
            />
            {symbolHeader ? (
              <>
                <SheetTitle mono>{symbolHeader.name}</SheetTitle>
                <p className="mt-1 truncate text-meta text-[var(--dim)]">
                  {kindName(symbolHeader.kind)} ·{" "}
                  <span className="font-mono">
                    {doc.F[i].split("/").pop()}:{symbolHeader.lines}
                  </span>
                </p>
              </>
            ) : (
              <>
                <SheetTitle mono>{doc.F[i].split("/").pop()}</SheetTitle>
                <p className="mt-1 truncate font-mono text-meta text-[var(--dim)]">{doc.F[i]}</p>
              </>
            )}
          </div>
          <CloseButton label="Clear selection" onClick={onClose} />
        </div>
        {/* The prototype's "Open <file>" and "References" buttons are the
            breadcrumb's file segment and the sheet's own Half: with the
            shared breadcrumb in the header (44 px segments), Peek has no
            room for a second row of the same actions. */}
        <LinkCounts
          outDeg={outFiles.length}
          inDeg={inFiles.length}
          open={listDir}
          onOpen={(dir) => {
            setListDir((cur) => (cur === dir ? null : dir));
            if (detent === "peek") onDetent("half");
          }}
        />
      </div>
      {unconnected && <p className="mt-3 text-small text-[var(--dim)]">not connected to anything, so it isn't placed on the map</p>}
      {listDir && (
        <>
          <Section title={listDir === "out" ? `Imports ${outFiles.length}` : `Imported by ${inFiles.length}`} />
          <FileRows doc={doc} files={(listDir === "out" ? outFiles : inFiles).slice(0, 50)} onSelectFile={onSelectFile} dataKey={listDir} />
          {(listDir === "out" ? outFiles : inFiles).length > 50 && (
            <p className="mt-1 text-meta text-[var(--dim)]">+{(listDir === "out" ? outFiles : inFiles).length - 50} more</p>
          )}
        </>
      )}
      {decoded && selHSym != null && (
        <div className="text-small">
          <SymbolRelationsCard decoded={decoded} global={selHSym} onSelectHierSymbol={onSelectHierSymbol} />
        </div>
      )}
      <div className="mt-3 flex flex-wrap items-center gap-x-3.5 gap-y-1.5 text-small text-[var(--dim)]" data-file-facts>
        {lm && (
          <span
            className="rounded-[6px] px-2 py-0.5 text-meta font-semibold capitalize"
            style={{ color: WHY_COLOR[lm[1]], background: `color-mix(in srgb, ${WHY_COLOR[lm[1]] ?? "var(--dim)"} 16%, transparent)` }}
          >
            {lm[1]}
          </span>
        )}
        <span>
          <span className="font-mono text-[var(--on)]">{LOC(doc, i)}</span> lines
        </span>
        <span>
          <span className="font-mono text-[var(--on)]">{symbolCount}</span> symbols
        </span>
        <span>
          <span className="font-mono text-[var(--on)]">{CH(doc, i)}</span> commits
        </span>
      </div>
      {hood && (
        <p className="mt-1.5 text-small text-[var(--dim)]">
          neighborhood <span className="text-[var(--on)]">{hood.label}</span> · {CODE_LINES(doc, i)} code lines
        </p>
      )}
      <BlastLine blast={blast} doc={doc} />
      {decoded ? (
        detent === "full" ? (
          <HierOutline outline={outline} external={external} selHSym={selHSym} onSelectHierSymbol={onSelectHierSymbol} onHoverHierSymbol={undefined} nested={false} />
        ) : (
          keyRows.length > 0 && (
            <>
              <Section
                title="Key symbols"
                aside={
                  <button type="button" onClick={() => onDetent("full")} className="min-h-[44px] px-1 text-small text-[var(--accent)]">
                    All {symbolCount}
                  </button>
                }
              />
              {keyRows.map((r) => (
                <button
                  key={r.global}
                  type="button"
                  data-outline-row={r.global}
                  onClick={() => onSelectHierSymbol(r.global)}
                  className={`flex min-h-[44px] w-full items-center justify-between gap-2 border-b border-[var(--rule)] text-left ${selHSym === r.global ? "text-[var(--accent)]" : ""}`}
                >
                  <span className="truncate font-mono text-small">{rowName(r.row)}</span>
                  <span className="font-mono text-meta text-[var(--dim)]">
                    {rowStart(r.row)}–{rowEnd(r.row)}
                  </span>
                </button>
              ))}
            </>
          )
        )
      ) : symbolsLoading ? (
        <p className="my-2 text-small text-[var(--dim)]" data-symbols-loading>
          loading symbols…
        </p>
      ) : detent === "full" ? (
        <SymbolDirectory doc={doc} i={i} sy={sy} cur={selSym} onSelectSymbol={onSelectSymbol} />
      ) : (
        flatKey.length > 0 && (
          <>
            <Section
              title="Key symbols"
              aside={
                <button type="button" onClick={() => onDetent("full")} className="min-h-[44px] px-1 text-small text-[var(--accent)]">
                  All {sy.length}
                </button>
              }
            />
            {flatKey.map(({ s, n }) => (
              <button
                key={n}
                type="button"
                data-symbol-row={`${i}:${n}`}
                onClick={() => onSelectSymbol(i, n)}
                className={`flex min-h-[44px] w-full items-center justify-between gap-2 border-b border-[var(--rule)] text-left ${selSym === n ? "text-[var(--accent)]" : ""}`}
              >
                <span className="truncate font-mono text-small">{s[0]}</span>
                <span className="font-mono text-meta text-[var(--dim)]">
                  {s[2]}–{s[3]}
                </span>
              </button>
            ))}
          </>
        )
      )}
      <div className="mt-3.5 flex gap-2">
        <SheetButton data-path-start="from" onClick={() => onPath(i, "from")}>
          Path from here
        </SheetButton>
        <SheetButton data-path-start="to" onClick={() => onPath(i, "to")}>
          Path to here
        </SheetButton>
      </div>
    </div>
  );
}

// ---------- path mode (§4.7) ----------

export interface PathPick {
  /** The file the path was started from ("Path from here") or to ("Path to
   * here"). */
  anchor: number;
  dir: "from" | "to";
}

export function PathSheetCard({
  doc,
  pick,
  route,
  ends,
  onCancel,
  onSelectFile,
  onDetent,
}: {
  doc: MapDocument;
  pick: PathPick;
  route: Route | null;
  /** [from, to] once both ends are picked; null while picking. */
  ends: [number, number] | null;
  onCancel(): void;
  onSelectFile(i: number): void;
  onDetent(d: Detent): void;
}) {
  const name = (i: number) => doc.F[i].split("/").pop();
  if (!ends) {
    return (
      <div data-sheet-card="path" data-path-state="picking">
        <div data-sheet-dragzone>
          <Eyebrow>Path</Eyebrow>
          <SheetTitle>
            {pick.dir === "from" ? "From " : "To "}
            <span className="font-mono text-[20px]">{name(pick.anchor)}</span> · pick {pick.dir === "from" ? "a destination" : "a start"}
          </SheetTitle>
          <p className="mt-1 truncate text-small text-[var(--dim)]">Tap another file on the map</p>
          <div className="mt-3 flex gap-2">
            <SheetButton onClick={onCancel} data-path-cancel="">
              Cancel
            </SheetButton>
          </div>
        </div>
      </div>
    );
  }
  const [from, to] = ends;
  const hops = route ? route.path.length - 1 : 0;
  return (
    <div data-sheet-card="path" data-path-state={route ? "found" : "none"}>
      <div data-sheet-dragzone>
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1">
            <Eyebrow>{route ? `Path · ${hops} hop${hops === 1 ? "" : "s"}` : "Path"}</Eyebrow>
            <SheetTitle>
              <span className="font-mono text-[19px]">{name(from)}</span> → <span className="font-mono text-[19px]">{name(to)}</span>
            </SheetTitle>
            <p className="mt-1 truncate text-small text-[var(--dim)]">{route ? KIND_TEXT[route.kind] : "No path between these files: nothing links them."}</p>
          </div>
          <CloseButton label="Close path" onClick={onCancel} />
        </div>
        {route && (
          <div className="mt-3 flex gap-2">
            <SheetButton onClick={() => onDetent("half")}>Show the files</SheetButton>
          </div>
        )}
      </div>
      {route && (
        <ol className="mt-3 flex flex-col border-l-2 border-[var(--accent)] pl-3.5" data-path-files>
          {route.path.map((f) => (
            <li key={f}>
              <button type="button" onClick={() => onSelectFile(f)} className="flex min-h-[44px] w-full items-center text-left font-mono text-small">
                <span className="truncate">{doc.F[f]}</span>
              </button>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

// ---------- road, street, neighborhood (§3.2) ----------

export interface StructureCardState {
  key: string;
  lines: string[];
  detail: StructureDetail;
}

export function StructureSheetCard({
  doc,
  card,
  onClose,
  onSelectFile,
  onSelectDistrict,
}: {
  doc: MapDocument;
  card: StructureCardState;
  onClose(): void;
  onSelectFile(i: number): void;
  onSelectDistrict(d: number): void;
}) {
  const { detail, lines } = card;
  const eyebrow = detail.kind === "road" ? "Road" : detail.kind === "street" ? "Street" : "Neighborhood";
  const name = (i: number) => doc.F[i].split("/").pop();
  return (
    <div data-sheet-card="structure" data-structure-card={detail.kind}>
      <div data-sheet-dragzone className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <Eyebrow>{eyebrow}</Eyebrow>
          <SheetTitle>{lines[0]}</SheetTitle>
          {detail.kind === "neighborhood" ? (
            <p className="mt-1 text-small text-[var(--dim)]">
              {lines[1]}
              {detail.district != null && (
                <>
                  {" "}· in{" "}
                  <button type="button" onClick={() => onSelectDistrict(detail.district!)} className="min-h-[44px] text-[var(--on)] underline decoration-[var(--rule)] underline-offset-2">
                    {doc.names[detail.district] ?? `district ${detail.district}`}
                  </button>
                </>
              )}
            </p>
          ) : (
            <p className="mt-1 text-small text-[var(--dim)]" data-structure-summary>
              {lines[1]}
            </p>
          )}
        </div>
        <CloseButton label="Close" onClick={onClose} />
      </div>
      {detail.kind === "neighborhood" ? (
        <>
          <Section title={`${detail.files.length} files`} />
          <FileRows doc={doc} files={detail.files.slice(0, 80)} onSelectFile={onSelectFile} />
        </>
      ) : (
        <>
          <Section title={`${detail.pairTotal} import${detail.pairTotal === 1 ? "" : "s"} behind it`} />
          <div className="-mx-5 border-t border-[var(--rule)]">
            {detail.pairs.map(([a, b], n) => (
              <div key={n} className="flex min-h-[44px] items-center gap-1.5 border-b border-[var(--rule)] px-5 font-mono text-meta">
                <button type="button" onClick={() => onSelectFile(a)} className="min-h-[44px] min-w-0 truncate text-left">
                  {name(a)}
                </button>
                <span className="text-[var(--dim)]">→</span>
                <button type="button" onClick={() => onSelectFile(b)} className="min-h-[44px] min-w-0 truncate text-left">
                  {name(b)}
                </button>
              </div>
            ))}
          </div>
          {detail.pairTotal > detail.pairs.length && <p className="mt-1 text-meta text-[var(--dim)]">+{detail.pairTotal - detail.pairs.length} more</p>}
        </>
      )}
    </div>
  );
}
