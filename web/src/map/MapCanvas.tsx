import { useEffect, useImperativeHandle, useLayoutEffect, useRef } from "react";
import type { MapDocument } from "@/types";
import { MapRenderer, type MapRenderState, type MapRendererCallbacks } from "./MapRenderer";
import type { Geo, Layer } from "./constants";
import type { Route } from "./graph";
import type { FolderLabel, PackageGrouping } from "./packageLayout";

export interface MapCanvasHandle {
  fit(anim?: boolean): void;
  /** Pan-only selection (issue #82 A1): never changes k, and does nothing at
   * all if `i` is already on screen. Replaces the old flyTo/flyToDetail,
   * which used to zoom in on every sidebar pick, search result and deep
   * link -- see MapRenderer.panTo's own doc comment. */
  panTo(i: number, anim?: boolean): void;
  panToDistrict(d: number, anim?: boolean): void;
  /** Still a real zoom -- the one thing selecting is still allowed to do,
   * because it's the user explicitly asking via the ⤢ button. */
  zoomDistrict(d: number): void;
  zoomBy(f: number): void;
}

interface MapCanvasProps {
  doc: MapDocument;
  geo: Geo;
  layer: Layer;
  sel: number | null;
  selSym: number | null;
  selD: number | null;
  route: Route | null;
  packageGrouping: PackageGrouping;
  folderFiles: ReadonlySet<number> | null;
  folderOnlyIslands: boolean;
  folderLabels: readonly FolderLabel[];
  activeDirectory?: string;
  callbacks: MapRendererCallbacks;
  handleRef?: React.Ref<MapCanvasHandle>;
}

/** The one non-React piece of the map, held behind a ref exactly as
 * docs/ARCHITECTURE.md specifies: React owns this component's props (all
 * application state), but the SVG inside it is drawn by MapRenderer, an
 * imperative class that never re-renders through React's reconciler. At
 * ~1000 files and several thousand symbols, diffing that as JSX on every
 * pan frame is the thing this split avoids. */
