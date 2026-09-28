#!/usr/bin/env node
// Pure checks for the map-document overview calculations on each layer.

import { buildLayerOverview, CHURN_HISTORY_WINDOW_COMMITS, LAYER_OVERVIEW_FILE_LIMIT, LAYER_OVERVIEW_MIN_DISTRICT_FILES, layerOverviewHeadline } from "../src/map/layerOverview";
import { districtIndexDistrictIds } from "../src/map/districtIndex";
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

function checkDistrictScope(label: string, fixtureDoc: MapDocument, fixtureGrouping: PackageGrouping, fixtureOverview: ReturnType<typeof buildLayerOverview>) {
  const districtIndex = new Set(districtIndexDistrictIds(fixtureDoc));
  const headlineCount = fixtureOverview.district.mainlandDistricts + fixtureOverview.district.islandDistricts;
  const packageSpans = new Set(fixtureOverview.package.groups.flatMap((row) => row.districtIds));
  report(districtIndex.size === headlineCount, `${label}: District headline count matches the District index domain`, { index: districtIndex.size, headlineCount });
  report(packageSpans.size <= headlineCount, `${label}: distinct districts across package spans do not exceed the District headline count`, { spanCount: packageSpans.size, headlineCount });
  report([...packageSpans].every((id) => districtIndex.has(id)), `${label}: package spans only use District index districts`, [...packageSpans]);
  report(fixtureOverview.churn.districts.every((row) => districtIndex.has(row.district)), `${label}: churn ranked districts are a subset of the District index`);
  report(fixtureOverview.complexity.districts.every((row) => districtIndex.has(row.district)), `${label}: complexity ranked districts are a subset of the District index`);
  report(fixtureOverview.package.mixedDistricts.every((row) => districtIndex.has(row.district)), `${label}: mixed-package rows are a subset of the District index`);
  const rankedFiles = [...fixtureOverview.churn.files, ...fixtureOverview.complexity.files];
  report(rankedFiles.every((row) => {
    const district = fixtureDoc.N[row.file]?.[0];
    return district != null && districtIndex.has(district);
  }), `${label}: ranked file rows belong to District index districts`);
  report(fixtureGrouping.filePackages.length === fixtureDoc.F.length, `${label}: package grouping covers every file`, { packages: fixtureGrouping.filePackages.length, files: fixtureDoc.F.length });
}

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
report(overview.district.mainlandDistricts === 3 && overview.district.islandDistricts === 0 && overview.district.files === 6, "District keeps its current district and file counts", overview.district);
report(overview.churn.windowCommits === CHURN_HISTORY_WINDOW_COMMITS && overview.churn.windowCommits === 4000, "Churn names the indexer's 4,000 non-merge commit window", overview.churn.windowCommits);
report(overview.churn.knownFiles === 5 && overview.churn.fileCommitTouches === 16, "missing churn is excluded while per-file commit counts remain exact", overview.churn);
report(overview.churn.districts.length === 0 && overview.churn.smallDistrictCount === 2, "churn excludes the two small districts that have metric values", overview.churn);
report(overview.churn.files.length === 5 && overview.churn.files[0].name === "packages/c/src/c.ts", "changed files rank by churn and omit the file without churn", overview.churn.files);
report(eq(overview.churn.files.slice(2).map((row) => row.name), ["packages/a/test/a.test.ts", "packages/b/src/b.ts", "packages/c/src/d.ts"]), "equal churn file values sort by path", overview.churn.files);
report(LAYER_OVERVIEW_FILE_LIMIT === 10 && overview.churn.files.length <= LAYER_OVERVIEW_FILE_LIMIT, "file indexes use the named ten-row limit", overview.churn.files.length);
report(LAYER_OVERVIEW_MIN_DISTRICT_FILES === 10, "district rankings use the named ten-file minimum", LAYER_OVERVIEW_MIN_DISTRICT_FILES);

report(overview.complexity.median === 4, "complexity uses the median of file values", overview.complexity.median);
report(overview.complexity.districts.length === 0 && overview.complexity.smallDistrictCount === 3, "complexity excludes all three small districts that have values", overview.complexity);
report(overview.complexity.files[0].name === "packages/z/z.ts" && !overview.complexity.files.some((row) => row.file === 4), "complex files rank by value and omit a missing value", overview.complexity.files);
report(eq(overview.complexity.files.slice(2, 5).map((row) => row.name), ["packages/a/test/a.test.ts", "packages/b/src/b.ts", "packages/c/src/c.ts"]), "equal complexity file values sort by path", overview.complexity.files);

