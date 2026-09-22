import { useMemo, useRef, useState } from "react";
import { useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { useCatalogue, useMapDocument } from "@/data/queries";
import { MapCanvas, type MapCanvasHandle } from "@/map/MapCanvas";
import type { MapRendererCallbacks, TerrainSelection } from "@/map/MapRenderer";
import { buildAdj, findRoute, type Route } from "@/map/graph";
import type { SearchHit } from "@/map/search";
import { TopBar } from "@/components/TopBar";
import { Sidebar } from "@/components/Sidebar";
import { SearchBox } from "@/components/SearchBox";
import { SelectionPanel } from "@/components/SelectionPanel";
import { RouteBox } from "@/components/RouteBox";
import { FooterStats } from "@/components/FooterStats";
import { ZoomControls } from "@/components/ZoomControls";

/** The /:owner/:repo page: assembles chrome (TopBar, Sidebar, SearchBox,
 * SelectionPanel, RouteBox, FooterStats, ZoomControls) around one MapCanvas.
 * This component owns every piece of application state — selection, geo,
 * layer, route — either directly or via the URL; MapCanvas/MapRenderer only
 * ever receive it as props and report gestures back through callbacks. */
export function MapView() {
  const { owner, repo } = useParams({ strict: false }) as { owner: string; repo: string };
  const search = useSearch({ strict: false }) as {
    file?: string;
    sym?: number;
    d?: number;
    geo: "r" | "p" | "t";
    layer: "d" | "c" | "x";
  };
  const navigate = useNavigate();
  const { data: catalogue } = useCatalogue();
  const { data: doc, isLoading, isError, error } = useMapDocument(owner, repo);

  const canvasRef = useRef<MapCanvasHandle>(null);
  const [panelOpen, setPanelOpen] = useState(false);
  const [sideOpen, setSideOpen] = useState(false);
  const [routeFrom, setRouteFrom] = useState<number | null>(null);
  const [route, setRoute] = useState<Route | null>(null);
  const [selTerrain, setSelTerrain] = useState<TerrainSelection | null>(null);

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
      search: (prev: Record<string, unknown>) => ({ ...prev, ...patch }),
      replace: true,
    });
  }

  function selectFile(i: number, opts: { fly?: boolean; symbol?: number } = {}) {
    if (!doc) return;
    setSelTerrain(null);
    updateSearch({ file: doc.F[i], sym: opts.symbol, d: undefined });
    setPanelOpen(true);
    setSideOpen(false);
    if (opts.fly !== false) canvasRef.current?.flyTo(i);
  }
  function selectSymbolDetail(i: number, s: number) {
    if (!doc) return;
    setSelTerrain(null);
    updateSearch({ file: doc.F[i], sym: s, d: undefined });
    setPanelOpen(true);
    setSideOpen(false);
    canvasRef.current?.flyToDetail(i);
  }
  function selectDistrict(d: number) {
    setSelTerrain(null);
    updateSearch({ file: undefined, sym: undefined, d });
    setPanelOpen(true);
    setSideOpen(false);
  }
  function clearSelection() {
    setSelTerrain(null);
    updateSearch({ file: undefined, sym: undefined, d: undefined });
    setPanelOpen(false);
  }

  const rendererCallbacks: MapRendererCallbacks = {
    // Taps directly on the map never fly — the file is already in view.
    onSelectFile: (i) => selectFile(i, { fly: false }),
    onSelectSymbol: (i, s) => selectFile(i, { fly: false, symbol: s }),
    onSelectSubdistrict: (district, index) => {
      updateSearch({ file: undefined, sym: undefined, d: undefined });
      setSelTerrain({ kind: "subdistrict", district, index });
      setPanelOpen(true);
      setSideOpen(false);
    },
    onSelectParcel: (district, index) => {
      updateSearch({ file: undefined, sym: undefined, d: undefined });
      setSelTerrain({ kind: "parcel", district, index });
      setPanelOpen(true);
      setSideOpen(false);
    },
    onSelectDistrict: (d) => selectDistrict(d),
    onClearSelection: () => clearSelection(),
  };

  if (isLoading) {
    return (
      <div className="flex h-full items-center justify-center bg-[var(--chrome)] text-sm text-[var(--dim)]">
        loading {owner}/{repo}…
      </div>
    );
  }
  if (isError || !doc) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 bg-[var(--chrome)] p-6 text-center text-sm text-[var(--hot)]">
        <p>couldn't load {owner}/{repo}.</p>
        <p className="text-[var(--dim)]">{error instanceof Error ? error.message : "not in the catalogue"}</p>
      </div>
    );
  }

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
            selTerrain={selTerrain}
            route={route}
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
            selTerrain={selTerrain}
            adj={adj}
            radj={radj}
            open={panelOpen}
            onToggleOpen={() => setPanelOpen((v) => !v)}
            onSelectFile={(i, opts) => selectFile(i, opts)}
            onSelectSymbol={(i, s) => selectFile(i, { fly: false, symbol: s })}
            onZoomDistrict={(d) => canvasRef.current?.zoomDistrict(d)}
            onRouteFrom={(i) => setRouteFrom(i)}
            onRouteTo={(i) => {
              if (routeFrom == null) {
                setRouteFrom(i);
                return;
              }
              setRoute(findRoute(doc, adj, routeFrom, i));
            }}
          />
          <FooterStats doc={doc} layer={search.layer} maxCh={maxCh} maxCx={maxCx} />
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
