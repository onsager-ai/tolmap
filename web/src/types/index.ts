// The map JSON schema is defined once, in Rust, and generated to these
// bindings (ts-rs) at ../../bindings/*.ts. Re-exporting instead of
// hand-writing a second definition is a CLAUDE.md rule, not a preference —
// the two would drift the first time a field is added on the Rust side.
export type { MapDocument } from "@bindings/MapDocument";
export type { District } from "@bindings/District";
export type { NodeRow } from "@bindings/NodeRow";
export type { LandmarkRow } from "@bindings/LandmarkRow";
export type { RoadRow } from "@bindings/RoadRow";
export type { SymbolRow } from "@bindings/SymbolRow";
export type { TerrainArterial } from "@bindings/TerrainArterial";
export type { TerrainDistrict } from "@bindings/TerrainDistrict";
export type { TerrainParcel } from "@bindings/TerrainParcel";
export type { TerrainSubdistrict } from "@bindings/TerrainSubdistrict";

/** One entry in /maps/index.json, emitted by scripts/collect-maps.mjs, or
 * one entry from GET /api/maps normalised to the same shape (see
 * src/data/queries.ts). `source` and `commit` distinguish the two — a slug
 * present in both sources prefers "service" (docs/ARCHITECTURE.md: cache by
 * (repo, commit_sha), so the service copy is the fresher one). */
export interface CatalogueEntry {
  slug: string;
  owner: string;
  repo: string;
  /** Public path under /maps/ this entry's MapDocument was copied to.
   * Empty for a service-sourced entry — it's fetched from the API instead. */
  file: string;
  files: number;
  districts: number;
  modularity: number;
  lang: string;
  source: "static" | "service";
  commit?: string;
}