// A larger fixture keeps meaningful district rankings while putting a tiny
// island and a high-scoring unconnected group beside them. The island belongs
// to the District index but falls below the ranking floor; unconnected groups
// do not belong to that index at all.
const rankFiles: string[] = [];
const rankRows: (number | undefined)[][] = [];
const rankPackages: string[] = [];
const rankColors: string[] = [];
const rankCounts = new Map<string, number>();
const rankColorByPackage: Record<string, string> = { "packages/a": "p1", "packages/b": "p2", "packages/c": "p3", "packages/d": "p0" };
function addRankFiles(district: number, packagePath: string, count: number, complexity: number, churn: number) {
  for (let i = 0; i < count; i++) {
    rankFiles.push(`${packagePath}/file-${String(i).padStart(2, "0")}.ts`);
    rankRows.push([district, 0, 0, 1, complexity, churn, 0, 0, 0, 1, 1]);
    rankPackages.push(packagePath);
    rankColors.push(rankColorByPackage[packagePath]);
    rankCounts.set(packagePath, (rankCounts.get(packagePath) ?? 0) + 1);
  }
}
addRankFiles(0, "packages/a", 10, 4, 1);
addRankFiles(1, "packages/a", 6, 4, 1);
addRankFiles(1, "packages/b", 4, 4, 1);
addRankFiles(2, "packages/c", 4, 99, 99);
addRankFiles(3, "packages/d", 20, 999, 999);
const rankDoc = {
  repo: "example/rank-fixture",
  q: 0.5,
  names: { "0": "Alpha", "1": "Beta", "2": "Tiny island", "3": "Unconnected group" },
  districts: {
    "0": { size: 10, c: [0, 0], blob: [], class: "mainland" },
    "1": { size: 10, c: [0, 0], blob: [], class: "mainland" },
    "2": { size: 4, c: [0, 0], blob: [], class: "island" },
    "3": { size: 20, c: [0, 0], blob: [], class: "unconnected" },
  },
  F: rankFiles,
  N: rankRows,
  E: [],
  L: [],
  S: {},
  U: {},
  roads: [],
  lang: "ts",
  coverage: { zero_edge_files: 0, total_files: rankFiles.length, by_language: { ts: { zero_edge_files: 0, total_files: rankFiles.length } } },
  P: null,
} as unknown as MapDocument;
const rankGrouping: PackageGrouping = {
  depth: 2,
  groups: [...rankCounts]
    .sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0))
    .map(([path, count]) => ({ path, count, color: rankColorByPackage[path], other: false })),
  filePackages: rankPackages,
  fileColors: rankColors,
};
const rankOverview = buildLayerOverview(rankDoc, rankGrouping);
report(rankOverview.district.mainlandDistricts === 2 && rankOverview.district.islandDistricts === 1, "District headline labels mainland districts and islands separately", rankOverview.district);
report(layerOverviewHeadline(rankOverview, "d").primary === "2 districts + 1 island · 44 files", "District headline text counts the separate island set", layerOverviewHeadline(rankOverview, "d"));
report(eq(rankOverview.churn.districts.map((row) => row.name), ["Alpha", "Beta"]) && rankOverview.churn.smallDistrictCount === 0 && rankOverview.churn.smallIslandCount === 1, "churn ranks only districts at or above the minimum and reports the excluded island", rankOverview.churn);
report(rankOverview.churn.mostActiveDistrict?.name === "Alpha", "equal churn totals sort by district name after size filtering", rankOverview.churn.mostActiveDistrict);
report(eq(rankOverview.complexity.districts.map((row) => row.name), ["Alpha", "Beta"]) && rankOverview.complexity.smallDistrictCount === 0 && rankOverview.complexity.smallIslandCount === 1, "complexity ranks only districts at or above the minimum and reports the excluded island", rankOverview.complexity);
report(rankOverview.complexity.mostComplexDistrict?.name === "Alpha", "equal complexity medians sort by district name after size filtering", rankOverview.complexity.mostComplexDistrict);
report(!rankOverview.churn.districts.some((row) => row.name === "Unconnected group") && !rankOverview.complexity.districts.some((row) => row.name === "Unconnected group"), "unconnected groups are excluded because they have no District index row");
report(rankOverview.package.groups.find((row) => row.path === "packages/c")?.districtCount === 0 && rankOverview.package.groups.find((row) => row.path === "packages/c")?.islandCount === 1, "package spans count islands separately from districts", rankOverview.package.groups);
report(rankOverview.package.groups.find((row) => row.path === "packages/d")?.districtIds.length === 0, "package files in unconnected groups do not inflate District index spans", rankOverview.package.groups);

