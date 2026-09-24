import { useMemo, useState, type ReactNode } from "react";
import type { DistrictSymbols, MapDocument, SymbolRow } from "@/types";
import { CH, CODE_LINES, CX_, D_, FI, LOC, districtClass, districtColor, symbolsOf } from "@/map/geometry";
import { neighbourhoodOf } from "@/map/neighbourhoods";
import { KCOL, KIND, LINK_PREVIEW_MAX } from "@/map/constants";
import { computeBlast, type AdjMap } from "@/map/graph";
import {
  countOutlineSymbols,
  decodeDistrictSymbols,
  externalReferences,
  fileOutline,
  isBoldKind,
  symbolLabel,
  type ExternalRefGroup,
  type OutlineRow,
} from "@/map/symbolCards";
import { useIsNarrow } from "@/hooks/useIsNarrow";
import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Breadcrumb } from "@/components/Breadcrumb";
import type { DirectoryNode, DistrictPathRow, PackageLayout } from "@/map/packageLayout";
import { formatDirectory } from "@/map/packageLayout";
import { Input } from "@/components/ui/input";
import { LinkCountsLabel } from "@/components/LinkLegend";

interface Props {
  doc: MapDocument;
  sel: number | null;
  selSym: number | null;
  selD: number | null;
  /** Issue #82 C2: the GLOBAL hierarchical-symbol index (search.ts's `hsym`)
   * and the selected file's district symbols document (undecoded -- this
   * component and Breadcrumb each decode their own copy for their own
   * purpose). `symbolsDoc` is `undefined` while unfetched or for a map with
   * no symbols sibling at all -- every reader below degrades to what it
   * showed before this feature. */
  selHSym?: number | null;
  symbolsDoc?: DistrictSymbols;
  /** Issue #82 C2 follow-up: true while `symbolsDoc` for the selected
   * file's district is being fetched (or is about to be) -- lets the file
   * card show "loading symbols…" instead of flashing the old flat list
   * before the outline tree is ready. `undefined`/`false` otherwise,
   * including when nothing is selected or the map has no symbols sibling
   * at all. */
  symbolsLoading?: boolean;
  adj: AdjMap;
  radj: AdjMap;
  packageLayout: PackageLayout;
  activeDirectory?: string;
  showUnconnected: boolean;
  open: boolean;
  onToggleOpen(): void;
  onSelectFile(i: number): void;
  onSelectSymbol(i: number, s: number): void;
  /** A card tap or an outline-tree row click -- selects the symbol and pans
   * if needed (MapView's selectHierSymbol). */
  onSelectHierSymbol(global: number): void;
  /** Outline row hover -> highlight the card on the map (spec item 5);
   * `null` on mouse-leave. */
  onHoverHierSymbol?(global: number | null): void;
  onSelectDistrict(d: number): void;
  onZoomDistrict(d: number): void;
  onRouteFrom(i: number): void;
  onRouteTo(i: number): void;
  onSelectDirectory(path?: string): void;
  /** Breadcrumb-only (issue #82 A1 scope item 3): jump straight to "nothing
   * selected", and drop the symbol while keeping the same file, respectively
   * -- both guaranteed not to move the view, unlike onSelectFile/
   * onSelectDistrict above whose OTHER callers (sidebar, search) may pan.
   * The district segment reuses onSelectDistrict directly, since that one's
   * already pan-free for every caller in this file. */
  onBreadcrumbRepo(): void;
  onBreadcrumbFile(i: number): void;
}

/** District, file and symbol cards — one component, three bodies, because
 * the reference's #panel is a single DOM node whose innerHTML is one of
 * three shapes depending on what's selected (selD / sel+selSym / sel alone).
 * On a phone it opens as a single line (.phead) and expands only when
 * tapped; anything more covers the map before the reader has touched it. */
