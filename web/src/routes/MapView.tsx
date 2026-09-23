import { useMemo, useRef, useState } from "react";
import { useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { useCatalogue, useMapDocument } from "@/data/queries";
import { MapCanvas, type MapCanvasHandle } from "@/map/MapCanvas";
import type { MapRendererCallbacks } from "@/map/MapRenderer";
import { buildAdj, findRoute, type Route } from "@/map/graph";
import { districtClass } from "@/map/geometry";
import type { SearchHit } from "@/map/search";
import { TopBar } from "@/components/TopBar";
import { Sidebar } from "@/components/Sidebar";
import { SearchBox } from "@/components/SearchBox";
import { SelectionPanel } from "@/components/SelectionPanel";
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
  const [panelOpen, setPanelOpen] = useState(false);
  const [sideOpen, setSideOpen] = useState(false);
  const [routeFrom, setRouteFrom] = useState<number | null>(null);
  const [route, setRoute] = useState<Route | null>(null);
  const [unconnectedRepo, setUnconnectedRepo] = useState<string | null>(null);
  const [previewDirectory, setPreviewDirectory] = useState<{ repo: string; path: string } | null>(null);
  const packageLayout = useMemo(() => (doc ? buildPackageLayout(doc) : null), [doc]);

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

  function selectFile(i: number, opts: { fly?: boolean; symbol?: number } = {}) {
    if (!doc) return;
    setUnconnectedRepo(null);
    updateSearch({ file: doc.F[i], sym: opts.symbol, d: undefined, dir: undefined });
    setPanelOpen(true);
    setSideOpen(false);
    if (opts.fly !== false && districtClass(doc.districts[String(doc.N[i][0])]) !== "unconnected") canvasRef.current?.flyTo(i);
  }
  function selectSymbolDetail(i: number, s: number) {
    if (!doc) return;
    setUnconnectedRepo(null);
    updateSearch({ file: doc.F[i], sym: s, d: undefined, dir: undefined });
    setPanelOpen(true);
    setSideOpen(false);
    if (districtClass(doc.districts[String(doc.N[i][0])]) !== "unconnected") canvasRef.current?.flyToDetail(i);
  }
  function selectDistrict(d: number) {
    setUnconnectedRepo(null);
    updateSearch({ file: undefined, sym: undefined, d, dir: undefined });
    setPanelOpen(true);
    setSideOpen(false);
  }
  function clearSelection() {
    setUnconnectedRepo(null);
    updateSearch({ file: undefined, sym: undefined, d: undefined, dir: undefined });
    setPanelOpen(false);
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
    // Taps directly on the map never fly — the file is already in view.
    onSelectFile: (i) => selectFile(i, { fly: false }),
    onSelectSymbol: (i, s) => selectFile(i, { fly: false, symbol: s }),
    onSelectDistrict: (d) => selectDistrict(d),
    onClearSelection: () => clearSelection(),
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
      <TopBar
        catalogue={catalogue}
        owner={owner}
        repo={repo}
        layer={search.layer}
        onLayer={(l) => updateSearch({ layer: l })}
      />
      <div className="relative flex min-h-0 flex-1">
        <Sidebar
          doc={doc}
          open={sideOpen}
          onToggleOpen={() => setSideOpen((v) => !v)}
          onPickLandmark={(i) => {
            setSideOpen(false);
            selectFile(i, { fly: true });
          }}
          onFlyDistrict={(d) => {
            setSideOpen(false);
            // Issue #63: an island row also SELECTS the district -- selecting is
            // what exempts an island from the fade (MapRenderer's
            // islandExceptionDistricts reads `selD`) and is also what opens
            // its card (selectDistrict's own setPanelOpen), matching #63's
            // own check ("select an island from the drawer list, and it's
            // visible and its card opens"). A mainland row keeps today's
            // fly-only behaviour: mainland is never faded, and changing its
            // established "fly to look, don't select" affordance is out of
            // this issue's scope.
            if (doc && districtClass(doc.districts[String(d)]) === "island") selectDistrict(d);
            canvasRef.current?.zoomDistrict(d);
          }}
        />
        <div className="relative min-w-0 flex-1 bg-[var(--canvas)]">
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
            onPick={(hit: SearchHit) => {
              if (hit.s != null) selectSymbolDetail(hit.i, hit.s);
              else selectFile(hit.i, { fly: true });
            }}
          />
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
            onSelectFile={(i, opts) => selectFile(i, opts)}
            onSelectSymbol={(i, s) => selectFile(i, { fly: false, symbol: s })}
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
          />
          {search.layer === "p" && (
            <PackageLegend
              grouping={packageGrouping}
              auto={search.depth == null}
              minDepth={packageLayout.minDepth}
              maxDepth={packageLayout.maxDepth}
              onDepth={(depth) => updateSearch({ depth: depth === packageLayout.autoDepth ? undefined : depth })}
            />
          )}
          <FooterStats doc={doc} layer={search.layer} maxCh={maxCh} maxCx={maxCx}
            unconnectedCount={packageLayout.unconnectedFiles.length}
            mobileHidden={panelOpen}
            onOpenUnconnected={() => { updateSearch({ file: undefined, sym: undefined, d: undefined, dir: undefined }); setUnconnectedRepo(doc.repo); setPanelOpen(true); setSideOpen(false); }} />
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
          />
        </div>
      </div>
    </div>
  );
}
