// Issue #82 "link colour legend" (owner feedback: "what's the colored
// circles?"). A selected file draws solid red rings/lines to what it
// imports (MapRenderer's selection-links block, `var(--hot)`) and dashed
// blue rings/lines to what imports it (`var(--cold)`) -- nothing on screen
// explained that mapping. Colour-coding the file card's own "imported by N"
// / "imports M" counts the same way, each with a tiny glyph matching the
// map's own dash pattern (dashed for --cold/"imported by", solid for
// --hot/"imports"), makes this line double as the legend instead of adding
// a separate one somewhere else. One shared component so the desktop card,
// the phone sheet (same markup -- SelectionPanel's FileHead renders both)
// and the fullscreen summary bar (SelectionSummaryBar) can't drift apart on
// wording or colour the way two independent implementations could.
function LineGlyph({ color, dashed }: { color: string; dashed: boolean }) {
  return (
    <svg
      width="13"
      height="8"
      viewBox="0 0 13 8"
      aria-hidden="true"
      className="inline-block align-middle"
      style={{ marginRight: 3, marginBottom: 1 }}
    >
      <line x1="1" y1="4" x2="12" y2="4" stroke={color} strokeWidth="1.6" strokeDasharray={dashed ? "3 2" : undefined} />
    </svg>
  );
}

/** "imported by N file(s) · imports M", both halves colour- and glyph-coded
 * to match the map's own selection rings/lines for the same direction.
 *
 * `stacked` (owner follow-up, issue #82): the desktop file card's fixed
 * 230px panel ellipsised this line once either count hit three digits
 * (`langgenius__dify-desktop-file-selected.png`: "imported by 687 files ·
 * — imp…", `imports 11` cut off entirely) -- a real count, not a label,
 * belongs fully on screen rather than truncated. `stacked` puts each half on
 * its own line instead of joining them with " · ", so neither ever competes
 * with the other for width; SelectionPanel's FileHead (the desktop card AND
 * the phone sheet, same markup) passes it, SelectionSummaryBar's fullscreen
 * bar does not -- that bar is one line by design (its own "Details" button
 * is the way to see the rest), and a two-line subtitle there would fight its
 * own `truncate` for height instead of width. */
export function LinkCountsLabel({ inDeg, outDeg, stacked = false }: { inDeg: number; outDeg: number; stacked?: boolean }) {
  const importedBy = (
    <span className={stacked ? "block truncate" : undefined} style={{ color: "var(--cold)" }}>
      <LineGlyph color="var(--cold)" dashed />
      imported by {inDeg} file{inDeg === 1 ? "" : "s"}
    </span>
  );
  const imports = (
    <span className={stacked ? "block truncate" : undefined} style={{ color: "var(--hot)" }}>
      <LineGlyph color="var(--hot)" dashed={false} />
      imports {outDeg}
    </span>
  );
  if (stacked) {
    return (
      <span data-link-legend className="flex flex-col gap-0.5">
        {importedBy}
        {imports}
      </span>
    );
  }
  return (
    <span data-link-legend>
      {importedBy}
      {" · "}
      {imports}
    </span>
  );
}
