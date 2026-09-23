import { useMemo, useState, type ReactNode } from "react";
import type { MapDocument, SymbolRow } from "@/types";
import { CH, CX_, D_, FI, LOC, districtClass, districtColor, symbolsOf } from "@/map/geometry";
import { KCOL, KIND, LINK_PREVIEW_MAX } from "@/map/constants";
import { computeBlast, type AdjMap } from "@/map/graph";
import { useIsNarrow } from "@/hooks/useIsNarrow";
import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import type { TerrainSelection } from "@/map/MapRenderer";
import type { DirectoryNode, DistrictPathRow, PackageLayout } from "@/map/packageLayout";
import { formatDirectory } from "@/map/packageLayout";
import { Input } from "@/components/ui/input";

interface Props {
  doc: MapDocument;
  sel: number | null;
  selSym: number | null;
  selD: number | null;
  selTerrain: TerrainSelection | null;
  adj: AdjMap;
  radj: AdjMap;
  packageLayout: PackageLayout;
  activeDirectory?: string;
  open: boolean;
  onToggleOpen(): void;
  onSelectFile(i: number, opts?: { fly?: boolean }): void;
  onSelectSymbol(i: number, s: number): void;
  onZoomDistrict(d: number): void;
  onRouteFrom(i: number): void;
  onRouteTo(i: number): void;
  onSelectDirectory(path?: string): void;
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
  selTerrain,
  adj,
  radj,
  packageLayout,
  activeDirectory,
  open,
  onToggleOpen,
  onSelectFile,
  onSelectSymbol,
  onZoomDistrict,
  onRouteFrom,
  onRouteTo,
  onSelectDirectory,
}: Props) {
  const narrow = useIsNarrow();

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
          {selTerrain == null && selD == null && sel == null ? (
            <FolderHead layout={packageLayout} activeDirectory={activeDirectory} />
          ) : selTerrain != null ? (
            <TerrainHead doc={doc} selection={selTerrain} />
          ) : selD != null ? (
            <DistrictHead doc={doc} d={selD} />
          ) : (
            <FileHead doc={doc} i={sel!} selSym={selSym} />
          )}
        </div>
        {narrow && (
          <span className={`text-[13px] text-[var(--dim)] transition-transform ${isOpen ? "rotate-180" : ""}`}>⌄</span>
        )}
      </div>
      {(!narrow || isOpen) && (
        <div className={narrow ? "max-h-[44vh] overflow-y-auto px-3.5 pb-3" : ""}>
          {selTerrain == null && selD == null && sel == null ? (
            <FolderBody
              layout={packageLayout}
              activeDirectory={activeDirectory}
              onSelectDirectory={onSelectDirectory}
            />
          ) : selTerrain != null ? (
            <TerrainBody doc={doc} selection={selTerrain} onSelectFile={onSelectFile} />
          ) : selD != null ? (
            <DistrictBody
              doc={doc}
              d={selD}
              paths={packageLayout.districtPaths.get(selD) ?? []}
              onSelectFile={onSelectFile}
              onZoomDistrict={onZoomDistrict}
              onSelectDirectory={onSelectDirectory}
            />
          ) : (
            <FileBody
              doc={doc}
              i={sel!}
              selSym={selSym}
              adj={adj}
              radj={radj}
              onSelectSymbol={onSelectSymbol}
              onRouteFrom={onRouteFrom}
              onRouteTo={onRouteTo}
            />
          )}
        </div>
      )}
    </Card>
  );
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
  active,
  onPick,
}: {
  directory: DirectoryNode;
  active: boolean;
  onPick(path: string): void;
}) {
  return (
    <button
      type="button"
      data-folder-path={directory.path}
      onClick={() => onPick(directory.path)}
      className={`grid w-full grid-cols-[1fr_auto] gap-2 border-t border-[var(--rule)] px-1 py-1 text-left text-[10px] first:border-t-0 ${active ? "text-[var(--hot)]" : "text-[var(--on)]"}`}
    >
      <span className="overflow-hidden text-ellipsis whitespace-nowrap">{formatDirectory(directory.path)}</span>
      <span className="text-[var(--dim)]">{directory.count}</span>
    </button>
  );
}