export function SelectionPanel({
  doc,
  sel,
  selSym,
  selD,
  selHSym,
  symbolsDoc,
  symbolsLoading,
  adj,
  radj,
  packageLayout,
  activeDirectory,
  showUnconnected,
  open,
  onToggleOpen,
  onSelectFile,
  onSelectSymbol,
  onSelectHierSymbol,
  onHoverHierSymbol,
  onSelectDistrict,
  onZoomDistrict,
  onRouteFrom,
  onRouteTo,
  onSelectDirectory,
  onBreadcrumbRepo,
  onBreadcrumbFile,
}: Props) {
  const narrow = useIsNarrow();
  const unconnectedTotal = packageLayout.unconnectedFiles.length;
  const [foldersExpanded, setFoldersExpanded] = useState(false);
  const [filesExpanded, setFilesExpanded] = useState(false);

  const isOpen = narrow ? open : true;

  return (
    <Card
      data-selection-panel
      className={`absolute z-10 border-[var(--rule)] bg-[var(--chrome)] text-[var(--on)] shadow-lg max-[820px]:inset-x-2.5 max-[820px]:bottom-[calc(58px+env(safe-area-inset-bottom,0px))] max-[820px]:top-auto max-[820px]:w-auto max-[820px]:overflow-hidden max-[820px]:p-0 min-[821px]:right-2.5 min-[821px]:top-2.5 min-[821px]:w-[230px] min-[821px]:p-3`}
      style={narrow ? { maxHeight: isOpen ? "58vh" : "46px" } : undefined}
    >
      <div
        onClick={narrow ? onToggleOpen : undefined}
        className={narrow ? "grid cursor-pointer grid-cols-[1fr_auto] items-center gap-2.5 px-3.5 py-2.5" : ""}
      >
        <div className="overflow-hidden">
          {!showUnconnected && (
            <Breadcrumb
              doc={doc}
              sel={sel}
              selSym={selSym}
              selD={selD}
              selHSym={selHSym}
              symbolsDoc={symbolsDoc}
              onSelectRepo={onBreadcrumbRepo}
              onSelectDistrict={onSelectDistrict}
              onSelectFile={onBreadcrumbFile}
              onSelectHierSymbol={onSelectHierSymbol}
            />
          )}
          {showUnconnected ? (
            <><h3 className="font-sans text-[13px] font-semibold">Unconnected files</h3>
              {doc.coverage && <p className="text-[10px] text-[var(--dim)]" data-coverage-detail>
                {/* Two different counts: the list is files in unconnected
                    districts (not placed on the map); coverage counts every
                    file with no kept edge, including ones merge_tiny placed
                    into a district by folder (#46). Say which is which. */}
                {unconnectedTotal.toLocaleString("en-US")} not placed on the map. {doc.coverage.zero_edge_files.toLocaleString("en-US")} files have no detected link in all ({Object.entries(doc.coverage.by_language).map(([lang, row]) =>
                  `${lang}: ${row.zero_edge_files.toLocaleString("en-US")}/${row.total_files.toLocaleString("en-US")}`).join(" · ")}); the rest sit in districts by folder.
              </p>}
            </>
          ) : selD == null && sel == null ? (
            <FolderHead layout={packageLayout} activeDirectory={activeDirectory} />
          ) : selD != null ? (
            <DistrictHead doc={doc} d={selD} onZoomDistrict={onZoomDistrict} />
          ) : (
            <FileHead doc={doc} i={sel!} selSym={selSym} adj={adj} radj={radj} />
          )}
        </div>
        {narrow && (
          <span className={`text-[13px] text-[var(--dim)] transition-transform ${isOpen ? "rotate-180" : ""}`}>⌄</span>
        )}
      </div>
      {(!narrow || isOpen) && (
        <div className={narrow ? "max-h-[44vh] overflow-y-auto px-3.5 pb-3" : ""}>
          {showUnconnected ? (
            <UnconnectedList layout={packageLayout} doc={doc} onSelectFile={onSelectFile} />
          ) : selD == null && sel == null ? (
            <FolderBody
              layout={packageLayout}
              activeDirectory={activeDirectory}
              onSelectDirectory={onSelectDirectory}
            />
          ) : selD != null ? (
            <DistrictBody
              doc={doc}
              d={selD}
              paths={packageLayout.districtPaths.get(selD) ?? []}
              onSelectFile={onSelectFile}
              onSelectDistrict={onSelectDistrict}
              onSelectDirectory={onSelectDirectory}
              foldersExpanded={foldersExpanded}
              filesExpanded={filesExpanded}
              onToggleFolders={() => setFoldersExpanded((value) => !value)}
              onToggleFiles={() => setFilesExpanded((value) => !value)}
            />
          ) : (
            <FileBody
              doc={doc}
              i={sel!}
              selSym={selSym}
              selHSym={selHSym}
              symbolsDoc={symbolsDoc}
              symbolsLoading={symbolsLoading}
              adj={adj}
              radj={radj}
              onSelectSymbol={onSelectSymbol}
              onSelectHierSymbol={onSelectHierSymbol}
              onHoverHierSymbol={onHoverHierSymbol}
              onRouteFrom={onRouteFrom}
              onRouteTo={onRouteTo}
            />
          )}
        </div>
      )}
    </Card>
  );
}

function UnconnectedList({ layout, doc, onSelectFile }: { layout: PackageLayout; doc: MapDocument; onSelectFile: Props["onSelectFile"] }) {
  return <div className="mt-2 max-h-[44vh] overflow-y-auto" data-unconnected-list>
    {layout.unconnectedGroups.map((group) => <details key={group.path} className="border-t border-[var(--rule)] py-1">
      <summary className="cursor-pointer break-all text-[10px] text-[var(--on)]">{formatDirectory(group.path)} <span className="text-[var(--dim)]">({group.files.length})</span></summary>
      {group.files.map((i) => <button type="button" key={i} data-unconnected-file={i}
        className="block w-full break-all py-1 pl-2 text-left text-[10px] text-[var(--dim)] hover:text-[var(--on)]"
        onClick={() => onSelectFile(i)}>{doc.F[i].split("/").pop()}</button>)}
    </details>)}
  </div>;
}

