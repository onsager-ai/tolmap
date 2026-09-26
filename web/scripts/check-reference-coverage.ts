#!/usr/bin/env -S npx tsx
// Regression check for map/referenceCoverage.ts's summarizeReferenceCoverage():
// the pure function behind the issue #110 P2 "exact (SCIP) vs heuristic"
// indicator. Exercises it directly with scripted CoverageReport shapes
// rather than a fixture file -- no committed map needs a `coverage.references`
// key for this check, only for the screenshot fixture (see
// check-fixtures/README.md's "scip-coverage/flask" entry).
//
// Run: npx tsx web/scripts/check-reference-coverage.ts
// (no test runner exists in this project yet -- see web/README.md -- so this
// is a standalone script, the same pattern as check-pinch-math.ts and
// check-district-colours.ts)

import { summarizeReferenceCoverage } from "../src/map/referenceCoverage";
import type { CoverageReport, ReferenceCoverage } from "../src/types";

let failures = 0;
let checks = 0;

function report(ok: boolean, label: string, detail?: string) {
  checks++;
  if (ok) console.log(`  ok    ${label}`);
  else {
    failures++;
    console.log(`  FAIL  ${label}${detail ? " -- " + detail : ""}`);
  }
}

function refRow(overrides: Partial<ReferenceCoverage>): ReferenceCoverage {
  return {
    path: "hand",
    reason: "",
    indexer: null,
    exit_code: null,
    files: 10,
    files_indexed: null,
    hand_pairs: 5,
    scip_pairs: null,
    recall: null,
    min_recall: 0.85,
    granularity: "file",
    ...overrides,
  };
}

function coverage(references: Record<string, ReferenceCoverage> | null): CoverageReport {
  return {
    zero_edge_files: 0,
    total_files: 10,
    by_language: {},
    references,
  };
}

// 1. No `coverage` at all -- old maps (every fixture in data/ today) must
// produce no summary, so a caller that gates rendering on `!= null` shows
// nothing, exactly as before this indicator existed.
{
  const summary = summarizeReferenceCoverage(null);
  report(summary === null, "coverage: null -> no summary", `got ${JSON.stringify(summary)}`);
}
{
  const summary = summarizeReferenceCoverage(undefined);
  report(summary === null, "coverage: undefined -> no summary", `got ${JSON.stringify(summary)}`);
}

// 2. `coverage` present, `references` absent (or null) -- the `--refs hand`
// default. Collapsed label is "Heuristic references" (spec: "or
// coverage.references is absent"), and there's nothing to list per language.
{
  const summary = summarizeReferenceCoverage(coverage(null));
  report(summary !== null && summary.status === "heuristic", "references absent -> status heuristic", JSON.stringify(summary));
  report(summary?.label === "Heuristic references", "references absent -> label 'Heuristic references'", JSON.stringify(summary));
  report((summary?.languages.length ?? -1) === 0, "references absent -> no per-language rows", JSON.stringify(summary));
}

// 3. Every language on SCIP -> "Exact references", each row reports its
// recall as a rounded percentage and no reason (nothing to explain).
{
  const summary = summarizeReferenceCoverage(
    coverage({
      py: refRow({ path: "scip", reason: "indexed", recall: 0.9781, indexer: "scip-python 0.1" }),
      go: refRow({ path: "scip", reason: "indexed", recall: 1, indexer: "scip-go 0.1" }),
    }),
  );
  report(summary?.status === "exact", "all scip -> status exact", JSON.stringify(summary));
  report(summary?.label === "Exact references", "all scip -> label 'Exact references'", JSON.stringify(summary));
  const py = summary?.languages.find((l) => l.language === "py");
  report(py?.exact === true && py?.reasonLabel === null, "exact row has no reason", JSON.stringify(py));
  report(py?.recallPercent === 98, "0.9781 recall rounds to 98%", JSON.stringify(py));
  const go = summary?.languages.find((l) => l.language === "go");
  report(go?.recallPercent === 100, "1.0 recall rounds to 100%", JSON.stringify(go));
  // Sorted by language key, not insertion/object order.
  report(summary?.languages.map((l) => l.language).join(",") === "go,py", "languages sorted by key", JSON.stringify(summary?.languages));
}

