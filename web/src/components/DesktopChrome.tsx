import { useEffect, useMemo, useRef, useState, type ComponentProps, type KeyboardEvent, type ReactNode } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import type { CatalogueEntry, MapDocument } from "@/types";
import type { Layer } from "@/map/constants";
import { CH, CX_, districtColor, ramp } from "@/map/geometry";
import { displayLanguage, layerOverviewHeadline } from "@/map/layerOverview";
import { buildDistrictIndexRow, districtIndexDistrictIds } from "@/map/districtIndex";
import type { PackageGrouping, PackageLayout } from "@/map/packageLayout";
import {
  COMMAND_BAR_HEIGHT_PX,
  COMMAND_BAR_TOP_PX,
  DESKTOP_CONTROL_BOTTOM_PX,
  DESKTOP_GUTTER_PX,
  DESKTOP_LEGEND_BOTTOM_PX,
  DESKTOP_PANEL_BOTTOM_PX,
  DESKTOP_PANEL_LEFT_PX,
  DESKTOP_PANEL_TOP_PX,
  DESKTOP_PANEL_WIDTH_PX,
  desktopControlSize,
} from "@/map/layoutProfile";
import { FolderBody } from "@/components/SelectionPanel";
import { ThemeToggle } from "@/components/ThemeToggle";
import { PackageLegend } from "@/components/PackageLegend";
import { SearchBox } from "@/components/SearchBox";
import type { SearchCommandDefinition, SearchPick } from "@/map/searchResults";
import { desktopPanelCrumbs, type DesktopPanelView } from "@/map/desktopPanel";
import { dispatchDesktopKey, type DesktopFocusScope } from "@/map/desktopKeyboard";
import { nextThemeChoice, useThemeChoice } from "@/lib/theme";
import { SelectionCard } from "@/components/phone/SheetCards";
import { HeadlineText, LayerOverviewHeadline, LayerOverviewIndex, useLayerOverview } from "@/components/LayerOverview";
import {
  BackIcon,
  ChevronDownIcon,
  ChevronIcon,
  FitIcon,
  FullscreenIcon,
  HomeIcon,
  MinusIcon,
  PanelIcon,
  PlusIcon,
  SearchIcon,
} from "@/components/phone/icons";

const LAYERS: { id: Layer; label: string; key: string; icon: ReactNode }[] = [
  {
    id: "d",
    label: "District",
    key: "1",
    icon: <><circle cx="8.5" cy="9" r="5" /><circle cx="16.5" cy="9.5" r="3.5" /><circle cx="12.5" cy="17" r="4" /></>,
  },
  {
    id: "c",
    label: "Churn",
    key: "2",
    icon: <path d="M3 12h4l3-7 4 14 3-7h4" />,
  },
  {
    id: "x",
    label: "Complexity",
    key: "3",
    icon: <><circle cx="6" cy="5" r="2" /><circle cx="6" cy="19" r="2" /><circle cx="18" cy="7" r="2" /><path d="M6 7v10M18 9c0 5-7 4-11 8" /></>,
  },
  {
    id: "p",
    label: "Package",
    key: "4",
    icon: <><path d="M12 3l8 4.5v9L12 21l-8-4.5v-9z" /><path d="M4 7.5l8 4.5 8-4.5M12 12v9" /></>,
  },
];

// The overview headline on every layer: one balanced line pair at most, in
// tabular numerals so counts don't jitter as the layer changes.
const DESKTOP_HEADLINE = "mt-1 text-balance text-[20px] font-semibold leading-[1.25] tracking-[-.015em] tabular-nums";

interface RepoRow {
  slug: string;
  files?: number;
  districts?: number;
  lang?: string;
  current: boolean;
}

function repoRows(catalogue: CatalogueEntry[] | undefined, doc: MapDocument, owner: string, repo: string): RepoRow[] {
  const slug = `${owner}/${repo}`;
  const rows: RepoRow[] = (catalogue ?? []).map((entry) => ({
    slug: entry.slug,
    files: typeof entry.files === "number" ? entry.files : undefined,
    districts: typeof entry.districts === "number" ? entry.districts : undefined,
    lang: typeof entry.lang === "string" && entry.lang.length > 0 ? entry.lang : undefined,
    current: entry.slug === slug,
  }));
  const current = rows.find((row) => row.current);
  if (current) {
    // Issue #171: the map document on screen is authoritative for the
    // current repository's file count, even when the catalogue is stale.
    current.files = doc.F.length;
  } else {
    // The map document supplies the current file count, but the menu only
    // shows catalogue metadata for fields other than that explicit fix.
    rows.unshift({ slug, files: doc.F.length, current: true });
  }
  return rows;
}

function MonoIcon({ children, size = 16 }: { children: ReactNode; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {children}
    </svg>
  );
}

function HomeMark() {
  return <span aria-hidden="true" className="h-[18px] w-[18px] shrink-0 rounded-[6px] bg-[conic-gradient(from_200deg,var(--link-in),var(--accent),var(--link-out),var(--link-in))]" />;
}

function InfoIcon() {
  return <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden="true" className="shrink-0 text-[var(--dim)]"><circle cx="12" cy="12" r="9" /><path d="M12 11v5M12 8h.01" /></svg>;
}

