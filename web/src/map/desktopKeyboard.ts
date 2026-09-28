import type { Layer } from "./constants";

export type DesktopFocusScope = "page" | "text" | "palette" | "menu" | "dialog";

export interface DesktopKeyInput {
  key: string;
  focus?: DesktopFocusScope;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
  overviewOpen?: boolean;
}

export type DesktopKeyAction =
  | { type: "search" }
  | { type: "escape" }
  | { type: "toggle-panel" }
  | { type: "layer"; layer: Layer }
  | { type: "zoom-in" }
  | { type: "zoom-out" }
  | { type: "fit" }
  | { type: "zoom-selection" }
  | { type: "cycle-theme" }
  | { type: "keyboard-list" }
  | { type: "overview-move"; direction: -1 | 1 }
  | { type: "overview-activate" }
  | { type: "none" };

const NONE: DesktopKeyAction = { type: "none" };
const LAYER_KEYS: Record<string, Layer> = { "1": "d", "2": "c", "3": "x", "4": "p" };

/** Pure dispatch for docs/UX.md §5.2. Dialogs, menus and text fields own
 * their keys; this page-level map only runs when focus is outside them. */
export function dispatchDesktopKey(input: DesktopKeyInput): DesktopKeyAction {
  const focus = input.focus ?? "page";
  const key = input.key;
  const command = !!(input.metaKey || input.ctrlKey);

  if (command && key.toLowerCase() === "k" && !input.altKey) return { type: "search" };
  if (focus !== "page") return NONE;
  if (input.altKey || input.metaKey || input.ctrlKey) return NONE;

  if (key === "Escape") return { type: "escape" };
  if (key === "/" && !input.shiftKey) return { type: "search" };
  if (key === "[" && !input.shiftKey) return { type: "toggle-panel" };
  if (key in LAYER_KEYS) return { type: "layer", layer: LAYER_KEYS[key] };
  if (key === "+" || (key === "=" && input.shiftKey)) return { type: "zoom-in" };
  if (key === "-" || key === "−") return { type: "zoom-out" };
  if (key.toLowerCase() === "f") return { type: "fit" };
  if (key.toLowerCase() === "z") return { type: "zoom-selection" };
  if (key.toLowerCase() === "t") return { type: "cycle-theme" };
  if (key === "?") return { type: "keyboard-list" };

  if (input.overviewOpen) {
    if (key === "ArrowDown") return { type: "overview-move", direction: 1 };
    if (key === "ArrowUp") return { type: "overview-move", direction: -1 };
    if (key === "Enter") return { type: "overview-activate" };
  }
  return NONE;
}
