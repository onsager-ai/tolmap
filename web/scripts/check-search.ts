#!/usr/bin/env -S npx tsx
// Unit checks for search's pure parts (docs/UX.md §4.8 and §7.2, phase 3),
// in map/searchResults.ts:
//   - grouping: District, Files, Symbols, in that order, each omitted when
//     empty, files and symbols kept in map/search.ts's own ranked order;
//   - district ranking (new with this phase): exact, prefix, word start,
//     substring, then size, then id;
//   - highlighting: every occurrence, case-insensitive, split across a
//     file's directory and name lines;
//   - the keyboard reducer: arrows wrap, Enter picks the highlighted or the
//     first result, Esc closes.
// No browser: the same standalone-script pattern as check-phone-shell.ts. CI
// runs it in the `web build and lint` job; the browser side is
// check-view-stability.mjs's checkSearch.
//
// Run: npx tsx web/scripts/check-search.ts

import type { MapDocument } from "../src/types";
import { searchHits } from "../src/map/search";
import {
  EMPTY_STATE_DISTRICTS,
  MAX_DISTRICT_HITS,
  districtHits,
  findMarks,
  groupResults,
  navKey,
  paletteResults,
  segments,
  sliceMarks,
} from "../src/map/searchResults";

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
const eq = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

// A small map: NodeRow is [d, x, y, loc, cplx, churn, fanin, rx, ry, rw, rh];
// only d and fanin matter to search.
function node(d: number, fanin: number) {
  return [d, 0, 0, 10, 1, 1, fanin, 0, 0, 1, 1];
}
function district(size: number, cls: "mainland" | "island" | "unconnected" = "mainland") {
  return { size, c: [0, 0], blob: [], class: cls };
}
const doc = {
  repo: "demo/demo",
  q: 0.5,
  names: {
    "0": "workflow & components",
    "1": "api models",
    "2": "Workflow",
    "3": "sub-workflow runner",
    "4": "unconnected workflow files",
    "5": "plugins",
    "6": "a workflowish island",
  },
  districts: {
    "0": district(900),
    "1": district(300),
    "2": district(12),
    "3": district(40),
    "4": district(7, "unconnected"),
    "5": district(200),
    "6": district(3, "island"),
  },
  F: [
    "web/app/components/workflow/types.ts", // 0
    "web/types/workflow.ts", // 1
    "api/models/model.py", // 2
    "README.md", // 3
    "web/app/components/workflow/nodes/index.tsx", // 4
  ],
  N: [node(0, 687), node(0, 40), node(1, 259), node(4, 0), node(0, 3)],
  E: [],
  L: [],
  // [name, kind, line start, line end]
  S: {
    "0": [
      ["WorkflowRunningData", 0, 422, 470],
      ["BlockEnum", 5, 10, 60],
    ],
    "2": [["workflow_run", 1, 30, 90]],
  },
  U: { "0:0": [1, 2, 3] },
  roads: [],
  lang: "ts",
  P: null,
} as unknown as MapDocument;

// ---------------------------------------------------------------- districts
console.log("\ndistrict hits (§4.8's District group)");
{
  const hits = districtHits(doc, "workflow");
  // "Workflow" exact (tier 0), "workflow & components" prefix (1),
  // "sub-workflow runner" word start (2), "a workflowish island" word start
  // (2, smaller), never the unconnected pseudo-district.
  report(eq(hits, [2, 0, 3, 6]), "exact, then prefix, then word start by size; never unconnected files", JSON.stringify(hits));
  report(eq(districtHits(doc, "  WORKFLOW "), hits), "the query is trimmed and case-insensitive");
  report(eq(districtHits(doc, "comp"), [0]), "a word inside a name matches ('comp' in 'workflow & components')");
  report(eq(districtHits(doc, "orkflo"), [0, 3, 2, 6]), "a bare substring ranks by size within its tier, then id");
  report(districtHits(doc, "").length === 0, "an empty query matches no district");
  report(eq(districtHits(doc, "w.rk"), []), "regex characters in the query are literal");
  const many = {
    ...doc,
    names: Object.fromEntries(Array.from({ length: 9 }, (_, n) => [String(n), `area ${n}`])),
    districts: Object.fromEntries(Array.from({ length: 9 }, (_, n) => [String(n), district(10)])),
  } as unknown as MapDocument;
  const capped = districtHits(many, "area");
  report(capped.length === MAX_DISTRICT_HITS && eq(capped, [0, 1, 2, 3, 4]), `at most ${MAX_DISTRICT_HITS} districts; equal sizes fall back to the lower id`, JSON.stringify(capped));
}

