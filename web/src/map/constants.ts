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

export type Geo = "r" | "p" | "t";
export type Layer = "d" | "c" | "x";
