import { useEffect, useState } from "react";
import type { PhoneMetrics } from "@/map/phoneShell";

// docs/UX.md §3.1: the phone shell's heights come from the VISIBLE viewport
// (window.visualViewport) and the real safe-area insets, never from `vh`.
// env(safe-area-inset-*) has no JS API, so it is read the one way the
// platform allows: a throwaway element sized by it.

function readInset(side: "top" | "bottom"): number {
  const probe = document.createElement("div");
  probe.style.cssText = `position:fixed;left:0;top:0;width:1px;visibility:hidden;pointer-events:none;height:env(safe-area-inset-${side},0px)`;
  document.body.appendChild(probe);
  const h = probe.offsetHeight;
  probe.remove();
  return h;
}

function measure(): PhoneMetrics {
  const vv = window.visualViewport;
  return {
    width: vv?.width ?? window.innerWidth,
    height: vv?.height ?? window.innerHeight,
    safeTop: readInset("top"),
    safeBottom: readInset("bottom"),
  };
}

function same(a: PhoneMetrics, b: PhoneMetrics) {
  return a.width === b.width && a.height === b.height && a.safeTop === b.safeTop && a.safeBottom === b.safeBottom;
}

/** Measured synchronously on first render (the map's opening fit needs the
 * sheet's height before its first paint), then on every window or visual
 * viewport resize. */
export function usePhoneMetrics(): PhoneMetrics {
  const [metrics, setMetrics] = useState<PhoneMetrics>(measure);
  useEffect(() => {
    const update = () => {
      const next = measure();
      setMetrics((prev) => (same(prev, next) ? prev : next));
    };
    window.addEventListener("resize", update);
    window.visualViewport?.addEventListener("resize", update);
    update();
    return () => {
      window.removeEventListener("resize", update);
      window.visualViewport?.removeEventListener("resize", update);
    };
  }, []);
  return metrics;
}