export interface DesktopChromeProps {
  catalogue: CatalogueEntry[] | undefined;
  doc: MapDocument;
  owner: string;
  repo: string;
  layer: Layer;
  onLayer(layer: Layer): void;
  touch: boolean;
  panelTabLabel: string;
  onOpenSearch(): void;
  searchOpen: boolean;
  onCloseSearch(): void;
  onSearchPick(pick: SearchPick): void;
  pathMode: boolean;
  panelOpen: boolean;
  onPanelOpen(open: boolean): void;
  onOpenQuality(): void;
  keyboardOpen: boolean;
  onKeyboardOpen(open: boolean): void;
  onZoomIn(): void;
  onZoomOut(): void;
  onFit(): void;
  isFullscreen: boolean;
  onToggleFullscreen(): void;
}

/** Full-bleed desktop/tablet command bar, actions, dialogs and map controls. */
export function DesktopChrome(p: DesktopChromeProps) {
  const navigate = useNavigate();
  const [menuOpen, setMenuOpen] = useState(false);
  const repoButtonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuRows = useMemo(() => repoRows(p.catalogue, p.doc, p.owner, p.repo), [p.catalogue, p.doc, p.owner, p.repo]);
  const [themeChoice, setThemeChoice] = useThemeChoice();
  const commands = useMemo<SearchCommandDefinition[]>(() => [
    ...LAYERS.map(({ id, label, key }) => ({ id: `layer:${id}`, label: `Switch to ${label} layer`, detail: `Layer ${key}` })),
    { id: "fit", label: "Fit map", detail: "Fit the map to the safe rectangle" },
    { id: "quality", label: "Open map quality", detail: "Files without links" },
    { id: "theme", label: "Cycle theme", detail: `Theme · ${themeChoice} → ${nextThemeChoice(themeChoice)}` },
    { id: "panel", label: `${p.panelOpen ? "Hide" : "Show"} panel`, detail: "Toggle the overview panel · [" },
    ...menuRows.map((row) => ({ id: `repo:${row.slug}`, label: `Switch to ${row.slug}`, detail: `Repository${row.current ? " · current" : ""}` })),
  ], [menuRows, p.panelOpen, themeChoice]);
  const searchButtonRef = useRef<HTMLButtonElement>(null);
  const searchPopoverRef = useRef<HTMLDivElement>(null);
  const searchReturnRef = useRef<HTMLElement | null>(null);
  const restoreSearchFocusRef = useRef(true);
  const keyboardButtonRef = useRef<HTMLButtonElement>(null);
  const hadSearchRef = useRef(false);
  const hadKeyboardRef = useRef(false);
  const restoreKeyboardFocusRef = useRef(true);

  useEffect(() => {
    if (hadSearchRef.current && !p.searchOpen) {
      const opener = searchReturnRef.current;
      searchReturnRef.current = null;
      if (restoreSearchFocusRef.current) {
        if (opener?.isConnected) opener.focus({ preventScroll: true });
        else searchButtonRef.current?.focus({ preventScroll: true });
      }
      restoreSearchFocusRef.current = true;
    }
    hadSearchRef.current = p.searchOpen;
  }, [p.searchOpen]);
  useEffect(() => {
    if (hadKeyboardRef.current && !p.keyboardOpen && restoreKeyboardFocusRef.current) keyboardButtonRef.current?.focus({ preventScroll: true });
    if (hadKeyboardRef.current && !p.keyboardOpen) restoreKeyboardFocusRef.current = true;
    hadKeyboardRef.current = p.keyboardOpen;
  }, [p.keyboardOpen]);

  useEffect(() => {
    if (!menuOpen) return;
    const first = menuRef.current?.querySelector<HTMLButtonElement>("[role=menuitem]");
    first?.focus();
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!menuRef.current?.contains(target) && !repoButtonRef.current?.contains(target)) setMenuOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    return () => document.removeEventListener("pointerdown", onPointerDown, true);
  }, [menuOpen]);

  useEffect(() => {
    if (!menuOpen) return;
    const onEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setMenuOpen(false);
      repoButtonRef.current?.focus({ preventScroll: true });
    };
    window.addEventListener("keydown", onEscape, true);
    return () => window.removeEventListener("keydown", onEscape, true);
  }, [menuOpen]);

  useEffect(() => {
    if (!p.searchOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node;
      if (searchPopoverRef.current?.contains(target) || searchButtonRef.current?.contains(target)) return;
      restoreSearchFocusRef.current = false;
      searchReturnRef.current = null;
      p.onCloseSearch();
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    return () => document.removeEventListener("pointerdown", onPointerDown, true);
  }, [p.searchOpen, p.onCloseSearch]);

  useEffect(() => {
    function onKeyDown(event: globalThis.KeyboardEvent) {
      const target = event.target instanceof HTMLElement ? event.target : null;
      if (event.defaultPrevented) return;
      const focus: DesktopFocusScope = target?.closest("[data-desktop-search], [data-search-palette]")
        ? "palette"
        : target?.closest('[role="menu"]')
          ? "menu"
          : target?.closest('[role="dialog"]')
            ? "dialog"
            : target?.isContentEditable || target?.closest("input, textarea, select, [contenteditable]")
              ? "text"
              : "page";
      const action = dispatchDesktopKey({ key: event.key, focus, metaKey: event.metaKey, ctrlKey: event.ctrlKey, altKey: event.altKey, shiftKey: event.shiftKey });
      if (action.type !== "search") return;
      event.preventDefault();
      if (p.searchOpen) {
        restoreSearchFocusRef.current = true;
        p.onCloseSearch();
        return;
      }
      searchReturnRef.current = target && target !== document.body ? target : searchButtonRef.current;
      restoreSearchFocusRef.current = true;
      setMenuOpen(false);
      if (p.keyboardOpen) {
        restoreKeyboardFocusRef.current = false;
        p.onKeyboardOpen(false);
      }
      p.onOpenSearch();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [p.keyboardOpen, p.onCloseSearch, p.onKeyboardOpen, p.onOpenSearch, p.searchOpen]);

  function openSearchFromButton() {
    if (p.searchOpen) {
      searchReturnRef.current = searchButtonRef.current;
      restoreSearchFocusRef.current = true;
      p.onCloseSearch();
      return;
    }
    searchReturnRef.current = searchButtonRef.current;
    restoreSearchFocusRef.current = true;
    setMenuOpen(false);
    p.onOpenSearch();
  }

  function closeSearch(restoreFocus = true) {
    restoreSearchFocusRef.current = restoreFocus;
    p.onCloseSearch();
  }

  function runCommand(id: string) {
    if (id.startsWith("layer:")) {
      p.onLayer(id.slice("layer:".length) as Layer);
      return;
    }
    if (id.startsWith("repo:")) {
      const row = menuRows.find((candidate) => candidate.slug === id.slice("repo:".length));
      if (row) chooseRepository(row);
      return;
    }
    if (id === "fit") p.onFit();
    else if (id === "quality") p.onOpenQuality();
    else if (id === "theme") setThemeChoice(nextThemeChoice(themeChoice));
    else if (id === "panel") p.onPanelOpen(!p.panelOpen);
  }

  function menuKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      setMenuOpen(false);
      repoButtonRef.current?.focus({ preventScroll: true });
      return;
    }
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const items = Array.from(menuRef.current?.querySelectorAll<HTMLElement>("[role=menuitem]") ?? []);
    const index = items.indexOf(document.activeElement as HTMLElement);
    const next = index < 0
      ? (event.key === "ArrowDown" ? 0 : items.length - 1)
      : Math.max(0, Math.min(items.length - 1, index + (event.key === "ArrowDown" ? 1 : -1)));
    items[next]?.focus();
  }

  function chooseRepository(row: RepoRow) {
    setMenuOpen(false);
    if (row.current) {
      repoButtonRef.current?.focus({ preventScroll: true });
      return;
    }
    const [nextOwner, nextRepo] = row.slug.split("/");
    navigate({ to: "/$owner/$repo", params: { owner: nextOwner, repo: nextRepo }, search: { geo: "r", layer: "d" } });
  }

  const countLabel = (row: RepoRow) => {
    const values = [
      row.files == null ? null : `${row.files.toLocaleString("en-US")} files`,
      row.districts == null ? null : `${row.districts.toLocaleString("en-US")} districts`,
      row.lang == null ? null : displayLanguage(row.lang),
    ].filter((part): part is string => part != null);
    return values.join(" · ");
  };

  return (
    <>
      <div
        className="glass absolute z-40 flex items-center rounded-[14px] px-1"
        data-command-bar
        style={{
          left: `calc(${DESKTOP_GUTTER_PX}px + env(safe-area-inset-left, 0px))`,
          top: `calc(${COMMAND_BAR_TOP_PX}px + env(safe-area-inset-top, 0px))`,
          height: COMMAND_BAR_HEIGHT_PX,
        }}
      >
        <Link to="/" data-wordmark aria-label="tolmap home" className={`flex items-center gap-2 rounded-[10px] pl-2 pr-2.5 font-bold text-[16px] tracking-[-.01em] hover:bg-[var(--chrome-hover)] ${p.touch ? "h-11" : "h-9"}`}>
          <HomeMark />
          tolmap
        </Link>
        <span aria-hidden="true" className="mx-1 h-5 w-px bg-[var(--rule)]" />
        <button
          ref={repoButtonRef}
          type="button"
          data-repo-switcher
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          aria-label={`Repository ${p.owner}/${p.repo}`}
          onClick={() => setMenuOpen((open) => !open)}
          className={`flex max-w-[min(260px,calc(100vw-488px))] items-center gap-1.5 rounded-[10px] pl-2.5 pr-2 font-mono text-small hover:bg-[var(--chrome-hover)] ${p.touch ? "h-11" : "h-9"}`}
        >
          <span className="truncate">{p.owner}/{p.repo}</span>
          <ChevronDownIcon />
        </button>
        {menuOpen && (
          <div
            ref={menuRef}
            role="menu"
            aria-label="Mapped repositories"
            data-repository-menu
            onKeyDown={menuKeyDown}
            className="absolute left-[70px] top-[54px] z-[61] flex max-h-[min(520px,calc(100vh-110px))] min-w-[300px] flex-col overflow-hidden rounded-[14px] border border-[var(--chrome-glass-border)] bg-[var(--chrome-solid)] p-1.5 text-[var(--on)] shadow-[var(--chrome-shadow)]"
          >
            <div data-repository-list className="min-h-0 flex-auto overflow-y-auto">
              <div className="px-2.5 pb-1 pt-1.5 text-label font-semibold uppercase tracking-[.06em] text-[var(--dim)]">Mapped repositories</div>
              {menuRows.map((row) => (
                <button
                  type="button"
                  role="menuitem"
                  key={row.slug}
                  aria-current={row.current ? "true" : undefined}
                  data-repository-row={row.slug}
                  onClick={() => chooseRepository(row)}
                  className="flex min-h-[48px] w-full items-center gap-2.5 rounded-[9px] px-2.5 py-1 text-left hover:bg-[var(--chrome-hover)] focus-visible:bg-[var(--chrome-hover)]"
                >
                  <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                    <span className="truncate font-mono text-small">{row.slug}</span>
                    {countLabel(row) && <span className="truncate text-meta text-[var(--dim)]">{countLabel(row)}</span>}
                  </span>
                  {row.current && <MonoIcon><path d="M5 12l5 5 9-10" /></MonoIcon>}
                </button>
              ))}
            </div>
            <div data-repository-footer className="shrink-0 border-t border-[var(--rule)] pt-1">
              <Link to="/" hash="repo-field" replace data-repo-map-another role="menuitem" onClick={() => setMenuOpen(false)} className="flex min-h-[44px] items-center gap-2.5 rounded-[9px] px-2.5 text-small hover:bg-[var(--chrome-hover)]">
                <PlusIcon size={16} />
                <span>Map another repository</span>
              </Link>
              <Link to="/" data-repo-home role="menuitem" onClick={() => setMenuOpen(false)} className="flex min-h-[44px] items-center gap-2.5 rounded-[9px] px-2.5 text-small hover:bg-[var(--chrome-hover)]">
                <MonoIcon><path d="M4 11l8-7 8 7v9H4z" /></MonoIcon>
                <span>Home</span>
              </Link>
            </div>
          </div>
        )}
      </div>

      <div
        className="absolute z-40 flex items-center gap-2"
        data-desktop-actions
        style={{
          right: `calc(${DESKTOP_GUTTER_PX}px + env(safe-area-inset-right, 0px))`,
          top: `calc(${COMMAND_BAR_TOP_PX}px + env(safe-area-inset-top, 0px))`,
        }}
      >
        <div role="group" aria-label="Map layer" data-layer-segmented className="glass flex h-11 items-center gap-0.5 rounded-[14px] p-1">
          {LAYERS.map(({ id, label, key, icon }) => {
            const active = p.layer === id;
            return (
              <button
                key={id}
                type="button"
                data-layer={id}
                aria-label={`${label} layer`}
                aria-pressed={active}
                title={`${label}  ${key}`}
                onClick={() => p.onLayer(id)}
                className={`flex items-center justify-center gap-[7px] rounded-[10px] px-3 text-small ${p.touch ? "h-11 min-w-11" : "h-[34px]"} ${active ? "bg-[var(--chrome2)] text-[var(--on)] shadow-[var(--chrome-segment-shadow)]" : "text-[var(--dim)] hover:text-[var(--on)]"}`}
              >
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke={active ? "var(--accent)" : "currentColor"} strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{icon}</svg>
                <span className="hidden min-[1180px]:inline" data-layer-label>{label}</span>
              </button>
            );
          })}
        </div>
        <div className="glass flex items-center gap-0.5 rounded-[14px] p-1" data-action-buttons>
          <button ref={searchButtonRef} type="button" data-open-desktop-search aria-haspopup="dialog" aria-expanded={p.searchOpen} aria-label="Search districts, files and symbols" title={`Search  ${typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent) ? "⌘K" : "Ctrl K"}`} onClick={openSearchFromButton} className={`flex ${p.touch ? "h-11 w-11" : "h-9 w-9"} items-center justify-center rounded-[10px] text-[var(--dim)] hover:bg-[var(--chrome-hover)] hover:text-[var(--on)]`}>
            <SearchIcon size={18} />
          </button>
          <ThemeToggle iconSize={18} className={`border-0 bg-transparent text-[var(--dim)] hover:bg-[var(--chrome-hover)] hover:text-[var(--on)] ${p.touch ? "h-11 w-11 rounded-[10px]" : "h-9 w-9 rounded-[10px]"}`} />
          <button ref={keyboardButtonRef} type="button" data-open-keyboard aria-label="Keyboard shortcuts" title="Keyboard shortcuts" aria-haspopup="dialog" aria-expanded={p.keyboardOpen} onClick={() => p.onKeyboardOpen(!p.keyboardOpen)} className={`flex ${p.touch ? "h-11 w-11" : "h-9 w-9"} items-center justify-center rounded-[10px] text-[var(--dim)] hover:bg-[var(--chrome-hover)] hover:text-[var(--on)]`}>
            <MonoIcon size={18}><rect x="3" y="6" width="18" height="12" rx="2" /><path d="M7 10h.01M11 10h.01M15 10h.01M7 14h10" /></MonoIcon>
          </button>
        </div>
      </div>

      {menuOpen && <button type="button" aria-label="Close repository menu" tabIndex={-1} onClick={() => setMenuOpen(false)} className="absolute inset-0 z-[35] cursor-default bg-transparent" />}

      {p.searchOpen && (
        <div ref={searchPopoverRef} data-desktop-search-shell className="absolute inset-0 z-[70]">
          <SearchBox
            doc={p.doc}
            onPick={p.onSearchPick}
            onCommand={runCommand}
            commands={commands}
            pathMode={p.pathMode}
            variant="palette"
            autoFocus
            onClose={() => closeSearch()}
            touch={p.touch}
          />
        </div>
      )}
      {p.keyboardOpen && (
        <div className="absolute inset-0 z-[70]" data-keyboard-dialog-layer>
          <button type="button" aria-label="Close keyboard shortcuts" onClick={() => p.onKeyboardOpen(false)} className="absolute inset-0 h-full w-full cursor-default bg-[var(--chrome-scrim)]" />
          <KeyboardDialog touch={p.touch} onClose={() => p.onKeyboardOpen(false)} />
        </div>
      )}

      <ZoomControls
        onZoomIn={p.onZoomIn}
        onZoomOut={p.onZoomOut}
        onFit={p.onFit}
        isFullscreen={p.isFullscreen}
        onToggleFullscreen={p.onToggleFullscreen}
        size={desktopControlSize(p.touch ? "tablet" : "desktop")}
      />
    </>
  );
}

