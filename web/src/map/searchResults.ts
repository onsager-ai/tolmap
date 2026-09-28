// docs/UX.md §4.8 and §7.2: what the search layer shows, as pure functions,
// so web/scripts/check-search.ts can test the grouping, the highlighting and
// the keyboard reducer without a browser (the same reason map/backStack.ts
// and map/gestures.ts are pure).
//
// Ranking of files and symbols is map/search.ts's, unchanged: this module
// only splits its output into the File and Symbol groups, in the order that
// matcher returned. Districts are new to search (the spec's first group), so
// their ranking is defined here, and tested.

import type { MapDocument } from "@/types";
import { KIND } from "./constants";
import { searchHits } from "./search";

export type SearchKind = "district" | "file" | "symbol";

export type SearchPick = { kind: "district"; d: number } | { kind: "file"; i: number } | { kind: "symbol"; i: number; s: number };

export interface SearchItem {
  /** Stable within one result list: the listbox option's id suffix and the
   * React key. */
  key: string;
  pick: SearchPick;
  /** The row's first line: a district name, a file's basename, a symbol
   * name. Shown in full (wrapped), never only in a title. */
  name: string;
  /** The row's second line: a file's directory, a symbol's kind and file.
   * Empty for districts, whose detail is `aside`. */
  detail: string;
  /** Right-aligned data: a district's file count. */
  aside: string;
  /** Where the query matched, as ranges into `name` and into `detail`. For
   * a file both come from one match over its full path (directory then
   * basename), so a query that spans the last slash still highlights on
   * both lines. */
  nameMarks: readonly Mark[];
  detailMarks: readonly Mark[];
}

export interface SearchGroup {
  kind: SearchKind;
  /** "District"/"Districts", "File"/"Files", "Symbol"/"Symbols". */
  label: string;
  items: readonly SearchItem[];
}

export interface SearchResults {
  /** The trimmed, lower-cased query the results were computed for. */
  query: string;
  groups: readonly SearchGroup[];
  /** Every item in display order: the order the keyboard walks and the
   * index `cursor` refers to. */
  flat: readonly SearchItem[];
}

export interface SearchCommandDefinition {
  id: string;
  label: string;
  detail?: string;
  /** The §5.2 key that runs the same command, shown as a key cap on the
   * row (the row's `aside`); never matched against the query. */
  shortcut?: string;
}

export interface SearchCommandItem {
  key: string;
  commandId: string;
  name: string;
  detail: string;
  aside: string;
  nameMarks: readonly Mark[];
  detailMarks: readonly Mark[];
}

export type PaletteItem = SearchItem | SearchCommandItem;

export interface PaletteGroup {
  kind: SearchKind | "command";
  label: string;
  items: readonly PaletteItem[];
}

export interface PaletteResults {
  query: string;
  groups: readonly PaletteGroup[];
  flat: readonly PaletteItem[];
}

/** At most this many districts: the district group leads, and a short query
 * ("a") would otherwise push every file below the fold. */
export const MAX_DISTRICT_HITS = 5;
/** The empty state: the largest districts, as places to start. */
export const EMPTY_STATE_DISTRICTS = 6;

/** A highlighted run: [start, end) in the string it was computed on. */
export type Mark = readonly [number, number];

/** Every non-overlapping, case-insensitive occurrence of `q` in `text`. */
export function findMarks(text: string, q: string): Mark[] {
  const needle = q.trim().toLowerCase();
  if (!needle) return [];
  const hay = text.toLowerCase();
  const out: Mark[] = [];
  for (let at = hay.indexOf(needle); at >= 0; at = hay.indexOf(needle, at + needle.length)) {
    out.push([at, at + needle.length]);
  }
  return out;
}

/** Marks computed on `whole`, restricted to [from, to) and re-based to
 * `from`: how one match over a file's full path is split between its
 * directory line and its name line. */
export function sliceMarks(marks: readonly Mark[], from: number, to: number): Mark[] {
  const out: Mark[] = [];
  for (const [a, b] of marks) {
    const s = Math.max(a, from);
    const e = Math.min(b, to);
    if (s < e) out.push([s - from, e - from]);
  }
  return out;
}

export interface Segment {
  text: string;
  hit: boolean;
}

/** `text` cut into plain and highlighted runs, in order; joining the runs'
 * text gives `text` back exactly. */
