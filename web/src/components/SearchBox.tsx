import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import type { MapDocument } from "@/types";
import {
  groupResults,
  isNavKey,
  navKey,
  paletteResults,
  segments,
  type Mark,
  type PaletteItem,
  type PaletteResults,
  type SearchCommandDefinition,
  type SearchPick,
  type SearchResults,
} from "@/map/searchResults";
import { BackIcon, ClearIcon, DistrictIcon, FileIcon, SearchIcon, SymbolIcon } from "@/components/phone/icons";

interface SearchBoxProps {
  doc: MapDocument;
  onPick(pick: SearchPick): void;
  /** "overlay": the phone's full-screen search layer (docs/UX.md §4.8).
   * "float" (default): the desktop and tablet box with its dropdown (§5,
   * §7.2) -- in the top bar, or floating over the map in fullscreen. It
   * fills the width its container gives it. */
  variant?: "float" | "overlay" | "palette";
  /** The close action for an overlay, or the parent dialog for a floating
   * desktop search opened from a separate icon. */
  onClose?(): void;
  /** Float dialogs may open from an icon or keyboard shortcut rather than
   * from the input itself. */
  autoFocus?: boolean;
  /** Palette only: Commands share the same result list, cursor and matcher. */
  commands?: readonly SearchCommandDefinition[];
  onCommand?(id: string): void;
  /** Palette only: the path card's next endpoint accepts files only. */
  pathMode?: boolean;
  /** Palette path mode: which end the picked file becomes. "Path to here"
   * waits for a start, "Path from here" for a destination; the field's tag
   * names the one being chosen. */
  pathEnd?: "start" | "destination";
  /** Float only: 44 px (a tablet is touch, §9) instead of 36. */
  touch?: boolean;
}

/** What the input is called, for people and for check:view: it searches
 * districts as well as files and symbols. */
export const SEARCH_LABEL = "Search districts, files and symbols";
const PLACEHOLDER = "Districts, files, classes…";
/** The palette's field also answers commands (§5.1), so it says so. */
const PALETTE_PLACEHOLDER = "Search districts, files, classes, or type a command";

/** docs/UX.md §4.8 and §7.2: one search, two containers. The input is an
 * ARIA combobox driving a listbox through aria-activedescendant, so focus
 * stays in the input while the arrow keys move through the results; the
 * results are grouped District / Files / Symbols (map/searchResults.ts,
 * whose file and symbol ranking is map/search.ts's, unchanged). */
