#!/usr/bin/env node
// Issue #110 P2: a small synthetic fixture carrying `coverage.references`,
// for the screenshot check and manual review only -- no committed map (the
// nine in data/, django/dify's pinned check-view-stability fixtures) was
// ever built with `--refs scip`, so none has this field to screenshot.
//
// Derived from data/flask.json (24 files, the smallest committed fixture)
// by ADDING a synthetic `coverage` block only -- every other key (F, N, E,
// L, S, U, districts, q, roads, lang, P) is copied through unchanged, so
// this is not a second copy of the map schema and stays byte-identical to
// flask.json wherever this PR doesn't touch it. The "js" language and its
// recall numbers are invented (flask has no JS files); the point is
// exercising the indicator's "Partly exact" state (one exact, one fallback
// with a reason and no recall) end to end, not modelling a real repo.
//
// Never hand-edit the output file: rerun this script if the indicator's
// data shape changes (e.g. a new ReferenceCoverage field). Regenerate:
//   node web/scripts/make-scip-coverage-fixture.mjs
import { readFile, writeFile } from "node:fs/promises";
import { gzipSync } from "node:zlib";
import path from "node:path";
import { fileURLToPath } from "node:url";

const WEB_ROOT = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const REPO_ROOT = path.resolve(WEB_ROOT, "..");
const SRC = path.join(REPO_ROOT, "data", "flask.json");
const OUT = path.join(WEB_ROOT, "check-fixtures", "scip-coverage__flask.json.gz");

const doc = JSON.parse(await readFile(SRC, "utf8"));

doc.coverage = {
  zero_edge_files: 0,
  total_files: doc.F.length,
  by_language: {
    py: { zero_edge_files: 0, total_files: doc.F.length },
  },
  references: {
    // Exact: SCIP ran, the fallback gate (scip_ingest::gate) admitted it.
    py: {
      path: "scip",
      reason: "indexed",
      indexer: "scip-python 0.5.6",
      exit_code: 0,
      files: doc.F.length,
      files_indexed: doc.F.length,
      hand_pairs: 102,
      scip_pairs: 99,
      recall: 0.9706,
      min_recall: 0.85,
      granularity: "file",
    },
    // Heuristic fallback: the binary was never on PATH
    // (src/indexers.rs's IndexFailure::NotFound -> "indexer_not_found"),
    // so nothing was ingested -- files_indexed/scip_pairs/recall/indexer
    // all stay null/None, exactly as extract.rs's union_references leaves
    // `row` when `crate::indexers::run` returns `Err`.
    js: {
      path: "hand",
      reason: "indexer_not_found",
      indexer: null,
      exit_code: null,
      files: 3,
      files_indexed: null,
      hand_pairs: 5,
      scip_pairs: null,
      recall: null,
      min_recall: 0.85,
      granularity: "file",
    },
  },
};
// repo/names carry the real "flask" slug; give this variant its own name so
// it's never mistaken for the real pallets/flask fixture (collect-maps.mjs
// never reads this file -- it's a check-fixtures/ file, unpacked directly
// by viewer-check.yml -- but the map's own `repo` field should still say
// what it actually is).
doc.repo = "scip-coverage/flask";

const json = JSON.stringify(doc);
await writeFile(OUT, gzipSync(Buffer.from(json)));
console.log(`wrote ${OUT} (${json.length} bytes uncompressed)`);