// 4. No language on SCIP (every entry fell back) -> "Heuristic references",
// same as an absent `references` map -- the empty-array vacuous-truth trap
// this function's own doc comment calls out.
{
  const summary = summarizeReferenceCoverage(
    coverage({
      py: refRow({ path: "hand", reason: "indexer_not_found" }),
      go: refRow({ path: "hand", reason: "no_tsconfig" }),
    }),
  );
  report(summary?.status === "heuristic", "all hand -> status heuristic", JSON.stringify(summary));
  report(summary?.label === "Heuristic references", "all hand -> label 'Heuristic references'", JSON.stringify(summary));
}

// 5. A mix -> "Partly exact", and the fallback row explains itself in plain
// words (never the raw reason code) with no recall to show.
{
  const summary = summarizeReferenceCoverage(
    coverage({
      py: refRow({ path: "scip", reason: "indexed", recall: 0.94 }),
      js: refRow({ path: "hand", reason: "indexer_not_found" }),
    }),
  );
  report(summary?.status === "partial", "mixed -> status partial", JSON.stringify(summary));
  report(summary?.label === "Partly exact", "mixed -> label 'Partly exact'", JSON.stringify(summary));
  const js = summary?.languages.find((l) => l.language === "js");
  report(js?.exact === false, "fallback row is not exact", JSON.stringify(js));
  report(js?.reasonLabel === "the indexer isn't installed", "fallback reason is plain words, not a code", JSON.stringify(js));
  report(js?.recallPercent === null, "fallback with no attempt has no recall", JSON.stringify(js));
  report(js?.label === "js", "unrecognised language key falls back to itself", JSON.stringify(js));
}

// 6. below_min_recall: SCIP ran, recall was computed, but the gate rejected
// it -- still `path: "hand"`, still has a recall to show alongside the reason.
{
  const summary = summarizeReferenceCoverage(
    coverage({
      ts: refRow({ path: "hand", reason: "below_min_recall", recall: 0.61, min_recall: 0.85, indexer: "scip-typescript 0.3" }),
    }),
  );
  const ts = summary?.languages.find((l) => l.language === "ts");
  report(ts?.exact === false, "below_min_recall row is not exact", JSON.stringify(ts));
  report(
    ts?.reasonLabel === "the index's recall against the hand-written graph was too low",
    "below_min_recall reason is plain words",
    JSON.stringify(ts),
  );
  report(ts?.recallPercent === 61, "below_min_recall still reports its (rejected) recall", JSON.stringify(ts));
}

// 7. Every reason string src/schema.rs documents resolves to a plain-word
// label, never falls through to the raw-code default (a stale label map
// silently regressing to "below min recall" -> "below min recall" would
// still pass a looser check; this asserts the space-for-underscore fallback
// is never actually hit for a documented reason).
{
  const documented = [
    "below_min_recall",
    "indexer_not_found",
    "indexer_spawn_failed",
    "indexer_failed",
    "no_index_written",
    "no_tsconfig",
    "no_documents",
    "ingest_failed",
    "no_product_indexer",
  ];
  let allHumanised = true;
  for (const reason of documented) {
    const summary = summarizeReferenceCoverage(coverage({ x: refRow({ path: "hand", reason }) }));
    const row = summary?.languages[0];
    if (!row || row.reasonLabel == null || row.reasonLabel.includes("_")) allHumanised = false;
  }
  report(allHumanised, "every documented reason string has a plain-word label (no raw underscore leaks through)");
}

console.log(`\n${checks - failures}/${checks} checks passed`);
if (failures > 0) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
