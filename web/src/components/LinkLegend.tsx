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
 * to match the map's own selection rings/lines for the same direction. */
export function LinkCountsLabel({ inDeg, outDeg }: { inDeg: number; outDeg: number }) {
  return (
    <span data-link-legend>
      <LineGlyph color="var(--cold)" dashed />
      <span style={{ color: "var(--cold)" }}>
        imported by {inDeg} file{inDeg === 1 ? "" : "s"}
      </span>
      {" · "}
      <LineGlyph color="var(--hot)" dashed={false} />
      <span style={{ color: "var(--hot)" }}>imports {outDeg}</span>
    </span>
  );
}
