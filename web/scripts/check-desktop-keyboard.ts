#!/usr/bin/env -S npx tsx
// Pure checks for docs/UX.md §5.2's desktop and tablet keyboard dispatch.

import { dispatchDesktopKey } from "../src/map/desktopKeyboard";

let checks = 0;
let failures = 0;
function report(ok: boolean, label: string, got?: unknown) {
  checks++;
  if (ok) console.log(`  ok    ${label}`);
  else {
    failures++;
    console.log(`  FAIL  ${label}${got === undefined ? "" : ` -- ${JSON.stringify(got)}`}`);
  }
}
const eq = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

console.log("\ndesktop keyboard dispatch (§5.2)");
for (const [key, layer] of [["1", "d"], ["2", "c"], ["3", "x"], ["4", "p"]] as const) {
  report(eq(dispatchDesktopKey({ key }), { type: "layer", layer }), `${key} switches to ${layer} layer`);
}
report(eq(dispatchDesktopKey({ key: "[" }), { type: "toggle-panel" }), "[ toggles the panel");
report(eq(dispatchDesktopKey({ key: "f", focus: "text" }), { type: "none" }), "single-letter shortcuts are ignored in text fields");
report(eq(dispatchDesktopKey({ key: "1", focus: "palette" }), { type: "none" }), "layer keys are left to the palette");
report(eq(dispatchDesktopKey({ key: "?", focus: "menu" }), { type: "none" }), "shortcuts are left to open menus");
report(eq(dispatchDesktopKey({ key: "k", ctrlKey: true, focus: "menu" }), { type: "search" }), "Ctrl K opens search from a menu");
report(eq(dispatchDesktopKey({ key: "/", focus: "text" }), { type: "none" }), "/ is left to text fields");
report(eq(dispatchDesktopKey({ key: "ArrowDown", overviewOpen: true }), { type: "overview-move", direction: 1 }), "Down moves through the open overview index");
report(eq(dispatchDesktopKey({ key: "ArrowUp", overviewOpen: true }), { type: "overview-move", direction: -1 }), "Up moves through the open overview index");
report(eq(dispatchDesktopKey({ key: "Enter", overviewOpen: true }), { type: "overview-activate" }), "Enter opens the focused overview row");
report(eq(dispatchDesktopKey({ key: "Enter", overviewOpen: false }), { type: "none" }), "Enter is not captured outside the overview");
report(eq(dispatchDesktopKey({ key: "Escape" }), { type: "escape" }), "Esc steps back when no dialog owns it");
report(eq(dispatchDesktopKey({ key: "+" }), { type: "zoom-in" }) && eq(dispatchDesktopKey({ key: "-" }), { type: "zoom-out" }), "+ and - zoom");
report(eq(dispatchDesktopKey({ key: "F" }), { type: "fit" }) && eq(dispatchDesktopKey({ key: "Z" }), { type: "zoom-selection" }) && eq(dispatchDesktopKey({ key: "T" }), { type: "cycle-theme" }), "F, Z and T are case-insensitive");
report(eq(dispatchDesktopKey({ key: "?" }), { type: "keyboard-list" }), "? opens the keyboard list");
report(eq(dispatchDesktopKey({ key: "/", shiftKey: true }), { type: "keyboard-list" }), "Shift+/ opens the keyboard list across browser key event forms");

console.log(`\n${checks - failures}/${checks} checks passed`);
if (failures) process.exit(1);