export function SearchBox({
  doc,
  onPick,
  variant = "float",
  onClose,
  autoFocus = false,
  touch = false,
  commands = [],
  onCommand,
  pathMode = false,
  pathEnd = "destination",
}: SearchBoxProps) {
  const [value, setValue] = useState("");
  const [cursor, setCursor] = useState(-1);
  const [open, setOpen] = useState(false);
  const results = useMemo(
    () => variant === "palette" ? paletteResults(doc, value, commands, pathMode) : groupResults(doc, value),
    [doc, value, variant, commands, pathMode],
  );
  const inputRef = useRef<HTMLInputElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  /** Desktop: the element that had focus when `/` opened search. */
  const openerRef = useRef<Element | null>(null);
  const listId = useId();
  const optionId = (n: number) => `${listId}-o${n}`;
  const overlay = variant === "overlay";
  const palette = variant === "palette";
  const expanded = overlay || palette || open;

  useEffect(() => {
    if (!autoFocus) return;
    inputRef.current?.focus();
    setOpen(true);
  }, [autoFocus]);

  // The highlighted row follows the keyboard into view.
  useEffect(() => {
    if (cursor < 0) return;
    document.getElementById(optionId(cursor))?.scrollIntoView({ block: "nearest" });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cursor]);

  /** §7.2: focus goes back to the element that opened search. Opened by a
   * click in the box, there is no other opener, and the box lets go of
   * focus instead of reopening on it. */
  function returnFocus() {
    const opener = openerRef.current;
    openerRef.current = null;
    if (opener instanceof HTMLElement && opener !== inputRef.current && opener !== document.body && opener.isConnected) {
      opener.focus({ preventScroll: true });
    } else {
      inputRef.current?.blur();
    }
  }

  function close() {
    setCursor(-1);
    if (overlay || palette) {
      onClose?.();
      return;
    }
    setOpen(false);
    // A box opened by a click keeps focus on Esc (the combobox pattern);
    // one opened by `/` hands it back.
    if (openerRef.current) returnFocus();
    onClose?.();
  }

  function pick(item: PaletteItem) {
    setValue("");
    setCursor(-1);
    if ("commandId" in item) {
      if (!palette) return;
      onCommand?.(item.commandId);
      onClose?.();
      return;
    }
    if (palette) {
      onPick(item.pick);
      onClose?.();
      return;
    }
    if (!overlay) {
      setOpen(false);
      returnFocus();
    }
    onPick(item.pick);
  }

  function onKeyDown(e: KeyboardEvent<HTMLInputElement>) {
    if (!isNavKey(e.key)) return;
    // Desktop, dropdown closed (after Esc): an arrow reopens it.
    if (!expanded && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      e.preventDefault();
      setOpen(true);
      return;
    }
    if (!expanded && e.key === "Escape") return;
    const r = navKey(cursor, results.flat.length, e.key);
    if (r.type === "none") return;
    e.preventDefault();
    if (r.type === "move") setCursor(r.cursor);
    else if (r.type === "pick") pick(results.flat[r.index]);
    else close();
  }

  // Desktop, §7.2: `/` focuses search from anywhere that isn't a text field.
  useEffect(() => {
    if (variant !== "float") return;
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "/" || e.ctrlKey || e.metaKey || e.altKey || e.defaultPrevented) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.isContentEditable || t.closest("input, textarea, select, [contenteditable]"))) return;
      e.preventDefault();
      openerRef.current = document.activeElement;
      inputRef.current?.focus();
      setOpen(true);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [variant]);

  // Desktop, §7.2: a pointerdown anywhere outside the box closes the
  // dropdown (audit defect 8: it never closed on an outside tap).
  useEffect(() => {
    if (variant !== "float" || !open) return;
    const onDown = (e: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
        setCursor(-1);
        openerRef.current = null;
      }
    };
    document.addEventListener("pointerdown", onDown, true);
    return () => document.removeEventListener("pointerdown", onDown, true);
  }, [variant, open]);

  const comboProps = {
    ref: inputRef,
    role: "combobox",
    "aria-label": pathMode ? `Search files for path ${pathEnd}` : SEARCH_LABEL,
    "aria-expanded": expanded,
    "aria-controls": listId,
    "aria-autocomplete": "list" as const,
    "aria-activedescendant": expanded && cursor >= 0 && cursor < results.flat.length ? optionId(cursor) : undefined,
    value,
    placeholder: PLACEHOLDER,
    autoComplete: "off",
    autoCapitalize: "off",
    autoCorrect: "off",
    spellCheck: false,
    enterKeyHint: "search" as const,
    "data-search-input": "",
    onChange: (e: { target: { value: string } }) => {
      setValue(e.target.value);
      setCursor(-1);
      if (!overlay) setOpen(true);
    },
    onKeyDown,
  };

  const list = (
    <ResultList
      id={listId}
      results={results}
      value={value}
      cursor={cursor}
      optionId={optionId}
      variant={variant}
      onPick={pick}
      onHover={overlay ? undefined : setCursor}
    />
  );

  if (palette) {
    return (
      <div className="absolute inset-0 z-[70] flex items-start justify-center bg-[var(--chrome-scrim)] px-4 pt-[14vh]" data-desktop-search>
        <button type="button" aria-label="Close search" tabIndex={-1} onClick={() => close()} className="absolute inset-0 h-full w-full cursor-default" />
        <section
          role="dialog"
          aria-modal="true"
          aria-label="Search and commands"
          data-search-palette
          className="relative z-[1] flex w-[min(640px,100%)] flex-col overflow-hidden rounded-[16px] border border-[var(--chrome-glass-border)] bg-[var(--chrome-solid)] text-[var(--on)] shadow-[var(--chrome-shadow)]"
          style={{ maxHeight: "min(70vh, calc(100vh - 32px))" }}
          onKeyDown={(event) => {
            if (event.defaultPrevented) return;
            if (event.key === "Escape") {
              event.preventDefault();
              event.stopPropagation();
              close();
              return;
            }
            if (event.key === "Tab") {
              const focusable = Array.from(event.currentTarget.querySelectorAll<HTMLElement>("input:not([disabled]), button:not([disabled]):not([tabindex='-1'])"));
              const first = focusable[0];
              const last = focusable[focusable.length - 1];
              if (event.shiftKey && document.activeElement === first) {
                event.preventDefault();
                last?.focus();
              } else if (!event.shiftKey && document.activeElement === last) {
                event.preventDefault();
                first?.focus();
              }
            }
          }}
        >
          {/* docs/UX.md §5.1 and the approved prototype: the field IS the
              header -- magnifier, input, the path tag, Esc -- so the dialog
              carries no second title or close button over it. The scrim and
              Esc close it; the footer names the keys. */}
          <label className="flex h-14 shrink-0 items-center gap-2.5 border-b border-[var(--chrome-glass-border)] px-4 text-[var(--dim)]">
            <SearchIcon size={18} />
            {pathMode && (
              <span
                data-path-destination-tag
                className="shrink-0 whitespace-nowrap rounded-[6px] px-2 py-[3px] text-meta text-[var(--accent)]"
                style={{ background: "color-mix(in srgb, var(--accent) 12%, transparent)" }}
              >
                {pathEnd === "start" ? "Path start" : "Path destination"}
              </span>
            )}
            <input
              {...comboProps}
              autoFocus
              type="text"
              className="h-10 min-w-0 flex-1 bg-transparent text-[16px] text-[var(--on)] outline-none placeholder:text-[var(--dim)]"
              placeholder={pathMode ? (pathEnd === "start" ? "Search files for the start" : "Search files for the destination") : PALETTE_PLACEHOLDER}
            />
            {value && (
              <button
                type="button"
                aria-label="Clear search"
                data-search-clear
                onClick={() => { setValue(""); setCursor(-1); inputRef.current?.focus(); }}
                className="flex h-8 w-8 shrink-0 items-center justify-center rounded-[8px] hover:bg-[var(--chrome-hover)] hover:text-[var(--on)]"
              >
                <ClearIcon />
              </button>
            )}
            <Kbd>esc</Kbd>
          </label>
          <div data-search-results className="min-h-0 flex-1 overflow-y-auto p-1.5" style={{ overscrollBehavior: "contain", scrollbarWidth: "thin" }}>
            {list}
          </div>
          <div aria-hidden="true" className="flex shrink-0 items-center gap-4 border-t border-[var(--chrome-glass-border)] px-4 py-2.5 text-meta text-[var(--dim)]">
            <span className="flex items-center gap-1.5"><Kbd>↑</Kbd><Kbd>↓</Kbd> move</span>
            <span className="flex items-center gap-1.5"><Kbd>↵</Kbd> open</span>
            <span className="flex items-center gap-1.5"><Kbd>esc</Kbd> close</span>
          </div>
        </section>
      </div>
    );
  }

  if (overlay) {
    return (
      <div className="flex h-full min-h-0 flex-col">
        <div
          className="flex shrink-0 items-center gap-1.5 border-b border-[var(--rule)] px-3 pb-2.5"
          style={{ paddingTop: "calc(12px + env(safe-area-inset-top, 0px))" }}
        >
          <button
            type="button"
            aria-label="Close search"
            onClick={() => close()}
            className="flex h-11 w-11 shrink-0 items-center justify-center rounded-full text-[var(--on)] outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
          >
            <BackIcon />
          </button>
          <div className="flex h-12 min-w-0 flex-1 items-center gap-1 rounded-[24px] border-2 border-[var(--accent)] bg-[var(--chrome2)] pl-3.5 pr-0.5">
            {/* 16 px: anything smaller makes iPhone Safari zoom on focus (§4.8). */}
            <input
              {...comboProps}
              autoFocus
              type="text"
              className="h-10 min-w-0 flex-1 bg-transparent text-body text-[var(--on)] outline-none placeholder:text-[var(--dim)]"
            />
            {value && (
              <button
                type="button"
                aria-label="Clear search"
                data-search-clear
                onClick={() => {
                  setValue("");
                  setCursor(-1);
                  inputRef.current?.focus();
                }}
                className="flex h-11 w-11 shrink-0 items-center justify-center rounded-full outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
              >
                <span className="flex h-9 w-9 items-center justify-center rounded-full bg-[var(--rule)] text-[var(--on)]">
                  <ClearIcon />
                </span>
              </button>
            )}
          </div>
        </div>
        <div
          data-search-results
          className="min-h-0 flex-1 overflow-y-auto"
          style={{ touchAction: "pan-y", overscrollBehavior: "contain", paddingBottom: "calc(20px + env(safe-area-inset-bottom, 0px))" }}
        >
          {list}
        </div>
      </div>
    );
  }

  return (
    <div
      ref={rootRef}
      data-search-box
      className="relative w-full"
      onBlur={(e) => {
        if (open && !rootRef.current?.contains(e.relatedTarget as Node | null)) {
          setOpen(false);
          setCursor(-1);
        }
      }}
    >
      {/* docs/UX.md §5's search field: magnifier, the input, and the `/`
          shortcut it answers to. */}
      <label
        className={`flex w-full items-center gap-2 rounded-[10px] border border-[var(--rule)] bg-[var(--canvas)] px-2.5 text-[var(--dim)] focus-within:ring-2 focus-within:ring-[var(--accent)] ${touch ? "h-11" : "h-9"}`}
      >
        <SearchIcon size={16} />
        <input
          {...comboProps}
          type="text"
          onFocus={() => setOpen(true)}
          className="h-full min-w-0 flex-1 bg-transparent text-small text-[var(--on)] outline-none placeholder:text-[var(--dim)]"
        />
        <kbd aria-hidden="true" className="rounded-[5px] border border-[var(--rule)] px-1.5 font-mono text-label font-normal text-[var(--dim)] max-[899px]:hidden">
          /
        </kbd>
      </label>
      {open && (
        <div
          data-search-dropdown
          className="absolute left-0 top-full mt-1 max-h-[min(460px,calc(100vh-140px))] w-full min-w-[min(360px,calc(100vw-24px))] overflow-y-auto rounded-[10px] border border-[var(--rule)] bg-[var(--chrome)] pb-1 shadow-lg"
          style={{ overscrollBehavior: "contain" }}
        >
          {list}
        </div>
      )}
    </div>
  );
}

