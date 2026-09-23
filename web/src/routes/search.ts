import type { Geo, Layer } from "@/map/constants";

/** Everything selectable on a map view, mirrored into the query string —
 * this closes the "the map cannot be linked to" gap HANDOFF.md lists against
 * the prototype. `file` is a path rather than an index so a link keeps
 * meaning if the fixture is ever regenerated with files in a different
 * order; `sym` and `d` are indices because they are meaningless without the
 * `file` (or the district set) they index into. */
export interface MapSearch {
  file?: string;
  sym?: number;
  /** Issue #82 C2: a GLOBAL hierarchical-symbol index into the district
   * symbols sibling document (docs/API.md's `DistrictSymbols.symbols`,
   * resolved via its index-aligned `symbol_indices`) -- a different index
   * space from `sym`, which indexes the map document's own parity-
   * constrained `S` list and stays exactly as it was for search. `hsym`
   * requires `file` (a symbol makes no sense without the file selection it
   * refines), same as `sym` does. */
  hsym?: number;
  d?: number;
  dir?: string;
  depth?: number;
  geo: Geo;
  layer: Layer;
}

// "r" only: the geometry toggle that used to set "p" (plots) or "t"
// (treemap) is gone from the UI (owner decision, 2026-09-22 -- see
// GeoLayerControls.tsx's GEO_ORDER comment). A stored/shared link that
// still carries ?geo=p or ?geo=t must not land on a hidden mode with no way
// back to it, so those values fall through to the same "r" default an
// omitted geo already gets, exactly as GEOS.includes() below already
// handles any OTHER invalid value -- this is that same fallback, just
// narrowed to accept one fewer value than before. The renderer/schema
// still understand "p"/"t" (MapRenderer.ts is untouched); only the URL
// surface stops offering them.
const GEOS: Geo[] = ["r"];
const LAYERS: Layer[] = ["d", "c", "x", "p"];

export function validateMapSearch(search: Record<string, unknown>): MapSearch {
  const geo = GEOS.includes(search.geo as Geo) ? (search.geo as Geo) : "r";
  const layer = LAYERS.includes(search.layer as Layer) ? (search.layer as Layer) : "d";
  // Accept older ?sel=<path> links as an alias; the UI writes ?file=.
  const selectedPath = search.file ?? search.sel;
  const file = typeof selectedPath === "string" && selectedPath.length > 0 ? selectedPath : undefined;
  const dirRaw = typeof search.dir === "string" ? search.dir.trim().replace(/^\/+|\/+$/g, "") : "";
  const dir = !file && dirRaw ? dirRaw : undefined;
  const symRaw = Number(search.sym);
  const sym = file && Number.isInteger(symRaw) && symRaw >= 0 ? symRaw : undefined;
  const hsymRaw = Number(search.hsym);
  const hsym = file && Number.isInteger(hsymRaw) && hsymRaw >= 0 ? hsymRaw : undefined;
  const dRaw = Number(search.d);
  const d = !file && !dir && Number.isInteger(dRaw) && dRaw >= 0 ? dRaw : undefined;
  const depthRaw = Number(search.depth);
  const depth = Number.isInteger(depthRaw) && depthRaw > 0 ? depthRaw : undefined;
  return { file, sym, hsym, d, dir, depth, geo, layer };
}

/** Search state for the /new progress route (see routes/IndexJobView.tsx).
 * `job` is the id being watched; `slug` (`owner/name`) is carried along
 * only so the view has something to display before the first job status
 * arrives, and to re-POST the same repo on retry after a failure. */
export interface JobSearch {
  job?: string;
  slug?: string;
}

const SLUG_RE = /^[^/\s]+\/[^/\s]+$/;

export function validateJobSearch(search: Record<string, unknown>): JobSearch {
  const job = typeof search.job === "string" && search.job.length > 0 ? search.job : undefined;
  const slug = typeof search.slug === "string" && SLUG_RE.test(search.slug) ? search.slug : undefined;
  return { job, slug };
}
