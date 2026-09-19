import type { MapDocument } from "@/types";
import type { Route } from "@/map/graph";

interface Props {
  doc: MapDocument;
  routeFrom: number | null;
  route: Route | null;
  onClear(): void;
}

const KIND_TEXT: Record<Route["kind"], string> = {
  imports: "follows imports, source → target",
  "imported-by": "reverse direction — the target imports the source",
  undirected: "no directed path; shown ignoring edge direction",
};

/** Blast-radius's sibling feature: an explicit two-click "route from / route
 * to" path query over the directed import graph, rendered as a polyline on
 * the map (see MapRenderer.draw()) and as hop-by-hop text here. Not part of
 * the URL-state requirement — it's a transient tool, not a view worth
 * bookmarking — so this state lives in MapView's local state, unlike
 * selection/geo/layer. */
export function RouteBox({ doc, routeFrom, route, onClear }: Props) {
  if (routeFrom == null && !route) return null;
  return (
    <div className="absolute bottom-2.5 left-2.5 max-w-[min(430px,calc(100%-22px))] rounded-md border border-[var(--rule)] bg-[rgba(21,28,33,.96)] px-3 py-2.5 text-[10.5px] leading-relaxed text-[var(--on)] max-[820px]:inset-x-2.5 max-[820px]:max-w-none">
      <span onClick={onClear} className="float-right ml-2.5 cursor-pointer text-[var(--dim)]">
        clear
      </span>
      {!route && routeFrom != null && (
        <>
          <b>Route from</b> {doc.F[routeFrom]}
          <div className="break-all text-[var(--dim)]">now pick a destination and press "route to".</div>
        </>
      )}
      {route && (
        <>
          <b>
            {route.path.length - 1} hop{route.path.length === 2 ? "" : "s"}
          </b>{" "}
          · {KIND_TEXT[route.kind]}
          <div className="break-all text-[var(--dim)]">
            {route.path.map((i, n) => (
              <span key={i}>
                {n ? <i className="not-italic text-[var(--hot)]"> → </i> : null}
                {doc.F[i].split("/").slice(1).join("/") || doc.F[i]}
              </span>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
