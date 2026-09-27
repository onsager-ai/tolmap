import { useEffect, useRef } from "react";
import { useNavigate } from "@tanstack/react-router";
import type { CatalogueEntry, DistrictSymbols, MapDocument } from "@/types";
import type { Layer } from "@/map/constants";
import type { AdjMap, Route } from "@/map/graph";
import { RAMP_STOPS } from "@/map/geometry";
import type { PackageGrouping, PackageLayout } from "@/map/packageLayout";
import type { SearchPick } from "@/map/searchResults";
import type { Detent, DetentHeights } from "@/map/phoneShell";
import { SearchBox } from "@/components/SearchBox";
import { useVisualViewportBox } from "@/hooks/useVisualViewportBox";
import { PackageLegend } from "@/components/PackageLegend";
import { ThemeSegmented } from "@/components/ThemeToggle";
import { SIDE_SHEET_WIDTH_PX } from "@/map/layoutProfile";
import { BottomSheet, SideSheet } from "./BottomSheet";
import { CloseIcon, FitIcon, LayersIcon, MinusIcon, PanelIcon, PlusIcon, SearchIcon, SwitchIcon } from "./icons";
import { OverviewCard, SelectionCard, type PathPick, type StructureCardState } from "./SheetCards";

export interface PhoneChromeProps {
  doc: MapDocument;
  packageLayout: PackageLayout;
  packageGrouping: PackageGrouping;
  catalogue: CatalogueEntry[] | undefined;
  owner: string;
  repo: string;
  layer: Layer;
  depthAuto: boolean;
  maxCh: number;
  maxCx: number;
  sel: number | null;
  selSym: number | null;
  selD: number | null;
  selHSym: number | null;
  symbolsDoc?: DistrictSymbols;
  symbolsLoading: boolean;
  adj: AdjMap;
  radj: AdjMap;
  activeDirectory?: string;

  /** docs/UX.md §9 phone landscape: the sheet is a 360 px side sheet on the
   * left, full height; the pill and the control column sit over the map
   * beside it. */
  landscape: boolean;
  /** Landscape: the side sheet is open (360) or collapsed (0). */
  sideOpen: boolean;
  onToggleSide(): void;
  heights: DetentHeights;
  /** The visible viewport height (map/phoneShell.ts's PhoneMetrics). */
  viewportHeight: number;
  /** env(safe-area-inset-top), measured. */
  safeTop: number;
  detent: Detent;
  onDetent(d: Detent): void;
  searchOpen: boolean;
  onOpenSearch(): void;
  onCloseSearch(): void;
  onSearchPick(pick: SearchPick): void;
  layersOpen: boolean;
  onOpenLayers(): void;
  onCloseLayers(): void;
  quality: boolean;
  onQuality(open: boolean): void;
  indexTab: "districts" | "folders";
  onIndexTab(t: "districts" | "folders"): void;
  structure: StructureCardState | null;
  onCloseStructure(): void;
  pathPick: PathPick | null;
  pathEnds: [number, number] | null;
  route: Route | null;
  onPath(i: number, dir: "from" | "to"): void;
  onPathCancel(): void;

  onZoomIn(): void;
  onZoomOut(): void;
  onFit(): void;
  onLayer(l: Layer): void;
  onDepth(depth: number): void;

  onClearSelection(): void;
  onSelectFile(i: number): void;
  onSelectSymbol(i: number, s: number): void;
  onSelectHierSymbol(global: number): void;
  /** Pan-free: breadcrumb and "near" links (issue #82 A1). */
  onSelectDistrict(d: number): void;
  /** A district-index row: selects and pans it into the safe rect. */
  onPickDistrict(d: number): void;
  onPickKeyFile(i: number): void;
  onZoomDistrict(d: number): void;
  onSelectDirectory(path?: string): void;
  onBreadcrumbRepo(): void;
  onBreadcrumbFile(i: number): void;
}

/** Four 44 px buttons and the column's border. */
const CONTROL_COLUMN_PX = 4 * 44 + 2;

const LAYERS: { id: Layer; name: string; meaning: string }[] = [
  { id: "d", name: "District", meaning: "One colour per district" },
  { id: "c", name: "Churn", meaning: "How often each file changed" },
  { id: "x", name: "Complexity", meaning: "Branch points in each file" },
  { id: "p", name: "Package", meaning: "Folders, drawn over the districts" },
];

