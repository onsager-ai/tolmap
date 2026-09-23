import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { useCatalogue, useMapDocument } from "@/data/queries";
import { MapCanvas, type MapCanvasHandle } from "@/map/MapCanvas";
import type { MapRendererCallbacks } from "@/map/MapRenderer";
import { buildAdj, findRoute, type Route } from "@/map/graph";
import { D_, districtClass } from "@/map/geometry";
import type { SearchHit } from "@/map/search";
import { TopBar } from "@/components/TopBar";
import { Sidebar } from "@/components/Sidebar";
import { SearchBox } from "@/components/SearchBox";
import { SelectionPanel } from "@/components/SelectionPanel";
import { SelectionSummaryBar } from "@/components/SelectionSummaryBar";
import { RouteBox } from "@/components/RouteBox";
import { FooterStats } from "@/components/FooterStats";
import { ZoomControls } from "@/components/ZoomControls";
import { PackageLegend } from "@/components/PackageLegend";
import { buildPackageLayout } from "@/map/packageLayout";
import type { MapSearch } from "@/routes/search";

/** The /:owner/:repo page: assembles chrome (TopBar, Sidebar, SearchBox,
 * SelectionPanel, RouteBox, FooterStats, ZoomControls) around one MapCanvas.
 * This component owns every piece of application state — selection, geo,
 * layer, route — either directly or via the URL; MapCanvas/MapRenderer only
 * ever receive it as props and report gestures back through callbacks. */