export function segments(text: string, marks: readonly Mark[]): Segment[] {
  const out: Segment[] = [];
  let at = 0;
  for (const [a, b] of [...marks].sort((x, y) => x[0] - y[0])) {
    if (a < at || b > text.length || a >= b) continue;
    if (a > at) out.push({ text: text.slice(at, a), hit: false });
    out.push({ text: text.slice(a, b), hit: true });
    at = b;
  }
  if (at < text.length || out.length === 0) out.push({ text: text.slice(at), hit: false });
  return out;
}

/** Districts whose name contains `q`: an exact name first, then names
 * starting with it, then a word inside the name starting with it ("comp"
 * in "workflow & components"), then any substring; within a tier, the
 * larger district first, then the lower id (CLAUDE.md's determinism rule:
 * nothing here depends on object key order). Unconnected files are not a
 * place, so their pseudo-district is never a result (the District index
 * does not list it either). */
export function districtHits(doc: MapDocument, q: string): number[] {
  const v = q.trim().toLowerCase();
  if (!v) return [];
  const scored: { d: number; tier: number; size: number }[] = [];
  for (const id of Object.keys(doc.districts)) {
    const district = doc.districts[id];
    if (district.class === "unconnected") continue;
    const name = (doc.names[id] ?? "").toLowerCase();
    const at = name.indexOf(v);
    if (at < 0) continue;
    const wordStart = new RegExp(`(^|[^a-z0-9])${v.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`).test(name);
    const tier = name === v ? 0 : at === 0 ? 1 : wordStart ? 2 : 3;
    scored.push({ d: Number(id), tier, size: district.size });
  }
  scored.sort((a, b) => a.tier - b.tier || b.size - a.size || a.d - b.d);
  return scored.slice(0, MAX_DISTRICT_HITS).map((s) => s.d);
}

function plural(n: number, one: string, many: string) {
  return n === 1 ? one : many;
}

function dirOf(path: string): string {
  const cut = path.lastIndexOf("/");
  return cut < 0 ? "" : path.slice(0, cut + 1);
}

