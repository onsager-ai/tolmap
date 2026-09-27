import type { MapDocument } from "@/types";
import { D_ } from "./geometry";

// docs/UX.md §3.2: a tapped road, street or neighborhood gets a card in the
// phone's bottom sheet -- Peek shows the renderer's own one-line explanation
// (the same text its floating card shows on desktop, MapRenderer's
// hoverContent), Half/Full show what is behind it: the file pairs a road or
// street aggregates, a neighborhood's files. Pure, keyed by the map's own
// `data-k` strings, so it can never disagree with what was tapped.

export type StructureKind = "road" | "street" | "neighborhood";

export interface StructureDetail {
  kind: StructureKind;
  /** Import edges behind a road or street, `[from, to]` file indices, the
   * first PAIR_LIMIT in document order; `pairTotal` is the full count. */
  pairs: Array<[number, number]>;
  pairTotal: number;
  /** A neighborhood's own files, largest first. */
  files: number[];
  /** A neighborhood's parent district. */
  district: number | null;
}

export const PAIR_LIMIT = 60;

export function structureDetail(doc: MapDocument, key: string): StructureDetail | null {
  const parts = key.split(":");
  const nb = doc.file_neighbourhoods ?? null;
  const collect = (match: (a: number, b: number) => boolean): Pick<StructureDetail, "pairs" | "pairTotal"> => {
    const pairs: Array<[number, number]> = [];
    let pairTotal = 0;
    for (const [a, b] of doc.E) {
      if (!match(a, b)) continue;
      pairTotal++;
      if (pairs.length < PAIR_LIMIT) pairs.push([a, b]);
    }
    return { pairs, pairTotal };
  };
  if (parts[0] === "r") {
    const da = +parts[1];
    const db = +parts[2];
    const { pairs, pairTotal } = collect((a, b) => {
      const x = D_(doc, a);
      const y = D_(doc, b);
      return (x === da && y === db) || (x === db && y === da);
    });
    return { kind: "road", pairs, pairTotal, files: [], district: null };
  }
  if (parts[0] === "st" && parts[1] === "n" && nb) {
    const na = parts[2];
    const nbId = parts[3];
    const { pairs, pairTotal } = collect((a, b) => (nb[a] === na && nb[b] === nbId) || (nb[a] === nbId && nb[b] === na));
    return { kind: "street", pairs, pairTotal, files: [], district: null };
  }
  if (parts[0] === "st" && parts[1] === "c" && nb) {
    const n = parts[2];
    const d = +parts[3];
    const { pairs, pairTotal } = collect((a, b) => (nb[a] === n && D_(doc, b) === d) || (D_(doc, a) === d && nb[b] === n));
    return { kind: "street", pairs, pairTotal, files: [], district: null };
  }
  if (parts[0] === "n") {
    const id = parts.slice(1).join(":");
    const hood = doc.neighbourhoods?.[id];
    if (!hood) return null;
    const files: number[] = [];
    if (nb) nb.forEach((n, i) => {
      if (n === id) files.push(i);
    });
    // Largest first, ties by index: deterministic (CLAUDE.md).
    files.sort((a, b) => doc.N[b][3] - doc.N[a][3] || a - b);
    return { kind: "neighborhood", pairs: [], pairTotal: 0, files, district: hood.d };
  }
  return null;
}