function FolderHead({ layout, activeDirectory }: { layout: PackageLayout; activeDirectory?: string }) {
  return (
    <>
      <h3 className="truncate font-sans text-[13px] font-semibold">Folders</h3>
      <p className="mt-0.5 truncate text-[10px] text-[var(--dim)]">
        {activeDirectory ? formatDirectory(activeDirectory) : `${layout.directories.length} directories`}
      </p>
    </>
  );
}

function FolderBody({
  layout,
  activeDirectory,
  onSelectDirectory,
}: {
  layout: PackageLayout;
  activeDirectory?: string;
  onSelectDirectory: Props["onSelectDirectory"];
}) {
  const [query, setQuery] = useState("");
  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return [];
    return layout.directories.filter((directory) => directory.path.toLowerCase().includes(q)).slice(0, 20);
  }, [layout, query]);
  const pick = (path: string) => onSelectDirectory(path === activeDirectory ? undefined : path);

  return (
    <div className="mt-2" data-folder-browser>
      <div className="flex gap-1.5">
        <Input
          value={query}
          aria-label="Filter folder paths"
          placeholder="filter folder paths…"
          autoComplete="off"
          className="h-7 min-w-0 bg-[var(--chrome2)] px-2 text-[10.5px] text-[var(--on)]"
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key !== "Enter" || matches.length === 0) return;
            event.preventDefault();
            pick(matches[0].path);
          }}
        />
        {activeDirectory && (
          <Button size="sm" variant="outline" aria-label="Clear folder highlight" onClick={() => onSelectDirectory(undefined)}>
            clear
          </Button>
        )}
      </div>
      <div className="mt-1.5 max-h-[300px] overflow-y-auto">
        {query.trim() ? (
          matches.length > 0 ? (
            matches.map((directory) => (
              <FolderPickRow
                key={directory.path}
                directory={directory}
                totalFiles={layout.filesByDirectory.get(".")?.size ?? 0}
                active={directory.path === activeDirectory}
                onPick={pick}
              />
            ))
          ) : (
            <p className="px-1 py-2 text-[10px] italic text-[var(--dim)]">no folder matches</p>
          )
        ) : (
          layout.directoryRoots.map((directory) => (
            <FolderTreeRow
              key={`${directory.path}:${activeDirectory ?? ""}`}
              directory={directory}
              totalFiles={layout.filesByDirectory.get(".")?.size ?? 0}
              depth={0}
              activeDirectory={activeDirectory}
              onPick={pick}
            />
          ))
        )}
      </div>
    </div>
  );
}

function FolderPickRow({
  directory,
  totalFiles,
  active,
  onPick,
}: {
  directory: DirectoryNode;
  totalFiles: number;
  active: boolean;
  onPick(path: string): void;
}) {
  return (
    <button
      type="button"
      data-folder-path={directory.path}
      onClick={() => onPick(directory.path)}
      aria-pressed={active}
      className={`grid w-full grid-cols-[1fr_auto_auto] gap-2 border-t border-[var(--rule)] px-1 py-1 text-left text-[10px] first:border-t-0 ${active ? "text-[var(--hot)]" : "text-[var(--on)]"}`}
    >
      <span className="overflow-hidden text-ellipsis whitespace-nowrap">{formatDirectory(directory.path)}</span>
      <span className="text-[var(--dim)]">{repositoryShare(directory.count, totalFiles)}</span>
      <span className="text-[var(--dim)]">{directory.count.toLocaleString("en-US")}</span>
    </button>
  );
}

