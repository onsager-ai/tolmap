import type { CSSProperties } from "react";
import { ExitFullscreenIcon, FitIcon, FullscreenIcon, MinusIcon, PlusIcon } from "@/components/phone/icons";

interface Props {
  onZoomIn(): void;
  onZoomOut(): void;
  onFit(): void;
  isFullscreen: boolean;
  onToggleFullscreen(): void;
  /** §5: 40 px on desktop (a pointer), 44 on a tablet (touch, §8.2). */
  size: number;
  /** Where it sits: MapView places it from the same constants the fit safe
   * rectangle is computed from (map/layoutProfile.ts). */
  style: CSSProperties;
}

/** docs/UX.md §5: the desktop and tablet controls, bottom-right of the map --
 * zoom in, zoom out, fit (the inward-corners glyph, owner 2026-09-24) and
 * fullscreen. The phone has its own control column and no fullscreen
 * button (§3). Deliberately no `title` on the fit button (owner correction,
 * issue #82: it would show a second, redundant tooltip); `aria-label="Fit
 * map"` stays -- the checks key off it. */
export function ZoomControls({ onZoomIn, onZoomOut, onFit, isFullscreen, onToggleFullscreen, size, style }: Props) {
  const btn = "flex items-center justify-center border-b border-[var(--rule)] text-[var(--on)] last:border-b-0 hover:bg-subtle";
  const box = { width: size, height: size };
  return (
    <div
      data-map-controls
      className="absolute z-20 flex flex-col overflow-hidden rounded-[12px] border border-[var(--rule)] bg-[var(--chrome2)] shadow-[0_6px_18px_rgba(0,0,0,.18)]"
      style={style}
    >
      <button type="button" className={btn} style={box} onClick={onZoomIn} aria-label="Zoom in">
        <PlusIcon size={18} />
      </button>
      <button type="button" className={btn} style={box} onClick={onZoomOut} aria-label="Zoom out">
        <MinusIcon size={18} />
      </button>
      <button type="button" className={btn} style={box} onClick={onFit} aria-label="Fit map">
        <FitIcon size={18} />
      </button>
      <button
        type="button"
        className={btn}
        style={box}
        onClick={onToggleFullscreen}
        aria-label={isFullscreen ? "Exit fullscreen" : "Enter fullscreen"}
        aria-pressed={isFullscreen}
      >
        {isFullscreen ? <ExitFullscreenIcon /> : <FullscreenIcon />}
      </button>
    </div>
  );
}
