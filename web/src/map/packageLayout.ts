import type { MapDocument } from "@/types";
import { D_, districtClass } from "./geometry";

export const PACKAGE_GROUP_LIMIT = 10;
export const PACKAGE_OTHER_COLOR = "var(--dim)";
const DISTRICT_PATH_LIMIT = 5;
const DISTRICT_REFINE_MIN_SHARE = 0.15;
const DISTRICT_SPLIT_MIN_COVERAGE = 0.8;

export interface PackageGroup {
  path: string | null;
  count: number;
  color: string;
  other: boolean;
}

export interface PackageGrouping {
  depth: number;
  groups: readonly PackageGroup[];
  /** One already-resolved colour per file. MapRenderer only indexes this
   * array in its dot loop; it never splits or walks a file path per paint. */
  fileColors: readonly string[];
}

export interface DirectoryNode {
  name: string;
  path: string;
  count: number;
  children: readonly DirectoryNode[];
}

export interface DistrictPathRow {
  path: string | null;
  count: number;
  share: number;
  other: boolean;
}

export interface PackageLayout {
  autoDepth: number;
  minDepth: number;
  maxDepth: number;
  groupings: ReadonlyMap<number, PackageGrouping>;
  directoryRoots: readonly DirectoryNode[];
  directories: readonly DirectoryNode[];
  filesByDirectory: ReadonlyMap<string, ReadonlySet<number>>;
  islandOnlyDirectories: ReadonlySet<string>;
  districtPaths: ReadonlyMap<number, readonly DistrictPathRow[]>;
}

interface MutableDirectoryNode {
  name: string;
  path: string;
  count: number;
  children: Map<string, MutableDirectoryNode>;
}

function directoryParts(path: string): string[] {
  const parts = path.split("/").filter(Boolean);
  return parts.slice(0, -1);
}

function commonPrefixLength(paths: readonly (readonly string[])[]): number {
  if (paths.length === 0) return 0;
  let length = 0;
  while (paths.every((parts) => length < parts.length && parts[length] === paths[0][length])) length++;
  return length;
}

function compareText(a: string, b: string): number {
  // Package rank decides colour, so ordering must not depend on the host's
  // locale. Plain code-point order is deterministic everywhere JS runs.
  return a < b ? -1 : a > b ? 1 : 0;
}

function packageColor(index: number): string {
  // Package groups have their own categorical palette: district hues carry
  // spatial meaning and reusing them made unrelated package ranks look the
  // same. Groups are ranked deterministically by count then path.
  return `var(--p${index % PACKAGE_GROUP_LIMIT})`;
}

function packagePath(parts: readonly string[], depth: number): string {
  return parts.slice(0, depth).join("/") || ".";
}

function makeGrouping(fileParts: readonly (readonly string[])[], depth: number): PackageGrouping {
  const counts = new Map<string, number>();
  const paths = fileParts.map((parts) => packagePath(parts, depth));
  for (const path of paths) counts.set(path, (counts.get(path) ?? 0) + 1);

  const ranked = [...counts.entries()].sort((a, b) => b[1] - a[1] || compareText(a[0], b[0]));
  const shown = ranked.slice(0, PACKAGE_GROUP_LIMIT);
  const shownIndex = new Map(shown.map(([path], index) => [path, index]));
  const groups: PackageGroup[] = shown.map(([path, count], index) => ({
    path,
    count,
    color: packageColor(index),
    other: false,
  }));
  if (ranked.length > PACKAGE_GROUP_LIMIT) {
    groups.push({
      path: null,
      count: ranked.slice(PACKAGE_GROUP_LIMIT).reduce((sum, [, count]) => sum + count, 0),
      color: PACKAGE_OTHER_COLOR,
      other: true,
    });
  }
  return {
    depth,
    groups,
    fileColors: paths.map((path) => {
      const index = shownIndex.get(path);
      return index == null ? PACKAGE_OTHER_COLOR : packageColor(index);
    }),
  };
}

function freezeTree(node: MutableDirectoryNode): DirectoryNode {
  return {
    name: node.name,
    path: node.path,
    count: node.count,
    children: [...node.children.values()]
      .sort((a, b) => b.count - a.count || compareText(a.path, b.path))
      .map(freezeTree),
  };
}

