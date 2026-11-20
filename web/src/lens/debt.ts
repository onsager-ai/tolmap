import type { ResolvedPath } from "@bindings/ResolvedPath";

/** The built-in debt lens (issue #250). Served by `/maps/{owner}/{repo}/debt`
 * and `/compare/{owner}/{repo}/debt`, never read from `.tolmap/lenses/`.
 * Repository lens names start with [a-z0-9], so this value cannot collide
 * with one; it is also the `name`/`lens` the service reports. */
export const DEBT_LENS = "@debt";

export const isDebtLens = (name?: string) => name === DEBT_LENS;

/** What the picker and headers show for a lens name. */
export const lensDisplayName = (name: string) => (isDebtLens(name) ? "Debt" : name);

/** Issue #348: the kind of finding a debt path shows, read from the name
 * `tolmap debt --format lens` gives it (src/debt.rs: `cycle-`, `cycle-cut-`,
 * `hidden-coupling-` followed by a hash of its files). */
export type DebtPathKind = "cycle" | "cycle-cut" | "hidden-coupling";

export function debtPathKind(name: string): DebtPathKind | undefined {
  if (name.startsWith("cycle-cut-")) return "cycle-cut";
  if (name.startsWith("cycle-")) return "cycle";
  if (name.startsWith("hidden-coupling-")) return "hidden-coupling";
  return undefined;
}

export const DEBT_PATH_LABEL: Record<DebtPathKind, string> = {
  cycle: "District cycle: its closing edges",
  "cycle-cut": "Cycle cut: one bundle of a heuristic cut",
  "hidden-coupling": "Hidden coupling: co-changed, no static edge",
};

/** Issue #348: one suggested cut on a district pair: the debt lens's
 * cycle-cut paths' computed hops from district `from` to `to`, and how many
 * edges they cross. */
export interface CutRoad { from: number; to: number; edges: number }

/** Issue #348: every directed district pair the cycle-cut paths cross on
 * computed hops, with its edge count. `tolmap debt --format lens` emits one
 * path per cut bundle (one directed pair, one computed hop per edge) for at
 * most 20 bundles, so a pair past that cap is absent (a lower bound).
 * Declared hops claim no edge and count for nothing. Sorted by pair. */
export function cutRoads(paths: readonly ResolvedPath[], files: readonly string[], district: (file: number) => number): CutRoad[] {
  const index = new Map(files.map((file, i) => [file, i]));
  const pairs = new Map<string, CutRoad>();
  for (const path of paths) {
    if (debtPathKind(path.name) !== "cycle-cut") continue;
    for (const hop of path.hops) {
      if (!hop.resolved || hop.source !== "computed") continue;
      const districts = hop.via.map(file => index.get(file)).filter((file): file is number => file != null).map(district);
      for (let i = 1; i < districts.length; i++) {
        const from = districts[i - 1], to = districts[i];
        if (from === to) continue;
        const key = `${from}-${to}`;
        const road = pairs.get(key) ?? { from, to, edges: 0 };
        road.edges++;
        pairs.set(key, road);
      }
    }
  }
  return [...pairs.values()].sort((a, b) => a.from - b.from || a.to - b.to);
}