// ---------------------------------------------------------------- grouping
console.log("\ngrouping (§4.8: District, Files, Symbols)");
{
  const r = groupResults(doc, "Workflow");
  report(eq(r.groups.map((g) => g.kind), ["district", "file", "symbol"]), "groups in the spec's order", JSON.stringify(r.groups.map((g) => g.kind)));
  report(eq(r.groups.map((g) => g.label), ["Districts", "Files", "Symbols"]), "plural labels for several results", JSON.stringify(r.groups.map((g) => g.label)));
  report(r.query === "workflow", "the query is normalised once");
  report(eq(r.flat.map((it) => it.key), r.groups.flatMap((g) => g.items.map((it) => it.key))), "flat is the display order (what the keyboard walks)");

  // Files and symbols keep map/search.ts's order exactly.
  const hits = searchHits(doc, "workflow");
  const fileOrder = hits.filter((h) => h.s == null).map((h) => `f${h.i}`);
  const symOrder = hits.filter((h) => h.s != null).map((h) => `s${h.i}:${h.s}`);
  const files = r.groups.find((g) => g.kind === "file")!.items.map((it) => it.key);
  const syms = r.groups.find((g) => g.kind === "symbol")!.items.map((it) => it.key);
  report(eq(files, fileOrder) && eq(syms, symOrder), "files and symbols keep the matcher's ranking", JSON.stringify({ files, fileOrder, syms, symOrder }));
  report(files[0] === "f1", "the matcher's own rule stands: an exact stem (workflow.ts) leads the files", JSON.stringify(files));

  const file = r.flat.find((it) => it.key === "f0")!;
  report(file.name === "types.ts" && file.detail === "web/app/components/workflow/", "a file row: basename, then its directory", JSON.stringify(file));
  const sym = r.flat.find((it) => it.key === "s0:0")!;
  report(sym.name === "WorkflowRunningData" && sym.detail === "class · workflow/types.ts:422", "a symbol row: name, then kind, parent folder / file and line", JSON.stringify(sym));
  const fn = r.flat.find((it) => it.key === "s2:0")!;
  report(fn.detail === "function · models/model.py:30", "kinds in the glossary's words (func is 'function')", fn.detail);
  const dist = r.flat.find((it) => it.key === "d0")!;
  report(dist.aside === "900 files" && dist.detail === "", "a district row: name and its file count", JSON.stringify(dist));

  const one = groupResults(doc, "readme");
  report(eq(one.groups.map((g) => g.label), ["File"]) && one.flat[0].detail === "(repo root)", "a single result's label is singular; a root file says so", JSON.stringify(one.groups));

  const none = groupResults(doc, "zzzz");
  report(none.groups.length === 0 && none.flat.length === 0, "no match: no groups (the component shows the no-results state)");

  const empty = groupResults(doc, "   ");
  const emptyKeys = empty.flat.map((it) => it.key);
  report(
    empty.groups.length === 1 && empty.groups[0].label === "Largest districts" && eq(emptyKeys, ["d0", "d1", "d5", "d3", "d2", "d6"].slice(0, EMPTY_STATE_DISTRICTS)),
    "empty query: the largest districts, never unconnected files",
    JSON.stringify(emptyKeys),
  );
}

// ---------------------------------------------------------------- desktop palette
console.log("\ndesktop palette (§5.1: commands share search grouping and highlights)");
{
  const commands = [
    { id: "layer:d", label: "Switch to District layer", detail: "Layer 1" },
    { id: "fit", label: "Fit map", detail: "Fit the map" },
  ];
  const empty = paletteResults(doc, "", commands);
  report(eq(empty.groups.map((group) => group.kind), ["district", "command"]), "empty palette shows largest districts and commands", JSON.stringify(empty.groups.map((group) => group.kind)));
  report(empty.groups.at(-1)?.label === "Commands" && empty.groups.at(-1)?.items.length === 2, "empty palette includes every command");
  const commandHit = paletteResults(doc, "district", commands);
  const districtCommand = commandHit.flat.find((item) => "commandId" in item && item.commandId === "layer:d");
  report(!!districtCommand && eq(districtCommand.nameMarks, [[10, 18]]), "commands use the same case-insensitive highlighting helper");
  const path = paletteResults(doc, "workflow", commands, true);
  report(path.groups.every((group) => group.kind === "file") && path.flat.every((item) => "pick" in item && item.pick.kind === "file"), "path mode contains files only");
  const sharedFile = path.flat.find((item) => "pick" in item && item.key === "f0");
  const phoneFile = groupResults(doc, "workflow").flat.find((item) => item.key === "f0");
  report(!!sharedFile && !!phoneFile && eq(sharedFile.nameMarks, phoneFile.nameMarks) && eq(sharedFile.detailMarks, phoneFile.detailMarks), "path mode keeps phone search file ranking and match highlights");
  const emptyPath = paletteResults(doc, "", commands, true);
  report(emptyPath.groups.every((group) => group.kind === "file"), "empty path mode stays file-only");
}