/** docs/UX.md §3: the phone shell -- the search pill, the control column,
 * the one bottom sheet, the Layers sheet and the search layer. Everything
 * here floats over the full-screen map; nothing else does (§3: the path,
 * legends, chips and road cards are sheet content). The same component
 * serves a phone held sideways (§9): the sheet becomes a side sheet and the
 * pill and control column move over the map beside it. The desktop and
 * tablet layout is MapView's (top bar, rail, inspector), hosting the same
 * cards (SheetCards.tsx's SelectionCard). State lives in MapView, which
 * also owns the back stack (map/backStack.ts). */
export function PhoneChrome(p: PhoneChromeProps) {
  const navigate = useNavigate();
  const slug = `${p.owner}/${p.repo}`;
  const searchBox = useVisualViewportBox(p.searchOpen);
  // §7.2: closing search returns focus to what opened it, the pill's search
  // button (a person on a keyboard or a screen reader lands where they were).
  const searchWasOpen = useRef(p.searchOpen);
  useEffect(() => {
    if (searchWasOpen.current && !p.searchOpen) {
      document.querySelector<HTMLElement>("[data-open-search]")?.focus({ preventScroll: true });
    }
    searchWasOpen.current = p.searchOpen;
  }, [p.searchOpen]);
  // TopBar's fix, kept: the current repo always has an option, or a native
  // select silently shows a stale one.
  const options = (p.catalogue ?? []).map((m) => m.slug);
  const withCurrent = options.includes(slug) ? options : [slug, ...options];

  const content = (
    <SelectionCard
      doc={p.doc}
      packageLayout={p.packageLayout}
      sel={p.sel}
      selSym={p.selSym}
      selD={p.selD}
      selHSym={p.selHSym}
      symbolsDoc={p.symbolsDoc}
      symbolsLoading={p.symbolsLoading}
      adj={p.adj}
      radj={p.radj}
      detent={p.detent}
      onDetent={p.onDetent}
      quality={p.quality}
      onCloseQuality={() => p.onQuality(false)}
      structure={p.structure}
      onCloseStructure={p.onCloseStructure}
      pathPick={p.pathPick}
      pathEnds={p.pathEnds}
      route={p.route}
      onPath={p.onPath}
      onPathCancel={p.onPathCancel}
      onClearSelection={p.onClearSelection}
      onSelectFile={p.onSelectFile}
      onSelectSymbol={p.onSelectSymbol}
      onSelectHierSymbol={p.onSelectHierSymbol}
      onSelectDistrict={p.onSelectDistrict}
      onZoomDistrict={p.onZoomDistrict}
      onSelectDirectory={p.onSelectDirectory}
      onBreadcrumbRepo={p.onBreadcrumbRepo}
      onBreadcrumbFile={p.onBreadcrumbFile}
      // Landscape: the side sheet already shows the whole card (§9), so
      // there is no "Details" to raise it to.
      onDistrictDetails={p.landscape ? undefined : () => p.onDetent("half")}
    />
  );
  const overview = (
    <OverviewCard
      doc={p.doc}
      slug={slug}
      packageLayout={p.packageLayout}
      activeDirectory={p.activeDirectory}
      tab={p.indexTab}
      onTab={p.onIndexTab}
      onOpenQuality={() => p.onQuality(true)}
      onSelectDistrict={p.onPickDistrict}
      onPickKeyFile={p.onPickKeyFile}
      onSelectDirectory={p.onSelectDirectory}
    />
  );
  const somethingShown = !!p.pathPick || !!p.structure || p.quality || p.sel != null || p.selD != null;
  const sheetContent = somethingShown ? content : overview;
  // §9: in landscape the pill and the control column float over the map
  // beside the side sheet (the sheet's width plus the left safe inset when
  // open; the left inset alone when collapsed).
  const mapLeft = p.landscape && p.sideOpen ? `${SIDE_SHEET_WIDTH_PX}px + env(safe-area-inset-left, 0px)` : "env(safe-area-inset-left, 0px)";

  const controlBtn = "flex h-11 w-11 items-center justify-center border-b border-[var(--rule)] text-[var(--on)] last:border-b-0";
  return (
    <>
      {/* §3: the search pill -- search and the repository, one 48 px row. */}
      <div
        data-search-pill
        className="absolute z-20 flex h-12 items-center gap-0.5 rounded-[24px] border border-[var(--rule)] bg-[var(--chrome2)] px-0.5 shadow-[0_6px_18px_rgba(0,0,0,.25)]"
        style={{
          top: "calc(12px + env(safe-area-inset-top, 0px))",
          left: `calc(${mapLeft} + 12px)`,
          right: "calc(12px + env(safe-area-inset-right, 0px))",
        }}
      >
        <button
          type="button"
          aria-label={`Search ${slug}`}
          data-open-search
          onClick={p.onOpenSearch}
          className="flex h-11 min-w-0 flex-1 items-center gap-2.5 px-2 text-left"
        >
          <span className="shrink-0 text-[var(--dim)]">
            <SearchIcon />
          </span>
          <span className="truncate text-body text-[var(--dim)]">
            Search <span className="font-mono text-[15px] text-[var(--on)]">{slug}</span>
          </span>
        </button>
        {/* The switch-repository button is a native select laid over the
            icon: a tap opens the phone's own picker, with no overlay of ours
            to dismiss or put on the back stack. */}
        <span className="relative flex h-11 w-11 shrink-0 items-center justify-center rounded-full text-[var(--on)]" data-switch-repo>
          <SwitchIcon />
          <select
            aria-label="Repository"
            title="Switch repository"
            value={slug}
            onChange={(e) => {
              const [o, r] = e.target.value.split("/");
              navigate({ to: "/$owner/$repo", params: { owner: o, repo: r }, search: { geo: "r", layer: "d" } });
            }}
            className="absolute inset-0 h-full w-full cursor-pointer appearance-none opacity-0"
          >
            {withCurrent.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
        </span>
      </div>

      {/* §3: the control column. No fullscreen button on phones. Hidden
          whenever the sheet reaches up into it (always at Full; at Half on a
          short phone) -- never in landscape, where the sheet is beside it.
          Landscape adds a fifth button that shows and hides the side sheet
          (§9: "collapsed 0, peek 360"). */}
      {(p.landscape || p.viewportHeight - p.heights[p.detent] >= p.safeTop + 68 + CONTROL_COLUMN_PX + 8) && (
        <div
          data-control-column
          className="absolute z-20 flex w-[46px] flex-col overflow-hidden rounded-[12px] border border-[var(--rule)] bg-[var(--chrome2)] shadow-[0_6px_18px_rgba(0,0,0,.25)]"
          style={{ top: "calc(68px + env(safe-area-inset-top, 0px))", right: "calc(12px + env(safe-area-inset-right, 0px))" }}
        >
          <button type="button" className={controlBtn} onClick={p.onZoomIn} aria-label="Zoom in">
            <PlusIcon />
          </button>
          <button type="button" className={controlBtn} onClick={p.onZoomOut} aria-label="Zoom out">
            <MinusIcon />
          </button>
          <button type="button" className={controlBtn} onClick={p.onFit} aria-label="Fit map">
            <FitIcon />
          </button>
          <button type="button" className={controlBtn} onClick={p.onOpenLayers} aria-label="Map layers" aria-expanded={p.layersOpen}>
            <LayersIcon />
          </button>
          {p.landscape && (
            <button
              type="button"
              className={controlBtn}
              onClick={p.onToggleSide}
              data-side-toggle
              aria-label={p.sideOpen ? "Hide the side sheet" : "Show the side sheet"}
              aria-expanded={p.sideOpen}
              style={p.sideOpen ? { color: "var(--accent)" } : undefined}
            >
              <PanelIcon />
            </button>
          )}
        </div>
      )}

      {p.landscape ? (
        <SideSheet open={p.sideOpen} detent={p.detent}>
          {sheetContent}
        </SideSheet>
      ) : (
        <BottomSheet detent={p.detent} heights={p.heights} onDetent={p.onDetent}>
          {sheetContent}
        </BottomSheet>
      )}

      {/* §4.6: the Layers sheet, modal over the map sheet. */}
      {p.layersOpen && (
        <>
          <div data-layers-scrim className="absolute inset-0 z-40 bg-[rgba(4,8,10,.55)]" onClick={p.onCloseLayers} />
          {/* §9: in landscape the Layers sheet takes the side sheet's place
              (left, 360 px, full height) rather than rising over a 390 px
              tall screen. */}
          <section
            aria-label="Map layers and display"
            data-layers-sheet
            className={`absolute z-40 overflow-y-auto border-[var(--rule)] bg-[var(--chrome)] px-5 text-[var(--on)] ${
              p.landscape
                ? "inset-y-0 left-0 border-r shadow-[8px_0_24px_rgba(0,0,0,.3)]"
                : "inset-x-0 bottom-0 max-h-[88%] rounded-t-[20px] border-t shadow-[0_-8px_24px_rgba(0,0,0,.3)]"
            }`}
            style={{
              paddingBottom: "calc(20px + env(safe-area-inset-bottom, 0px))",
              overscrollBehavior: "contain",
              ...(p.landscape
                ? {
                    width: `calc(${SIDE_SHEET_WIDTH_PX}px + env(safe-area-inset-left, 0px))`,
                    paddingLeft: "calc(20px + env(safe-area-inset-left, 0px))",
                    paddingTop: "env(safe-area-inset-top, 0px)",
                  }
                : {}),
            }}
          >
            <div className="flex justify-center">
              <span className="mb-3 mt-[11px] block h-[5px] w-9 rounded-[3px] bg-[var(--grabber)]" />
            </div>
            <div className="flex items-start gap-2">
              <h2 className="min-w-0 flex-1 text-sheet-title">Map layer</h2>
              <button
                type="button"
                aria-label="Close layers"
                onClick={p.onCloseLayers}
                className="-mr-2.5 -mt-1.5 flex h-11 w-11 shrink-0 items-center justify-center rounded-full bg-[var(--chrome2)]"
              >
                <CloseIcon />
              </button>
            </div>
            <div role="radiogroup" aria-label="Map layer" className="mt-1.5 overflow-hidden rounded-[14px] border border-[var(--rule)]">
              {LAYERS.map((l) => {
                const on = p.layer === l.id;
                return (
                  <div key={l.id} className="border-b border-[var(--rule)] last:border-b-0" style={on ? { background: "color-mix(in srgb, var(--accent) 10%, transparent)" } : undefined}>
                    <button
                      type="button"
                      role="radio"
                      aria-checked={on}
                      data-layer-option={l.id}
                      onClick={() => p.onLayer(l.id)}
                      className="flex min-h-[60px] w-full items-center gap-3 px-3.5 py-2 text-left"
                    >
                      <span
                        className="flex h-[22px] w-[22px] shrink-0 items-center justify-center rounded-full border-2"
                        style={{ borderColor: on ? "var(--accent)" : "var(--grabber)" }}
                      >
                        {on && <span className="h-2.5 w-2.5 rounded-full bg-[var(--accent)]" />}
                      </span>
                      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                        <span className="text-row">{l.name}</span>
                        <span className="text-meta text-[var(--dim)]">{l.meaning}</span>
                        {(l.id === "c" || l.id === "x") && (
                          <span className="mt-1 flex items-center gap-2" data-ramp-legend={l.id}>
                            <span
                              className="h-2 w-32 shrink-0 rounded"
                              style={{ background: `linear-gradient(90deg,${RAMP_STOPS.join(",")})` }}
                            />
                            <span className="font-mono text-meta text-[var(--dim)]">
                              {l.id === "c" ? `1 → ${p.maxCh} commits` : `0 → ${p.maxCx}`}
                            </span>
                          </span>
                        )}
                      </span>
                    </button>
                    {l.id === "p" && on && (
                      <div className="px-3.5 pb-3">
                        <PackageLegend
                          grouping={p.packageGrouping}
                          auto={p.depthAuto}
                          minDepth={p.packageLayout.minDepth}
                          maxDepth={p.packageLayout.maxDepth}
                          onDepth={p.onDepth}
                          variant="sheet"
                        />
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
            <h3 className="mt-5 text-[15px] font-semibold">Appearance</h3>
            <ThemeSegmented />
          </section>
        </>
      )}

      {/* §4.8: search, full screen and above every map control (z-50; the
          pill and control column are z-20, the sheet z-30, Layers z-40).
          Fixed to the visual viewport, so the results end at the keyboard's
          top edge. */}
      {p.searchOpen && (
        <section
          aria-label="Search"
          data-search-layer
          className="fixed inset-x-0 top-0 z-50 h-full bg-[var(--chrome)] text-[var(--on)]"
          style={searchBox ? { top: searchBox.top, height: searchBox.height } : undefined}
        >
          <SearchBox doc={p.doc} variant="overlay" onPick={p.onSearchPick} onClose={p.onCloseSearch} />
        </section>
      )}
    </>
  );
}
