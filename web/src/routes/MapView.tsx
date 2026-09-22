import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { useCatalogue, useMapDocument } from "@/data/queries";
import { MapCanvas, type MapCanvasHandle } from "@/map/MapCanvas";
import type { MapRendererCallbacks, TerrainSelection } from "@/map/MapRenderer";
import { buildAdj, findRoute, type Route } from "@/map/graph";
import type { SearchHit } from "@/map/search";
import { useIsNarrow } from "@/hooks/useIsNarrow";
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
  const narrow = useIsNarrow();
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

  // Issue #51: desktop's landmark auto-select (further down) used to run in
  // its own useEffect, so the first paint showed the map with nothing
  // selected and a SECOND paint moments later added the selection once that
  // effect had flipped the URL -- one full extra draw() on every desktop
  // load. `doc`, `narrow` and `search` are all already available synchronously
  // by the time this component first renders with a real `doc` (useIsNarrow
  // reads matchMedia() directly in getSnapshot, no effect-driven correction;
  // there's no SSR here), so the choice of what to auto-select doesn't
  // actually need to wait for an effect -- only WRITING it into the URL does
  // (navigate() is a side effect, not something render can do). Computing it
  // here means `sel` below already reflects the auto-selected file on the
  // very first commit, and fit() bakes it into the one paint MapCanvas's
  // repoKey effect already does.
  //
  // autoSelectRef latches WHAT to auto-select, once per repo, in a ref
  // written during render -- safe because the guard (`repo !== doc.repo`)
  // makes it idempotent under StrictMode's double-render (the second call
  // sees the latch already set and leaves it alone), the same "lazy
  // initialization" shape React's own docs allow for a ref written during
  // render.
  //
  // That alone isn't enough: `pendingAutoSelectFile` must also stop being a
  // fallback for `effectiveFile` after the FIRST commit, or a later
  // onClearSelection() -- which sets search.file back to undefined, looking
  // identical to "nothing selected yet" -- would fall through to it again
  // and make the selection uncloseable on desktop. committedRef is the gate:
  // it starts pointing at whatever repo was last PAINTED (not just
  // rendered), so the first commit for a new repo still sees it stale and
  // gets to use the fallback, and every commit after that (including one
  // from a clear, still within the same repo) sees it caught up and doesn't.
  // It's only ever written from an effect (after a commit has already
  // happened), never mutated during render, so it can't fall into the same
  // StrictMode-double-render trap a `consumed` flag written during render
  // would: both of StrictMode's render calls for a given commit read the
  // SAME committedRef value, because nothing changes it between them.
  const autoSelectRef = useRef<{ repo: string; file: string | undefined } | null>(null);
  if (doc && (!autoSelectRef.current || autoSelectRef.current.repo !== doc.repo)) {
    autoSelectRef.current = {
      repo: doc.repo,
      file: !narrow && !search.file && search.d == null && doc.L.length ? doc.F[doc.L[0][0]] : undefined,
    };
  }
  const committedRef = useRef<string | null>(null);
  const pendingAutoSelectFile =
    doc && autoSelectRef.current?.repo === doc.repo && committedRef.current !== doc.repo
      ? autoSelectRef.current.file
      : undefined;
  useEffect(() => {
    if (doc) committedRef.current = doc.repo;
  }, [doc]);

  const effectiveFile = search.file ?? pendingAutoSelectFile;
  const sel = doc && effectiveFile ? (() => { const i = doc.F.indexOf(effectiveFile!); return i >= 0 ? i : null; })() : null;
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

  // On desktop, land on the top landmark the way the reference does
  // (`if(R.L.length && !NARROW()) select(R.L[0][0],true)`); on a phone the
  // map opens unobstructed. Only when the URL didn't already ask for
  // something, and only once per repo — a deep link always wins. `sel`
  // above already reflects this decision (pendingAutoSelectFile) so the map
  // itself is correct from the first paint; all this effect does is persist
  // it into the URL for shareability/back-button parity. It must NOT be the
  // thing that flips `sel` for the first time -- that used to be a second
  // draw() (issue #51, draw #5 of 5), because navigate() can't resolve
  // before the browser paints what render() already returned. Once this
  // fires, `search.file` reads back the same file pendingAutoSelectFile
  // already held, so `sel`'s VALUE doesn't change and MapCanvas's own
  // effects don't re-run -- no draw from this either. Deliberately excludes
  // search.file/search.d from deps, same as the original version of this
  // effect: they're read once, at the moment doc/pendingAutoSelectFile
  // change (i.e. once per repo), not on every subsequent change (a later
  // onClearSelection() must not re-trigger this).
  useEffect(() => {
    if (!doc || !pendingAutoSelectFile) return;
    if (search.file || search.d != null) return;
    updateSearch({ file: pendingAutoSelectFile });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [doc, pendingAutoSelectFile]);

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
