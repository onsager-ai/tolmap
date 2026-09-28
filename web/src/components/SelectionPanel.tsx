import { useMemo, useState } from "react";
import type { MapDocument, SymbolRow } from "@/types";
import { D_, FI, LOC, districtColor } from "@/map/geometry";
import { KCOL } from "@/map/constants";
import type { computeBlast } from "@/map/graph";
import {
  isBoldKind,
  KIND_CLASS,
  KIND_INTERFACE,
  KIND_METHOD,
  rowAbstract,
  rowKind,
  symbolHierarchy,
  symbolLabel,
  type DecodedDistrictSymbols,
  type ExternalRefGroup,
  type HierarchyRelation,
  type OutlineRow,
} from "@/map/symbolCards";
import { Button } from "@/components/ui/button";
import type { DirectoryNode, DistrictPathRow, PackageLayout } from "@/map/packageLayout";
import { formatDirectory } from "@/map/packageLayout";
import { Input } from "@/components/ui/input";

// The detail blocks of the district, file and symbol cards (docs/UX.md
// principle 10, "one component, two containers"): the phone sheet and the
// desktop/tablet panel both render the cards in phone/SheetCards.tsx,
// which compose these. The old desktop-only SelectionPanel card that used to
// live here (a 260 px corner card with its own header and body per
// selection) was replaced by the shared phone cards in phase 5; phase 7a
// moves their desktop container to the floating panel.

/** The callbacks the blocks below share. */
interface Props {
  onSelectFile(i: number): void;
  onSelectSymbol(i: number, s: number): void;
  /** A card tap or an outline-tree row click -- selects the symbol and pans
   * if needed (MapView's selectHierSymbol). */
  onSelectHierSymbol(global: number): void;
  /** Outline row hover -> highlight the card on the map (spec item 5);
   * `null` on mouse-leave. */
  onHoverHierSymbol?(global: number | null): void;
  onSelectDistrict(d: number): void;
  onSelectDirectory(path?: string): void;
}

/** `nested={false}` (the phone sheet, docs/UX.md §3.1): no scroller of its
 * own -- the sheet scrolls at Full, and a list scrolling inside it is the
 * scroll-inside-scroll §3.1 removes. Rows are 44 px there (§8.2). */
export function UnconnectedList({ layout, doc, onSelectFile, nested = true, touchTargets = false, compactRows = false }: { layout: PackageLayout; doc: MapDocument; onSelectFile: Props["onSelectFile"]; nested?: boolean; touchTargets?: boolean; compactRows?: boolean }) {
  return <div className={nested ? "mt-2 max-h-[44vh] overflow-y-auto" : "mt-2"} data-unconnected-list>
    {layout.unconnectedGroups.map((group) => <details key={group.path} className="border-t border-[var(--rule)] py-1">
      <summary className={`cursor-pointer break-all font-mono text-meta text-[var(--on)] ${nested ? touchTargets ? "min-h-[44px]" : "" : `flex ${touchTargets ? "min-h-[44px]" : compactRows ? "min-h-8" : "min-h-[44px]"} items-center`}`}>{formatDirectory(group.path)} <span className="text-[var(--dim)]">({group.files.length})</span></summary>
      {group.files.map((i) => <button type="button" key={i} data-unconnected-file={i}
        className={`block w-full break-all py-1 pl-2 text-left font-mono text-meta text-[var(--dim)] hover:text-[var(--on)] ${nested ? touchTargets ? "min-h-[44px]" : "" : touchTargets ? "min-h-[44px]" : compactRows ? "min-h-8" : "min-h-[44px]"}`}
        onClick={() => onSelectFile(i)}>{doc.F[i].split("/").pop()}</button>)}
    </details>)}
  </div>;
}

/** `nested={false}`: the phone sheet's Folders tab -- no inner scroller
 * (see UnconnectedList) and 44 px rows. */