/** The prototype's key cap: 20 px, mono, on the key token. */
function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="inline-flex h-5 min-w-5 shrink-0 items-center justify-center rounded-[5px] border border-[var(--chrome-glass-border)] bg-[var(--chrome-key)] px-[5px] font-mono text-[11px] font-normal leading-none text-[var(--dim)]">
      {children}
    </kbd>
  );
}

function Marked({ text, marks, accent = false }: { text: string; marks: readonly Mark[]; accent?: boolean }) {
  return (
    <>
      {segments(text, marks).map((s, n) =>
        s.hit ? (
          <mark key={n} className={`bg-transparent ${accent ? "font-semibold text-[var(--accent)]" : "font-bold text-[var(--on)]"}`}>
            {s.text}
          </mark>
        ) : (
          <span key={n}>{s.text}</span>
        ),
      )}
    </>
  );
}

const ICONS: Record<SearchPick["kind"], (p: { size?: number }) => ReactNode> = {
  district: DistrictIcon,
  file: FileIcon,
  symbol: SymbolIcon,
};

interface ResultListProps {
  id: string;
  results: SearchResults | PaletteResults;
  value: string;
  cursor: number;
  optionId(n: number): string;
  variant: "float" | "overlay" | "palette";
  onPick(item: PaletteItem): void;
  onHover?(n: number): void;
}

