import type { MapDocument } from "@/types";
import { D_, districtClass } from "./geometry";
import type { Layer } from "./constants";
import type { PackageGrouping } from "./packageLayout";

// src/extract.rs passes this maximum to git_history(..., max_commits, ...).
// git log excludes merge commits before applying the limit.
export const CHURN_HISTORY_WINDOW_COMMITS = 4_000;

// The overview keeps the same ten-file cap at every profile; these are index
// rows, not a claim that the map document contains a complete file history.
export const LAYER_OVERVIEW_FILE_LIMIT = 10;

export interface DistrictOverviewStats {
  mainlandDistricts: number;
  files: number;
}

export interface ChurnDistrictRow {
  district: number;
  name: string;
  knownFiles: number;
  commits: number;
  commitsPerFile: number;
  bar: number;
}

export interface MetricFileRow {
  file: number;
  district: number;
  name: string;
  value: number;
}

export interface ComplexityDistrictRow {
  district: number;
  name: string;
  knownFiles: number;
  median: number;
}

export interface PackageOverviewRow {
  path: string | null;
  other: boolean;
  files: number;
  districtIds: readonly number[];
}

export interface MixedPackageDistrictRow {
  district: number;
  name: string;
  files: number;
  packages: readonly (string | null)[];
}

export interface LayerOverview {
  district: DistrictOverviewStats;
  churn: {
    windowCommits: number;
    knownFiles: number;
    /** Sum of per-file commit counts; a commit touching multiple files is
     * counted once for each file because MapDocument stores no commit IDs. */
    fileCommitTouches: number;
    districts: readonly ChurnDistrictRow[];
    mostActiveDistrict: ChurnDistrictRow | null;
    files: readonly MetricFileRow[];
  };
  complexity: {
    median: number | null;
    districts: readonly ComplexityDistrictRow[];
    mostComplexDistrict: ComplexityDistrictRow | null;
    files: readonly MetricFileRow[];
  };
  package: {
    language: string | null;
    groups: readonly PackageOverviewRow[];
    mixedDistricts: readonly MixedPackageDistrictRow[];
  };
}

function compareText(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

function districtName(doc: MapDocument, district: number): string {
  return doc.names[String(district)] ?? `District ${district}`;
}

function metricValue(doc: MapDocument, file: number, column: 4 | 5): number | null {
  const value: unknown = doc.N[file]?.[column];
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}

function median(values: readonly number[]): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? (sorted[middle - 1] + sorted[middle]) / 2
    : sorted[middle];
}

function compareDistrictNames(a: { name: string; district: number }, b: { name: string; district: number }): number {
  return compareText(a.name, b.name) || a.district - b.district;
}

function metricFiles(doc: MapDocument, column: 4 | 5): MetricFileRow[] {
  const files: MetricFileRow[] = [];
  for (let file = 0; file < doc.F.length; file++) {
    const value = metricValue(doc, file, column);
    if (value == null) continue;
    files.push({ file, district: D_(doc, file), name: doc.F[file], value });
  }
  return files.sort((a, b) => b.value - a.value || compareText(a.name, b.name) || a.file - b.file)
    .slice(0, LAYER_OVERVIEW_FILE_LIMIT);
}

/** Derive the layer summaries from one loaded map. Package membership comes
 * from packageLayout.ts's already-computed grouping so its depth, package
 * cap and "other" bucket stay identical to the Package renderer layer. */
