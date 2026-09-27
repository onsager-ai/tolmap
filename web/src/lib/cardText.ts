import type { Route } from "@/map/graph";

// Plain values the desktop panel and the phone sheet's cards both show
// (docs/UX.md principle 10), kept out of the component files so each of
// those exports only components.

// Theme-alignment audit: these used to be fixed hex values, which read fine
// against the old dark-only --chrome but drop under WCAG AA against the new
// light --chrome (index.css's own --why-* comment has the measured ratios).
// Indirected through CSS custom properties (light/dark variants defined
// there) rather than a second JS-side light/dark table here, so this file
// doesn't need to know which theme is active.
export const WHY_COLOR: Record<string, string> = {
  entry: "var(--why-entry)",
  bridge: "var(--why-bridge)",
  hub: "var(--why-hub)",
  capital: "var(--why-capital)",
  hazard: "var(--why-hazard)",
};

export function compactCount(count: number): string {
  if (count < 1000) return String(count);
  const unit = count >= 1_000_000 ? 1_000_000 : 1000;
  const value = count / unit;
  const digits = value < 10 ? Math.floor(value * 10) / 10 : Math.floor(value);
  return `${digits}${unit === 1000 ? "k" : "m"}`;
}

/** One line on which way a found path runs (RouteBox, the phone's path card). */
export const PATH_KIND_TEXT: Record<Route["kind"], string> = {
  imports: "follows imports, source → target",
  "imported-by": "reverse direction — the target imports the source",
  undirected: "no directed path; shown ignoring edge direction",
};
