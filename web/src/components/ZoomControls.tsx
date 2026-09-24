interface Props {
  onZoomIn(): void;
  onZoomOut(): void;
  onFit(): void;
  isFullscreen: boolean;
  onToggleFullscreen(): void;
}

/** Issue #82 A1 scope item 5: one fullscreen button, desktop and phone
 * alike -- the ⛶/✕ glyph pair the prototype used for the same toggle
 * (arch20.body.html's `$("#z-fs")`), reused rather than inventing a new
 * pictogram for the same action. */
export function ZoomControls({ onZoomIn, onZoomOut, onFit, isFullscreen, onToggleFullscreen }: Props) {
  const btn = "h-7 w-[30px] border-0 border-b border-[var(--rule)] bg-[var(--chrome)] text-[var(--on)] text-[15px] max-[820px]:h-9 max-[820px]:w-[38px] max-[820px]:text-lg";
  return (
    <div className="absolute bottom-2.5 right-2.5 flex flex-col overflow-hidden rounded-md border border-[var(--rule)] max-[820px]:bottom-auto max-[820px]:top-[74px]">
      <button className={btn} onClick={onZoomIn} aria-label="Zoom in">
        +
      </button>
      <button className={btn} onClick={onZoomOut} aria-label="Zoom out">
        −
      </button>
      {/* Issue #82 "fit icon": text "fit" replaced with a corner-brackets
          glyph (four L-shaped brackets pointing inward, the conventional
          "fit to view" pictogram) -- visually distinct from the zoom +/−
          glyphs above and the ⛶/✕ fullscreen glyph below it, at the same
          size/stroke as the fullscreen icon (see its own inline svg).
          Deliberately no `title` attribute (owner correction, issue #82):
          it would show a second, redundant tooltip. `aria-label="Fit map"`
          stays -- the checks key off it. */}
      <button className={`${btn} flex items-center justify-center`} onClick={onFit} aria-label="Fit map">
        <svg width="15" height="15" viewBox="0 0 15 15" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round">
          <path d="M1 5V1.6C1 1.27 1.27 1 1.6 1H5" />
          <path d="M10 1H13.4C13.73 1 14 1.27 14 1.6V5" />
          <path d="M14 10V13.4C14 13.73 13.73 14 13.4 14H10" />
          <path d="M5 14H1.6C1.27 14 1 13.73 1 13.4V10" />
        </svg>
      </button>
      <button
        className={`${btn} border-b-0`}
        onClick={onToggleFullscreen}
        aria-label={isFullscreen ? "Exit fullscreen" : "Enter fullscreen"}
        aria-pressed={isFullscreen}
      >
        {isFullscreen ? "✕" : "⛶"}
      </button>
    </div>
  );
}
