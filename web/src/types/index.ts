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

/** One entry in /maps/index.json, emitted by scripts/collect-maps.mjs. */
export interface CatalogueEntry {
  slug: string;
  owner: string;
  repo: string;
  /** Public path under /maps/ this entry's MapDocument was copied to. */
  file: string;
  files: number;
  districts: number;
  modularity: number;
  lang: string;
}
