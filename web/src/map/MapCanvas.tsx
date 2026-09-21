import { useEffect, useImperativeHandle, useLayoutEffect, useRef } from "react";
import type { MapDocument } from "@/types";
import { MapRenderer, type MapRenderState, type MapRendererCallbacks, type TerrainSelection } from "./MapRenderer";
import type { Geo, Layer } from "./constants";
import type { Route } from "./graph";

export interface MapCanvasHandle {
  fit(anim?: boolean): void;
  flyTo(i: number, zoomTo?: number): void;
  flyToDetail(i: number): void;
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
  selTerrain: TerrainSelection | null;
  route: Route | null;
  callbacks: MapRendererCallbacks;
  handleRef?: React.Ref<MapCanvasHandle>;
}

/** The one non-React piece of the map, held behind a ref exactly as
 * docs/ARCHITECTURE.md specifies: React owns this component's props (all
 * application state), but the SVG inside it is drawn by MapRenderer, an
 * imperative class that never re-renders through React's reconciler. At
 * ~1000 files and several thousand symbols, diffing that as JSX on every
 * pan frame is the thing this split avoids. */
export function MapCanvas({ doc, geo, layer, sel, selSym, selD, selTerrain, route, callbacks, handleRef }: MapCanvasProps) {
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
      onSelectSubdistrict: (d, index) => callbacksRef.current.onSelectSubdistrict(d, index),
      onSelectParcel: (d, index) => callbacksRef.current.onSelectParcel(d, index),
      onClearSelection: () => callbacksRef.current.onClearSelection(),
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
      flyTo: (i, zoomTo) => rendererRef.current?.flyTo(i, zoomTo),
      flyToDetail: (i) => rendererRef.current?.flyToDetail(i),
      zoomDistrict: (d) => rendererRef.current?.zoomDistrict(d),
      zoomBy: (f) => rendererRef.current?.zoomBy(f),
    }),
    [],
  );

  // Repo changed: reset derived indices and fit before the first paint of
  // the new document, so the map never flashes the old repo's zoom level.
  const repoKey = doc.repo;
  useLayoutEffect(() => {
    const renderer = rendererRef.current;
    if (!renderer || !wrapRef.current) return;
    renderer.loadDocument(doc);
    const r = wrapRef.current.getBoundingClientRect();
    renderer.resize(r.width, r.height, false);
    renderer.fit(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [repoKey]);

  useLayoutEffect(() => {
    const renderer = rendererRef.current;
    if (!renderer) return;
    const state: MapRenderState = { doc, geo, layer, sel, selSym, selD, selTerrain, route };
    renderer.render(state);
  }, [doc, geo, layer, sel, selSym, selD, selTerrain, route]);

  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const ro = new ResizeObserver(() => {
      const r = wrap.getBoundingClientRect();
      rendererRef.current?.resize(r.width, r.height, false);
      rendererRef.current?.render({ doc, geo, layer, sel, selSym, selD, selTerrain, route });
    });
    ro.observe(wrap);
    return () => ro.disconnect();
    // Re-observing on every prop change is unnecessary — resize() reads
    // current props via the closure captured at effect-run time, which is
    // fine because ResizeObserver only fires on actual size changes, not on
    // re-render; the render call inside always uses the latest values from
    // this effect's own closure since it re-subscribes whenever any of them
    // change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [doc, geo, layer, sel, selSym, selD, selTerrain, route]);

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
