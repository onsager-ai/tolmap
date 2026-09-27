import { useSyncExternalStore } from "react";
import { layoutProfile, type LayoutProfile } from "@/map/layoutProfile";

// docs/UX.md §9: the layout follows the available box -- the layout
// viewport's width AND height (map/layoutProfile.ts has the rules and why).
// This replaces useIsNarrow's single `(max-width: 820px)` query, which was
// width alone. innerWidth/innerHeight, not visualViewport: the on-screen
// keyboard and pinch zoom shrink the visual viewport, and neither should
// swap the page's layout under a person typing in search.

function subscribe(cb: () => void) {
  window.addEventListener("resize", cb);
  window.addEventListener("orientationchange", cb);
  return () => {
    window.removeEventListener("resize", cb);
    window.removeEventListener("orientationchange", cb);
  };
}
function getSnapshot(): LayoutProfile {
  return layoutProfile(window.innerWidth, window.innerHeight);
}

export function useLayoutProfile(): LayoutProfile {
  return useSyncExternalStore(subscribe, getSnapshot, () => "desktop");
}