export function FolderBody({
  layout,
  activeDirectory,
  onSelectDirectory,
  nested = true,
  touchTargets = false,
}: {
  layout: PackageLayout;
  activeDirectory?: string;
  onSelectDirectory: Props["onSelectDirectory"];
  nested?: boolean;
  touchTargets?: boolean;
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
          className={`${nested ? touchTargets ? "h-11" : "h-8" : "h-11"} min-w-0 bg-[var(--chrome2)] px-2 font-mono text-[var(--on)]`}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key !== "Enter" || matches.length === 0) return;
            event.preventDefault();
            pick(matches[0].path);
          }}
        />
        {activeDirectory && (
          <Button size="sm" variant="outline" aria-label="Clear folder highlight" onClick={() => onSelectDirectory(undefined)} className={touchTargets ? "h-11 min-w-[44px]" : ""}>
            clear
          </Button>
        )}
      </div>
      <div className={nested ? "mt-1.5 max-h-[300px] overflow-y-auto" : "mt-1.5"} data-folder-rows-large={nested ? undefined : ""}>
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
            <p className="px-1 py-2 text-meta italic text-[var(--dim)]">no folder matches</p>
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
      className={`grid w-full grid-cols-[1fr_auto_auto] items-center gap-2 border-t border-[var(--rule)] px-1 py-1 text-left font-mono text-meta first:border-t-0 touch:min-h-[44px] ${active ? "text-[var(--accent)]" : "text-[var(--on)]"}`}
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
            className="h-6 text-meta text-[var(--dim)] touch:h-11"
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
          className={`min-w-0 overflow-hidden text-ellipsis whitespace-nowrap py-1 text-left font-mono text-meta touch:min-h-[44px] ${activeDirectory === directory.path ? "text-[var(--accent)]" : "text-[var(--on)]"}`}
          style={{ paddingLeft: `${depth * 7}px` }}
        >
          {directory.name}/
        </button>
        <span className="font-mono text-meta text-[var(--dim)]">{repositoryShare(directory.count, totalFiles)}</span>
        <span className="pr-1 font-mono text-meta text-[var(--dim)]">{directory.count.toLocaleString("en-US")}</span>
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


export function DistrictBody({
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
    <div className="mt-1.5 space-y-1 text-meta" data-district-summary>
      {largest && largest.share >= 40 && (
        <p className="truncate text-[var(--dim)]" title={`mostly ${formatDirectory(largest.path!)}`}>
          mostly <span className="font-mono text-[var(--on)]">{formatDirectory(largest.path!)}</span>
        </p>
      )}
      {nb.length > 0 && (
        <p className="truncate text-[var(--dim)]">
          near{" "}
          {nb.map((neighbour, index) => (
            <span key={neighbour.id}>
              {index > 0 ? ", " : ""}
              <button type="button" data-neighbour-district={neighbour.id} className="text-[var(--on)] hover:text-[var(--accent)] touch:inline-flex touch:min-h-[44px] touch:items-center" onClick={() => onSelectDistrict(neighbour.id)}>
                {neighbour.name}
              </button>
            </span>
          ))}
        </p>
      )}
      {paths.length > 0 && (
        <div data-district-path-breakdown>
          <button type="button" data-district-folders-toggle aria-expanded={foldersExpanded} onClick={onToggleFolders} className="w-full text-left text-[var(--on)] touch:min-h-[44px] touch:text-small">
            <span className="text-[var(--dim)]">{foldersExpanded ? "⌄" : "›"}</span> folders ({paths.filter((path) => !path.other).length})
          </button>
          {foldersExpanded && paths.map((path, index) =>
            path.other ? (
              <div
                key="other"
                className="grid grid-cols-[36px_1fr_auto] gap-1.5 border-t border-[var(--rule)] py-1 text-meta text-[var(--dim)]"
              >
                <b className="font-mono text-[var(--on)]">{path.share}%</b>
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
                className="grid w-full grid-cols-[36px_1fr_auto] items-center gap-1.5 border-t border-[var(--rule)] py-1 text-left text-meta text-[var(--on)] hover:text-[var(--accent)] touch:min-h-[44px]"
              >
                <b className="font-mono">{path.share}%</b>
                <span className="overflow-hidden text-ellipsis whitespace-nowrap font-mono">{formatDirectory(path.path!)}</span>
                <span className="text-[var(--dim)]">({path.count} {path.count === 1 ? "file" : "files"})</span>
              </button>
            ),
          )}
        </div>
      )}
      <div>
        <button type="button" data-district-files-toggle aria-expanded={filesExpanded} onClick={onToggleFiles} className="w-full text-left text-[var(--on)] touch:min-h-[44px] touch:text-small">
          <span className="text-[var(--dim)]">{filesExpanded ? "⌄" : "›"}</span> key files ({top.length})
        </button>
        {filesExpanded && top.map((i) => (
          <button
            key={i}
            data-district-key-file={i}
            onClick={() => onSelectFile(i)}
            className="grid w-full grid-cols-[8px_1fr_auto] items-center gap-1.5 border-t border-[var(--rule)] py-1 text-left text-meta text-[var(--on)] hover:text-[var(--accent)] touch:min-h-[44px]"
          >
            <i className="h-2 w-2 rounded-sm" style={{ background: districtColor(doc, d) }} />
            <span className="overflow-hidden text-ellipsis whitespace-nowrap font-mono">{doc.F[i].split("/").slice(1).join("/")}</span>
            <span className="font-mono text-[var(--dim)]">{LOC(doc, i)} lines</span>
          </button>
        ))}
      </div>
    </div>
  );
}