function FolderTreeRow({
  directory,
  totalFiles,
  depth,
  activeDirectory,
  onPick,
}: {
  directory: DirectoryNode;
  totalFiles: number;
  depth: number;
  activeDirectory?: string;
  onPick(path: string): void;
}) {
  const containsActive = !!activeDirectory && (activeDirectory === directory.path || activeDirectory.startsWith(`${directory.path}/`));
  const [expanded, setExpanded] = useState(containsActive);
  return (
    <div>
      <div className="grid grid-cols-[18px_1fr_auto_auto] items-center gap-x-1.5 border-t border-[var(--rule)] first:border-t-0">
        {directory.children.length > 0 ? (
          <button
            type="button"
            aria-label={`${expanded ? "Collapse" : "Expand"} ${directory.path}`}
            onClick={() => setExpanded((value) => !value)}
            className="h-6 text-[11px] text-[var(--dim)]"
          >
            <span className={`inline-block transition-transform ${expanded ? "rotate-90" : ""}`}>›</span>
          </button>
        ) : (
          <span />
        )}
        <button
          type="button"
          data-folder-path={directory.path}
          aria-pressed={activeDirectory === directory.path}
          onClick={() => onPick(directory.path)}
          className={`min-w-0 overflow-hidden text-ellipsis whitespace-nowrap py-1 text-left text-[10px] ${activeDirectory === directory.path ? "text-[var(--hot)]" : "text-[var(--on)]"}`}
          style={{ paddingLeft: `${depth * 7}px` }}
        >
          {directory.name}/
        </button>
        <span className="text-[9.5px] text-[var(--dim)]">{repositoryShare(directory.count, totalFiles)}</span>
        <span className="pr-1 text-[9.5px] text-[var(--dim)]">{directory.count.toLocaleString("en-US")}</span>
      </div>
      {expanded &&
        directory.children.map((child) => (
          <FolderTreeRow
            key={`${child.path}:${activeDirectory ?? ""}`}
            directory={child}
            totalFiles={totalFiles}
            depth={depth + 1}
            activeDirectory={activeDirectory}
            onPick={onPick}
          />
        ))}
    </div>
  );
}

function repositoryShare(count: number, total: number): string {
  const share = total > 0 ? (count / total) * 100 : 0;
  return `${share < 10 ? share.toFixed(1) : Math.round(share)}%`;
}

function compactCount(count: number): string {
  if (count < 1000) return String(count);
  const unit = count >= 1_000_000 ? 1_000_000 : 1000;
  const value = count / unit;
  const digits = value < 10 ? Math.floor(value * 10) / 10 : Math.floor(value);
  return `${digits}${unit === 1000 ? "k" : "m"}`;
}

function DistrictHead({ doc, d, onZoomDistrict }: { doc: MapDocument; d: number; onZoomDistrict: Props["onZoomDistrict"] }) {
  const files = [...doc.N.keys()].filter((i) => D_(doc, i) === d);
  const lines = files.reduce((a, i) => a + LOC(doc, i), 0);
  return (
    <>
      <div className="flex items-center gap-1">
        <h3 className="min-w-0 flex-1 truncate font-sans text-[13px] font-semibold">{doc.names[d]}</h3>
        <button
          type="button"
          aria-label="Zoom to district"
          title="Zoom to district"
          onClick={(event) => { event.stopPropagation(); onZoomDistrict(d); }}
          className="flex h-6 w-6 shrink-0 items-center justify-center rounded text-[15px] text-[var(--dim)] hover:text-[var(--on)]"
        >
          ⤢
        </button>
      </div>
      <p className="mt-0.5 truncate text-[10px] text-[var(--dim)]">
        {compactCount(files.length)} files · {compactCount(lines)} lines
      </p>
    </>
  );
}