function KeyboardDialog({ onClose, touch }: { onClose(): void; touch: boolean }) {
  const rootRef = useRef<HTMLElement>(null);
  const mac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
  useEffect(() => { rootRef.current?.focus(); }, []);
  return (
    <section
      ref={rootRef}
      role="dialog"
      aria-label="Keyboard shortcuts"
      aria-modal="true"
      data-keyboard-dialog
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); onClose(); }
      }}
      className="absolute left-1/2 top-1/2 z-[71] w-[min(560px,calc(100%-32px))] -translate-x-1/2 -translate-y-1/2 rounded-[16px] border border-[var(--chrome-glass-border)] bg-[var(--chrome-solid)] p-5 text-[var(--on)] shadow-[var(--chrome-shadow)]"
    >
      <div className="mb-3 flex items-center">
        <h2 className="mr-auto text-[17px] font-semibold">Keyboard</h2>
        <button type="button" aria-label="Close keyboard shortcuts" onClick={onClose} className={`flex items-center justify-center rounded-[10px] hover:bg-[var(--chrome-hover)] ${touch ? "h-11 w-11" : "h-9 w-9"}`}>
          <MonoIcon><path d="M6 6l12 12M18 6L6 18" /></MonoIcon>
        </button>
      </div>
      <div className="grid grid-cols-2 gap-x-7">
        <Shortcut label="Search" keys={mac ? ["⌘K", "/"] : ["Ctrl K", "/"]} />
        <Shortcut label="Move through search results" keys={["↑", "↓"]} />
        <Shortcut label="Open highlighted result or index row" keys={["Enter"]} />
        <Shortcut label="Move through the overview index" keys={["↑", "↓"]} />
        <Shortcut label="Hide or show the panel" keys={["["]} />
        <Shortcut label="District, Churn, Complexity, Package" keys={["1", "2", "3", "4"]} />
        <Shortcut label="Zoom about the safe rectangle centre" keys={["+", "−"]} />
        <Shortcut label="Fit the map" keys={["F"]} />
        <Shortcut label="Zoom to the selection" keys={["Z"]} />
        <Shortcut label="Cycle the theme" keys={["T"]} />
        <Shortcut label="Open this keyboard list" keys={["?"]} />
        <Shortcut label="Close a menu or dialog; step back a card" keys={["Esc"]} />
      </div>
    </section>
  );
}