// The count is an IDE fact. The number of DISTRICTS it crosses is the one
// the map can tell you and the editor cannot: a change contained in one
// concern is a different risk from a change that reaches five.
export function BlastLine({ blast, doc }: { blast: ReturnType<typeof computeBlast>; doc: MapDocument }) {
  if (!blast) return null;
  const n = blast.files.length;
  const d = blast.districts.size;
  const names = [...blast.districts].map((x) => doc.names[String(x)]).slice(0, 4);
  return (
    // The --hot rule is deliberate: it keys this line to the blast radius,
    // which the renderer draws in --hot (the same reason LinkLegend reads
    // --link-out/--link-in). Only the rule, not the text: --hot is a map
    // stroke colour and measures 3.4:1 as text on the dark chrome.
    <div className="my-2 border-l-2 border-[var(--hot)] py-1 pl-2.5 text-meta">
      <b className="font-mono text-[var(--on)]">{n}</b> file{n === 1 ? "" : "s"} reference it, across{" "}
      <b className="font-mono text-[var(--on)]">{d}</b> district{d === 1 ? "" : "s"}
      <span className="block text-meta text-[var(--dim)]">
        {names.join(" · ")}
        {blast.districts.size > 4 ? " …" : ""}
      </span>
    </div>
  );
}

// A file's contents belong in a directory, not on the map: you navigate to
// the building, then read the board. The bar shows what share of the file
// each symbol occupies, which is the one spatial fact worth carrying up.
export function SymbolDirectory({
  doc,
  i,
  sy,
  cur,
  onSelectSymbol,
  compactRows = false,
  touchTargets = false,
}: {
  doc: MapDocument;
  i: number;
  sy: SymbolRow[];
  cur: number | null;
  onSelectSymbol: Props["onSelectSymbol"];
  compactRows?: boolean;
  touchTargets?: boolean;
}) {
  if (!sy.length) return null;
  const loc = LOC(doc, i) || 1;
  const used = sy.reduce((a, sm) => a + Math.max(0, sm[3] - sm[2] + 1), 0);
  const usedShare = Math.min(1, used / loc);
  const rest = Math.max(0, 1 - usedShare);
  const rows = sy.map((sm, n) => ({ sm, n, span: sm[3] - sm[2] + 1 })).sort((a, b) => b.span - a.span);
  const shown = rows.slice(0, 9);
  return (
    <>
      <div
        className="my-2 flex h-[7px] gap-px overflow-hidden rounded-sm"
        title={`${Math.round(usedShare * 100)}% of the file's lines sit inside one of its symbols`}
      >
        {sy.map((sm, n) => {
          const share = Math.max(0, sm[3] - sm[2] + 1) / loc;
          if (share <= 0.012) return null;
          return <i key={n} style={{ flex: share, background: KCOL[sm[1]] }} />;
        })}
        {rest > 0.012 && <i style={{ flex: rest, background: "var(--rule)" }} />}
      </div>
      {/* Ties the otherwise-unlabelled bar above to the one number it draws
          (comment above): what share of the file's lines the bar's coloured
          run represents. */}
      <p className="mb-1.5 text-meta text-[var(--dim)]">
        <span className="font-mono text-[var(--on)]">{Math.round(usedShare * 100)}%</span> of the file is inside a symbol
      </p>
      <div className="mb-0.5 max-h-[148px] overflow-y-auto">
        {shown.map((r) => (
          <button
            key={r.n}
            data-symbol-row={`${i}:${r.n}`}
            onClick={() => onSelectSymbol(i, r.n)}
            className={`grid w-full grid-cols-[8px_1fr_auto] items-center gap-1.5 border-t border-[var(--rule)] py-1 text-left font-mono text-meta first:border-t-0 ${compactRows ? "min-h-[34px]" : touchTargets ? "min-h-[44px]" : ""} ${cur === r.n ? "text-[var(--accent)]" : "text-[var(--on)]"}`}
          >
            <i className="h-2 w-2 rounded-sm" style={{ background: KCOL[r.sm[1]] }} />
            <span className="overflow-hidden text-ellipsis whitespace-nowrap">{r.sm[0]}</span>
            <span className="text-[var(--dim)]">
              {r.sm[2]}–{r.sm[3]}
            </span>
          </button>
        ))}
      </div>
      {rows.length > shown.length && (
        <div className="pt-0.5 text-meta text-[var(--dim)]">+{rows.length - shown.length} more symbols</div>
      )}
    </>
  );
}

