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
      {/* Issue #82 "fit icon", owner review follow-up: the FIRST version of
          this glyph (four brackets sitting flush at the box's own corners)
          read as visually identical to the fullscreen glyph below it ("⛶" is
          exactly that same "four corner marks on a square" shape) -- not
          distinct at all, per the owner's screenshot review. This version
          instead draws two short arrows converging INWARD from opposite
          corners toward the centre (the conventional "compress to fit"
          pictogram, e.g. Lucide's Minimize2) -- diagonal lines through the
          middle of the glyph, the opposite silhouette from a square's own
          corner marks, so it can't be confused with "⛶"/"✕" at a glance.
          Same size/stroke as the other glyphs. Deliberately no `title`
          attribute (owner correction, issue #82): it would show a second,
          redundant tooltip. `aria-label="Fit map"` stays -- the checks key
          off it. */}
      <button className={`${btn} flex items-center justify-center`} onClick={onFit} aria-label="Fit map">
        <svg width="15" height="15" viewBox="0 0 15 15" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
          <polyline points="11 5.5 7.7 5.5 7.7 2.2" />
          <line x1="7.7" y1="5.5" x2="11.6" y2="1.7" />
          <polyline points="2.2 7.7 5.5 7.7 5.5 11" />
          <line x1="1.7" y1="11.6" x2="5.5" y2="7.7" />
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
