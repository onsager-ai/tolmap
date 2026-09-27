import type { Config } from "tailwindcss";

// Chrome/panel/card tokens only. The map surface never reads Tailwind classes —
// it is drawn by the imperative SVG renderer against the CSS custom properties
// defined in src/index.css (district colours, canvas, ink), which must stay
// byte-identical to viewer/template.html's palette: colour-by-district-id is a
// determinism requirement, not a style choice.
export default {
  darkMode: ["class", '[data-theme="dark"]'],
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  // docs/UX.md §7.3: `hover:` styles only apply on devices that can hover,
  // so a tap on a phone never leaves a button stuck in its hover style.
  future: {
    hoverOnlyWhenSupported: true,
  },
  theme: {
    extend: {
      // The shadcn component names resolve to the chrome tokens of
      // docs/UX.md §8.3 (src/index.css), not to a second palette -- see the
      // comment where the old shadcn HSL values used to be in index.css.
      colors: {
        border: "var(--rule)",
        input: "var(--rule)",
        ring: "var(--accent)",
        background: "var(--chrome2)",
        foreground: "var(--on)",
        primary: {
          DEFAULT: "var(--accent)",
          foreground: "var(--on-accent)",
        },
        secondary: {
          DEFAULT: "var(--chrome)",
          foreground: "var(--on)",
        },
        muted: {
          DEFAULT: "var(--chrome)",
          foreground: "var(--dim)",
        },
        accent: {
          DEFAULT: "var(--accent)",
          foreground: "var(--on-accent)",
        },
        // shadcn's old "accent": the faint hover wash behind ghost and
        // outline buttons. A mix with --on, so it reads on --chrome and
        // --chrome2 alike, in both themes.
        subtle: {
          DEFAULT: "color-mix(in srgb, var(--on) 8%, transparent)",
          foreground: "var(--on)",
        },
        card: {
          DEFAULT: "var(--chrome2)",
          foreground: "var(--on)",
        },
      },
      borderRadius: {
        lg: "var(--radius)",
        md: "calc(var(--radius) - 2px)",
        sm: "calc(var(--radius) - 4px)",
      },
      fontFamily: {
        sans: ["Archivo", "ui-sans-serif", "system-ui", "sans-serif"],
        mono: ["IBM Plex Mono", "ui-monospace", "monospace"],
      },
      // docs/UX.md §8.1's type scale, by role. Nothing is smaller than 12 px,
      // and 12 px is only for uppercase group labels (`text-label uppercase`).
      // Inputs are 16 px on narrow widths (index.css enforces it for every
      // input, select and textarea).
      fontSize: {
        title: ["30px", { lineHeight: "36px", fontWeight: "700" }],
        "sheet-title": ["22px", { lineHeight: "28px", fontWeight: "600" }],
        "section-title": ["20px", { lineHeight: "26px", fontWeight: "600" }],
        row: ["16px", { lineHeight: "22px", fontWeight: "600" }],
        body: ["16px", { lineHeight: "24px" }],
        small: ["14px", { lineHeight: "20px" }],
        meta: ["13px", { lineHeight: "18px" }],
        label: ["12px", { lineHeight: "16px", fontWeight: "600", letterSpacing: "0.06em" }],
      },
    },
  },
  plugins: [],
} satisfies Config;
