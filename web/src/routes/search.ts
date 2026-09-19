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
  d?: number;
  geo: Geo;
  layer: Layer;
}

const GEOS: Geo[] = ["r", "p", "t"];
const LAYERS: Layer[] = ["d", "c", "x"];

export function validateMapSearch(search: Record<string, unknown>): MapSearch {
  const geo = GEOS.includes(search.geo as Geo) ? (search.geo as Geo) : "r";
  const layer = LAYERS.includes(search.layer as Layer) ? (search.layer as Layer) : "d";
  const file = typeof search.file === "string" && search.file.length > 0 ? search.file : undefined;
  const symRaw = Number(search.sym);
  const sym = file && Number.isInteger(symRaw) && symRaw >= 0 ? symRaw : undefined;
  const dRaw = Number(search.d);
  const d = !file && Number.isInteger(dRaw) && dRaw >= 0 ? dRaw : undefined;
  return { file, sym, d, geo, layer };
}