export function MapCanvas({
  doc,
  geo,
  layer,
  sel,
  selSym,
  selD,
  route,
  packageGrouping,
  folderFiles,
  folderOnlyIslands,
  folderLabels,
  activeDirectory,
  callbacks,
  handleRef,
}: MapCanvasProps) {
  const svgRef = useRef<SVGSVGElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const rendererRef = useRef<MapRenderer | null>(null);
  // Callbacks close over React state/URL setters that change every render;
  // stash the latest in a ref so the renderer (constructed once) always
  // calls the current version without needing to be torn down and rebuilt.
  const callbacksRef = useRef(callbacks);
  callbacksRef.current = callbacks;

  useLayoutEffect(() => {
    if (!svgRef.current) return;
    const stableCallbacks: MapRendererCallbacks = {
      onSelectDistrict: (d) => callbacksRef.current.onSelectDistrict(d),
      onSelectFile: (i) => callbacksRef.current.onSelectFile(i),
      onSelectSymbol: (i, s) => callbacksRef.current.onSelectSymbol(i, s),
      onClearSelection: () => callbacksRef.current.onClearSelection(),
      onSelectDirectory: (path) => callbacksRef.current.onSelectDirectory(path),
      onPreviewDirectory: (path) => callbacksRef.current.onPreviewDirectory(path),
      onDragStart: () => callbacksRef.current.onDragStart?.(),
    };
    const renderer = new MapRenderer(svgRef.current, stableCallbacks);
    rendererRef.current = renderer;
    return () => {
      renderer.destroy();
      rendererRef.current = null;
    };
  }, []);

  useImperativeHandle(
    handleRef,
    () => ({
      fit: (anim = true) => rendererRef.current?.fit(anim),
      panTo: (i, anim = true) => rendererRef.current?.panTo(i, anim),
      panToDistrict: (d, anim = true) => rendererRef.current?.panToDistrict(d, anim),
      zoomDistrict: (d) => rendererRef.current?.zoomDistrict(d),
      zoomBy: (f) => rendererRef.current?.zoomBy(f),
    }),
    [],
  );

  // Repo changed: reset derived indices and fit before the first paint of
  // the new document, so the map never flashes the old repo's zoom level.
  // fit() below is passed this render's state explicitly rather than relying
  // on the state-render effect further down to have set it first: layout
  // effects run in the order they're declared, so at this point
  // rendererRef.current's own `state` field still holds the PREVIOUS repo's
  // document (that effect hasn't run yet this commit) -- frameBounds() would
  // fit the wrong document's bounds otherwise. See MapRenderer.fit()'s doc
  // comment for the full story and why this used to work by accident.
  //
  // Issue #51: justFittedRef flags that the fit() call below just baked this
  // exact (doc, geo, layer, sel, selSym, selD, route) into the
  // DOM, so the state-render effect immediately following it in this SAME
  // commit (React runs a component's layout effects in declaration order,
  // and repoKey changing always also changes `doc`, which is in that
  // effect's own dep array) would otherwise call render() a second time
  // with a byte-identical state -- a full second paint of exactly what fit()
  // already drew, on every repo load and every repo switch. Measured as
  // draw #2 of 5 on load (see the PR description); this is what collapses it
  // back to one. The flag is scoped to "the very next state-render effect
  // run", not "every run after a repoKey change": it's cleared the first
  // time that effect sees it, so a later real state change in the same
  // render pass this component happens to also pick up still repaints.
  const justFittedRef = useRef(false);
  const repoKey = doc.repo;
  useLayoutEffect(() => {
    const renderer = rendererRef.current;
    if (!renderer || !wrapRef.current) return;
    renderer.loadDocument(doc);
    const r = wrapRef.current.getBoundingClientRect();
    renderer.resize(r.width, r.height);
    const state: MapRenderState = {
      doc,
      geo,
      layer,
      sel,
      selSym,
      selD,
      route,
      packageGrouping,
      folderFiles,
      folderOnlyIslands,
      folderLabels,
      activeDirectory,
    };
    renderer.fit(false, state);
    // Issue #82 A1: a `?file=`/`?d=` deep link must land with its target
    // panned into view (never zoomed -- "selecting never moves the map"
    // applies to a fresh load exactly as much as a later click), but fit()
    // above already frames every mainland file, so this is a no-op except
    // for a deep link into an ISLAND district (outside mainlandBounds, see
    // MapRenderer.fit's own doc comment) or one otherwise placed outside the
    // opening frame. `anim=false` to match fit()'s own instant (non-glide)
    // first paint -- animating a pan immediately after an instant fit would
    // read as two separate view changes instead of one settled opening view.
    if (sel != null) renderer.panTo(sel, false);
    else if (selD != null) renderer.panToDistrict(selD, false);
    justFittedRef.current = true;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [repoKey]);

  useLayoutEffect(() => {
    const renderer = rendererRef.current;
    if (!renderer) return;
    if (justFittedRef.current) {
      justFittedRef.current = false;
      return;
    }
    const state: MapRenderState = {
      doc,
      geo,
      layer,
      sel,
      selSym,
      selD,
      route,
      packageGrouping,
      folderFiles,
      folderOnlyIslands,
      folderLabels,
      activeDirectory,
    };
    renderer.render(state);
  }, [doc, geo, layer, sel, selSym, selD, route, packageGrouping, folderFiles, folderOnlyIslands, folderLabels, activeDirectory]);

  // The wrapper's box, watched once for the component's life. Selection,
  // layer, geo and route changes must never touch this subscription: the
  // wrapper's size doesn't depend on any of them, and this effect used to
  // re-subscribe on every one of those prop changes so it could read fresh
  // values out of its own closure. That was the bug (see MapRenderer.resize's
  // comment) — ResizeObserver.observe() always delivers one synchronous
  // "initial size" callback on subscribe, size unchanged or not, so tapping a
  // file dot, tapping it away, or switching layers each re-subscribed and
  // each delivered a same-size callback that snapped the view back to fit.
  // Reading the current size and comparing against the last one actually
  // observed (below) is what makes this safe to subscribe once: a real
  // resize (rotation, the phone URL bar, the sidebar opening) still reaches
  // resize(), an initial or spurious same-size callback does not.
  const lastSizeRef = useRef<{ w: number; h: number } | null>(null);
  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const ro = new ResizeObserver(() => {
      const r = wrap.getBoundingClientRect();
      const last = lastSizeRef.current;
      if (last && last.w === r.width && last.h === r.height) return;
      lastSizeRef.current = { w: r.width, h: r.height };
      rendererRef.current?.resize(r.width, r.height);
    });
    ro.observe(wrap);
    return () => ro.disconnect();
  }, []);

  return (
    <div ref={wrapRef} className="absolute inset-0">
      <svg
        ref={svgRef}
        className="map-svg"
        role="img"
        aria-label="Pannable, zoomable map of a source repository with computed districts, landmark pins and import routes"
      />
    </div>
  );
}