interface BreakdownBucket {
  path: string;
  parts: readonly string[];
  members: readonly number[];
}

function breakdownBucket(members: readonly number[], fileParts: readonly (readonly string[])[]): BreakdownBucket {
  const memberPaths = members.map((index) => fileParts[index]);
  const prefix = commonPrefixLength(memberPaths);
  const parts = memberPaths[0].slice(0, prefix);
  return { path: parts.join("/") || ".", parts, members };
}

/** Split one already-compressed row at its next directory boundary. Files
 * directly in the row's directory cannot become a narrower, non-overlapping
 * folder highlight, so the caller folds them into the non-tappable `other`
 * count. A one-child chain is deliberately not a split: breakdownBucket()
 * already extended the row through that chain to its longest common prefix. */
function splitBreakdownBucket(
  row: BreakdownBucket,
  fileParts: readonly (readonly string[])[],
): { children: BreakdownBucket[]; directCount: number } | null {
  const childMembers = new Map<string, number[]>();
  let directCount = 0;
  for (const index of row.members) {
    const child = fileParts[index][row.parts.length];
    if (child == null) {
      directCount++;
      continue;
    }
    const members = childMembers.get(child);
    if (members) members.push(index);
    else childMembers.set(child, [index]);
  }
  if (childMembers.size <= 1) return null;
  return {
    children: [...childMembers.values()].map((members) => breakdownBucket(members, fileParts)),
    directCount,
  };
}

/** Begin with the branches at a district's common directory root, extending
 * each through its own single-child chain. Refine large rows only when the
 * five visible rows still represent most of the split parent. Otherwise a
 * wide folder looks like one dominant child while its siblings disappear
 * into `other`. Rows remain disjoint and ranked by file count. */
function districtBreakdown(indices: readonly number[], fileParts: readonly (readonly string[])[]): readonly DistrictPathRow[] {
  if (indices.length === 0) return [];
  const paths = indices.map((index) => fileParts[index]);
  const common = commonPrefixLength(paths);
  const childMembers = new Map<string, number[]>();
  let otherCount = 0;
  for (const index of indices) {
    const parts = fileParts[index];
    const branch = parts[common];
    if (branch == null) {
      otherCount++;
      continue;
    }
    const members = childMembers.get(branch);
    if (members) members.push(index);
    else childMembers.set(branch, [index]);
  }

  let rows = [...childMembers.values()].map((members) => breakdownBucket(members, fileParts));
  // If every file is directly in one non-root directory, that common folder
  // is still a useful row. At repository root it would highlight the whole
  // map, so it remains `other` instead.
  if (rows.length === 0 && common > 0) {
    rows = [breakdownBucket(indices, fileParts)];
    otherCount = 0;
  }

  const rankRows = () => rows.sort((a, b) => b.members.length - a.members.length || compareText(a.path, b.path));
  rankRows();
  if (rows.length > DISTRICT_PATH_LIMIT) {
    otherCount += rows.slice(DISTRICT_PATH_LIMIT).reduce((sum, row) => sum + row.members.length, 0);
    rows = rows.slice(0, DISTRICT_PATH_LIMIT);
  }
  if (otherCount > (rows[0]?.members.length ?? 0)) {
    // Even the first partition can be wider than the card. Keep its common
    // parent in that case, including the repository root if necessary.
    rows = [breakdownBucket(indices, fileParts)];
    otherCount = 0;
  }

  while (true) {
    const candidates = rows
      .map((row) => ({ row, split: splitBreakdownBucket(row, fileParts) }))
      .filter((candidate): candidate is { row: BreakdownBucket; split: NonNullable<typeof candidate.split> } => candidate.split != null)
      .sort((a, b) => b.row.members.length - a.row.members.length || compareText(a.row.path, b.row.path));
    let accepted = false;
    for (const { row, split } of candidates) {
      if (row.members.length / indices.length < DISTRICT_REFINE_MIN_SHARE) break;
      const ranked = rows.filter((existing) => existing !== row).concat(split.children)
        .sort((a, b) => b.members.length - a.members.length || compareText(a.path, b.path));
      const shown = ranked.slice(0, DISTRICT_PATH_LIMIT);
      const visibleChildren = new Set(shown);
      const covered = split.children.reduce((sum, child) => sum + (visibleChildren.has(child) ? child.members.length : 0), 0);
      if (covered / row.members.length < DISTRICT_SPLIT_MIN_COVERAGE) continue;
      const nextOther = otherCount + split.directCount +
        ranked.slice(DISTRICT_PATH_LIMIT).reduce((sum, hidden) => sum + hidden.members.length, 0);
      if (nextOther > shown[0].members.length) continue;
      rows = shown;
      otherCount = nextOther;
      accepted = true;
      break;
    }
    if (!accepted) break;
  }

  const shown: DistrictPathRow[] = rows.map((row) => ({
    path: row.path,
    count: row.members.length,
    share: Math.round((row.members.length / indices.length) * 1000) / 10,
    other: false,
  }));
  if (otherCount > 0)
    shown.push({ path: null, count: otherCount, share: Math.round((otherCount / indices.length) * 1000) / 10, other: true });
  return shown.sort((a, b) => b.count - a.count || compareText(a.path ?? "", b.path ?? ""));
}

