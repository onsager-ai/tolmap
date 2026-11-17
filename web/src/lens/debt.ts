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

/** Issue #348: the directed district pairs `[from, to]` that the debt lens's
 * cycle and cycle-cut paths cross on computed hops. Every such edge joins
 * two districts of one district cycle, so its road closes that cycle. Declared
 * hops claim no edge and contribute nothing. Sorted, without duplicates. */
export function cycleRoads(paths: readonly ResolvedPath[], files: readonly string[], district: (file: number) => number): [number, number][] {
  const index = new Map(files.map((file, i) => [file, i]));
  const pairs = new Map<string, [number, number]>();
  for (const path of paths) {
    if (!debtPathKind(path.name)?.startsWith("cycle")) continue;
    for (const hop of path.hops) {
      if (!hop.resolved || hop.source !== "computed") continue;
      const districts = hop.via.map(file => index.get(file)).filter((file): file is number => file != null).map(district);
      for (let i = 1; i < districts.length; i++) {
        const from = districts[i - 1], to = districts[i];
        if (from !== to) pairs.set(`${from}-${to}`, [from, to]);
      }
    }
  }
  return [...pairs.values()].sort((a, b) => a[0] - b[0] || a[1] - b[1]);
}