function DistrictBody({
  doc,
  d,
  paths,
  onSelectFile,
  onSelectDistrict,
  onSelectDirectory,
  foldersExpanded,
  filesExpanded,
  onToggleFolders,
  onToggleFiles,
}: {
  doc: MapDocument;
  d: number;
  paths: readonly DistrictPathRow[];
  onSelectFile: Props["onSelectFile"];
  onSelectDistrict: Props["onSelectDistrict"];
  onSelectDirectory: Props["onSelectDirectory"];
  foldersExpanded: boolean;
  filesExpanded: boolean;
  onToggleFolders(): void;
  onToggleFiles(): void;
}) {
  const files = [...doc.N.keys()].filter((i) => D_(doc, i) === d);
  const top = files
    .slice()
    .sort((a, b) => FI(doc, b) + LOC(doc, b) / 60 - (FI(doc, a) + LOC(doc, a) / 60))
    .slice(0, 6);
  // which districts it actually borders, by edge weight — a fact about the
  // system that the file-level view never states outright. Empty when the
  // repo has one district and no roads (tolmap's own self-map, for now: see
  // finding/issue #12 on relative-import resolution) — the row is simply
  // omitted rather than shown empty.
  const nb = doc.roads
    .filter((r) => r[0] === d || r[1] === d)
    .sort((a, b) => b[2] - a[2])
    .slice(0, 2)
    .map((r) => ({ id: r[0] === d ? r[1] : r[0], name: doc.names[String(r[0] === d ? r[1] : r[0])] }));
  const largest = paths.find((path) => !path.other);
  return (
    <div className="mt-1.5 space-y-1 text-[10px]" data-district-summary>
      {largest && largest.share >= 40 && (
        <p className="truncate text-[var(--dim)]" title={`mostly ${formatDirectory(largest.path!)}`}>
          mostly <span className="text-[var(--on)]">{formatDirectory(largest.path!)}</span>
        </p>
      )}
      {nb.length > 0 && (
        <p className="truncate text-[var(--dim)]">
          near{" "}
          {nb.map((neighbour, index) => (
            <span key={neighbour.id}>
              {index > 0 ? ", " : ""}
              <button type="button" data-neighbour-district={neighbour.id} className="text-[var(--on)] hover:text-[var(--hot)]" onClick={() => onSelectDistrict(neighbour.id)}>
                {neighbour.name}
              </button>
            </span>
          ))}
        </p>
      )}
      {paths.length > 0 && (
        <div data-district-path-breakdown>
          <button type="button" data-district-folders-toggle aria-expanded={foldersExpanded} onClick={onToggleFolders} className="w-full text-left text-[var(--on)]">
            <span className="text-[var(--dim)]">{foldersExpanded ? "⌄" : "›"}</span> folders ({paths.filter((path) => !path.other).length})
          </button>
          {foldersExpanded && paths.map((path, index) =>
            path.other ? (
              <div
                key="other"
                className="grid grid-cols-[30px_1fr_auto] gap-1.5 border-t border-[var(--rule)] py-1 text-[9.5px] text-[var(--dim)]"
              >
                <b className="text-[var(--on)]">{path.share}%</b>
                <span>other</span>
                <span>({path.count} {path.count === 1 ? "file" : "files"})</span>
              </div>
            ) : (
              <button
                type="button"
                key={path.path ?? index}
                data-district-path={path.path}
                aria-label={`Highlight folder ${path.path}`}
                onClick={() => onSelectDirectory(path.path!)}
                className="grid w-full grid-cols-[30px_1fr_auto] gap-1.5 border-t border-[var(--rule)] py-1 text-left text-[9.5px] text-[var(--on)] hover:text-white"
              >
                <b>{path.share}%</b>
                <span className="overflow-hidden text-ellipsis whitespace-nowrap">{formatDirectory(path.path!)}</span>
                <span className="text-[var(--dim)]">({path.count} {path.count === 1 ? "file" : "files"})</span>
              </button>
            ),
          )}
        </div>
      )}
      <div>
        <button type="button" data-district-files-toggle aria-expanded={filesExpanded} onClick={onToggleFiles} className="w-full text-left text-[var(--on)]">
          <span className="text-[var(--dim)]">{filesExpanded ? "⌄" : "›"}</span> key files ({top.length})
        </button>
        {filesExpanded && top.map((i) => (
          <button
            key={i}
            data-district-key-file={i}
            onClick={() => onSelectFile(i)}
            className="grid w-full grid-cols-[8px_1fr_auto] items-center gap-1.5 border-t border-[var(--rule)] py-1 text-left text-[10.5px] text-[var(--on)] hover:text-white"
          >
            <i className="h-2 w-2 rounded-sm" style={{ background: districtColor(doc, d) }} />
            <span className="overflow-hidden text-ellipsis whitespace-nowrap">{doc.F[i].split("/").slice(1).join("/")}</span>
            <span className="text-[9.5px] text-[var(--dim)]">{LOC(doc, i)} lines</span>
          </button>
        ))}
      </div>
    </div>
  );
}

// Issue #82 A1 scope item 5: the subtitle here is also what the phone's
// collapsed sheet shows (SelectionPanel renders this same header whether or
// not the body below it is expanded) -- checking it against the fullscreen
// summary bar's own spec ("e.g. for a file: 'imported by N files · imports
// M'") is what caught that it used to show the district name instead, which
// isn't a one-line SUMMARY of the file so much as a second name. Reusing the
// exact line SelectionSummaryBar.tsx shows means the phone sheet and the
// fullscreen bar can't drift apart the way two independent implementations
// would.
function FileHead({ doc, i, selSym, adj, radj }: { doc: MapDocument; i: number; selSym: number | null; adj: AdjMap; radj: AdjMap }) {
  const sy = symbolsOf(doc, i);
  const sm = selSym != null ? sy[selSym] : null;
  const outDeg = adj.get(i)?.length ?? 0;
  const inDeg = radj.get(i)?.length ?? 0;
  return (
    <>
      <h3 className="truncate font-sans text-[13px] font-semibold">{sm ? sm[0] : doc.F[i].split("/").pop()}</h3>
      <p className={`mt-0.5 text-[10px] text-[var(--dim)] ${sm ? "truncate" : ""}`}>
        {sm ? (
          `${doc.F[i].split("/").pop()}:${sm[2]} · ${KIND[sm[1]]}`
        ) : (
          <LinkCountsLabel inDeg={inDeg} outDeg={outDeg} stacked />
        )}
      </p>
    </>
  );
}

