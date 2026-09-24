import type { ReactElement } from "react";
import { useThemeChoice, type ThemeChoice } from "@/lib/theme";

const NEXT: Record<ThemeChoice, ThemeChoice> = { system: "light", light: "dark", dark: "system" };
const LABEL: Record<ThemeChoice, string> = { system: "System", light: "Light", dark: "Dark" };

const ICON_PROPS = {
  width: 15,
  height: 15,
  viewBox: "0 0 15 15",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.3,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
};

function SunIcon() {
  return (
    <svg {...ICON_PROPS} aria-hidden="true">
      <circle cx="7.5" cy="7.5" r="2.6" />
      <line x1="7.5" y1="0.9" x2="7.5" y2="2.4" />
      <line x1="7.5" y1="12.6" x2="7.5" y2="14.1" />
      <line x1="0.9" y1="7.5" x2="2.4" y2="7.5" />
      <line x1="12.6" y1="7.5" x2="14.1" y2="7.5" />
      <line x1="2.7" y1="2.7" x2="3.75" y2="3.75" />
      <line x1="11.25" y1="11.25" x2="12.3" y2="12.3" />
      <line x1="12.3" y1="2.7" x2="11.25" y2="3.75" />
      <line x1="3.75" y1="11.25" x2="2.7" y2="12.3" />
    </svg>
  );
}

function MoonIcon() {
  return (
    <svg {...ICON_PROPS} aria-hidden="true">
      <path d="M 12.6 9.4 A 5.4 5.4 0 1 1 5.6 2.4 A 6.6 6.6 0 0 0 12.6 9.4 Z" strokeLinejoin="round" />
    </svg>
  );
}

function MonitorIcon() {
  return (
    <svg {...ICON_PROPS} aria-hidden="true">
      <rect x="1.4" y="2.4" width="12.2" height="8" rx="1" />
      <line x1="5.2" y1="13.1" x2="9.8" y2="13.1" />
      <line x1="7.5" y1="10.4" x2="7.5" y2="13.1" />
    </svg>
  );
}

const ICON: Record<ThemeChoice, () => ReactElement> = { system: MonitorIcon, light: SunIcon, dark: MoonIcon };

/** Issue #82 owner decision ("Follow system + toggle", AskUserQuestion
 * 2026-09-24): a compact three-state control -- System -> Light -> Dark ->
 * System -- rather than a plain on/off switch, since "system" is a real,
 * distinct state (chrome and map both track the OS) and not just a synonym
 * for whichever of light/dark it happens to resolve to right now. One tap
 * always advances to the NEXT state instead of opening a menu -- the same
 * interaction GeoLayerControls' phone cycle-button already uses for the same
 * reason (one thumb, one tap, nothing to dismiss), and it works identically
 * on desktop and phone without a separate narrow-width branch.
 *
 * No native `title` (owner correction on the fit-map icon, issue #82: "it
 * would show a second, redundant tooltip" -- the same reasoning applies
 * here): the icon plus `aria-label` is the whole affordance. */
export function ThemeToggle() {
  const [choice, setChoice] = useThemeChoice();
  const Icon = ICON[choice];
  return (
    <button
      type="button"
      data-theme-toggle
      data-theme-choice={choice}
      aria-label={`Theme: ${LABEL[choice]}. Click to switch to ${LABEL[NEXT[choice]]}.`}
      onClick={() => setChoice(NEXT[choice])}
      className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-[var(--rule)] bg-[var(--chrome2)] text-[var(--on)]"
    >
      <Icon />
    </button>
  );
}
