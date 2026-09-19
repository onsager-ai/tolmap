import type { MapDocument } from "@/types";
import { D_ } from "./geometry";

export type AdjMap = Map<number, number[]>;

export function buildAdj(doc: MapDocument): { adj: AdjMap; radj: AdjMap } {
  const adj: AdjMap = new Map();
  const radj: AdjMap = new Map();
  for (const [a, b] of doc.E) {
    if (!adj.has(a)) adj.set(a, []);
    adj.get(a)!.push(b);
    if (!radj.has(b)) radj.set(b, []);
    radj.get(b)!.push(a);
  }
  return { adj, radj };
}

function bfs(from: number, to: number, graph: AdjMap): number[] | null {
  if (from === to) return [from];
  const prev = new Map<number, number>([[from, -1]]);
  const q = [from];
  for (let h = 0; h < q.length; h++) {
    const cur = q[h];
    for (const nx of graph.get(cur) ?? []) {
      if (prev.has(nx)) continue;
      prev.set(nx, cur);
      if (nx === to) {
        const p = [to];
        let c = cur;
        while (c !== -1) {
          p.push(c);
          c = prev.get(c)!;
        }
        return p.reverse();
      }
      q.push(nx);
    }
  }
  return null;
}

export type RouteKind = "imports" | "imported-by" | "undirected";
export interface Route {
  path: number[];
  kind: RouteKind;
}

/** Directed BFS a→b, then b→a (reported as "imported-by"), then an
 * undirected fallback. Order matters: a directed path is always preferred
 * over an undirected one even when the undirected one is shorter, because
 * the direction is the fact worth stating. */
export function findRoute(doc: MapDocument, adj: AdjMap, a: number, b: number): Route | null {
  let p = bfs(a, b, adj);
  if (p) return { path: p, kind: "imports" };
  p = bfs(b, a, adj);
  if (p) return { path: p.slice().reverse(), kind: "imported-by" };
  const und: AdjMap = new Map();
  for (const [x, y] of doc.E) {
    if (!und.has(x)) und.set(x, []);
    und.get(x)!.push(y);
    if (!und.has(y)) und.set(y, []);
    und.get(y)!.push(x);
  }
  p = bfs(a, b, und);
  return p ? { path: p, kind: "undirected" } : null;
}

export interface Blast {
  set: Set<number>;
  files: number[];
  districts: Set<number>;
}

/** Which files reference symbol `selSym` of file `sel`, and how many
 * districts that reaches. Blast radius and modularity "agree, independently"
 * (finding 6) — this is the number the map can state that an IDE cannot. */
export function computeBlast(doc: MapDocument, sel: number | null, selSym: number | null): Blast | null {
  if (selSym == null || sel == null || !doc.U) return null;
  const refs = doc.U[`${sel}:${selSym}`];
  if (!refs || !refs.length) return null;
  const ds = new Set(refs.map((j) => D_(doc, j)));
  return { set: new Set(refs), files: refs, districts: ds };
}