export function MapView() {
  const { owner, repo } = useParams({ strict: false }) as { owner: string; repo: string };
  const search = useSearch({ strict: false }) as MapSearch;
  const navigate = useNavigate();
  const { data: catalogue } = useCatalogue();
  const { data: doc, isLoading, isError, error } = useMapDocument(owner, repo);

  const canvasRef = useRef<MapCanvasHandle>(null);
  const mapAreaRef = useRef<HTMLDivElement>(null);
  const [panelOpen, setPanelOpen] = useState(false);
  const [sideOpen, setSideOpen] = useState(false);
  const [routeFrom, setRouteFrom] = useState<number | null>(null);
  const [route, setRoute] = useState<Route | null>(null);
  const [unconnectedRepo, setUnconnectedRepo] = useState<string | null>(null);
  const [previewDirectory, setPreviewDirectory] = useState<{ repo: string; path: string } | null>(null);
  const packageLayout = useMemo(() => (doc ? buildPackageLayout(doc) : null), [doc]);

  // Issue #82 A1 scope item 5 (fullscreen). Fullscreened element is the map
  // AREA (below), not the whole page: TopBar and Sidebar are its siblings,
  // so the Fullscreen API path hides them simply by not being part of what
  // the browser paints while fullscreen is active -- no separate
  // show/hide logic needed for that path. The CSS fallback (no element
  // Fullscreen API -- iOS Safari) instead covers them with `fixed inset-0`,
  // the same technique the prototype's own `.mapbox.fs` used (arch20 -- see
  // its setFS/reAspect); either way `isFullscreen` is the one source of
  // truth the JSX below reads, so React chrome and the CSS class agree.
  const [isFullscreen, setIsFullscreen] = useState(false);

  useEffect(() => {
    function onFsChange() {
      setIsFullscreen(!!document.fullscreenElement && document.fullscreenElement === mapAreaRef.current);
    }
    document.addEventListener("fullscreenchange", onFsChange);
    return () => document.removeEventListener("fullscreenchange", onFsChange);
  }, []);

  async function exitFullscreen() {
    if (document.fullscreenElement) {
      try {
        await document.exitFullscreen();
      } catch {
        /* already exiting, or the browser refused -- state is set below either way */
      }
    }
    setIsFullscreen(false);
  }
  async function enterFullscreen() {
    const el = mapAreaRef.current;
    if (el?.requestFullscreen) {
      try {
        await el.requestFullscreen();
        return; // onFsChange (above) flips isFullscreen once the browser confirms
      } catch {
        // iPhone Safari has no element Fullscreen API at all (requestFullscreen
        // is undefined there, so this branch is never reached on it) -- this
        // catch is for a DESKTOP browser that has the API but refuses the
        // call (e.g. not called from a direct user gesture). Either way, fall
        // through to the CSS-only mode rather than doing nothing.
      }
    }
    setIsFullscreen(true);
  }
  function toggleFullscreen() {
    if (isFullscreen) void exitFullscreen();
    else void enterFullscreen();
  }
  // Esc: the native path already exits via the browser and fires
  // fullscreenchange (handled above); this covers the CSS fallback, where
  // there is no native fullscreen state for Esc to exit on its own.
  // exitFullscreen()'s own guard makes calling it redundantly (native path,
  // already exiting) harmless.
  useEffect(() => {
    if (!isFullscreen) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") void exitFullscreen();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isFullscreen]);

  // radj (imported-by) is new here: SelectionPanel's "links" line and
  // MapRenderer's own selection-links feature both need it, and buildAdj
  // already computes both from one pass over doc.E -- discarding radj and
  // having MapRenderer separately rebuild it (it does, for the map surface
  // itself) would be a second identical scan of doc.E on this side too.
  const { adj, radj } = useMemo(() => (doc ? buildAdj(doc) : { adj: new Map(), radj: new Map() }), [doc]);
  const { maxCh, maxCx } = useMemo(() => {
    if (!doc) return { maxCh: 1, maxCx: 1 };
    return {
      maxCh: Math.max(1, ...doc.N.map((r) => r[5])),
      maxCx: Math.max(1, ...doc.N.map((r) => r[4])),
    };
  }, [doc]);

  // Desktop used to auto-select the top landmark on load (the reference did
  // this too: `if(R.L.length && !NARROW()) select(R.L[0][0],true)`), landing
  // every fresh /:owner/:repo view with something already highlighted; a
  // phone never did (that was the whole point of the NARROW() check).
  // Removed (review finding on the readable-overview PR): once selection
  // started dimming non-neighbour files (that PR's own change), the
  // auto-select made a FRESH desktop load of django/django open with 685 of
  // 851 dots dimmed -- the overview auto-obstructing itself before a reader
  // had asked for anything. The reference could afford landing on a
  // landmark because selection had no visual cost there; it does now, so
  // desktop opens unobstructed the same as phone always has. A `?file=`
  // deep link is unaffected -- it's an explicit selection, and dimming is
  // the correct, requested behaviour for one.
  const sel = doc && search.file ? (() => { const i = doc.F.indexOf(search.file!); return i >= 0 ? i : null; })() : null;
  const selSym = sel != null && search.sym != null ? search.sym : null;
  const selD = sel == null && search.d != null ? search.d : null;

  function updateSearch(patch: Partial<typeof search>) {
    navigate({
      to: ".",
      search: (prev: Record<string, unknown>) => ({ ...prev, sel: undefined, ...patch }),
      replace: true,
    });
  }

  // Issue #82 A1 scope item 1 ("selecting never moves the map"): there is no
  // more `fly` option. Every selection path -- a map tap, a sidebar pick, a
  // search result, a deep link, a breadcrumb segment -- goes through
  // panTo/panToDistrict now (MapRenderer's own pan-only methods), which are
  // no-ops when the target is already on screen. A map tap's target is
  // always already on screen by construction, so calling panTo
  // unconditionally here is exactly equivalent to the old "fly: false" for
  // that case, and correctly brings an off-screen sidebar/search/deep-link
  // target into view (at the CURRENT zoom) for the others -- one function,
  // no flag to thread through every caller.
  function selectFile(i: number, opts: { symbol?: number } = {}) {
    if (!doc) return;
    setUnconnectedRepo(null);
    updateSearch({ file: doc.F[i], sym: opts.symbol, d: undefined, dir: undefined });
    setPanelOpen(true);
    setSideOpen(false);
    if (districtClass(doc.districts[String(doc.N[i][0])]) !== "unconnected") canvasRef.current?.panTo(i);
  }
  // selectDistrict deliberately never pans on its own: it's reused by the
  // map's own district-polygon tap (already on screen), by SelectionPanel's
  // internal "near" neighbour and breadcrumb links (selecting a level that
  // was already reachable from the current card, so there's nothing new to
  // bring into view), and by the breadcrumb's own district segment, all of
  // which the spec requires to move the view NOT AT ALL, not "pan if
  // needed." The sidebar's district row is the one caller that DOES need
  // panning (its target can be genuinely off screen) -- see the dedicated
  // wrapper passed to <Sidebar> below, which calls this and then pans.
  function selectDistrict(d: number) {
    setUnconnectedRepo(null);
    updateSearch({ file: undefined, sym: undefined, d, dir: undefined });
    setPanelOpen(true);
    setSideOpen(false);
  }
  // Jumps straight to "nothing selected" -- the breadcrumb's own repo
  // segment, and an explicit clear. Distinct from stepBackSelection below,
  // which is one level at a time.
  function clearAll() {
    setUnconnectedRepo(null);
    updateSearch({ file: undefined, sym: undefined, d: undefined, dir: undefined });
    setPanelOpen(false);
  }
  // Issue #82 A1 scope item 2: an empty map tap used to clear the whole
  // selection in one step; now it steps back exactly one level --
  // symbol -> its file -> the file's district -> nothing -- matching the
  // prototype's own click handler (arch20.body.html's plain
  // `svg.addEventListener("click", ...)"`, not the file-hit one). A
  // directory highlight and the unconnected-files view aren't part of that
  // repo/district/file/symbol hierarchy, but each still reduces to "select
  // one level up" the same way: dropped in one tap, panel closed on the
  // step that reaches "nothing." An unconnected file's own "district" isn't
  // a place shown anywhere else in the UI (Sidebar never lists it, see
  // districtClass's "unconnected" branch), so stepping back from one skips
  // the district level entirely rather than landing on a card nothing else
  // can reach.
  function stepBackSelection() {
    if (!doc) return;
    if (selSym != null) {
      updateSearch({ sym: undefined });
      return;
    }
    if (sel != null) {
      const d = D_(doc, sel);
      if (districtClass(doc.districts[String(d)]) === "unconnected") clearAll();
      else updateSearch({ file: undefined, sym: undefined, d, dir: undefined });
      return;
    }
    if (selD != null) {
      clearAll();
      return;
    }
    if (search.dir) {
      updateSearch({ dir: undefined });
      setPanelOpen(false);
      return;
    }
    if (unconnectedRepo) {
      setUnconnectedRepo(null);
      setPanelOpen(false);
      return;
    }
    // Already at "nothing": matches the prototype's own `else return;`.
  }

  function selectDirectory(path?: string) {
    setUnconnectedRepo(null);
    setPreviewDirectory(null);
    setRoute(null);
    setRouteFrom(null);
    updateSearch({ file: undefined, sym: undefined, d: undefined, dir: path });
    setPanelOpen(true);
    setSideOpen(false);
  }

  const rendererCallbacks: MapRendererCallbacks = {
    onSelectFile: (i) => selectFile(i),
    onSelectSymbol: (i, s) => selectFile(i, { symbol: s }),
    onSelectDistrict: (d) => selectDistrict(d),
    onClearSelection: () => stepBackSelection(),
    onSelectDirectory: (path) => selectDirectory(path),
    onPreviewDirectory: (path) => setPreviewDirectory(path && doc ? { repo: doc.repo, path } : null),
  };

  if (isLoading) {
    return (
      <div className="flex h-full items-center justify-center bg-[var(--chrome)] text-sm text-[var(--dim)]">
        loading {owner}/{repo}…
      </div>
    );
  }
  if (isError || !doc || !packageLayout) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 bg-[var(--chrome)] p-6 text-center text-sm text-[var(--hot)]">
        <p>couldn't load {owner}/{repo}.</p>
        <p className="text-[var(--dim)]">{error instanceof Error ? error.message : "not in the catalogue"}</p>
      </div>
    );
  }

  const packageDepth = Math.max(
    packageLayout.minDepth,
    Math.min(packageLayout.maxDepth, search.depth ?? packageLayout.autoDepth),
  );
  const packageGrouping = packageLayout.groupings.get(packageDepth)!;
  const activeDirectory = search.dir && packageLayout.filesByDirectory.has(search.dir) ? search.dir : undefined;
  const previewPath = previewDirectory?.repo === doc.repo ? previewDirectory.path : undefined;
  const highlightedPath = previewPath ?? activeDirectory;
  const folderFiles = highlightedPath ? (packageLayout.filesByDirectory.get(highlightedPath) ?? null) : null;
  const folderOnlyIslands = highlightedPath ? packageLayout.islandOnlyDirectories.has(highlightedPath) : false;

  return (
    <div className="flex h-full flex-col">
      {!isFullscreen && (
        <TopBar
          catalogue={catalogue}
          owner={owner}
          repo={repo}
          layer={search.layer}
          onLayer={(l) => updateSearch({ layer: l })}
        />
      )}
      <div className="relative flex min-h-0 flex-1">
        {!isFullscreen && (
          <Sidebar
            doc={doc}
            open={sideOpen}
            onToggleOpen={() => setSideOpen((v) => !v)}
            onPickLandmark={(i) => {
              setSideOpen(false);
              selectFile(i);
            }}
            onPickHub={(i) => {
              setSideOpen(false);
              // A4 (hubs, issue #82): "tapping a row selects the file (no view
              // change)". Post-A1, selectFile() itself is now pan-only for
              // EVERY caller (search, landmarks, hubs alike) -- the old
              // per-caller `fly`/no-fly distinction this comment used to
              // describe was A1's own unification target, so this is just
              // the same call onPickLandmark makes above.
              selectFile(i);
            }}
            onSelectDistrict={(d) => {
              setSideOpen(false);
              // Issue #82 A1: a sidebar row now SELECTS the district (both
              // mainland and island rows alike -- the old mainland-only
              // "fly, don't select" affordance from issue #63 is gone, since
              // this issue's own spec explicitly lists "the sidebar
              // (district rows...)" as a place selecting must never zoom)
              // and pans to it if it's off screen, instead of always
              // zooming in. selectDistrict() itself stays pan-free (it's
              // shared with map taps and the breadcrumb, which must never
              // move the view at all); this wrapper is the one place that
              // adds the pan, because a sidebar target genuinely can be off
              // screen.
              selectDistrict(d);
              canvasRef.current?.panToDistrict(d);
            }}
          />
        )}
        <div
          ref={mapAreaRef}
          className={`relative min-w-0 flex-1 bg-[var(--canvas)]${isFullscreen ? " fixed inset-0 z-50" : ""}`}
        >
          <MapCanvas
            doc={doc}
            geo={search.geo}
            layer={search.layer}
            sel={sel}
            selSym={selSym}
            selD={selD}
            route={route}
            packageGrouping={packageGrouping}
            folderFiles={folderFiles}
            folderOnlyIslands={folderOnlyIslands}
            folderLabels={packageLayout.folderLabels}
            activeDirectory={activeDirectory}
            callbacks={rendererCallbacks}
            handleRef={canvasRef}
          />
          <SearchBox
            doc={doc}
            onPick={(hit: SearchHit) => selectFile(hit.i, hit.s != null ? { symbol: hit.s } : {})}
          />
          {isFullscreen ? (
            <SelectionSummaryBar
              doc={doc}
              sel={sel}
              selSym={selSym}
              selD={selD}
              adj={adj}
              radj={radj}
              onDetails={() => {
                void exitFullscreen();
                setPanelOpen(true);
              }}
            />
          ) : (
            <SelectionPanel
              doc={doc}
              sel={sel}
              selSym={selSym}
              selD={selD}
              adj={adj}
              radj={radj}
              packageLayout={packageLayout}
              activeDirectory={activeDirectory}
              showUnconnected={unconnectedRepo === doc.repo && sel == null && selD == null}
              open={panelOpen}
              onToggleOpen={() => setPanelOpen((v) => !v)}
              onSelectFile={(i) => selectFile(i)}
              onSelectSymbol={(i, s) => selectFile(i, { symbol: s })}
              onSelectDistrict={selectDistrict}
              onZoomDistrict={(d) => canvasRef.current?.zoomDistrict(d)}
              onRouteFrom={(i) => setRouteFrom(i)}
              onRouteTo={(i) => {
                if (routeFrom == null) {
                  setRouteFrom(i);
                  return;
                }
                setRoute(findRoute(doc, adj, routeFrom, i));
              }}
              onSelectDirectory={selectDirectory}
              onBreadcrumbRepo={clearAll}
              onBreadcrumbFile={(i) => updateSearch({ file: doc.F[i], sym: undefined, d: undefined, dir: undefined })}
            />
          )}
          {!isFullscreen && search.layer === "p" && (
            <PackageLegend
              grouping={packageGrouping}
              auto={search.depth == null}
              minDepth={packageLayout.minDepth}
              maxDepth={packageLayout.maxDepth}
              onDepth={(depth) => updateSearch({ depth: depth === packageLayout.autoDepth ? undefined : depth })}
            />
          )}
          {!isFullscreen && (
            <FooterStats doc={doc} layer={search.layer} maxCh={maxCh} maxCx={maxCx}
              unconnectedCount={packageLayout.unconnectedFiles.length}
              mobileHidden={panelOpen}
              onOpenUnconnected={() => { updateSearch({ file: undefined, sym: undefined, d: undefined, dir: undefined }); setUnconnectedRepo(doc.repo); setPanelOpen(true); setSideOpen(false); }} />
          )}
          <RouteBox
            doc={doc}
            routeFrom={routeFrom}
            route={route}
            onClear={() => {
              setRoute(null);
              setRouteFrom(null);
            }}
          />
          <ZoomControls
            onZoomIn={() => canvasRef.current?.zoomBy(1.6)}
            onZoomOut={() => canvasRef.current?.zoomBy(1 / 1.6)}
            onFit={() => canvasRef.current?.fit(true)}
            isFullscreen={isFullscreen}
            onToggleFullscreen={toggleFullscreen}
          />
        </div>
      </div>
    </div>
  );
}