function FileBody({
  doc,
  i,
  selSym,
  selHSym,
  symbolsDoc,
  symbolsLoading,
  adj,
  radj,
  onSelectSymbol,
  onSelectHierSymbol,
  onHoverHierSymbol,
  onRouteFrom,
  onRouteTo,
}: {
  doc: MapDocument;
  i: number;
  selSym: number | null;
  selHSym?: number | null;
  symbolsDoc?: DistrictSymbols;
  symbolsLoading?: boolean;
  adj: AdjMap;
  radj: AdjMap;
  onSelectSymbol: Props["onSelectSymbol"];
  onSelectHierSymbol: Props["onSelectHierSymbol"];
  onHoverHierSymbol: Props["onHoverHierSymbol"];
  onRouteFrom: Props["onRouteFrom"];
  onRouteTo: Props["onRouteTo"];
}) {
  const sy = symbolsOf(doc, i);
  const sm = selSym != null ? sy[selSym] : null;
  const lm = doc.L.find((l) => l[0] === i);
  const blast = computeBlast(doc, i, selSym);
  // Total degree only (O(1) -- adj/radj are already-built adjacency maps,
  // no sort needed for a count); MapRenderer's own selNeighbours branch on
  // the map only draws these links when blast is ALSO null (a symbol's
  // blast radius takes precedence there, same as here), so this line
  // states a fact about exactly what's currently drawn, never something
  // the map isn't showing.
  const linkTotal = blast ? 0 : (adj.get(i)?.length ?? 0) + (radj.get(i)?.length ?? 0);
  // Issue #82 C2 follow-up: the file card used to show BOTH the old flat,
  // span-sorted SymbolDirectory (built from the map's truncated `S`) and the
  // new hierarchical outline tree at once. Once a district's symbols have
  // loaded, the outline is strictly the richer view of the SAME
  // information (source order, real nesting, real reference counts), so it
  // replaces the flat list entirely rather than sitting below it. The flat
  // list stays only as the fallback: a map with no symbols sibling at all,
  // or a fetch that's genuinely settled with nothing for this district.
  const decoded = useMemo(() => (symbolsDoc ? decodeDistrictSymbols(symbolsDoc) : null), [symbolsDoc]);
  const outline = useMemo(() => (decoded ? fileOutline(decoded, i) : []), [decoded, i]);
  const external = useMemo(() => (decoded ? externalReferences(decoded, doc, i) : []), [decoded, doc, i]);
  // The "symbols" count below switches to this hierarchical total the
  // moment `decoded` exists -- "where a count is still shown, it should be
  // the hierarchical count." `sy.length` only backs it while there's no
  // hierarchical data to ask instead (including while it's still loading).
  const symbolCount = decoded ? countOutlineSymbols(outline) : sy.length;
  return (
    <div>
      {districtClass(doc.districts[String(D_(doc, i))]) === "unconnected" &&
        <p className="my-2 text-[10px] text-[var(--dim)]">not connected to anything, so it isn't placed on the map</p>}
      <p className="break-all text-[10px] text-[var(--dim)]">
        {doc.F[i]}
        {sm ? `:${sm[2]}` : ""}
      </p>
      {sm && (
        <>
          <Row label="kind">
            <b style={{ color: KCOL[sm[1]] }}>{KIND[sm[1]]}</b>
          </Row>
          <Row label="lines">
            <b>
              {sm[2]}–{sm[3]}
            </b>
          </Row>
        </>
      )}
      <div className="flex flex-wrap gap-x-3 gap-y-0.5">
        {lm && (
          <Row label="landmark">
            <b className="text-[9px] uppercase tracking-wide" style={{ color: WHY_COLOR[lm[1]] }}>
              {lm[1]}
            </b>
          </Row>
        )}
        <Row label="district">
          <b>{doc.names[String(D_(doc, i))]}</b>
        </Row>
        {/* B4 (nested footprints, issue #82 scope item 8): "the file card
            shows the file's neighbourhood label." Absent for a document with
            no neighbourhood data at all (pre-#85 maps) -- neighbourhoodOf
            returns null there and the row is simply omitted, the same
            pattern the "near" row above already uses for a repo with no
            roads. */}
        {neighbourhoodOf(doc, i) && (
          <Row label="neighbourhood">
            <b>{neighbourhoodOf(doc, i)!.label}</b>
          </Row>
        )}
        <Row label="file lines">
          <b>{LOC(doc, i)} lines · {CODE_LINES(doc, i)} code</b>
        </Row>
        {symbolCount > 0 && (
          <Row label="symbols">
            <b>{symbolCount}</b>
          </Row>
        )}
      </div>
      <BlastLine blast={blast} doc={doc} />
      {linkTotal > LINK_PREVIEW_MAX && (
        <p className="mt-1 text-[9.5px] text-[var(--dim)]">
          links: showing <b className="text-[var(--on)]">{LINK_PREVIEW_MAX}</b> of {linkTotal}
        </p>
      )}
      {decoded ? (
        <HierOutline
          outline={outline}
          external={external}
          selHSym={selHSym ?? null}
          onSelectHierSymbol={onSelectHierSymbol}
          onHoverHierSymbol={onHoverHierSymbol}
        />
      ) : symbolsLoading ? (
        <p className="my-2 text-[10px] text-[var(--dim)]" data-symbols-loading>
          loading symbols…
        </p>
      ) : (
        <SymbolDirectory doc={doc} i={i} sy={sy} cur={selSym} onSelectSymbol={onSelectSymbol} />
      )}
      <div className="flex flex-wrap gap-x-3 gap-y-0.5">
        <Row label="commits">
          <b>{CH(doc, i)}</b>
        </Row>
        <Row label="branches">
          <b>{CX_(doc, i)}</b>
        </Row>
        <Row label="imported by">
          <b>{FI(doc, i)}</b>
        </Row>
      </div>
      <div className="mt-2 flex gap-1.5">
        <Button size="sm" variant="outline" className="flex-1" onClick={() => onRouteFrom(i)}>
          route from
        </Button>
        <Button size="sm" variant="outline" className="flex-1" onClick={() => onRouteTo(i)}>
          route to
        </Button>
      </div>
    </div>
  );
}

// Theme-alignment audit: these used to be fixed hex values, which read fine
// against the old dark-only --chrome but drop under WCAG AA against the new
// light --chrome (index.css's own --why-* comment has the measured ratios).
// Indirected through CSS custom properties (light/dark variants defined
// there) rather than a second JS-side light/dark table here, so this file
// doesn't need to know which theme is active.
const WHY_COLOR: Record<string, string> = {
  entry: "var(--why-entry)",
  bridge: "var(--why-bridge)",
  hub: "var(--why-hub)",
  capital: "var(--why-capital)",
  hazard: "var(--why-hazard)",
};

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex justify-between gap-2 border-t border-[var(--rule)] py-0.5 text-[10.5px] first:border-t-0">
      <span className="text-[var(--dim)]">{label}</span>
      {children}
    </div>
  );
}