/** All package/directory derivation for one document. MapView memoises this
 * by document reference, and every renderer/component consumer receives
 * these arrays, maps and sets directly rather than re-walking `F`. */
export function buildPackageLayout(doc: MapDocument): PackageLayout {
  const fileParts = doc.F.map(directoryParts);
  const maxDepth = Math.max(1, ...fileParts.map((parts) => parts.length));
  const commonRootDepth = commonPrefixLength(fileParts);
  const minDepth = Math.min(maxDepth, commonRootDepth + 1);
  const groupings = new Map<number, PackageGrouping>();
  for (let depth = minDepth; depth <= maxDepth; depth++) groupings.set(depth, makeGrouping(fileParts, depth));

  let autoDepth = minDepth;
  while (autoDepth < maxDepth && (groupings.get(autoDepth)?.groups.length ?? 0) < 3) autoDepth++;

  const roots = new Map<string, MutableDirectoryNode>();
  const directoryMembers = new Map<string, number[]>();
  directoryMembers.set(".", doc.F.map((_, index) => index));
  for (let index = 0; index < fileParts.length; index++) {
    const parts = fileParts[index];
    let children = roots;
    let prefix = "";
    for (const name of parts) {
      prefix = prefix ? `${prefix}/${name}` : name;
      let node = children.get(name);
      if (!node) {
        node = { name, path: prefix, count: 0, children: new Map() };
        children.set(name, node);
      }
      node.count++;
      const members = directoryMembers.get(prefix);
      if (members) members.push(index);
      else directoryMembers.set(prefix, [index]);
      children = node.children;
    }
  }
  const directoryRoots = [...roots.values()]
    .sort((a, b) => b.count - a.count || compareText(a.path, b.path))
    .map(freezeTree);
  const directories: DirectoryNode[] = [];
  const visit = (node: DirectoryNode) => {
    directories.push(node);
    node.children.forEach(visit);
  };
  directoryRoots.forEach(visit);
  const filesByDirectory = new Map<string, ReadonlySet<number>>(
    [...directoryMembers].map(([path, members]) => [path, new Set(members)]),
  );
  // Island-fade precedence is path-derived state too. Resolve it once here,
  // rather than scanning a possibly-thousand-file folder on every SVG paint.
  const islandOnlyDirectories = new Set(
    [...directoryMembers]
      .filter(([, members]) =>
        members.every((index) => districtClass(doc.districts[String(D_(doc, index))]) === "island"),
      )
      .map(([path]) => path),
  );

  const membersByDistrict = new Map<number, number[]>();
  for (let index = 0; index < doc.N.length; index++) {
    const district = D_(doc, index);
    const members = membersByDistrict.get(district);
    if (members) members.push(index);
    else membersByDistrict.set(district, [index]);
  }
  const districtPaths = new Map<number, readonly DistrictPathRow[]>();
  for (const [district, indices] of membersByDistrict) districtPaths.set(district, districtBreakdown(indices, fileParts));

  return {
    autoDepth,
    minDepth,
    maxDepth,
    groupings,
    directoryRoots,
    directories,
    filesByDirectory,
    islandOnlyDirectories,
    districtPaths,
  };
}

export function formatDirectory(path: string): string {
  return path === "." ? "(repo root)" : `${path}/`;
}
