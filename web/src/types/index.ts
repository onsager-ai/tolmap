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
// Issue #110 P1a/P2: per-language reference-graph provenance
// (`MapDocument.coverage.references`), read by map/referenceCoverage.ts.
export type { CoverageReport } from "@bindings/CoverageReport";
export type { ReferenceCoverage } from "@bindings/ReferenceCoverage";

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

export type { CatalogueEntry } from "@bindings/CatalogueEntry";
export type { RepositoryHistory } from "@bindings/RepositoryHistory";
export type { RepositorySnapshot } from "@bindings/RepositorySnapshot";
export type { RepositoryStatus } from "@bindings/RepositoryStatus";
export type { RepositoryAction } from "@bindings/RepositoryAction";
