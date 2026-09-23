import type { MapDocument } from "@/types";
import { symbolsOf } from "@/map/geometry";
import { KIND } from "@/map/constants";
import type { AdjMap } from "@/map/graph";
import { Button } from "@/components/ui/button";

interface Props {
  doc: MapDocument;
  sel: number | null;
  selSym: number | null;
  selD: number | null;
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
export function SelectionSummaryBar({ doc, sel, selSym, selD, adj, radj, onDetails }: Props) {
  if (sel == null && selD == null) return null;

  let title: string;
  let subtitle: string;
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
      subtitle = `imported by ${inDeg} file${inDeg === 1 ? "" : "s"} · imports ${outDeg}`;
    }
  }

  return (
    <div
      data-fullscreen-summary
      className="absolute inset-x-2.5 bottom-2.5 z-20 flex items-center gap-2.5 rounded-md border border-[var(--rule)] bg-[rgba(21,28,33,.94)] px-3 py-2 text-[var(--on)] shadow-lg"
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
