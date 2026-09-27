import { clsx, type ClassValue } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

// tailwind.config.ts names the docs/UX.md §8.1 type scale by role
// (`text-meta`, `text-row`, ...). tailwind-merge doesn't read that config, so
// without this it would take `text-meta` for a text COLOUR and drop it the
// moment a component merged in `text-muted-foreground` -- silently falling
// back to the inherited size.
const twMerge = extendTailwindMerge({
  extend: {
    classGroups: {
      "font-size": [{ text: ["title", "sheet-title", "section-title", "row", "body", "small", "meta", "label"] }],
    },
  },
});

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
