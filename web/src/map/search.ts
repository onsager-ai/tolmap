import type { MapDocument } from "@/types";
import { FI } from "./geometry";

export interface SearchHit {
  i: number;
  s: number | null;
  rank: number;
}

/** File and symbol search, ranked so an exact match wins over a substring
 * match and a well-referenced symbol wins over an obscure one. An exact
 * symbol name beats a filename that merely contains the query: searching
 * "Crawler" means the class, not every file with crawler in its path. */
export function searchHits(doc: MapDocument, v: string): SearchHit[] {
  const out: SearchHit[] = [];
  for (let i = 0; i < doc.N.length; i++) {
    if (doc.F[i].toLowerCase().includes(v)) {
      const base = doc.F[i].split("/").pop()!.toLowerCase();
      const stem = base.replace(/\.[a-z]+$/, "");
      const r = stem === v ? -1000 : (base.startsWith(v) ? 0 : 1) * 100;
      out.push({ i, s: null, rank: r - FI(doc, i) });
    }
  }
  // symbols are addresses too — a class or function is a door number
  for (const key in doc.S ?? {}) {
    const i = +key;
    (doc.S![key] ?? []).forEach((sm, n) => {
      const nm = sm[0].toLowerCase();
      if (nm.includes(v)) {
        const refs = (doc.U?.[`${i}:${n}`] ?? []).length;
        const r = nm === v ? -2000 : (nm.startsWith(v) ? 0 : 1) * 100;
        out.push({ i, s: n, rank: r - refs * 2 - (sm[3] - sm[2]) / 20 });
      }
    });
  }
  return out.sort((a, b) => a.rank - b.rank).slice(0, 14);
}
