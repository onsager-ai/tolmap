// Ported 1:1 from viewer/template.html. Keep these arrays in the same order
// as the reference — symbol rows carry a kind INDEX (SymbolRow[1]), not a
// name, so KIND/KCOL must stay index-aligned with the Rust side's enum.
export const KIND = ["class", "func", "method", "interface", "type", "const"] as const;
export const KCOL = ["#3A6796", "#2F6F63", "#4E7A33", "#7E3F5D", "#8A6B1C", "#93502A"];

// The map stops at the file. Google Maps draws building footprints and stops
// too: below that, addressing is textual and you switch representation
// entirely (street view, a floor directory). Plots and rooms are an opt-in
// cadastral layer, not a zoom level — tiling a district with 200 cells erases
// the silhouette that made the district memorable in the first place.
export const PARCEL_ZOOM = 1.9;
export const BUILD_ZOOM = 5.2;

// Issue #48: at the overview, a large repo draws one SVG circle per file --
// n8n puts 11,982 of them on screen, most a handful of pixels apart. Rather
// than an all-or-nothing per-district gate (tried first, replaced by ranked
// thinning: a district's files fade in by prominence as its on-screen area
// grows, so the map fills in the way a map app reveals points of interest,
// not by popping a whole district on at once), every district gets a dot
// BUDGET = floor(on-screen area px² / DOT_DENSITY_FLOOR), and draws only its
// top `budget` files (see MapRenderer.dotFactor). The floor has to sit
// inside a gap wide enough that no acceptance fixture ever budgets below its
// own file count. Measured on-screen area per file at FIT ZOOM on a 390x700
// viewport, mainland districts (min / median px², cb04469 + #42):
//   flask 272/272  httpx 407/407  sqlalchemy 104/255  vue 77/145
//   prometheus 151/185  django 52/78  crawlab 50/97  dify 11/13  n8n 7/12
// Every fixture + crawlab clears 50; dify and n8n never clear 13. 30 sits in
// that gap: comfortably below every fixture's floor (so `budget >= size`
// there and the fast path in `dotFactor` keeps the DOM byte-identical to
// before this change -- see web/scripts/no-change-proof.ts), comfortably
// above dify/n8n's ceiling (so thinning always engages there at fit zoom).
export const DOT_DENSITY_FLOOR = 30;

// Cap on how many direct-import lines get DRAWN for a hovered or selected
// file (in + out, combined) -- shared by MapRenderer's hover preview and
// persistent selection links, and by SelectionPanel's "links: showing N of
// M" line, so the three can never disagree about where the cut is. Without
// one, a hub file's fan-in alone can run into the thousands (issue #57's
// build log: one file with a fan-in of 26,342) and "lines to every
// neighbour" stops being a preview and starts being another unreadable-
// overview problem. This caps DRAWING only -- the dim/highlight set that
// marks which files are connected is never capped (every neighbour still
// fades in or gets a ring; see MapRenderer.paint()'s selNeighbours/dim).
export const LINK_PREVIEW_MAX = 40;

// B4 (nested footprints, issue #82, scope item 5): neighbourhood labels.
// With no district focused, a neighbourhood needs at least this many files
// AND this many on-screen px of short-side blob extent to earn a label
// (spec). Inside a focused district the file floor is looser -- there's
// nothing else competing for the label budget at that zoom.
export const NEIGHBOURHOOD_LABEL_MIN_FILES = 4;
export const NEIGHBOURHOOD_LABEL_MIN_FILES_FOCUSED = 6;
export const NEIGHBOURHOOD_LABEL_MIN_EXTENT_PX = 120;

// Issue #82 follow-up: at deep zoom, a tiny leaf symbol is compacted by
// src/symbol_cards.rs to a small octagon raster (<=12 cells) and a small
// container keeps its plus-shaped raster region so its child still fits
// inside -- on screen these read as bare, meaningless squares/circles/
// crosses, because MapRenderer.drawSymbolCardsPass only skips a label when
// it doesn't fit, never the card underneath. 14px is the smallest box that
// still fits a ~9px label with a couple of px of padding on each side, so
// below it there is no useful label to withhold -- the shape carries no
// information a viewer could read, and is worse than not drawing it (the
// symbol stays reachable through the outline tree and its reference lines
// roll up to the nearest drawn ancestor card or the file -- see
// symbolCards.ts's rollReferences). The selection, its ancestors, and
// whatever's currently hovered are drawn regardless of size.
export const CARD_MIN_PX = 14;

export type Geo = "r" | "p" | "t";
export type Layer = "d" | "c" | "x" | "p";

// GEO_LABEL/GEO_ORDER used to drive GeoLayerControls.tsx's geometry toggle
// (regions/plots/treemap). Hidden from the UI on 2026-09-22 by owner
// decision: plots needs `P` parcel data only 2 of the 9 acceptance fixtures
// carry, and treemap trades away the map's own silhouette. Left here (not
// deleted, and moved from GeoLayerControls.tsx to this file specifically so
// it stays a plain constants module -- a component file re-exporting unused
// constants breaks Vite's fast-refresh detection) rather than in the
// component that used to render them: re-enable by adding "p"/"t" back to
// GEO_ORDER and restoring the toggle UI in GeoLayerControls.tsx (git
// history has it) and the "r"-only allowlist in routes/search.ts's GEOS.
// Nothing in MapRenderer.ts changed -- geo="p"/"t" still render correctly,
// there's just no control (or valid deep link, after search.ts's
// normalisation) that sets them anymore.
export const GEO_LABEL: Record<Geo, string> = { r: "regions", p: "plots", t: "treemap" };
export const GEO_ORDER: Geo[] = ["r"];
