// Label widths for the map's collision boxes (docs/UX.md §8.1: district and
// neighbourhood names are drawn in Archivo, not IBM Plex Mono).
//
// Every label placer in MapRenderer reserves a box before it draws, and
// rejects a label whose box hits one already placed. Those boxes were
// `text.length * size * 0.62` -- Plex Mono's advance (0.6 em) plus a little.
// That is wrong for a proportional face in both directions: Archivo's
// lowercase averages ~0.52 em at weight 600 (the old estimate reserved too
// much, hiding labels that fit), while capitals and "m"/"w" run 0.7-0.95 em
// (it reserved too little, letting boxes overlap). So the width is the sum of
// Archivo's own advance widths for the label's characters, at its weight.
//
// A table, not canvas measureText(): measuring would depend on whether the
// web font had finished loading when paint() ran (display=swap), so the same
// map could place different labels on two visits. The table makes placement
// a pure function of the text, as it was with the 0.62 estimate.
//
// Source: the advance widths (hmtx) of the static Archivo instances Google
// Fonts serves for wght 400, 500 and 600 (v25, units per em 1000, the same
// family and version index.html loads), read for U+0020-U+007E. Kerning is
// ignored, which only ever makes a box slightly wider than the text.

// Advance widths in thousandths of an em, for char codes 0x20..0x7E.
const ARCHIVO_400 = [
  209, 273, 374, 582, 510, 950, 692, 209, 355, 355, 407, 625, 277, 333, 277, 294, 573, 521, 567, 573, 555, 571, 573, 553,
  574, 573, 296, 296, 625, 625, 625, 578, 1005, 682, 698, 728, 734, 677, 612, 796, 736, 267, 559, 662, 536, 847, 736, 788,
  665, 788, 727, 673, 606, 731, 648, 924, 680, 655, 635, 296, 294, 296, 625, 485, 187, 545, 567, 519, 567, 548, 280, 556,
  563, 225, 223, 514, 225, 860, 563, 570, 567, 567, 332, 510, 297, 562, 504, 723, 513, 504, 498, 353, 245, 353, 625,
];
const ARCHIVO_500 = [
  205, 282, 407, 582, 524, 958, 710, 227, 356, 356, 407, 630, 288, 333, 288, 296, 574, 547, 571, 575, 566, 573, 575, 564,
  575, 574, 315, 315, 630, 630, 630, 595, 1002, 695, 702, 725, 731, 674, 611, 795, 734, 274, 571, 677, 552, 846, 734, 785,
  667, 785, 722, 670, 612, 728, 659, 938, 683, 666, 635, 316, 296, 316, 630, 495, 197, 550, 579, 532, 579, 554, 293, 573,
  573, 238, 236, 528, 238, 860, 573, 583, 579, 579, 346, 524, 305, 572, 516, 740, 529, 516, 503, 372, 245, 372, 630,
];
const ARCHIVO_600 = [
  200, 292, 444, 583, 541, 966, 729, 246, 357, 357, 407, 636, 300, 333, 300, 298, 575, 576, 576, 576, 577, 575, 576, 576,
  576, 575, 336, 336, 636, 636, 636, 613, 998, 709, 706, 721, 728, 672, 609, 794, 732, 282, 585, 695, 570, 844, 732, 782,
  670, 782, 717, 667, 619, 724, 671, 954, 686, 677, 634, 339, 298, 339, 636, 507, 209, 556, 592, 547, 592, 561, 307, 591,
  584, 252, 250, 543, 252, 861, 584, 598, 592, 592, 362, 541, 314, 583, 529, 758, 546, 529, 509, 394, 245, 394, 636,
];
// The few non-ASCII characters map labels actually use (the middle dot in
// "a · b" names, the ellipsis of a truncated name, dashes), same for every
// weight within a thousandth except the ellipsis, taken at 600 (widest).
const EXTRA: Record<string, number> = { "·": 333, "…": 965, "—": 1000, "–": 500 };
// Anything else (CJK, accented Latin outside the table): a full em, so an
// unknown glyph can only ever make its box too wide, never too narrow.
const UNKNOWN = 1000;

export const MAP_LABEL_FONT = "Archivo, ui-sans-serif, system-ui, sans-serif";

/** Width in px of `text` set in Archivo at `size` px and `weight`. Only 400,
 * 500 and 600 are tabled (all the map's Archivo labels use); a weight above
 * 600 would under-measure, so add its table before using one. */
export function archivoWidth(text: string, size: number, weight = 400): number {
  const table = weight <= 400 ? ARCHIVO_400 : weight <= 500 ? ARCHIVO_500 : ARCHIVO_600;
  let units = 0;
  for (const ch of text) {
    const code = ch.codePointAt(0)!;
    if (code >= 0x20 && code <= 0x7e) units += table[code - 0x20];
    else units += EXTRA[ch] ?? UNKNOWN;
  }
  return (units / 1000) * size;
}

/** The collision-box width for a map label drawn in Archivo: the text's own
 * width plus 0.4 em. Map labels carry a canvas-coloured halo (paint-order
 * stroke, 3-3.2 px wide, so ~1.6 px past each glyph edge) and two boxes that
 * merely touch let their halos and letters run together ("utilscommon.ts" in
 * the first CI frames of this change). The old mono estimate hid this by
 * accident: 0.62 em per character against Plex Mono's real 0.6 em left a
 * few pixels of slack on every label. */
export function archivoLabelWidth(text: string, size: number, weight = 400): number {
  return archivoWidth(text, size, weight) + size * 0.4;
}