/** The listbox: group headers with counts, then rows. Names wrap rather
 * than truncate (§7.2: nothing important only in a `title`); paths break
 * anywhere, so a long one wraps instead of widening the row. */
function ResultList({ id, results, value, cursor, optionId, variant, onPick, onHover }: ResultListProps) {
  const phone = variant === "overlay";
  const palette = variant === "palette";
  let n = -1;
  return (
    <>
      {!results.query && (
        // The palette's placeholder already says what to type (the
        // prototype has no hint line over the groups); the state stays for
        // screen readers and check:view.
        <p data-search-state="empty" className={palette ? "sr-only" : `text-[var(--dim)] ${phone ? "px-5 pt-4 text-small" : "px-3 pt-2.5 text-meta"}`}>
          {variant === "palette"
            ? (results.groups.some((group) => group.kind === "command") ? "Type a district, file, symbol or command." : "Type a file path to choose a destination.")
            : "Type a district, file, class or function name."}
        </p>
      )}
      {results.query && results.flat.length === 0 && (
        <p data-search-state="no-results" role="status" className={`text-[var(--dim)] ${phone ? "px-5 py-7 text-body" : palette ? "px-2.5 py-6 text-small" : "px-3 py-3 text-small"}`}>
          {variant === "palette" ? (value.trim() ? `No results for “${value.trim()}”.` : "No files are available as a path destination.") : `No district, file or symbol matches “${value.trim()}”.`}
        </p>
      )}
      <div role="listbox" id={id} aria-label="Results">
        {results.groups.map((g) => (
          <div key={g.kind + g.label} role="group" aria-label={`${g.label}, ${g.items.length}`} data-search-group={g.kind}>
            <div
              aria-hidden="true"
              data-search-group-header
              className={`flex items-baseline justify-between gap-3 uppercase text-[var(--dim)] ${phone ? "px-5 pb-1.5 pt-3.5 text-label" : palette ? "px-2.5 pb-1 pt-2.5 text-[11px] font-semibold tracking-[0.06em]" : "px-3 pb-1 pt-2.5 text-label"}`}
            >
              <span>{g.label}</span>
              <span className="font-mono tabular-nums">{g.items.length}</span>
            </div>
            {g.items.map((it) => {
              n++;
              const at = n;
              const active = at === cursor;
              const command = "commandId" in it;
              const kind = command ? "command" : it.pick.kind;
              const Icon = command ? null : ICONS[it.pick.kind];
              const mono = !command && it.pick.kind !== "district";
              return (
                <button
                  type="button"
                  role="option"
                  id={optionId(at)}
                  key={it.key}
                  aria-selected={active}
                  tabIndex={-1}
                  data-search-option={kind}
                  data-search-key={it.key}
                  // Keep focus in the input (the combobox) on desktop.
                  onMouseDown={(e) => e.preventDefault()}
                  onMouseMove={onHover ? () => at !== cursor && onHover(at) : undefined}
                  onClick={() => onPick(it)}
                  className={`flex w-full items-center text-left text-[var(--on)] outline-none ${
                    phone
                      ? "min-h-[56px] gap-3 border-b border-[var(--rule)] px-5 py-2"
                      : palette
                        ? "min-h-[44px] gap-3 rounded-[9px] px-2.5 py-1.5"
                        : "min-h-[40px] gap-2.5 px-3 py-1.5 touch:min-h-[44px]"
                  }`}
                  style={active ? { background: palette ? "var(--chrome-hover)" : "color-mix(in srgb, var(--accent) 14%, transparent)" } : undefined}
                >
                  {palette ? (
                    // The prototype's 28 px kind tile: one quiet square per
                    // row, so districts, files, symbols and commands read
                    // apart at a glance without four icon styles.
                    <span aria-hidden="true" className="flex h-7 w-7 shrink-0 items-center justify-center rounded-[7px] bg-[var(--chrome-hover)] text-[var(--dim)]">
                      {Icon ? <Icon size={15} /> : <span className="font-mono text-[14px] leading-none">›</span>}
                    </span>
                  ) : (
                    <span className="shrink-0 text-[var(--dim)]">
                      {Icon ? <Icon size={phone ? 20 : 16} /> : null}
                    </span>
                  )}
                  <span className="flex min-w-0 flex-1 flex-col gap-px">
                    <span
                      data-search-name
                      className={`[overflow-wrap:anywhere] ${
                        palette
                          ? mono ? "font-mono text-[13.5px] leading-5" : "text-[14px] leading-5"
                          : mono ? `font-mono ${phone ? "text-[15px] leading-[21px]" : "text-small"}` : phone ? "text-body" : "text-small"
                      }`}
                    >
                      <Marked text={it.name} marks={it.nameMarks} accent={palette} />
                    </span>
                    {it.detail && (
                      <span
                        data-search-detail
                        className={`${command ? "" : "font-mono"} ${palette ? (command ? "text-[12px]" : "text-[11.5px]") : "text-meta"} text-[var(--dim)] [overflow-wrap:anywhere]`}
                      >
                        <Marked text={it.detail} marks={it.detailMarks} accent={palette} />
                      </span>
                    )}
                  </span>
                  {command && it.aside ? (
                    <Kbd>{it.aside}</Kbd>
                  ) : it.aside ? (
                    <span className="shrink-0 font-mono text-meta text-[var(--dim)]">{it.aside}</span>
                  ) : palette ? (
                    <span aria-hidden="true" className={`shrink-0 text-[11px] text-[var(--dim)] ${active ? "opacity-100" : "opacity-0"}`}>↵</span>
                  ) : null}
                </button>
              );
            })}
          </div>
        ))}
      </div>
    </>
  );
}
