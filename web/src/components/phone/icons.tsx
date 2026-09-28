import type { ReactNode } from "react";

// The phone shell's icons (docs/UX.md §3), drawn as the approved canvas and
// the prototype draw them: 24-unit strokes in currentColor, so every icon
// takes the chrome's text colour in both themes.

function Icon({ size = 20, children }: { size?: number; children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  );
}

export const SearchIcon = ({ size = 20 }: { size?: number }) => (
  <Icon size={size}>
    <circle cx="11" cy="11" r="7" />
    <path d="M20 20l-3.5-3.5" />
  </Icon>
);
/** The repository sheet's "Home" row: a house. */
export const HomeIcon = ({ size = 20 }: { size?: number }) => (
  <Icon size={size}>
    <path d="M4 11l8-7 8 7" />
    <path d="M6 9.5V20h12V9.5" />
  </Icon>
);
export const PlusIcon = ({ size = 20 }: { size?: number }) => (
  <Icon size={size}>
    <path d="M12 5v14M5 12h14" />
  </Icon>
);
export const MinusIcon = ({ size = 20 }: { size?: number }) => (
  <Icon size={size}>
    <path d="M5 12h14" />
  </Icon>
);
/** Inward corners: "fit" (owner, 2026-09-24), distinct from a fullscreen glyph. */
export const FitIcon = ({ size = 20 }: { size?: number }) => (
  <Icon size={size}>
    <path d="M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5" />
  </Icon>
);
export const LayersIcon = () => (
  <Icon>
    <path d="M12 3l9 5-9 5-9-5 9-5z" />
    <path d="M3 13l9 5 9-5" />
  </Icon>
);
export const CloseIcon = () => (
  <Icon size={18}>
    <path d="M6 6l12 12M18 6L6 18" />
  </Icon>
);
export const ChevronIcon = () => (
  <Icon size={14}>
    <path d="M9 6l6 6-6 6" />
  </Icon>
);
export const BackIcon = ({ size = 22 }: { size?: number } = {}) => (
  <Icon size={size}>
    <path d="M15 5l-7 7 7 7" />
  </Icon>
);
export const ClearIcon = () => (
  <Icon size={14}>
    <path d="M6 6l12 12M18 6L6 18" />
  </Icon>
);
/** docs/UX.md §4.8's result-row glyphs: a district (a folded map), a file,
 * a symbol (braces). Drawn as the "Search active" artboard draws them. */
export const DistrictIcon = ({ size = 20 }: { size?: number }) => (
  <Icon size={size}>
    <path d="M4 6l5-2 6 2 5-2v14l-5 2-6-2-5 2z" />
  </Icon>
);
export const FileIcon = ({ size = 20 }: { size?: number }) => (
  <Icon size={size}>
    <path d="M6 3h8l4 4v14H6z" />
  </Icon>
);
export const SymbolIcon = ({ size = 20 }: { size?: number }) => (
  <Icon size={size}>
    <path d="M8 4c-2 0-3 1-3 3v2c0 1.5-1 2.5-2 3 1 .5 2 1.5 2 3v2c0 2 1 3 3 3M16 4c2 0 3 1 3 3v2c0 1.5 1 2.5 2 3-1 .5-2 1.5-2 3v2c0 2-1 3-3 3" />
  </Icon>
);
/** docs/UX.md §5's desktop controls: outward corners for fullscreen (the
 * opposite silhouette of the fit glyph's inward ones), a cross to leave it. */
export const FullscreenIcon = ({ size = 18 }: { size?: number }) => (
  <Icon size={size}>
    <path d="M9 4H4v5M15 4h5v5M9 20H4v-5M15 20h5v-5" />
  </Icon>
);
export const ExitFullscreenIcon = ({ size = 18 }: { size?: number }) => (
  <Icon size={size}>
    <path d="M6 6l12 12M18 6L6 18" />
  </Icon>
);
/** The tablet top bar's rail toggle, and the landscape side sheet's (§9): a
 * panel on the left. */
export const PanelIcon = ({ size = 20 }: { size?: number }) => (
  <Icon size={size}>
    <rect x="3" y="4" width="18" height="16" rx="2" />
    <path d="M9 4v16" />
  </Icon>
);
export const ChevronDownIcon = ({ size = 14 }: { size?: number }) => (
  <Icon size={size}>
    <path d="M6 9l6 6 6-6" />
  </Icon>
);
