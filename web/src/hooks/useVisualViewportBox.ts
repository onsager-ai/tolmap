import { useEffect, useState } from "react";

// docs/UX.md §4.8: the search results sit above the on-screen keyboard. On
// iOS the keyboard does not shrink the layout viewport, it shrinks the
// VISUAL one (and may scroll it by `offsetTop`), so a layer sized to the
// page puts its last rows under the keyboard. A `position: fixed` layer
// placed at the visual viewport's offset and height keeps the list's bottom
// on the keyboard's top edge. Android Chrome resizes the layout viewport as
// well, where this is the same box as the page.

export interface ViewportBox {
  top: number;
  height: number;
}

function read(): ViewportBox | null {
  const vv = typeof window === "undefined" ? undefined : window.visualViewport;
  return vv ? { top: vv.offsetTop, height: vv.height } : null;
}

/** The visual viewport's box, tracked while `active`; null where the API is
 * missing (the caller then fills the page). */
export function useVisualViewportBox(active: boolean): ViewportBox | null {
  const [box, setBox] = useState<ViewportBox | null>(read);
  useEffect(() => {
    if (!active) return;
    const vv = window.visualViewport;
    if (!vv) return;
    const update = () => {
      const next = read();
      setBox((prev) => (prev && next && prev.top === next.top && prev.height === next.height ? prev : next));
    };
    update();
    vv.addEventListener("resize", update);
    vv.addEventListener("scroll", update);
    return () => {
      vv.removeEventListener("resize", update);
      vv.removeEventListener("scroll", update);
    };
  }, [active]);
  return box;
}
