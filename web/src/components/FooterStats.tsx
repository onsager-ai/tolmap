import type { MapDocument } from "@/types";
import type { Layer } from "@/map/constants";

interface Props {
  doc: MapDocument;
  layer: Layer;
  maxCh: number;
  maxCx: number;
}

/** Hidden on phones (the reference drops `.foot` below 820px — there's no
 * room once the drawer and search box are on screen) and replaced by
 * RouteBox/panel occupying the same corner. */
export function FooterStats({ doc, layer, maxCh, maxCx }: Props) {
  return (
    <div className="pointer-events-none absolute bottom-2.5 left-2.5 hidden max-w-[min(440px,calc(100%-22px))] rounded-md border border-[var(--rule)] bg-[rgba(21,28,33,.94)] px-2.5 py-2 text-[9.5px] leading-relaxed text-[var(--dim)] min-[821px]:block">
      {layer === "d" ? (
        <>
          <b className="font-medium text-[var(--on)]">{doc.F.length} files</b> ·{" "}
          <b className="font-medium text-[var(--on)]">{Object.keys(doc.districts).length} districts</b> · modularity{" "}
          <b className="font-medium text-[var(--on)]">{doc.q}</b> · {doc.E.length} import edges. Scroll to zoom, drag to
          pan, search a file <i className="not-italic">or a symbol</i> to fly. The map stops at the file; a file's
          classes and functions are listed in its directory. Switch geometry to <i className="not-italic">plots</i> for
          the cadastral view.
        </>
      ) : (
        <>
          {layer === "c" ? "Commits touching each file" : "Branch points per file"}
          <span
            className="mx-1.5 inline-block h-[7px] w-24 rounded-sm align-middle"
            style={{ background: "linear-gradient(90deg,#3E6E88,#B8B06A,#C0472F)" }}
          />
          <b className="font-medium text-[var(--on)]">{layer === "c" ? `1 → ${maxCh}` : `0 → ${maxCx}`}</b>
        </>
      )}
    </div>
  );
}