// #103 build item 2: "the class card" -- extends/implements/subclasses/
// implemented-by/overridden-by, each a short tappable list, plus an
// "abstract" tag. Renders nothing for a plain function/type/const with no
// abstract flag and no hierarchy relations at all (a file card selecting an
// ordinary method, say) -- the same silent-degrade rule every other optional
// section in this file follows.
function RelationLine({
  label,
  items,
  dataKey,
  onSelectHierSymbol,
  max = 6,
}: {
  label: string;
  items: HierarchyRelation[];
  dataKey: string;
  onSelectHierSymbol: Props["onSelectHierSymbol"];
  max?: number;
}) {
  if (items.length === 0) return null;
  const shown = items.slice(0, max);
  const extra = items.length - shown.length;
  return (
    <p className="mt-1 text-meta" data-symbol-relation={dataKey}>
      <span className="text-[var(--dim)]">{label}: </span>
      {shown.map((it, idx) => (
        <span key={it.global}>
          {idx > 0 ? ", " : ""}
          <button
            type="button"
            data-relation-target={it.global}
            className="font-mono text-[var(--on)] underline decoration-[var(--rule)] underline-offset-2 hover:text-[var(--accent)]"
            onClick={() => onSelectHierSymbol(it.global)}
          >
            {it.name}
          </button>
        </span>
      ))}
      {extra > 0 && <span className="text-[var(--dim)]"> +{extra} more</span>}
    </p>
  );
}

