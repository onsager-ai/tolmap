// Issue #110 P2: summarises `MapDocument.coverage.references` (issue #110
// P1a, `--refs scip`) into what the "exact vs heuristic" indicator shows.
// Kept pure and separate from any component so `check-reference-coverage.ts`
// can exercise it directly from plain Node, the same reason map/colour.ts
// and map/pinch.ts are pure modules with their own check-*.ts scripts.
//
// Terminology (docs/GLOSSARY.md section 1): "reference" is a code-object
// term (symbol -> symbol / file -> file edges), not a map word -- never
// call these "roads".
import type { CoverageReport, ReferenceCoverage } from "@/types";

export type ReferenceCoverageStatus = "exact" | "partial" | "heuristic";

export interface ReferenceCoverageLanguageRow {
  /** The map's own language key ("py", "go", "ts", ...). */
  language: string;
  /** Display name, e.g. "Python" -- falls back to the raw key for a
   * language this indicator doesn't have a display name for yet, so an
   * unrecognised key still renders something rather than disappearing. */
  label: string;
  /** True when this language's edges came from the SCIP index
   * (`row.path === "scip"`); false means the hand-written resolver, whether
   * because SCIP was never attempted for this document or because it ran
   * and the fallback gate rejected it. */
  exact: boolean;
  /** Plain-word reason for the fallback, e.g. "indexer not installed" --
   * `null` when `exact` is true (nothing to explain) or the reason has no
   * known label yet: the code's stable `reason` string is used verbatim
   * with underscores turned to spaces. See src/schema.rs's `ReferenceCoverage`
   * doc comment for the exact set of reason strings written by scip_ingest.rs
   * / indexers.rs -- this map is deliberately exhaustive against that list,
   * not guessed. */
  reasonLabel: string | null;
  /** Recall against the hand-written graph, 0-100, rounded to the nearest
   * whole percent. `null` when the map records none (SCIP was never
   * attempted for this language, e.g. `indexer_not_found`/`no_tsconfig`). */
  recallPercent: number | null;
}

export interface ReferenceCoverageSummary {
  status: ReferenceCoverageStatus;
  /** The collapsed one-line label (spec: issue #110 P2). */
  label: string;
  /** Per-language detail for the expanded tooltip/popover. Empty when the
   * document carries no `coverage.references` at all (every language used
   * the hand-written resolver by default, nothing to break down). */
  languages: ReferenceCoverageLanguageRow[];
}

const LANGUAGE_NAMES: Record<string, string> = { py: "Python", go: "Go", ts: "TypeScript" };

// Every reason string src/schema.rs documents `ReferenceCoverage.reason`
// can hold, in plain words. Read from src/indexers.rs's `IndexFailure::reason()`
// and src/extract.rs's `union_references` (the only two places that write
// this field) -- never guessed. "indexed" never reaches a fallback reader
// (it's the `path === "scip"` case, which this module explains differently),
// but is listed so an unexpected call site can't produce a raw code instead
// of a sentence.
const REASON_LABELS: Record<string, string> = {
  indexed: "indexed by SCIP",
  below_min_recall: "the index's recall against the hand-written graph was too low",
  indexer_not_found: "the indexer isn't installed",
  indexer_spawn_failed: "the indexer could not start",
  indexer_failed: "the indexer exited with an error",
  no_index_written: "the indexer exited without writing an index",
  no_tsconfig: "no tracked tsconfig.json to index",
  no_documents: "the index had no documents for this repository",
  ingest_failed: "the index could not be read",
};

function reasonLabel(reason: string): string {
  return REASON_LABELS[reason] ?? reason.replace(/_/g, " ");
}

function languageRow(language: string, row: ReferenceCoverage): ReferenceCoverageLanguageRow {
  const exact = row.path === "scip";
  return {
    language,
    label: LANGUAGE_NAMES[language] ?? language,
    exact,
    reasonLabel: exact ? null : reasonLabel(row.reason),
    recallPercent: row.recall == null ? null : Math.round(row.recall * 100),
  };
}

/**
 * `coverage` is the whole `MapDocument.coverage` field (not just
 * `.references`): a document with no `coverage` at all -- every fixture
 * committed to `data/` today, and any map built before this field existed
 * -- must produce no summary, so the caller renders nothing and the map
 * looks exactly as it did before this indicator existed (CLAUDE.md
 * "New data goes in new, optional fields so old documents still load").
 *
 * A document that DOES carry `coverage` but no `.references` (the default,
 * `--refs hand` path -- see CoverageReport's own doc comment) still gets a
 * summary: every language is heuristic, because none took the SCIP path.
 * That's the collapsed-line spec's "or `coverage.references` is absent"
 * clause, not a special case bolted on here -- an empty `languages` array
 * falls into the same "no language is exact" branch as an explicit
 * `references` map where every entry is `path: "hand"`.
 */
export function summarizeReferenceCoverage(
  coverage: CoverageReport | null | undefined,
): ReferenceCoverageSummary | null {
  if (coverage == null) return null;

  const languages = coverage.references
    ? Object.entries(coverage.references)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([language, row]) => languageRow(language, row))
    : [];

  const exactCount = languages.filter((l) => l.exact).length;
  // languages.length === 0 must resolve to "heuristic", not the vacuously
  // true "every language is exact" an unguarded `exactCount === languages.length`
  // would give for an empty array.
  const status: ReferenceCoverageStatus =
    languages.length === 0 ? "heuristic" : exactCount === languages.length ? "exact" : exactCount === 0 ? "heuristic" : "partial";

  const label = status === "exact" ? "Exact references" : status === "partial" ? "Partly exact" : "Heuristic references";

  return { status, label, languages };
}