// The count is an IDE fact. The number of DISTRICTS it crosses is the one
// the map can tell you and the editor cannot: a change contained in one
// concern is a different risk from a change that reaches five.
function BlastLine({ blast, doc }: { blast: ReturnType<typeof computeBlast>; doc: MapDocument }) {
  if (!blast) return null;
  const n = blast.files.length;
  const d = blast.districts.size;
  const names = [...blast.districts].map((x) => doc.names[String(x)]).slice(0, 4);
  return (
    <div className="my-2 border-l-2 border-[var(--hot)] py-1 pl-2.5 text-[10.5px] leading-relaxed">
      <b className="text-[var(--hot)]">{n}</b> file{n === 1 ? "" : "s"} reference it, across{" "}
      <b className="text-[var(--hot)]">{d}</b> district{d === 1 ? "" : "s"}
      <span className="block text-[9.5px] text-[var(--dim)]">
        {names.join(" · ")}
        {blast.districts.size > 4 ? " …" : ""}
      </span>
    </div>
  );
}

// A file's contents belong in a directory, not on the map: you navigate to
// the building, then read the board. The bar shows what share of the file
// each symbol occupies, which is the one spatial fact worth carrying up.
function SymbolDirectory({
  doc,
  i,
  sy,
  cur,
  onSelectSymbol,
}: {
  doc: MapDocument;
  i: number;
  sy: SymbolRow[];
  cur: number | null;
  onSelectSymbol: Props["onSelectSymbol"];
}) {
  if (!sy.length) return null;
  const loc = LOC(doc, i) || 1;
  const used = sy.reduce((a, sm) => a + Math.max(0, sm[3] - sm[2] + 1), 0);
  const rest = Math.max(0, 1 - used / loc);
  const rows = sy.map((sm, n) => ({ sm, n, span: sm[3] - sm[2] + 1 })).sort((a, b) => b.span - a.span);
  const shown = rows.slice(0, 9);
  return (
    <>
      <div className="my-2 flex h-[7px] gap-px overflow-hidden rounded-sm">
        {sy.map((sm, n) => {
          const share = Math.max(0, sm[3] - sm[2] + 1) / loc;
          if (share <= 0.012) return null;
          return <i key={n} style={{ flex: share, background: KCOL[sm[1]] }} />;
        })}
        {rest > 0.012 && <i style={{ flex: rest, background: "var(--rule)" }} />}
      </div>
      <div className="mb-0.5 max-h-[148px] overflow-y-auto">
        {shown.map((r) => (
          <button
            key={r.n}
            data-symbol-row={`${i}:${r.n}`}
            onClick={() => onSelectSymbol(i, r.n)}
            className={`grid w-full grid-cols-[8px_1fr_auto] items-center gap-1.5 border-t border-[var(--rule)] py-1 text-left text-[10.5px] first:border-t-0 ${cur === r.n ? "text-[var(--hot)]" : "text-[var(--on)]"}`}
          >
            <i className="h-2 w-2 rounded-sm" style={{ background: KCOL[r.sm[1]] }} />
            <span className="overflow-hidden text-ellipsis whitespace-nowrap">{r.sm[0]}</span>
            <span className="text-[9.5px] text-[var(--dim)]">
              {r.sm[2]}–{r.sm[3]}
            </span>
          </button>
        ))}
      </div>
      {rows.length > shown.length && (
        <div className="pt-0.5 text-[9.5px] text-[var(--dim)]">+{rows.length - shown.length} more symbols</div>
      )}
    </>
  );
}

