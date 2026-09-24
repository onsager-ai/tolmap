// The map JSON schema is defined once, in Rust, and generated to these
// bindings (ts-rs) at ../../bindings/*.ts. Re-exporting instead of
// hand-writing a second definition is a CLAUDE.md rule, not a preference —
// the two would drift the first time a field is added on the Rust side.
export type { MapDocument } from "@bindings/MapDocument";
export type { District } from "@bindings/District";
export type { DistrictClass } from "@bindings/DistrictClass";
export type { Neighbourhood } from "@bindings/Neighbourhood";
export type { NodeRow } from "@bindings/NodeRow";
export type { LandmarkRow } from "@bindings/LandmarkRow";
export type { RoadRow } from "@bindings/RoadRow";
export type { SymbolRow } from "@bindings/SymbolRow";
// Symbol cards (#82 C2): the hierarchical symbols sibling document, fetched
// one district at a time (docs/API.md's `/symbols?district=` route and the
// static `<repo>.symbols/<district>.json` files) -- a separate document from
// MapDocument's own oracle-constrained `S`/`U` (finding 30), never merged
// into it.
export type { DistrictSymbols } from "@bindings/DistrictSymbols";
export type { HierSymbolRow } from "@bindings/HierSymbolRow";
export type { SymbolCoverage } from "@bindings/SymbolCoverage";

// Issue #97 (job progress, ETA, cancel): the job service's snapshot shape,
// generated the same way -- src/service/jobs.rs's JobSnapshot is the one
// definition, ts-rs emits these bindings, and src/api/client.ts re-exports
// them instead of the hand-written `JobStatus` interface this replaced.
export type { JobSnapshot } from "@bindings/JobSnapshot";
export type { JobStatus } from "@bindings/JobStatus";
export type { StageSnapshot } from "@bindings/StageSnapshot";
export type { StageState } from "@bindings/StageState";
export type { ProgressValue } from "@bindings/ProgressValue";
export type { StageId } from "@bindings/StageId";
export type { Eta } from "@bindings/Eta";
export type { EtaBasis } from "@bindings/EtaBasis";

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
