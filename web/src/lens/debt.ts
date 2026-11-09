/** The built-in debt lens (issue #250). Served by `/maps/{owner}/{repo}/debt`
 * and `/compare/{owner}/{repo}/debt`, never read from `.tolmap/lenses/`.
 * Repository lens names start with [a-z0-9], so this value cannot collide
 * with one; it is also the `name`/`lens` the service reports. */
export const DEBT_LENS = "@debt";

export const isDebtLens = (name?: string) => name === DEBT_LENS;

/** What the picker and headers show for a lens name. */
export const lensDisplayName = (name: string) => (isDebtLens(name) ? "Debt" : name);