report(eq(overview.package.groups.slice(0, 4).map((row) => [row.path, row.files]), [
  ["packages/a", 2], ["packages/c", 2], ["packages/b", 1], ["packages/z", 1],
]), "package rows preserve renderer grouping and deterministic tie order", overview.package.groups);
report(overview.package.groups.find((row) => row.path === "packages/c")?.color === "c", "package rows keep the renderer's exact group swatch", overview.package.groups);
report(overview.package.groups.every((row) => row.language === "ts"), "each package row shows the known single repository language", overview.package.groups);
report(overview.package.groups.find((row) => row.path === "packages/c")?.districtIds[0] === 1, "package rows count their distinct districts", overview.package.groups);
report(overview.package.mixedDistricts.length === 1 && overview.package.mixedDistricts[0].name === "Alpha" && eq(overview.package.mixedDistricts[0].packages, ["packages/a", "packages/b"]), "the package index identifies districts that mix packages", overview.package.mixedDistricts);
const collapsedGrouping: PackageGrouping = {
  ...grouping,
  groups: [
    { path: "packages/a", count: 2, color: "a", other: false },
    { path: null, count: 4, color: "var(--dim)", other: true },
  ],
};
const collapsedOverview = buildLayerOverview(doc, collapsedGrouping);
report(eq(collapsedOverview.package.mixedDistricts[0].packages, ["packages/a", null]), "mixed districts use the renderer's collapsed Other package grouping", collapsedOverview.package.mixedDistricts);
report(overview.package.language === "ts", "single-language coverage identifies the package language", overview.package.language);
const noLanguageOverview = buildLayerOverview({ ...doc, coverage: undefined } as unknown as MapDocument, grouping);
report(noLanguageOverview.package.language === null && noLanguageOverview.package.groups.every((row) => row.language === null), "package language is omitted when the document has no language assignment");
const partialLanguageDoc = {
  ...doc,
  coverage: { zero_edge_files: 0, total_files: 5, by_language: { ts: { zero_edge_files: 0, total_files: 5 } } },
} as unknown as MapDocument;
const partialLanguageOverview = buildLayerOverview(partialLanguageDoc, grouping);
report(partialLanguageOverview.package.groups.every((row) => row.language === null), "a language total that does not account for every file is not copied onto package rows");
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
const polyglotOverview = buildLayerOverview(polyglotDoc, grouping);
report(polyglotOverview.package.language === null && polyglotOverview.package.groups.every((row) => row.language === null), "polyglot package language is omitted because the map has no per-file language field");
report(layerOverviewHeadline(overview, "c").primary.includes("4,000") && layerOverviewHeadline(overview, "c").primary !== layerOverviewHeadline(overview, "d").primary, "layer headlines change with the active layer", overview);
report(layerOverviewHeadline(overview, "x").primary.includes("4") && layerOverviewHeadline(overview, "p").primary === "4 package groups · 6 files" && layerOverviewHeadline(overview, "p").secondary?.includes("1 district + 0 islands"), "complexity and package headlines use their own derived stats", overview);

checkDistrictScope("small fixture", doc, grouping, overview);
checkDistrictScope("rank fixture", rankDoc, rankGrouping, rankOverview);
checkDistrictScope("collapsed-package fixture", doc, collapsedGrouping, collapsedOverview);
checkDistrictScope("no-language fixture", { ...doc, coverage: undefined } as unknown as MapDocument, grouping, noLanguageOverview);
checkDistrictScope("partial-language fixture", partialLanguageDoc, grouping, partialLanguageOverview);
checkDistrictScope("polyglot fixture", polyglotDoc, grouping, polyglotOverview);

console.log(`\n${checks - failures}/${checks} checks passed`);
if (failures) process.exitCode = 1;
