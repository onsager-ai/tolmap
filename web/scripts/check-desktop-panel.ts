#!/usr/bin/env node
// Pure checks for the desktop panel's local view stack and crumb labels.

import { desktopPanelCrumbs, desktopPanelReducer, DESKTOP_PANEL_OVERVIEW, type DesktopPanelView } from "../src/map/desktopPanel";
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

let stack: readonly DesktopPanelView[] = DESKTOP_PANEL_OVERVIEW;
stack = desktopPanelReducer(stack, { type: "map-file", district: 4, file: 1 });
report(eq(stack.map((v) => v.type), ["overview", "district", "file"]), "a map file starts from overview and pushes its district then file", stack);
stack = desktopPanelReducer(stack, { type: "map-symbol", district: 2, file: 3, label: "MapView" });
report(eq(stack.map((v) => v.type), ["overview", "district", "file", "symbol"]), "a map symbol restarts the trail with its district and file", stack);
stack = desktopPanelReducer(stack, { type: "pop" });
report(stack.at(-1)?.type === "file", "Esc pops a symbol to its file", stack);
stack = desktopPanelReducer(stack, { type: "push", view: { type: "path" } });
stack = desktopPanelReducer(stack, { type: "push", view: { type: "quality" } });
report(eq(stack.map((v) => v.type), ["overview", "district", "file", "path", "quality"]), "path and map quality are stack views", stack);
stack = desktopPanelReducer(stack, { type: "jump", index: 1 });
report(eq(stack.map((v) => v.type), ["overview", "district"]), "a crumb jumps back through the trail", stack);
stack = desktopPanelReducer(stack, { type: "map-district", district: 7 });
report(eq(stack.map((v) => v.type), ["overview", "district"]) && (stack.at(-1) as { district?: number }).district === 7, "a new map selection starts a fresh trail", stack);

const doc = { repo: "langgenius__dify", names: { "4": "networking" }, F: ["src/main.ts", "pkg/file.py"] } as unknown as MapDocument;
const crumbs = desktopPanelCrumbs([
  { type: "overview" },
  { type: "district", district: 4 },
  { type: "file", district: 4, file: 1 },
  { type: "symbol", district: 4, file: 1, label: "Worker" },
  { type: "path" },
  { type: "quality" },
], doc, "langgenius/dify");
report(eq(crumbs.map((c) => c.label), ["owner/repo", "networking", "file.py", "Worker", "Path", "Map quality"]), "crumb labels use the repo, district, file, symbol and card names", crumbs);

console.log(`\n${checks - failures}/${checks} checks passed`);
if (failures) process.exitCode = 1;