function baseOf(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/** Symbol kinds in the glossary's words (`func` is a function). */
const KIND_WORD: Record<string, string> = { func: "function", const: "constant" };
export function kindWord(kind: number): string {
  const k = KIND[kind] ?? "symbol";
  return KIND_WORD[k] ?? k;
}

function districtItem(doc: MapDocument, d: number, q: string): SearchItem {
  const name = doc.names[String(d)] ?? `district ${d}`;
  const size = doc.districts[String(d)]?.size ?? 0;
  return {
    key: `d${d}`,
    pick: { kind: "district", d },
    name,
    detail: "",
    aside: `${size} ${plural(size, "file", "files")}`,
    nameMarks: findMarks(name, q),
    detailMarks: [],
  };
}

function fileItem(doc: MapDocument, i: number, q: string): SearchItem {
  const path = doc.F[i];
  const dir = dirOf(path);
  const base = baseOf(path);
  const marks = findMarks(path, q);
  return {
    key: `f${i}`,
    pick: { kind: "file", i },
    name: base,
    detail: dir || "(repo root)",
    aside: "",
    nameMarks: sliceMarks(marks, dir.length, path.length),
    detailMarks: dir ? sliceMarks(marks, 0, dir.length) : [],
  };
}

/** The grouped results for `q` (docs/UX.md §4.8): District, Files, Symbols,
 * each omitted when empty. An empty query gives the empty state: the
 * largest districts, so the layer is never blank. */
export function groupResults(doc: MapDocument, q: string, options: { filesOnly?: boolean } = {}): SearchResults {
  const query = q.trim().toLowerCase();
  const groups: SearchGroup[] = [];
  if (!query && !options.filesOnly) {
    const largest = Object.keys(doc.districts)
      .filter((id) => doc.districts[id].class !== "unconnected")
      .sort((a, b) => doc.districts[b].size - doc.districts[a].size || Number(a) - Number(b))
      .slice(0, EMPTY_STATE_DISTRICTS)
      .map((id) => districtItem(doc, Number(id), ""));
    if (largest.length) groups.push({ kind: "district", label: "Largest districts", items: largest });
    return { query, groups, flat: groups.flatMap((g) => g.items) };
  }

  const districts = options.filesOnly ? [] : districtHits(doc, query).map((d) => districtItem(doc, d, query));
  const files: SearchItem[] = [];
  const symbols: SearchItem[] = [];
  for (const hit of searchHits(doc, query)) {
    if (hit.s == null) {
      files.push(fileItem(doc, hit.i, query));
      continue;
    }
    if (options.filesOnly) continue;
    const sm = doc.S?.[String(hit.i)]?.[hit.s];
    if (!sm) continue;
    const path = doc.F[hit.i];
    const dir = dirOf(path);
    const base = baseOf(path);
    // The file as its parent folder and name ("workflow/types.ts"): enough
    // to tell two same-named files apart without the whole path.
    const parent = baseOf(dir.slice(0, -1));
    const where = parent ? `${parent}/${base}` : base;
    symbols.push({
      key: `s${hit.i}:${hit.s}`,
      pick: { kind: "symbol", i: hit.i, s: hit.s },
      name: sm[0],
      detail: `${kindWord(sm[1])} · ${where}:${sm[2]}`,
      aside: "",
      nameMarks: findMarks(sm[0], query),
      detailMarks: [],
    });
  }
  if (districts.length) groups.push({ kind: "district", label: plural(districts.length, "District", "Districts"), items: districts });
  if (files.length) groups.push({ kind: "file", label: plural(files.length, "File", "Files"), items: files });
  if (symbols.length) groups.push({ kind: "symbol", label: plural(symbols.length, "Symbol", "Symbols"), items: symbols });
  return { query, groups, flat: groups.flatMap((g) => g.items) };
}

/** The desktop palette adds command rows around the phone search's exact
 * result groups. File and symbol ranking still comes from searchHits(), and
 * command rows use the same case-folded occurrence/highlight helpers as the
 * phone rows. Path mode narrows the shared results to files. */
export function paletteResults(
  doc: MapDocument,
  q: string,
  commands: readonly SearchCommandDefinition[],
  pathMode = false,
): PaletteResults {
  const query = q.trim().toLowerCase();
  const search = groupResults(doc, query, { filesOnly: pathMode });
  const groups: PaletteGroup[] = search.groups.map((group) => ({ ...group }));
  if (!pathMode) {
    const commandItems: SearchCommandItem[] = commands.flatMap((command) => {
      const nameMarks = findMarks(command.label, query);
      const detail = command.detail ?? "";
      const detailMarks = findMarks(detail, query);
      if (query && nameMarks.length === 0 && detailMarks.length === 0) return [];
      return [{ key: `command:${command.id}`, commandId: command.id, name: command.label, detail, aside: command.shortcut ?? "", nameMarks, detailMarks }];
    });
    if (commandItems.length) groups.push({ kind: "command", label: "Commands", items: commandItems });
  }
  return { query, groups, flat: groups.flatMap((group) => group.items) };
}

// ---------------------------------------------------------------- keyboard
// §7.2: arrow keys move through the results, Enter picks, Esc closes. The
// cursor is an index into SearchResults.flat, or -1 for "none highlighted"
// (focus stays in the input either way: the listbox is driven through
// aria-activedescendant, the combobox pattern).

// Home and End are left to the input (they move the caret in an editable
// combobox, per the ARIA pattern).
export type NavKey = "ArrowDown" | "ArrowUp" | "Enter" | "Escape";

export type NavResult =
  | { type: "move"; cursor: number }
  | { type: "pick"; index: number }
  | { type: "close" }
  | { type: "none" };

export function isNavKey(key: string): key is NavKey {
  return key === "ArrowDown" || key === "ArrowUp" || key === "Enter" || key === "Escape";
}

/** One key press against `count` results with `cursor` highlighted.
 * Down from none goes to the first and wraps from the last to the first; Up
 * from none goes to the last and wraps from the first to the last. Enter
 * picks the highlighted result, or the first when none is (typing a name
 * and pressing Enter takes the best match); with no results it does
 * nothing. Esc always closes. */
export function navKey(cursor: number, count: number, key: NavKey): NavResult {
  if (key === "Escape") return { type: "close" };
  if (count <= 0) return key === "Enter" ? { type: "none" } : { type: "move", cursor: -1 };
  const at = cursor >= 0 && cursor < count ? cursor : -1;
  switch (key) {
    case "ArrowDown":
      return { type: "move", cursor: at < 0 ? 0 : (at + 1) % count };
    case "ArrowUp":
      return { type: "move", cursor: at < 0 ? count - 1 : (at - 1 + count) % count };
    case "Enter":
      return { type: "pick", index: at < 0 ? 0 : at };
  }
}
