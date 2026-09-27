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

export const SearchIcon = () => (
  <Icon>
    <circle cx="11" cy="11" r="7" />
    <path d="M20 20l-3.5-3.5" />
  </Icon>
);
export const SwitchIcon = () => (
  <Icon>
    <path d="M7 4v16M7 20l-3-3M7 20l3-3M17 20V4M17 4l-3 3M17 4l3 3" />
  </Icon>
);
export const PlusIcon = () => (
  <Icon>
    <path d="M12 5v14M5 12h14" />
  </Icon>
);
export const MinusIcon = () => (
  <Icon>
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
