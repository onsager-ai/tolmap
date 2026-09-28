import type { MapDocument } from "@/types";

/** The desktop panel's small, local history. Map selections replace this
 * trail with overview → district → file → symbol; card actions can then push
 * a path or quality view without changing the map's own URL history. */
export type DesktopPanelView =
  | { type: "overview" }
  | { type: "district"; district: number }
  | { type: "file"; district: number; file: number }
  | { type: "symbol"; district: number; file: number; label: string; symbol?: number; hierarchicalSymbol?: number }
  | { type: "path" }
  | { type: "quality" };

export type DesktopPanelAction =
  | { type: "overview" }
  | { type: "map-district"; district: number }
  | { type: "map-file"; district: number; file: number }
  | { type: "map-symbol"; district: number; file: number; label: string; symbol?: number; hierarchicalSymbol?: number }
  | { type: "push"; view: Exclude<DesktopPanelView, { type: "overview" }> }
  | { type: "pop" }
  | { type: "jump"; index: number };

export const DESKTOP_PANEL_OVERVIEW: readonly DesktopPanelView[] = [{ type: "overview" }];

export function desktopPanelReducer(
  state: readonly DesktopPanelView[],
  action: DesktopPanelAction,
): readonly DesktopPanelView[] {
  switch (action.type) {
    case "overview":
      return DESKTOP_PANEL_OVERVIEW;
    case "map-district":
      return [{ type: "overview" }, { type: "district", district: action.district }];
    case "map-file":
      return [
        { type: "overview" },
        { type: "district", district: action.district },
        { type: "file", district: action.district, file: action.file },
      ];
    case "map-symbol":
      return [
        { type: "overview" },
        { type: "district", district: action.district },
        { type: "file", district: action.district, file: action.file },
        {
          type: "symbol",
          district: action.district,
          file: action.file,
          label: action.label,
          ...(action.symbol == null ? {} : { symbol: action.symbol }),
          ...(action.hierarchicalSymbol == null ? {} : { hierarchicalSymbol: action.hierarchicalSymbol }),
        },
      ];
    case "push": {
      const top = state[state.length - 1];
      if (top?.type === action.view.type) return state;
      return [...state, action.view];
    }
    case "pop":
      return state.length > 1 ? state.slice(0, -1) : state;
    case "jump":
      return action.index < 0 || action.index >= state.length ? state : state.slice(0, action.index + 1);
  }
}

export interface DesktopPanelCrumb {
  index: number;
  label: string;
  type: DesktopPanelView["type"];
}

/** Labels are plain map-document values so the head and collapsed tab use
 * the same spelling as the map and card. */
export function desktopPanelCrumbs(
  views: readonly DesktopPanelView[],
  doc: MapDocument,
  repoSlug = doc.repo,
): DesktopPanelCrumb[] {
  return views.map((view, index) => {
    let label: string;
    switch (view.type) {
      case "overview":
        label = repoSlug;
        break;
      case "district":
        label = doc.names[String(view.district)] ?? `District ${view.district}`;
        break;
      case "file":
        label = doc.F[view.file]?.split("/").pop() ?? "File";
        break;
      case "symbol":
        label = view.label;
        break;
      case "path":
        label = "Path";
        break;
      case "quality":
        label = "Map quality";
        break;
    }
    return { index, label, type: view.type };
  });
}