function Shortcut({ label, keys }: { label: string; keys: string[] }) {
  return (
    <div className="flex min-h-9 items-center justify-between gap-2 border-b border-[var(--rule)] py-1.5 text-small">
      <span>{label}</span>
      <span className="flex shrink-0 gap-1">{keys.map((key) => <kbd key={key} className="rounded-[5px] border border-[var(--chrome-glass-border)] bg-[var(--chrome-key)] px-1.5 font-mono text-label text-[var(--dim)]">{key}</kbd>)}</span>
    </div>
  );
}

export interface DesktopPanelProps {
  doc: MapDocument;
  repoSlug: string;
  packageLayout: PackageLayout;
  packageGrouping: PackageGrouping;
  touch: boolean;
  panelOpen: boolean;
  panelTabLabel: string;
  views: readonly DesktopPanelView[];
  layer: Layer;
  indexTab: "districts" | "folders";
  activeDirectory?: string;
  onIndexTab(tab: "districts" | "folders"): void;
  onPanelOpen(open: boolean): void;
  onBack(): void;
  onJump(index: number): void;
  onOpenQuality(): void;
  onSelectDistrict(district: number): void;
  onHighlightDistricts(districts: readonly number[] | null): void;
  onFrameDistricts(districts: readonly number[]): void;
  onSelectDirectory(path?: string): void;
  onDepth(depth: number): void;
  packageAuto: boolean;
  cardProps: ComponentProps<typeof SelectionCard>;
}

