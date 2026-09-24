import { useMemo, type ReactNode } from "react";
import type { DistrictSymbols, MapDocument } from "@/types";
import { symbolsOf } from "@/map/geometry";
import { KIND } from "@/map/constants";
import type { AdjMap } from "@/map/graph";
import { decodeDistrictSymbols, symbolHierarchy } from "@/map/symbolCards";
import { Button } from "@/components/ui/button";
import { LinkCountsLabel } from "@/components/LinkLegend";

interface Props {
  doc: MapDocument;
  sel: number | null;
  selSym: number | null;
  selD: number | null;
  /** #103 build item 4: same GLOBAL hier-symbol index and (undecoded)
   * district symbols document SelectionPanel's FileHead reads, decoded
   * independently here for the same "extends / implements" legend segment
   * -- see FileHead's own comment for why each reader decodes its own
   * copy. `undefined` degrades to the plain two-line legend, same as
   * before #103. */
  selHSym?: number | null;
  symbolsDoc?: DistrictSymbols;
  adj: AdjMap;
  radj: AdjMap;
  onDetails(): void;
}

/** Issue #82 A1 scope item 5: fullscreen's bottom summary bar. The
 * SelectionPanel card (repo/district/file browsing, route buttons, the
 * symbol directory...) is hidden while fullscreen -- the whole point of
 * fullscreen is a map that fills the screen -- so this is deliberately just
 * a name, a one-line summary and a "Details" button, the same three things
 * the phone's collapsed sheet already shows outside fullscreen (see
 * SelectionPanel's FileHead/DistrictHead). "Details" exits fullscreen and
 * opens the real panel/sheet rather than growing this bar into one. Renders
 * nothing when nothing is selected -- fullscreen with an empty selection is
 * just the map, with no bar to show. */
export function SelectionSummaryBar({ doc, sel, selSym, selD, selHSym, symbolsDoc, adj, radj, onDetails }: Props) {
  const decoded = useMemo(() => (symbolsDoc ? decodeDistrictSymbols(symbolsDoc) : null), [symbolsDoc]);
  const hasInheritance = useMemo(() => {
    if (!decoded || selHSym == null) return false;
    const local = decoded.globalToLocal.get(selHSym);
    if (local == null) return false;
    const info = symbolHierarchy(decoded, local);
    return info.extends.length > 0 || info.implements.length > 0;
  }, [decoded, selHSym]);

  if (sel == null && selD == null) return null;

  let title: string;
  let subtitle: ReactNode;
  if (selD != null) {
    title = doc.names[selD] ?? `district ${selD}`;
    subtitle = `${doc.districts[selD].size} files`;
  } else {
    const i = sel!;
    const sy = symbolsOf(doc, i);
    const sm = selSym != null ? sy[selSym] : null;
    if (sm) {
      title = sm[0];
      subtitle = `${KIND[sm[1]]} · lines ${sm[2]}–${sm[3]}`;
    } else {
      title = doc.F[i].split("/").pop()!;
      const inDeg = radj.get(i)?.length ?? 0;
      const outDeg = adj.get(i)?.length ?? 0;
      // Issue #82 "link colour legend": the fullscreen bar shows the same
      // imported-by/imports counts SelectionPanel's FileHead does, so it
      // gets the same colour+glyph legend rather than a plain string here.
      // #103 build item 4: same triangle-glyph addition too.
      subtitle = <LinkCountsLabel inDeg={inDeg} outDeg={outDeg} hasInheritance={hasInheritance} />;
    }
  }

  return (
    <div
      data-fullscreen-summary
      className="absolute inset-x-2.5 bottom-2.5 z-20 flex items-center gap-2.5 rounded-md border border-[var(--rule)] bg-[rgba(var(--chrome-float-rgb),0.94)] px-3 py-2 text-[var(--on)] shadow-lg"
      style={{ paddingBottom: "calc(8px + env(safe-area-inset-bottom, 0px))" }}
    >
      <div className="min-w-0 flex-1">
        <div className="truncate font-sans text-[12.5px] font-semibold">{title}</div>
        <div className="truncate text-[10px] text-[var(--dim)]">{subtitle}</div>
      </div>
      <Button size="sm" variant="outline" onClick={onDetails}>
        Details
      </Button>
    </div>
  );
}