export function SymbolRelationsCard({
  decoded,
  global,
  onSelectHierSymbol,
}: {
  decoded: DecodedDistrictSymbols;
  global: number;
  onSelectHierSymbol: Props["onSelectHierSymbol"];
}) {
  const local = decoded.globalToLocal.get(global);
  const info = useMemo(() => (local != null ? symbolHierarchy(decoded, local) : null), [decoded, local]);
  if (local == null || !info) return null;
  const row = decoded.raw.symbols[local];
  const kind = rowKind(row);
  const abstract = rowAbstract(row);
  const isClassLike = kind === KIND_CLASS || kind === KIND_INTERFACE;
  const isAbstractMethod = kind === KIND_METHOD && abstract;
  const hasRelations =
    (isClassLike && (info.extends.length > 0 || info.implements.length > 0 || info.subclasses.length > 0 || info.implementedBy.length > 0)) ||
    (isAbstractMethod && info.overriddenBy.length > 0);
  if (!abstract && !hasRelations) return null;

  return (
    <div className="mt-1.5" data-symbol-relations>
      {abstract && (
        <span
          className="inline-block rounded-[6px] bg-[var(--chrome2)] px-1.5 py-0.5 text-label uppercase text-[var(--dim)]"
          data-abstract-tag
        >
          abstract
        </span>
      )}
      {isClassLike && (
        <>
          <RelationLine label="extends" items={info.extends} dataKey="extends" onSelectHierSymbol={onSelectHierSymbol} />
          <RelationLine label="implements" items={info.implements} dataKey="implements" onSelectHierSymbol={onSelectHierSymbol} />
          <RelationLine label={`subclasses (${info.subclasses.length})`} items={info.subclasses} dataKey="subclasses" onSelectHierSymbol={onSelectHierSymbol} />
          <RelationLine label={`implemented by (${info.implementedBy.length})`} items={info.implementedBy} dataKey="implemented-by" onSelectHierSymbol={onSelectHierSymbol} />
        </>
      )}
      {isAbstractMethod && (
        <RelationLine label={`overridden by (${info.overriddenBy.length})`} items={info.overriddenBy} dataKey="overridden-by" onSelectHierSymbol={onSelectHierSymbol} />
      )}
    </div>
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
export function HierOutline({
  outline,
  external,
  selHSym,
  onSelectHierSymbol,
  onHoverHierSymbol,
  nested = true,
  compactRows = false,
}: {
  outline: OutlineRow[];
  external: ExternalRefGroup[];
  selHSym: number | null;
  onSelectHierSymbol: Props["onSelectHierSymbol"];
  onHoverHierSymbol: Props["onHoverHierSymbol"];
  /** false: the phone sheet at Full -- no inner scrollers (docs/UX.md
   * §3.1) and 44 px rows (§8.2). */
  nested?: boolean;
  /** Desktop panel symbols use the prototype's denser 34 px rows. */
  compactRows?: boolean;
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
        className={`grid w-full grid-cols-[1fr_auto] items-center gap-1.5 border-t border-[var(--rule)] py-1 text-left font-mono text-meta first:border-t-0 ${compactRows ? "min-h-[34px]" : nested ? "" : "min-h-[44px] text-small"} ${selHSym === r.global ? "text-[var(--accent)]" : "text-[var(--on)]"}`}
      >
        <span className={`overflow-hidden text-ellipsis whitespace-nowrap ${isBoldKind(r.row[2]) ? "font-semibold" : ""}`}>
          {symbolLabel(r.row, r.children.length, false)}
          {/* #103 build item 3: a class/interface's own direct bases, dimly,
              right after its name -- "Migration ‹ BaseMigration›". Never for
              anything without at least one in-repo base (fileOutline's
              `bases` is empty otherwise, including an out-of-repo-only
              base -- see its own comment). */}
          {r.bases.length > 0 && (
            <span className="font-normal italic text-[var(--dim)]" data-outline-bases>
              {" "}‹ {r.bases.join(", ")}›
            </span>
          )}
          {rowAbstract(r.row) && (
            <span className="ml-1 font-sans text-label uppercase text-[var(--dim)]" data-outline-abstract>
              abstract
            </span>
          )}
        </span>
        <span className="text-[var(--dim)]" title="incoming references">
          {r.refsIn > 0 ? `← ${r.refsIn}` : ""}
        </span>
      </button>
      {r.children.map((c) => row(c, depth + 1))}
    </div>
  );

  return (
    <div className="mt-2 border-t border-[var(--rule)] pt-1.5">
      <div className="mb-0.5 text-label uppercase text-[var(--dim)]">outline</div>
      {outline.length > 0 ? (
        <div className={nested ? "mb-1.5 max-h-[180px] overflow-y-auto" : "mb-1.5"} data-outline-tree>
          {outline.map((r) => row(r, 0))}
        </div>
      ) : (
        <p className="mb-1.5 text-meta text-[var(--dim)]">no top-level symbols</p>
      )}
      {external.length > 0 && (
        <>
          <div className="mb-0.5 text-label uppercase text-[var(--dim)]">external references</div>
          <div className={nested ? "mb-1.5 max-h-[140px] overflow-y-auto" : "mb-1.5"}>
            {external.map((group) => (
              <div key={group.from.global} className="border-t border-[var(--rule)] py-1 font-mono text-meta first:border-t-0">
                <div className="font-semibold text-[var(--on)]">{symbolLabel(group.from.row, group.from.children.length, false)}</div>
                {group.targets.slice(0, 8).map((t) => (
                  <div key={t.key} className="flex justify-between gap-2 text-[var(--dim)]">
                    <span className="overflow-hidden text-ellipsis whitespace-nowrap">{t.label}</span>
                    <span>{t.count}</span>
                  </div>
                ))}
                {group.targets.length > 8 && (
                  <div className="font-sans text-[var(--dim)]">+{group.targets.length - 8} more</div>
                )}
              </div>
            ))}
          </div>
        </>
      )}
      {/* finding 30: resolution is a lower bound -- said here, plainly, once
          per file card that has any symbol data at all, not buried in docs. */}
      <p className="text-meta text-[var(--dim)]">
        Method references are under-counted: calls through instances and inherited methods aren&apos;t resolved.
      </p>
    </div>
  );
}
