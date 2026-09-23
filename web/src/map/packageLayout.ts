import type { MapDocument } from "@/types";
import { D_ } from "./geometry";

export const PACKAGE_GROUP_LIMIT = 10;
export const PACKAGE_OTHER_COLOR = "var(--dim)";

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

function packageColor(index: number): string {
  // The same fixed, light/dark-tested palette as districtColor. Package
  // groups are ranked deterministically by count then path, never hashed.
  return `var(--c${index % 12})`;
}

function packagePath(parts: readonly string[], depth: number): string {
  return parts.slice(0, depth).join("/") || ".";
}

function makeGrouping(fileParts: readonly (readonly string[])[], depth: number): PackageGrouping {
  const counts = new Map<string, number>();
  const paths = fileParts.map((parts) => packagePath(parts, depth));
  for (const path of paths) counts.set(path, (counts.get(path) ?? 0) + 1);

  const ranked = [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
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
      .sort((a, b) => b.count - a.count || a.path.localeCompare(b.path))
      .map(freezeTree),
  };
}

/** Partition a district beneath its own common directory root, then extend
 * each branch to the longest prefix shared by that branch. The rows do not
 * overlap, so their counts/shares add up to the district total. */
function districtBreakdown(indices: readonly number[], fileParts: readonly (readonly string[])[]): readonly DistrictPathRow[] {
  if (indices.length === 0) return [];
  const paths = indices.map((index) => fileParts[index]);
  const common = commonPrefixLength(paths);
  const buckets = new Map<string, number[]>();
  for (const index of indices) {
    const parts = fileParts[index];
    const branch = parts[common] ?? "";
    const bucket = buckets.get(branch);
    if (bucket) bucket.push(index);
    else buckets.set(branch, [index]);
  }

  const ranked = [...buckets.values()]
    .map((members) => {
      const memberPaths = members.map((index) => fileParts[index]);
      const prefix = commonPrefixLength(memberPaths);
      return { path: memberPaths[0].slice(0, prefix).join("/") || ".", count: members.length };
    })
    .sort((a, b) => b.count - a.count || a.path.localeCompare(b.path));
  // A file directly in the repository root has no narrower directory that
  // can be highlighted: selecting the root itself necessarily covers every
  // file below it too. Fold those direct-root files into "other" rather
  // than render a misleading tappable `(root)/` row for only their count.
  const named = ranked.filter((row) => row.path !== ".");
  const shown: DistrictPathRow[] = named.slice(0, 5).map(({ path, count }) => ({
    path,
    count,
    share: Math.round((count / indices.length) * 1000) / 10,
    other: false,
  }));
  const count =
    ranked.filter((row) => row.path === ".").reduce((sum, row) => sum + row.count, 0) +
    named.slice(5).reduce((sum, row) => sum + row.count, 0);
  if (count > 0) {
    shown.push({ path: null, count, share: Math.round((count / indices.length) * 1000) / 10, other: true });
  }
  return shown;
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
    .sort((a, b) => b.count - a.count || a.path.localeCompare(b.path))
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

  const membersByDistrict = new Map<number, number[]>();
  for (let index = 0; index < doc.N.length; index++) {
    const district = D_(doc, index);
    const members = membersByDistrict.get(district);
    if (members) members.push(index);
    else membersByDistrict.set(district, [index]);
  }
  const districtPaths = new Map<number, readonly DistrictPathRow[]>();
  for (const [district, indices] of membersByDistrict) districtPaths.set(district, districtBreakdown(indices, fileParts));

  return { autoDepth, minDepth, maxDepth, groupings, directoryRoots, directories, filesByDirectory, districtPaths };
}

export function formatDirectory(path: string): string {
  return path === "." ? "(root)/" : `${path}/`;
}