export function DesktopPanel(p: DesktopPanelProps) {
  const panelRef = useRef<HTMLElement>(null);
  const [districtFoldersExpanded, setDistrictFoldersExpanded] = useState(false);
  const [districtFilesExpanded, setDistrictFilesExpanded] = useState(false);
  const crumbs = desktopPanelCrumbs(p.views, p.doc, p.repoSlug);
  const top = p.views[p.views.length - 1] ?? { type: "overview" as const };
  const layerOverview = useLayerOverview(p.doc, p.packageGrouping);
  const onHighlightDistrictsRef = useRef(p.onHighlightDistricts);
  onHighlightDistrictsRef.current = p.onHighlightDistricts;
  const indexRows = useMemo(() => {
    const ids = districtIndexDistrictIds(p.doc).sort((a, b) =>
      p.doc.districts[String(b)].size - p.doc.districts[String(a)].size || a - b,
    );
    return ids.map((id) => buildDistrictIndexRow(p.doc, p.packageLayout, id));
  }, [p.doc, p.packageLayout]);
  const filesByDistrict = useMemo(() => {
    const files = new Map<number, number[]>();
    p.doc.N.forEach((row, index) => {
      const ids = files.get(row[0]) ?? [];
      ids.push(index);
      files.set(row[0], ids);
    });
    return files;
  }, [p.doc]);
  const [maxCh, maxCx] = useMemo(() => [
    Math.max(1, ...p.doc.N.map((row) => row[5])),
    Math.max(1, ...p.doc.N.map((row) => row[4])),
  ], [p.doc]);

  useEffect(() => {
    const panel = panelRef.current;
    if (!panel) return;
    if (p.panelOpen) panel.removeAttribute("inert");
    else panel.setAttribute("inert", "");
  }, [p.panelOpen]);

  useEffect(() => {
    setDistrictFoldersExpanded(false);
    setDistrictFilesExpanded(false);
  }, [p.doc]);

  useEffect(() => {
    onHighlightDistrictsRef.current(null);
  }, [p.layer, p.doc]);

  const colorForDistrict = (district: number): string => {
    const members = filesByDistrict.get(district) ?? [];
    if (p.layer === "d") return districtColor(p.doc, district);
    if (p.layer === "c" || p.layer === "x") {
      const values = members.map((index) => p.layer === "c" ? CH(p.doc, index) : CX_(p.doc, index)).sort((a, b) => a - b);
      if (values.length === 0) return "var(--dim)";
      // The median is a value in the map document, matching the one colour
      // used to summarize this district's current file layer in the row.
      const middle = values[Math.floor((values.length - 1) / 2)];
      return ramp(middle / (p.layer === "c" ? maxCh : maxCx));
    }
    const colors = new Map<string, number>();
    for (const index of members) {
      const color = p.packageGrouping.fileColors[index];
      colors.set(color, (colors.get(color) ?? 0) + 1);
    }
    return [...colors.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))[0]?.[0] ?? "var(--dim)";
  };

  const rowColor = (district: number) => colorForDistrict(district);
  const count = p.packageLayout.unconnectedFiles.length;

  return (
    <>
      <section
        ref={panelRef}
        data-desktop-panel
        aria-label="Map details"
        aria-hidden={!p.panelOpen}
        className={`glass desktop-panel absolute z-30 flex flex-col overflow-hidden rounded-[16px] text-[var(--on)] ${p.panelOpen ? "translate-x-0 opacity-100" : "-translate-x-[calc(100%+24px)] opacity-0 pointer-events-none"}`}
        style={{
          left: `calc(${DESKTOP_PANEL_LEFT_PX}px + env(safe-area-inset-left, 0px))`,
          top: `calc(${DESKTOP_PANEL_TOP_PX}px + env(safe-area-inset-top, 0px))`,
          width: DESKTOP_PANEL_WIDTH_PX,
          maxWidth: `calc(100% - ${DESKTOP_PANEL_LEFT_PX * 2}px - env(safe-area-inset-left, 0px) - env(safe-area-inset-right, 0px))`,
          maxHeight: `calc(100% - ${DESKTOP_PANEL_TOP_PX + DESKTOP_PANEL_BOTTOM_PX}px - env(safe-area-inset-top, 0px) - env(safe-area-inset-bottom, 0px))`,
        }}
      >
        <div className="flex min-h-11 items-center gap-1 pl-4 pr-2.5 pt-2.5" data-panel-head>
          {p.views.length > 1 && (
            <button type="button" data-panel-back aria-label="Back one card" title="Back  Esc" onClick={p.onBack} className="flex h-9 w-9 shrink-0 items-center justify-center rounded-[10px] text-[var(--dim)] hover:bg-[var(--chrome-hover)] hover:text-[var(--on)] touch:h-11 touch:w-11">
              <BackIcon size={16} />
            </button>
          )}
          <nav aria-label="Where you are" className="flex min-w-0 flex-1 items-center gap-0.5 overflow-hidden whitespace-nowrap" data-panel-breadcrumbs>
            {crumbs.map((crumb, index) => {
              const current = index === crumbs.length - 1;
              const compactOverview = crumbs.length >= 3 && index === 0 && crumb.type === "overview";
              const crumbLabel = compactOverview ? `Overview · ${crumb.label}` : crumb.label;
              return (
                // The brief: the last crumb gets roughly 60% and the
                // ancestors shrink first. The wrapper stays exactly as it
                // was originally -- `shrink-0` plus `max-w-[60%]` -- which
                // is what keeps it out of nav's shared shrink budget: giving
                // IT any flex-shrink, even a sliver weighted far behind the
                // ancestors', still took a nonzero (if tiny) share of
                // whatever deficit nav was resolving, which is exactly what
                // cut a short crumb like "types.ts" a pixel short of its
                // own content (check-view: 76px of content in a 75px box).
                // The flex algorithm DOES clamp a `flex-shrink:0` item's own
                // hypothetical size to its max-width up front (spec 9.7
                // step 3, before any freezing), so the wrapper itself was
                // already being held to 60% correctly.
                //
                // The bug was one level down: the *button* inside was ALSO
                // shrink-0, so once its wrapper had been clamped narrower
                // than its content, the button didn't shrink to match --
                // it simply overflowed its own (uncapped, non-clipping)
                // wrapper, and that overflow was only ever caught by nav's
                // outer `overflow-hidden`, which has no ellipsis to draw.
                // Making the button (only) shrinkable keeps it fully inside
                // its wrapper's already-correct 60% box, so it can use its
                // own `truncate` there -- and since this shrink is scoped to
                // the button's own two-item inner flex context (itself and
                // the fixed-width chevron), it has no effect on nav's
                // cross-crumb distribution at all, so the fits-case is
                // untouched pixel for pixel.
                <span key={`${crumb.type}-${index}`} className={`flex min-w-0 items-center gap-0.5 ${current ? "shrink-0 max-w-[60%]" : "shrink"}`}>
                  {index > 0 && <span className="shrink-0"><ChevronIcon /></span>}
                  <button
                    type="button"
                    data-panel-crumb={index}
                    aria-current={current ? "page" : undefined}
                    aria-label={crumbLabel}
                    title={crumbLabel}
                    onClick={() => p.onJump(index)}
                    className={`min-w-0 max-w-full shrink truncate rounded-[6px] px-1.5 py-1 text-left text-meta hover:bg-[var(--chrome-hover)] touch:min-h-[44px] ${crumb.type === "overview" || crumb.type === "file" || crumb.type === "symbol" ? "font-mono" : ""} ${current ? "text-[var(--on)]" : "text-[var(--dim)]"} ${compactOverview ? "flex h-7 w-7 shrink-0 items-center justify-center px-1" : ""}`}
                  >
                    {compactOverview ? <HomeIcon size={14} /> : crumb.label}
                  </button>
                </span>
              );
            })}
          </nav>
          <button type="button" data-hide-panel aria-label="Hide panel" title="Hide panel  [" onClick={() => p.onPanelOpen(false)} className="flex h-9 w-9 shrink-0 items-center justify-center rounded-[10px] text-[var(--dim)] hover:bg-[var(--chrome-hover)] hover:text-[var(--on)] touch:h-11 touch:w-11">
            <PanelIcon size={17} />
          </button>
        </div>
        <div key={`${top.type}-${JSON.stringify(top)}`} data-panel-content data-selection-panel={top.type === "overview" ? undefined : ""} className="min-h-0 overflow-y-auto px-4 pb-4 pt-1 [overscroll-behavior:contain] panel-enter" style={{ scrollbarWidth: "thin" }}>
          {top.type === "overview" ? (
            <>
              {p.layer === "d" ? (
                <div data-desktop-overview data-overview-layer="d">
                  {/* The same sans, tabular headline as the other three layers: a mono
                      "6,347" inside the sans line read as "6, 347" (Plex Mono's
                      comma is a full cell wide). */}
                  <h2 data-overview-headline data-overview-layer="d" className={DESKTOP_HEADLINE}>
                    <HeadlineText text={layerOverviewHeadline(layerOverview, "d").primary} />
                  </h2>
                  <p className="mt-1.5 flex flex-wrap items-center gap-x-1 text-meta leading-[1.45] text-[var(--dim)]">
                    modularity <span className="font-mono">{p.doc.q.toFixed(3)}</span> ·
                    <button type="button" data-open-map-quality onClick={p.onOpenQuality} className="inline-flex items-center gap-1 rounded px-0.5 hover:text-[var(--on)]" aria-label={`${count.toLocaleString("en-US")} files without links. Open map quality`}>
                      <span data-unconnected-count>{count.toLocaleString("en-US")} files without links</span><InfoIcon />
                    </button>
                  </p>
                  <div className="mt-4 flex min-h-8 items-center justify-between gap-2">
                    <h3 className="text-small font-semibold">Index</h3>
                    <div role="tablist" aria-label="Browse by" className="flex rounded-[8px] bg-[var(--chrome-hover)] p-0.5">
                      {(["districts", "folders"] as const).map((tab) => (
                        <button key={tab} type="button" role="tab" aria-selected={p.indexTab === tab} data-index-tab={tab} onClick={() => p.onIndexTab(tab)} className={`rounded-[6px] px-2.5 text-meta ${p.touch ? "h-11" : "h-[28px]"} ${p.indexTab === tab ? "bg-[var(--chrome2)] font-semibold text-[var(--on)] shadow-[var(--chrome-segment-shadow)]" : "text-[var(--dim)]"}`}>
                          {tab === "districts" ? "Districts" : "Folders"}
                        </button>
                      ))}
                    </div>
                  </div>
                  {p.indexTab === "districts" ? (
                    <div className="-mx-2 mt-1 flex flex-col gap-px" data-district-index>
                      {indexRows.map((row) => {
                        const keyFile = row.keyFiles[0];
                        return (
                          <button key={row.d} type="button" data-district-index-row={row.d} data-panel-district={row.d} onMouseEnter={() => p.onHighlightDistricts([row.d])} onMouseLeave={() => p.onHighlightDistricts(null)} onFocus={() => p.onHighlightDistricts([row.d])} onBlur={() => p.onHighlightDistricts(null)} onClick={() => { p.onHighlightDistricts(null); p.onSelectDistrict(row.d); }} className="flex h-[50px] min-h-[50px] w-full shrink-0 items-center gap-2.5 rounded-[10px] px-2.5 py-1.5 text-left hover:bg-[var(--chrome-hover)]">
                            <i aria-hidden="true" className="h-2.5 w-2.5 shrink-0 rounded-[3px]" style={{ background: rowColor(row.d) }} />
                            <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                              <span className="truncate text-small font-semibold">{row.name}</span>
                              <span className="flex min-w-0 items-center gap-1 text-meta text-[var(--dim)]">
                                {row.mostly && <span className="min-w-0 truncate">mostly <span className="font-mono">{row.mostly}</span></span>}
                                {row.mostly && keyFile && <span className="shrink-0">·</span>}
                                {keyFile && <span data-district-index-key-file={keyFile.file} className="shrink-0 truncate font-mono">{p.doc.F[keyFile.file]?.split("/").pop()}</span>}
                              </span>
                            </span>
                            <span className="shrink-0 font-mono text-meta text-[var(--dim)]">{row.size.toLocaleString("en-US")}</span>
                          </button>
                        );
                      })}
                    </div>
                  ) : (
                    <div className="px-1" data-folder-tab>
                      <FolderBody layout={p.packageLayout} activeDirectory={p.activeDirectory} onSelectDirectory={p.onSelectDirectory} touchTargets={p.touch} />
                    </div>
                  )}
                </div>
              ) : (
                <div data-desktop-overview data-overview-layer={p.layer}>
                  <LayerOverviewHeadline overview={layerOverview} layer={p.layer} className={DESKTOP_HEADLINE} />
                  <div className="mt-4">
                    <LayerOverviewIndex
                      doc={p.doc}
                      overview={layerOverview}
                      layer={p.layer}
                      touch={p.touch}
                      onSelectDistrict={p.onSelectDistrict}
                      onSelectFile={p.cardProps.onSelectFile}
                      onHighlightDistricts={p.onHighlightDistricts}
                      onFrameDistricts={p.onFrameDistricts}
                    />
                  </div>
                </div>
              )}
            </>
          ) : (
            <SelectionCard
              {...p.cardProps}
              quality={top.type === "quality" || p.cardProps.quality}
              desktopPanel
              compactRows={!p.touch}
              touchTargets={p.touch}
              districtFoldersExpanded={districtFoldersExpanded}
              districtFilesExpanded={districtFilesExpanded}
              onToggleDistrictFolders={() => setDistrictFoldersExpanded((expanded) => !expanded)}
              onToggleDistrictFiles={() => setDistrictFilesExpanded((expanded) => !expanded)}
            />
          )}
        </div>
      </section>
      {!p.panelOpen && (
        <button
          type="button"
          data-panel-tab
          title="Show panel  ["
          aria-label={`Show panel. ${p.panelTabLabel}`}
          onClick={() => p.onPanelOpen(true)}
          className="glass absolute z-30 flex h-10 max-w-[min(330px,calc(100%-32px))] items-center gap-2 rounded-[12px] px-3 text-small font-medium text-[var(--on)] hover:border-[var(--dim)] touch:h-11"
          style={{ left: "calc(16px + env(safe-area-inset-left, 0px))", top: "calc(72px + env(safe-area-inset-top, 0px))" }}
        >
          <PanelIcon size={17} />
          <span className="truncate">{p.panelTabLabel}</span>
        </button>
      )}
      <DesktopLegend
        layer={p.layer}
        packageLayout={p.packageLayout}
        packageGrouping={p.packageGrouping}
        panelOpen={p.panelOpen}
        onDepth={p.onDepth}
        packageAuto={p.packageAuto}
      />
    </>
  );
}

