import { useSyncExternalStore } from "react";

// Same 820px breakpoint as the reference's `@media (max-width:820px)` and its
// `NARROW()` helper — chrome (brand, segmented controls, footer) collapses
// into the compact phone bar below it, and the district/landmark list
// becomes a bottom drawer instead of a fixed sidebar.
const QUERY = "(max-width: 820px)";

function subscribe(cb: () => void) {
  const mq = matchMedia(QUERY);
  mq.addEventListener("change", cb);
  return () => mq.removeEventListener("change", cb);
}
function getSnapshot() {
  return matchMedia(QUERY).matches;
}

export function useIsNarrow(): boolean {
  return useSyncExternalStore(subscribe, getSnapshot, () => false);
}
