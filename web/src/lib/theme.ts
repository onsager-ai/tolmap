import { useSyncExternalStore } from "react";
import { invalidateColourCache } from "@/map/geometry";
import { invalidatePackageColourCache } from "@/map/packageLayout";

// Issue #82 "chrome follows the theme" (owner decision, AskUserQuestion
// 2026-09-24, "Follow system + toggle"): the one module that knows about the
// three-state System/Light/Dark choice, persists it, and applies it to the
// document. `web/index.html`'s inline bootstrap script duplicates the
// storage-read half of this (it runs before any bundle does, so it can't
// import this module) -- APPLY_ATTR's rule ("system" = no attribute,
// "light"/"dark" = set it) must stay identical in both places or the very
// first paint and a later toggle could disagree.
export type ThemeChoice = "system" | "light" | "dark";
export type EffectiveTheme = "light" | "dark";

const STORAGE_KEY = "tolmap:theme";
const CHANGE_EVENT = "tolmap:theme-change";

function isThemeChoice(value: string | null): value is ThemeChoice {
  return value === "system" || value === "light" || value === "dark";
}

// Every localStorage touch is wrapped: a private window, a user with storage
// blocked, or a strict cookie/storage policy can make even a plain read
// throw -- this module degrades to "system" (never persisted, but never
// crashing) rather than taking the app down over a preference.
function readStoredChoice(): ThemeChoice {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    return isThemeChoice(raw) ? raw : "system";
  } catch {
    return "system";
  }
}

function writeStoredChoice(choice: ThemeChoice) {
  try {
    window.localStorage.setItem(STORAGE_KEY, choice);
  } catch {
    /* storage can throw on write even when readable (quota, private mode) -- the
       choice still applies for this page life, it just won't survive a reload */
  }
}

function systemPrefersDark(): boolean {
  return matchMedia("(prefers-color-scheme: dark)").matches;
}

/** "system" resolved against what the OS/browser currently prefers -- the
 * theme actually painted right now, as opposed to the stored CHOICE (which
 * stays "system" until the person explicitly picks light or dark). */
export function resolveEffectiveTheme(choice: ThemeChoice): EffectiveTheme {
  return choice === "system" ? (systemPrefersDark() ? "dark" : "light") : choice;
}

/** Sets/removes `data-theme` on the root element -- the same rule
 * index.html's inline bootstrap script applies before this module ever
 * loads. Exported on its own (rather than folded into applyThemeChoice)
 * so a caller that only needs the DOM effect, never persistence or the
 * change event (there is none today, kept for symmetry with that script),
 * can use it directly. */
export function applyThemeAttribute(choice: ThemeChoice) {
  if (choice === "system") document.documentElement.removeAttribute("data-theme");
  else document.documentElement.dataset.theme = choice;
}

/** The one place a theme choice is ever applied: sets the DOM attribute,
 * persists it, invalidates the map's own colour caches (geometry.ts/
 * packageLayout.ts each resolve `--canvas`/`--H0..`/`--p0..` once and cache
 * the result -- see their own doc comments), and tells every subscriber
 * (the toggle's own icon, MapView's `useEffectiveTheme`) to re-read the
 * current state. Colour-cache invalidation happens here, synchronously,
 * BEFORE the change event fires -- so by the time a subscriber's React
 * update runs and (for MapView) rebuilds `packageLayout`, every colour
 * function it calls already sees the new theme's resolved CSS values. */
export function applyThemeChoice(choice: ThemeChoice) {
  applyThemeAttribute(choice);
  writeStoredChoice(choice);
  invalidateColourCache();
  invalidatePackageColourCache();
  window.dispatchEvent(new Event(CHANGE_EVENT));
}

// Fires the SAME re-render trigger the explicit applyThemeChoice above does,
// for the one theme change that doesn't come from this module's own
// setter: the OS flipping prefers-color-scheme while the stored choice is
// "system" (nothing here calls applyThemeChoice for that case -- there is no
// new CHOICE to persist, only a new EFFECTIVE theme to repaint for). Every
// useSyncExternalStore subscription below shares this one function, so the
// invalidation runs once per change per subscriber, not once per render.
function subscribe(onStoreChange: () => void): () => void {
  const onChange = () => {
    invalidateColourCache();
    invalidatePackageColourCache();
    onStoreChange();
  };
  window.addEventListener(CHANGE_EVENT, onChange);
  window.addEventListener("storage", onChange); // another tab changed the choice
  const mq = matchMedia("(prefers-color-scheme: dark)");
  mq.addEventListener("change", onChange);
  return () => {
    window.removeEventListener(CHANGE_EVENT, onChange);
    window.removeEventListener("storage", onChange);
    mq.removeEventListener("change", onChange);
  };
}

/** The stored choice ("system" by default) plus its setter -- the theme
 * toggle's own state. SSR/first-render snapshot is "system" (this app has
 * no SSR, but useSyncExternalStore requires a getServerSnapshot). */
export function useThemeChoice(): [ThemeChoice, (choice: ThemeChoice) => void] {
  const choice = useSyncExternalStore<ThemeChoice>(subscribe, readStoredChoice, () => "system");
  return [choice, applyThemeChoice];
}

/** The resolved light/dark theme, recomputed whenever the CHOICE or the OS
 * preference changes. MapView keys its colour-cache-sensitive `useMemo`s off
 * this (not the raw choice) precisely because "system" itself never changes
 * value but what it RESOLVES to does the moment the OS flips -- and that's
 * exactly the case with no explicit applyThemeChoice call, so nothing else
 * would otherwise tell a memo to recompute. */
export function useEffectiveTheme(): EffectiveTheme {
  const choice = useSyncExternalStore<ThemeChoice>(subscribe, readStoredChoice, () => "system");
  return resolveEffectiveTheme(choice);
}
