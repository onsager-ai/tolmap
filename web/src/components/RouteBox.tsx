import type { MapDocument } from "@/types";
import type { Route } from "@/map/graph";
import { PATH_KIND_TEXT as KIND_TEXT } from "@/lib/cardText";

interface Props {
  doc: MapDocument;
  routeFrom: number | null;
  route: Route | null;
  onClear(): void;
}


/** Blast-radius's sibling feature: an explicit two-click "path from / path
 * to" query (docs/UX.md §12: the UI says "path", never "route"; the code
 * keeps its `route` names) over the directed import graph, rendered as a polyline on
 * the map (see MapRenderer.draw()) and as hop-by-hop text here. Not part of
 * the URL-state requirement — it's a transient tool, not a view worth
 * bookmarking — so this state lives in MapView's local state, unlike
 * selection/geo/layer. */
/** Desktop only: on a phone the path is bottom-sheet content (docs/UX.md
 * §4.7, components/phone/SheetContent.tsx). */
export function RouteBox({ doc, routeFrom, route, onClear }: Props) {
  if (routeFrom == null && !route) return null;
  return (
    <div className="absolute bottom-2.5 left-2.5 max-w-[min(430px,calc(100%-22px))] rounded-md border border-[var(--rule)] bg-[rgba(var(--chrome-float-rgb),0.96)] px-3 py-2.5 text-meta text-[var(--on)] max-[820px]:inset-x-2.5 max-[820px]:max-w-none">
      <button type="button" onClick={onClear} className="float-right ml-2.5 cursor-pointer text-[var(--dim)]">
        clear
      </button>
      {!route && routeFrom != null && (
        <>
          <b>Path from</b> <span className="font-mono">{doc.F[routeFrom]}</span>
          <div className="break-all text-[var(--dim)]">now pick a destination and press "Path to here".</div>
        </>
      )}
      {route && (
        <>
          <b>
            {route.path.length - 1} hop{route.path.length === 2 ? "" : "s"}
          </b>{" "}
          · {KIND_TEXT[route.kind]}
          <div className="break-all font-mono text-[var(--dim)]">
            {route.path.map((i, n) => (
              <span key={i}>
                {n ? <i className="not-italic text-[var(--accent)]"> → </i> : null}
                {doc.F[i].split("/").slice(1).join("/") || doc.F[i]}
              </span>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