export function buildLayerOverview(doc: MapDocument, grouping: PackageGrouping): LayerOverview {
  const districtFiles = new Map<number, number[]>();
  for (let file = 0; file < doc.N.length; file++) {
    const district = D_(doc, file);
    const members = districtFiles.get(district);
    if (members) members.push(file);
    else districtFiles.set(district, [file]);
  }

  const district: DistrictOverviewStats = {
    mainlandDistricts: Object.values(doc.districts).filter((entry) => districtClass(entry) === "mainland").length,
    files: doc.F.length,
  };

  const churnByDistrict: ChurnDistrictRow[] = [];
  let knownChurnFiles = 0;
  let fileCommitTouches = 0;
  for (const [id, members] of districtFiles) {
    let commits = 0;
    let knownFiles = 0;
    for (const file of members) {
      const value = metricValue(doc, file, 5);
      if (value == null) continue;
      commits += value;
      knownFiles++;
    }
    if (knownFiles === 0) continue;
    knownChurnFiles += knownFiles;
    fileCommitTouches += commits;
    churnByDistrict.push({
      district: id,
      name: districtName(doc, id),
      knownFiles,
      commits,
      commitsPerFile: commits / knownFiles,
      bar: 0,
    });
  }
  churnByDistrict.sort((a, b) => b.commits - a.commits || compareDistrictNames(a, b));
  const maxDistrictCommits = churnByDistrict[0]?.commits ?? 0;
  for (const row of churnByDistrict) row.bar = maxDistrictCommits > 0 ? row.commits / maxDistrictCommits : 0;

  const complexityByDistrict: ComplexityDistrictRow[] = [];
  const allComplexities: number[] = [];
  for (const [id, members] of districtFiles) {
    const values = members.flatMap((file) => {
      const value = metricValue(doc, file, 4);
      return value == null ? [] : [value];
    });
    if (values.length === 0) continue;
    allComplexities.push(...values);
    complexityByDistrict.push({
      district: id,
      name: districtName(doc, id),
      knownFiles: values.length,
      median: median(values)!,
    });
  }
  complexityByDistrict.sort((a, b) => b.median - a.median || compareDistrictNames(a, b));

  const shownPackages = new Set(grouping.groups.flatMap((row) => row.path == null ? [] : [row.path]));
  const packageGroups: PackageOverviewRow[] = grouping.groups.map((group) => {
    const members: number[] = [];
    for (let file = 0; file < doc.F.length; file++) {
      const filePackage = grouping.filePackages[file];
      const matches = group.path == null ? !shownPackages.has(filePackage) : filePackage === group.path;
      if (matches) members.push(file);
    }
    const districtIds = [...new Set(members.map((file) => D_(doc, file)))].sort((a, b) =>
      compareDistrictNames(
        { district: a, name: districtName(doc, a) },
        { district: b, name: districtName(doc, b) },
      ),
    );
    return { path: group.path, other: group.other, files: group.count, districtIds };
  });

  const packagesByDistrict = new Map<number, Set<string | null>>();
  for (let file = 0; file < doc.F.length; file++) {
    const filePackage = grouping.filePackages[file];
    if (filePackage == null) continue;
    // Match the renderer's collapsed "other" bucket as one package group.
    const renderedPackage = shownPackages.has(filePackage) ? filePackage : null;
    const set = packagesByDistrict.get(D_(doc, file)) ?? new Set<string | null>();
    set.add(renderedPackage);
    packagesByDistrict.set(D_(doc, file), set);
  }
  const mixedDistricts: MixedPackageDistrictRow[] = [...packagesByDistrict]
    .filter(([, packages]) => packages.size > 1)
    .map(([id, packages]) => ({
      district: id,
      name: districtName(doc, id),
      files: districtFiles.get(id)?.length ?? 0,
      packages: [...packages].sort((a, b) => {
        if (a == null) return b == null ? 0 : 1;
        if (b == null) return -1;
        return compareText(a, b);
      }),
    }))
    .sort((a, b) => b.packages.length - a.packages.length || compareDistrictNames(a, b));

  // Coverage identifies an exact single-language map. In a polyglot map,
  // `lang` is only the dominant language and the schema does not retain
  // language per file, so showing it per package would claim information
  // the document does not carry. Older documents without this breakdown
  // also omit the package language.
  const languages = Object.keys(doc.coverage?.by_language ?? {});
  const packageLanguage = languages.length === 1 ? languages[0] : null;

  return {
    district,
    churn: {
      windowCommits: CHURN_HISTORY_WINDOW_COMMITS,
      knownFiles: knownChurnFiles,
      fileCommitTouches,
      districts: churnByDistrict,
      mostActiveDistrict: churnByDistrict[0] ?? null,
      files: metricFiles(doc, 5),
    },
    complexity: {
      median: median(allComplexities),
      districts: complexityByDistrict,
      mostComplexDistrict: complexityByDistrict[0] ?? null,
      files: metricFiles(doc, 4),
    },
    package: {
      language: packageLanguage,
      groups: packageGroups,
      mixedDistricts,
    },
  };
}

export function layerOverviewHeadline(overview: LayerOverview, layer: Layer): { primary: string; secondary?: string } {
  const number = (value: number) => value.toLocaleString("en-US", { maximumFractionDigits: 1 });
  switch (layer) {
    case "d":
      return { primary: `${overview.district.mainlandDistricts} districts · ${overview.district.files.toLocaleString("en-US")} files` };
    case "c":
      if (overview.churn.knownFiles === 0) return { primary: "No churn data" };
      return {
        primary: `Churn · up to ${overview.churn.windowCommits.toLocaleString("en-US")} non-merge commits`,
        ...(overview.churn.mostActiveDistrict ? { secondary: `Most active district: ${overview.churn.mostActiveDistrict.name}` } : {}),
      };
    case "x":
      return {
        primary: overview.complexity.median == null
          ? "No complexity data"
          : `Median complexity per file · ${number(overview.complexity.median)}`,
        ...(overview.complexity.mostComplexDistrict ? { secondary: `Most complex district: ${overview.complexity.mostComplexDistrict.name}` } : {}),
      };
    case "p":
      return {
        primary: `${overview.package.groups.length.toLocaleString("en-US")} package groups · ${overview.package.groups.reduce((sum, row) => sum + row.files, 0).toLocaleString("en-US")} files`,
        secondary: `${overview.package.mixedDistricts.length.toLocaleString("en-US")} districts mix packages`,
      };
  }
}

export function displayLanguage(language: string): string {
  const labels: Record<string, string> = { py: "Python", ts: "TypeScript", go: "Go", rs: "Rust", js: "JavaScript" };
  return labels[language] ?? language;
}
