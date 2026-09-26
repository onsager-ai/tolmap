// A4 (hubs, issue #82): a hub is a file with fan-in >= 30. `doc.N[i][6]`
// (the `FI` accessor in geometry.ts) already IS that fan-in -- extract.py's
// `fanin` Counter increments once per INCOMING doc.E edge
// (`fanin[tf] += 1` for each edge's target file, see src/tolmap/extract.py),
// which is exactly "counts incoming edges in doc.E" the spec asks to check
// against. No second fan-in computation needed; this module only picks
// which files clear the threshold, ranks them, and names them.
import type { MapDocument } from "@/types";
import { FI } from "./geometry";

export const HUB_FANIN_THRESHOLD = 30;

// Basenames generic enough that they don't identify a file on their own --
// ported from the spec's own list, plus the prototype's own extra generics
// (arch20.body.html's `hubName`) folded in as the "..." the spec leaves
// open. `utils.*` is a wildcard (utils.py, utils.ts, utils.js, ...), not a
// single literal, since the extension varies by language.
const GENERIC_BASENAMES = new Set([
  "__init__.py",
  "index.ts",
  "index.tsx",
  "index.js",
  "mod.rs",
  "lib.rs",
  "main.rs",
  "base.py",
  "types.ts",
  "wraps.py",
  "helper.py",
  "enums.py",
  "schema.py",
]);

function isGenericBasename(basename: string): boolean {
  return GENERIC_BASENAMES.has(basename) || /^utils\.[^./]+$/.test(basename);
}

export interface HubInfo {
  i: number;
  fi: number;
  /** Display name: basename alone, unless the basename is generic (see
   * GENERIC_BASENAMES) or shared by another hub, in which case the parent
   * directory is prefixed ("workers/base.py" rather than a bare "base.py"
   * that could be any of a dozen files). */
  name: string;
}

export interface HubSet {
  hubs: HubInfo[];
  /** The single largest fan-in among the hubs -- the denominator for both
   * `hubRingRadius` and the sidebar's own relative sizing, so a hub ring and
   * its sidebar row can never disagree about what "biggest" means. Never 0:
   * a HubSet with any hubs at all has a nonzero max by construction
   * (HUB_FANIN_THRESHOLD is itself positive). */
  maxFi: number;
}

/** Every hub in `doc`, ranked by fan-in descending (ties by file index --
 * CLAUDE.md's determinism rule: two files can tie on fan-in and nothing
 * upstream promises a stable iteration order otherwise). Called once per
 * document (MapRenderer.loadDocument), not per paint. */
export function computeHubs(doc: MapDocument): HubSet {
  const fi = doc.N.map((_, i) => FI(doc, i));
  const ids = doc.N.map((_, i) => i).filter((i) => fi[i] >= HUB_FANIN_THRESHOLD);
  ids.sort((a, b) => fi[b] - fi[a] || a - b);

  const basenames = new Map<number, string>();
  const basenameCounts = new Map<string, number>();
  for (const i of ids) {
    const base = doc.F[i].split("/").pop()!;
    basenames.set(i, base);
    basenameCounts.set(base, (basenameCounts.get(base) ?? 0) + 1);
  }

  const hubs: HubInfo[] = ids.map((i) => {
    const parts = doc.F[i].split("/");
    const base = basenames.get(i)!;
    const ambiguous = isGenericBasename(base) || (basenameCounts.get(base) ?? 0) > 1;
    const name = ambiguous && parts.length >= 2 ? `${parts[parts.length - 2]}/${base}` : base;
    return { i, fi: fi[i], name };
  });

  return { hubs, maxFi: Math.max(1, ...hubs.map((h) => h.fi)) };
}

/** Ring radius in on-screen px, `(3.5 + 9*sqrt(FI/maxFI))` (spec), scaled
 * 0.72x on narrow (phone) screens -- the same physical-size-on-a-small-
 * screen adjustment other fixed-px UI in this renderer makes (see
 * pins.ts/constants.ts's own narrow-screen ratios). Radius alone; callers
 * set an explicit stroke-width in px themselves (spec: "an unset
 * stroke-width on a circle bit the prototype" -- SVG's default is 1
 * user-unit, not 1px, and this renderer's user units ARE world units before
 * the root `<g>`'s transform is applied at the call site, so an unset
 * width would scale with zoom instead of staying constant). */
export function hubRingRadius(fi: number, maxFi: number, narrow: boolean): number {
  const r = 3.5 + 9 * Math.sqrt(fi / (maxFi || 1));
  return narrow ? r * 0.72 : r;
}