function DesktopLegend({
  layer,
  packageLayout,
  packageGrouping,
  panelOpen,
  onDepth,
  packageAuto,
}: {
  layer: Layer;
  packageLayout: PackageLayout;
  packageGrouping: PackageGrouping;
  panelOpen: boolean;
  onDepth(depth: number): void;
  packageAuto: boolean;
}) {
  if (layer === "d") return null;
  const style = {
    left: panelOpen
      ? `calc(${DESKTOP_PANEL_LEFT_PX + DESKTOP_PANEL_WIDTH_PX + 16}px + env(safe-area-inset-left, 0px))`
      : `calc(${DESKTOP_GUTTER_PX}px + env(safe-area-inset-left, 0px))`,
    bottom: `calc(${DESKTOP_LEGEND_BOTTOM_PX}px + env(safe-area-inset-bottom, 0px))`,
  };
  if (layer === "p") {
    return <div className="absolute z-20" style={style} data-desktop-legend><PackageLegend grouping={packageGrouping} auto={packageAuto} minDepth={packageLayout.minDepth} maxDepth={packageLayout.maxDepth} onDepth={onDepth} /></div>;
  }
  return (
    <div className="glass absolute z-20 w-[220px] rounded-[12px] px-3 py-2 text-meta text-[var(--dim)]" style={style} data-desktop-legend data-ramp-legend={layer}>
      <div className="font-semibold text-[var(--on)]">{layer === "c" ? "Commits per file" : "Complexity per file"}</div>
      <div className="my-1.5 h-1.5 rounded-full" style={{ background: "linear-gradient(90deg,var(--chrome-ramp-low),var(--chrome-ramp-mid),var(--chrome-ramp-high))" }} />
      <div className="flex justify-between font-mono"><span>low</span><span>high</span></div>
    </div>
  );
}