function FolderTreeRow({
  directory,
  depth,
  activeDirectory,
  onPick,
}: {
  directory: DirectoryNode;
  depth: number;
  activeDirectory?: string;
  onPick(path: string): void;
}) {
  const containsActive = !!activeDirectory && (activeDirectory === directory.path || activeDirectory.startsWith(`${directory.path}/`));
  const [expanded, setExpanded] = useState(containsActive);
  return (
    <div>
      <div className="grid grid-cols-[18px_1fr_auto] items-center border-t border-[var(--rule)] first:border-t-0">
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
          onClick={() => onPick(directory.path)}
          className={`min-w-0 overflow-hidden text-ellipsis whitespace-nowrap py-1 text-left text-[10px] ${activeDirectory === directory.path ? "text-[var(--hot)]" : "text-[var(--on)]"}`}
          style={{ paddingLeft: `${depth * 7}px` }}
        >
          {directory.name}/
        </button>
        <span className="pr-1 text-[9.5px] text-[var(--dim)]">{directory.count}</span>
      </div>
      {expanded &&
        directory.children.map((child) => (
          <FolderTreeRow
            key={`${child.path}:${activeDirectory ?? ""}`}
            directory={child}
            depth={depth + 1}
            activeDirectory={activeDirectory}
            onPick={onPick}
          />
        ))}
    </div>
  );
}

function TerrainHead({ doc, selection }: { doc: MapDocument; selection: TerrainSelection }) {
  const district = doc.terrain?.[String(selection.district)];
  if (!district) return null;
  if (selection.kind === "subdistrict") {
    const subdistrict = district.subdistricts[selection.index];
    if (!subdistrict) return null;
    return (
      <>
        <h3 className="truncate font-sans text-[13px] font-semibold">
          {doc.names[String(selection.district)]} · {subdistrict.suffix}
        </h3>
        <p className="mt-0.5 truncate text-[10px] text-[var(--dim)]">{subdistrict.members.length} files</p>
      </>
    );
  }
  const parcel = district.parcels[selection.index];
  if (!parcel) return null;
  return (
    <>
      <h3 className="truncate font-sans text-[13px] font-semibold">{parcel.address}</h3>
      <p className="mt-0.5 truncate text-[10px] text-[var(--dim)]">
        parcel · {parcel.members.length} file{parcel.members.length === 1 ? "" : "s"}
      </p>
    </>
  );
}

function TerrainBody({
  doc,
  selection,
  onSelectFile,
}: {
  doc: MapDocument;
  selection: TerrainSelection;
  onSelectFile: Props["onSelectFile"];
}) {
  const district = doc.terrain?.[String(selection.district)];
  if (!district) return null;
  const members =
    selection.kind === "subdistrict"
      ? district.subdistricts[selection.index]?.members
      : district.parcels[selection.index]?.members;
  if (!members) return null;
  return (
    <div className="mt-2 max-h-[240px] overflow-y-auto">
      {members.map((file) => (
        <button
          key={file}
          onClick={() => onSelectFile(file)}
          className="grid w-full grid-cols-[8px_1fr_auto] items-center gap-1.5 border-t border-[var(--rule)] py-1 text-left text-[10.5px] text-[var(--on)] first:border-t-0 hover:text-white"
        >
          <i className="h-2 w-2 rounded-sm" style={{ background: districtColor(selection.district) }} />
          <span className="overflow-hidden text-ellipsis whitespace-nowrap">{doc.F[file]}</span>
          <span className="text-[9.5px] text-[var(--dim)]">{LOC(doc, file)}</span>
        </button>
      ))}
    </div>
  );
}

function DistrictHead({ doc, d }: { doc: MapDocument; d: number }) {
  const files = [...doc.N.keys()].filter((i) => D_(doc, i) === d);
  const lines = files.reduce((a, i) => a + LOC(doc, i), 0);
  const cls = districtClass(doc.districts[d]);
  return (
    <>
      <h3 className="truncate font-sans text-[13px] font-semibold">{doc.names[d]}</h3>
      <p className="mt-0.5 truncate text-[10px] text-[var(--dim)]">
        {files.length} files · {lines.toLocaleString()} lines
      </p>
      {/* Say what the class MEANS, not the jargon word for it (spec): an
       * island is below the 1% mainland floor but still tied into the repo
       * by an import (it is drawn, offshore — not absent from the map);
       * unconnected has no such tie to anything else at all. Styled like
       * FileBody's own one-line callout (BlastLine below) — the panel's
       * existing idiom for "one fact worth calling out", not a new one. */}
      {cls !== "mainland" && (
        <p className="mt-1.5 border-l-2 border-[var(--hot)] py-0.5 pl-2 text-[10px] leading-snug text-[var(--dim)]">
          {cls === "island"
            ? "island — under 1% of the repo's files, still tied in by an import"
            : "unfiled — no import edge to anything else in the repo"}
        </p>
      )}
    </>
  );
}