// ---------------------------------------------------------------- highlight
console.log("\nhighlighting (§4.8: the match is bold)");
{
  report(eq(findMarks("WorkflowRunningData", "workflow"), [[0, 8]]), "case-insensitive");
  report(eq(findMarks("aaaa", "aa"), [[0, 2], [2, 4]]), "every occurrence, non-overlapping");
  report(eq(findMarks("abc", ""), []), "an empty query marks nothing");
  const r = groupResults(doc, "workflow");
  const f0 = r.flat.find((it) => it.key === "f0")!;
  report(eq(f0.nameMarks, []) && eq(f0.detailMarks, [[19, 27]]), "a match in the directory highlights the directory line only", JSON.stringify(f0));
  const f1 = r.flat.find((it) => it.key === "f1")!;
  report(eq(f1.nameMarks, [[0, 8]]) && eq(f1.detailMarks, []), "a match in the basename highlights the name line only", JSON.stringify(f1));
  const span = groupResults(doc, "workflow/types").flat.find((it) => it.key === "f0")!;
  report(eq(span.detailMarks, [[19, 28]]) && eq(span.nameMarks, [[0, 5]]), "a match across the last slash highlights both lines", JSON.stringify(span));
  report(eq(sliceMarks([[2, 6]], 4, 10), [[0, 2]]) && eq(sliceMarks([[2, 3]], 4, 10), []), "sliceMarks clips and re-bases");
  const segs = segments("web/types/workflow.ts", findMarks("web/types/workflow.ts", "w"));
  report(segs.map((s) => s.text).join("") === "web/types/workflow.ts", "segments join back to the original text");
  report(
    eq(segments("types.ts", [[0, 5]]), [{ text: "types", hit: true }, { text: ".ts", hit: false }]) &&
      eq(segments("abc", []), [{ text: "abc", hit: false }]) &&
      eq(segments("", []), [{ text: "", hit: false }]),
    "segments: leading hit, no hits, empty text",
  );
}

// ---------------------------------------------------------------- keyboard
console.log("\nkeyboard (§7.2: arrows move, Enter picks, Esc closes)");
{
  report(eq(navKey(-1, 4, "ArrowDown"), { type: "move", cursor: 0 }), "Down from none: the first result");
  report(eq(navKey(0, 4, "ArrowDown"), { type: "move", cursor: 1 }), "Down: the next result");
  report(eq(navKey(3, 4, "ArrowDown"), { type: "move", cursor: 0 }), "Down from the last wraps to the first");
  report(eq(navKey(-1, 4, "ArrowUp"), { type: "move", cursor: 3 }), "Up from none: the last result");
  report(eq(navKey(0, 4, "ArrowUp"), { type: "move", cursor: 3 }), "Up from the first wraps to the last");
  report(eq(navKey(2, 4, "Enter"), { type: "pick", index: 2 }), "Enter picks the highlighted result");
  report(eq(navKey(-1, 4, "Enter"), { type: "pick", index: 0 }), "Enter with none highlighted picks the first (the best match)");
  report(eq(navKey(9, 4, "Enter"), { type: "pick", index: 0 }), "a stale cursor past the end counts as none");
  report(eq(navKey(-1, 0, "Enter"), { type: "none" }), "Enter with no results does nothing");
  report(eq(navKey(-1, 0, "ArrowDown"), { type: "move", cursor: -1 }), "arrows with no results highlight nothing");
  report(eq(navKey(1, 4, "Escape"), { type: "close" }) && eq(navKey(-1, 0, "Escape"), { type: "close" }), "Esc closes, results or not");
}

console.log(`\n${checks - failures}/${checks} checks passed`);
if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