interface ZoomControlsProps {
  onZoomIn(): void;
  onZoomOut(): void;
  onFit(): void;
  isFullscreen: boolean;
  onToggleFullscreen(): void;
  size: number;
}

export function ZoomControls({ onZoomIn, onZoomOut, onFit, isFullscreen, onToggleFullscreen, size }: ZoomControlsProps) {
  const button = "flex items-center justify-center border-b border-[var(--chrome-glass-border)] text-[var(--dim)] last:border-b-0 hover:bg-[var(--chrome-hover)] hover:text-[var(--on)]";
  return (
    <div data-map-controls className="glass absolute z-20 flex flex-col overflow-hidden rounded-[12px]" style={{ right: `calc(${DESKTOP_GUTTER_PX}px + env(safe-area-inset-right, 0px))`, bottom: `calc(${DESKTOP_CONTROL_BOTTOM_PX}px + env(safe-area-inset-bottom, 0px))` }}>
      <button type="button" className={button} style={{ width: size, height: size }} onClick={onZoomIn} aria-label="Zoom in"><PlusIcon size={18} /></button>
      <button type="button" className={button} style={{ width: size, height: size }} onClick={onZoomOut} aria-label="Zoom out"><MinusIcon size={18} /></button>
      <button type="button" className={button} style={{ width: size, height: size }} onClick={onFit} aria-label="Fit map"><FitIcon size={18} /></button>
      <button type="button" className={button} style={{ width: size, height: size }} onClick={onToggleFullscreen} aria-label={isFullscreen ? "Exit fullscreen" : "Enter fullscreen"} aria-pressed={isFullscreen}><FullscreenIcon size={18} /></button>
    </div>
  );
}