// Issue #82 C2 scope item 5: the file's hierarchical outline (source order,
// nested -- classes contain their methods, unlike SymbolDirectory above's
// flat bar-chart list of the map's older, non-nested `S`), external
// references grouped by top-level class/function, and finding 30's
// under-count note. FileBody only renders this once `symbolsDoc` has
// decoded -- once it has, this replaces SymbolDirectory entirely (issue #82
// C2 follow-up) rather than sitting alongside it. Still renders nothing for
// a file with no top-level symbols AND no external references of its own
// (a config file, say), the same silent degrade the map itself uses.
function HierOutline({
  outline,
  external,
  selHSym,
  onSelectHierSymbol,
  onHoverHierSymbol,
}: {
  outline: OutlineRow[];
  external: ExternalRefGroup[];
  selHSym: number | null;
  onSelectHierSymbol: Props["onSelectHierSymbol"];
  onHoverHierSymbol: Props["onHoverHierSymbol"];
}) {
  if (outline.length === 0 && external.length === 0) return null;

  const row = (r: OutlineRow, depth: number) => (
    <div key={r.global}>
      <button
        type="button"
        data-outline-row={r.global}
        onMouseEnter={() => onHoverHierSymbol?.(r.global)}
        onMouseLeave={() => onHoverHierSymbol?.(null)}
        onClick={() => onSelectHierSymbol(r.global)}
        style={{ paddingLeft: 6 + depth * 12 }}
        className={`grid w-full grid-cols-[1fr_auto] items-center gap-1.5 border-t border-[var(--rule)] py-1 text-left text-[10.5px] first:border-t-0 ${selHSym === r.global ? "text-[var(--hot)]" : "text-[var(--on)]"}`}
      >
        <span className={`overflow-hidden text-ellipsis whitespace-nowrap ${isBoldKind(r.row[2]) ? "font-semibold" : ""}`}>
          {symbolLabel(r.row, r.children.length, false)}
        </span>
        <span className="text-[9.5px] text-[var(--dim)]" title="incoming references">
          {r.refsIn > 0 ? `← ${r.refsIn}` : ""}
        </span>
      </button>
      {r.children.map((c) => row(c, depth + 1))}
    </div>
  );

  return (
    <div className="mt-2 border-t border-[var(--rule)] pt-1.5">
      <div className="mb-0.5 text-[9.5px] uppercase tracking-wide text-[var(--dim)]">outline</div>
      {outline.length > 0 ? (
        <div className="mb-1.5 max-h-[180px] overflow-y-auto" data-outline-tree>
          {outline.map((r) => row(r, 0))}
        </div>
      ) : (
        <p className="mb-1.5 text-[9.5px] text-[var(--dim)]">no top-level symbols</p>
      )}
      {external.length > 0 && (
        <>
          <div className="mb-0.5 text-[9.5px] uppercase tracking-wide text-[var(--dim)]">external references</div>
          <div className="mb-1.5 max-h-[140px] overflow-y-auto">
            {external.map((group) => (
              <div key={group.from.global} className="border-t border-[var(--rule)] py-1 text-[10px] first:border-t-0">
                <div className="font-semibold text-[var(--on)]">{symbolLabel(group.from.row, group.from.children.length, false)}</div>
                {group.targets.slice(0, 8).map((t) => (
                  <div key={t.key} className="flex justify-between gap-2 text-[9.5px] text-[var(--dim)]">
                    <span className="overflow-hidden text-ellipsis whitespace-nowrap">{t.label}</span>
                    <span>{t.count}</span>
                  </div>
                ))}
                {group.targets.length > 8 && (
                  <div className="text-[9.5px] text-[var(--dim)]">+{group.targets.length - 8} more</div>
                )}
              </div>
            ))}
          </div>
        </>
      )}
      {/* finding 30: resolution is a lower bound -- said here, plainly, once
          per file card that has any symbol data at all, not buried in docs. */}
      <p className="text-[9px] leading-snug text-[var(--dim)]">
        Method references are under-counted: calls through instances and inherited methods aren&apos;t resolved.
      </p>
    </div>
  );
}