function DistrictBody({
  doc,
  d,
  paths,
  onSelectFile,
  onZoomDistrict,
  onSelectDirectory,
}: {
  doc: MapDocument;
  d: number;
  paths: readonly DistrictPathRow[];
  onSelectFile: Props["onSelectFile"];
  onZoomDistrict: Props["onZoomDistrict"];
  onSelectDirectory: Props["onSelectDirectory"];
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
    .slice(0, 3)
    .map((r) => doc.names[String(r[0] === d ? r[1] : r[0])]);
  const lm = doc.L.filter((l) => D_(doc, l[0]) === d);
  return (
    <div className="space-y-0.5">
      {nb.length > 0 && (
        <Row label="connects to">
          <b>{nb.join(", ")}</b>
        </Row>
      )}
      {lm.length > 0 && (
        <Row label="landmarks">
          <b>{lm.length}</b>
        </Row>
      )}
      {paths.length > 0 && (
        <div className="mt-2" data-district-path-breakdown>
          <p className="mb-0.5 font-sans text-[9px] font-semibold uppercase tracking-[0.12em] text-[var(--dim)]">paths</p>
          {paths.map((path, index) =>
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
      <div className="mb-0.5 mt-2 max-h-[148px] overflow-y-auto">
        {top.map((i) => (
          <button
            key={i}
            onClick={() => onSelectFile(i)}
            className="grid w-full grid-cols-[8px_1fr_auto] items-center gap-1.5 border-t border-[var(--rule)] py-1 text-left text-[10.5px] text-[var(--on)] hover:text-white"
          >
            <i className="h-2 w-2 rounded-sm" style={{ background: districtColor(d) }} />
            <span className="overflow-hidden text-ellipsis whitespace-nowrap">{doc.F[i].split("/").slice(1).join("/")}</span>
            <span className="text-[9.5px] text-[var(--dim)]">{LOC(doc, i)}</span>
          </button>
        ))}
      </div>
      <div className="mt-2">
        <Button size="sm" variant="outline" className="w-full" onClick={() => onZoomDistrict(d)}>
          zoom to district
        </Button>
      </div>
    </div>
  );
}

function FileHead({ doc, i, selSym }: { doc: MapDocument; i: number; selSym: number | null }) {
  const sy = symbolsOf(doc, i);
  const sm = selSym != null ? sy[selSym] : null;
  return (
    <>
      <h3 className="truncate font-sans text-[13px] font-semibold">{sm ? sm[0] : doc.F[i].split("/").pop()}</h3>
      <p className="mt-0.5 truncate text-[10px] text-[var(--dim)]">
        {sm ? `${doc.F[i].split("/").pop()}:${sm[2]} · ` : ""}
        {doc.names[String(D_(doc, i))]}
      </p>
    </>
  );
}

function FileBody({
  doc,
  i,
  selSym,
  adj,
  radj,
  onSelectSymbol,
  onRouteFrom,
  onRouteTo,
}: {
  doc: MapDocument;
  i: number;
  selSym: number | null;
  adj: AdjMap;
  radj: AdjMap;
  onSelectSymbol: Props["onSelectSymbol"];
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
  return (
    <div>
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
        <Row label="file lines">
          <b>{LOC(doc, i)}</b>
        </Row>
        {sy.length > 0 && (
          <Row label="symbols">
            <b>{sy.length}</b>
          </Row>
        )}
      </div>
      <BlastLine blast={blast} doc={doc} />
      {linkTotal > LINK_PREVIEW_MAX && (
        <p className="mt-1 text-[9.5px] text-[var(--dim)]">
          links: showing <b className="text-[var(--on)]">{LINK_PREVIEW_MAX}</b> of {linkTotal}
        </p>
      )}
      <SymbolDirectory doc={doc} i={i} sy={sy} cur={selSym} onSelectSymbol={onSelectSymbol} />
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

const WHY_COLOR: Record<string, string> = {
  entry: "#6FB39F",
  bridge: "#D79A4A",
  hub: "#79A7D4",
  capital: "#9FA8B0",
  hazard: "#E0705A",
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
