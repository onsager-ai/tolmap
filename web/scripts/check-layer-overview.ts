#!/usr/bin/env node
// Pure checks for the map-document overview calculations on each layer.

import { buildLayerOverview, CHURN_HISTORY_WINDOW_COMMITS, LAYER_OVERVIEW_FILE_LIMIT, layerOverviewHeadline } from "../src/map/layerOverview";
import type { PackageGrouping } from "../src/map/packageLayout";
import type { MapDocument } from "../src/types";

let failures = 0;
let checks = 0;
function report(ok: boolean, label: string, got?: unknown) {
  checks++;
  if (ok) console.log(`  ok    ${label}`);
  else {
    failures++;
    console.log(`  FAIL  ${label}${got === undefined ? "" : ` -- ${JSON.stringify(got)}`}`);
  }
}
const eq = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

const files = [
  "packages/a/src/a.ts",
  "packages/b/src/b.ts",
  "packages/a/test/a.test.ts",
  "packages/c/src/c.ts",
  "packages/c/src/d.ts",
  "packages/z/z.ts",
];
const rows = [
  [0, 0, 0, 10, 8, 4, 0, 0, 0, 1, 1],
  [0, 0, 0, 10, 4, 2, 0, 0, 0, 1, 1],
  [0, 0, 0, 10, 4, 2, 0, 0, 0, 1, 1],
  [1, 0, 0, 10, 4, 6, 0, 0, 0, 1, 1],
  [1, 0, 0, 10, undefined, 2, 0, 0, 0, 1, 1],
  [2, 0, 0, 10, 99, undefined, 0, 0, 0, 1, 1],
];
const doc = {
  repo: "example/repo",
  q: 0.5,
  names: { "0": "Alpha", "1": "Beta", "2": "Gamma" },
  districts: {
    "0": { size: 3, c: [0, 0], blob: [], class: "mainland" },
    "1": { size: 2, c: [0, 0], blob: [], class: "mainland" },
    "2": { size: 1, c: [0, 0], blob: [], class: "mainland" },
  },
  F: files,
  N: rows,
  E: [],
  L: [],
  S: {},
  U: {},
  roads: [],
  lang: "ts",
  coverage: {
    zero_edge_files: 0,
    total_files: 6,
    by_language: { ts: { zero_edge_files: 0, total_files: 6 } },
  },
  P: null,
} as unknown as MapDocument;
const grouping: PackageGrouping = {
  depth: 2,
  groups: [
    { path: "packages/a", count: 2, color: "a", other: false },
    { path: "packages/c", count: 2, color: "c", other: false },
    { path: "packages/b", count: 1, color: "b", other: false },
    { path: "packages/z", count: 1, color: "z", other: false },
  ],
  filePackages: ["packages/a", "packages/b", "packages/a", "packages/c", "packages/c", "packages/z"],
  fileColors: ["a", "b", "a", "c", "c", "z"],
};

const overview = buildLayerOverview(doc, grouping);
report(overview.district.mainlandDistricts === 3 && overview.district.files === 6, "District keeps its current district and file counts", overview.district);
report(overview.churn.windowCommits === CHURN_HISTORY_WINDOW_COMMITS && overview.churn.windowCommits === 4000, "Churn names the indexer's 4,000 non-merge commit window", overview.churn.windowCommits);
report(overview.churn.knownFiles === 5 && overview.churn.fileCommitTouches === 16, "missing churn is excluded while per-file commit counts remain exact", overview.churn);
report(eq(overview.churn.districts.slice(0, 2).map((row) => row.name), ["Alpha", "Beta"]), "equal churn totals sort by district name", overview.churn.districts);
report(overview.churn.mostActiveDistrict?.district === 0, "the most active district is the first deterministic ranked row", overview.churn.mostActiveDistrict);
report(overview.churn.files.length === 5 && overview.churn.files[0].name === "packages/c/src/c.ts", "changed files rank by churn and omit the file without churn", overview.churn.files);
report(eq(overview.churn.files.slice(2).map((row) => row.name), ["packages/a/test/a.test.ts", "packages/b/src/b.ts", "packages/c/src/d.ts"]), "equal churn file values sort by path", overview.churn.files);
report(LAYER_OVERVIEW_FILE_LIMIT === 10 && overview.churn.files.length <= LAYER_OVERVIEW_FILE_LIMIT, "file indexes use the named ten-row limit", overview.churn.files.length);

report(overview.complexity.median === 4, "complexity uses the median of file values", overview.complexity.median);
report(overview.complexity.mostComplexDistrict?.district === 2, "the most complex district ranks by its median", overview.complexity.mostComplexDistrict);
report(eq(overview.complexity.districts.slice(1).map((row) => row.name), ["Alpha", "Beta"]), "equal district medians sort by district name", overview.complexity.districts);
report(overview.complexity.files[0].name === "packages/z/z.ts" && !overview.complexity.files.some((row) => row.file === 4), "complex files rank by value and omit a missing value", overview.complexity.files);
report(eq(overview.complexity.files.slice(2, 5).map((row) => row.name), ["packages/a/test/a.test.ts", "packages/b/src/b.ts", "packages/c/src/c.ts"]), "equal complexity file values sort by path", overview.complexity.files);

report(eq(overview.package.groups.slice(0, 4).map((row) => [row.path, row.files]), [
  ["packages/a", 2], ["packages/c", 2], ["packages/b", 1], ["packages/z", 1],
]), "package rows preserve renderer grouping and deterministic tie order", overview.package.groups);
report(overview.package.groups.find((row) => row.path === "packages/c")?.districtIds[0] === 1, "package rows count their distinct districts", overview.package.groups);
report(overview.package.mixedDistricts.length === 1 && overview.package.mixedDistricts[0].name === "Alpha" && eq(overview.package.mixedDistricts[0].packages, ["packages/a", "packages/b"]), "the package index identifies districts that mix packages", overview.package.mixedDistricts);
const collapsedGrouping: PackageGrouping = {
  ...grouping,
  groups: [
    { path: "packages/a", count: 2, color: "a", other: false },
    { path: null, count: 4, color: "var(--dim)", other: true },
  ],
};
report(eq(buildLayerOverview(doc, collapsedGrouping).package.mixedDistricts[0].packages, ["packages/a", null]), "mixed districts use the renderer's collapsed Other package grouping", buildLayerOverview(doc, collapsedGrouping).package.mixedDistricts);
report(overview.package.language === "ts", "single-language coverage identifies the package language", overview.package.language);
report(buildLayerOverview({ ...doc, coverage: undefined }, grouping).package.language === null, "package language is omitted when the document has no per-file language breakdown");
const polyglotDoc = {
  ...doc,
  coverage: {
    zero_edge_files: 0,
    total_files: 6,
    by_language: {
      py: { zero_edge_files: 0, total_files: 3 },
      ts: { zero_edge_files: 0, total_files: 3 },
    },
  },
} as unknown as MapDocument;
report(buildLayerOverview(polyglotDoc, grouping).package.language === null, "polyglot package language is omitted because the map has no per-file language field");
report(layerOverviewHeadline(overview, "c").primary.includes("4,000") && layerOverviewHeadline(overview, "c").primary !== layerOverviewHeadline(overview, "d").primary, "layer headlines change with the active layer", overview);
report(layerOverviewHeadline(overview, "x").primary.includes("4") && layerOverviewHeadline(overview, "p").primary === "4 package groups · 6 files" && layerOverviewHeadline(overview, "p").secondary?.includes("1 district"), "complexity and package headlines use their own derived stats", overview);

console.log(`\n${checks - failures}/${checks} checks passed`);
if (failures) process.exitCode = 1;
