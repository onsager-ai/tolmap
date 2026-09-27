// docs/UX.md §3.4: the phone's back stack, as a pure reducer so
// web/scripts/check-phone-shell.ts can test it without a browser.
//
// Opening an overlay pushes exactly one history entry, so OS back (Android
// back, the iOS edge swipe, the browser button) closes overlays before it
// leaves the map. The UI opens them in a nesting order (a selection, then the
// sheet raised, then layers or search on top), so popping last-opened-first
// gives §3.4's order: search, layers, sheet height, selection, then the
// previous page.
//
// Entries are identified by the router's own history index (TanStack's
// `__TSR_index`), not by a count kept here: a `replace` keeps the index, so a
// selection change that replaces the URL on top of a pushed entry leaves the
// bookkeeping intact. Each entry also carries a marker in its history state
// (OVERLAY_MARKER), so after a reload the page can tell an overlay entry from
// an ordinary one: overlays themselves are in-memory and start closed, so a
// reload or a shared link never lands in one.
//
// Closing an overlay from the UI (its close button, picking a search result,
// a tap on empty map) does not touch history: it marks the entry dead, and a
// later back that closes nothing but dead entries goes back once more, so
// the person never presses back for nothing. (The prototype skips only when
// it LANDS on a dead entry; popping a dead entry onto a live one then did
// nothing visible. Here that press carries on to the live one.)

export type OverlayKind = "search" | "layers" | "sheet" | "sel";

/** Key in history.state that marks an entry this module pushed. */
export const OVERLAY_MARKER = "tolmapOverlay";

export interface BackEntry {
  kind: OverlayKind;
  /** The history index the entry lives at. */
  index: number;
  /** False once the overlay was closed from the UI. */
  live: boolean;
}

export interface BackState {
  /** The history index the page is at. */
  index: number;
  entries: readonly BackEntry[];
}

export function isOverlayKind(value: unknown): value is OverlayKind {
  return value === "search" || value === "layers" || value === "sheet" || value === "sel";
}

/** The page just loaded at `index`. `marker` is that entry's OVERLAY_MARKER,
 * if any: a reload inside the stack. A `sel` entry with a selection still in
 * the URL is adopted as live (back will clear it); any other marked entry
 * is dead, because its overlay did not survive the reload. */
export function initBack(index: number, marker: unknown, hasSelection: boolean): BackState {
  if (!isOverlayKind(marker)) return { index, entries: [] };
  return { index, entries: [{ kind: marker, index, live: marker === "sel" && hasSelection }] };
}

/** An overlay opened at history index `index`. Returns `push: true` when the
 * caller must push one history entry; opening an overlay that is already
 * open pushes nothing. */
export function openOverlay(state: BackState, kind: OverlayKind, index: number): { state: BackState; push: boolean } {
  if (state.entries.some((e) => e.live && e.kind === kind)) return { state: { ...state, index }, push: false };
  // A push discards forward history, and with it any entry above `index`.
  const kept = state.entries.filter((e) => e.index <= index);
  return { state: { index: index + 1, entries: [...kept, { kind, index: index + 1, live: true }] }, push: true };
}

/** An overlay closed from the UI: its entry stays in history, dead. */
export function closeOverlay(state: BackState, kind: OverlayKind): BackState {
  let at = -1;
  for (let i = state.entries.length - 1; i >= 0; i--) {
    if (state.entries[i].live && state.entries[i].kind === kind) {
      at = i;
      break;
    }
  }
  if (at < 0) return state;
  const entries = state.entries.map((e, i) => (i === at ? { ...e, live: false } : e));
  return { ...state, entries };
}

export interface PopResult {
  state: BackState;
  /** Live overlays the history move closed, most recent first: the caller
   * closes each. */
  undo: OverlayKind[];
  /** The move went back and closed nothing (it popped or landed on dead
   * entries only): go back once more. */
  skipBack: boolean;
}

/** History moved to `index` (popstate: back, forward or go). `marker` is the
 * landed entry's OVERLAY_MARKER; `hasSelection` is whether a selection is
 * still live once `undo` has been applied (a landed `sel` entry is adopted
 * as live only then). */
export function popTo(state: BackState, index: number, marker: unknown, hasSelection: boolean): PopResult {
  const back = index < state.index;
  const popped = state.entries.filter((e) => e.index > index).sort((a, b) => b.index - a.index);
  const undo = popped.filter((e) => e.live).map((e) => e.kind);
  let entries = state.entries.filter((e) => e.index <= index);
  let landed = entries.find((e) => e.index === index);
  if (!landed && isOverlayKind(marker)) {
    const liveSel = marker === "sel" && hasSelection && !undo.includes("sel") && !entries.some((e) => e.live && e.kind === "sel");
    landed = { kind: marker, index, live: liveSel };
    entries = [...entries, landed];
  }
  // A back press that closed nothing visible -- it only popped entries the
  // UI had already closed, or landed on one -- goes back again, so one
  // press always does something: closes the next live overlay, or leaves
  // the map.
  const skipBack = back && undo.length === 0 && (popped.length > 0 || (!!landed && !landed.live));
  return { state: { index, entries }, undo, skipBack };
}

/** Whether any overlay is live -- i.e. whether back stays on the map. */
export function hasLiveOverlay(state: BackState): boolean {
  return state.entries.some((e) => e.live);
}
