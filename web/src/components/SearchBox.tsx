import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import type { MapDocument } from "@/types";
import {
  groupResults,
  isNavKey,
  navKey,
  segments,
  type Mark,
  type SearchItem,
  type SearchPick,
  type SearchResults,
} from "@/map/searchResults";
import { BackIcon, ClearIcon, DistrictIcon, FileIcon, SymbolIcon } from "@/components/phone/icons";

interface SearchBoxProps {
  doc: MapDocument;
  onPick(pick: SearchPick): void;
  /** "overlay": the phone's full-screen search layer (docs/UX.md §4.8).
   * "float" (default): the desktop box with its dropdown (§5, §7.2). */
  variant?: "float" | "overlay";
  /** Overlay only: the back button and Esc close the layer. */
  onClose?(): void;
}

/** What the input is called, for people and for check:view: it searches
 * districts as well as files and symbols. */
export const SEARCH_LABEL = "Search districts, files and symbols";
const PLACEHOLDER = "Districts, files, classes…";

/** docs/UX.md §4.8 and §7.2: one search, two containers. The input is an
 * ARIA combobox driving a listbox through aria-activedescendant, so focus
 * stays in the input while the arrow keys move through the results; the
 * results are grouped District / Files / Symbols (map/searchResults.ts,
 * whose file and symbol ranking is map/search.ts's, unchanged). */
export function SearchBox({ doc, onPick, variant = "float", onClose }: SearchBoxProps) {
  const [value, setValue] = useState("");
  const [cursor, setCursor] = useState(-1);
  const [open, setOpen] = useState(false);
  const results = useMemo(() => groupResults(doc, value), [doc, value]);
  const inputRef = useRef<HTMLInputElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  /** Desktop: the element that had focus when `/` opened search. */
  const openerRef = useRef<Element | null>(null);
  const listId = useId();
  const optionId = (n: number) => `${listId}-o${n}`;
  const overlay = variant === "overlay";
  const expanded = overlay || open;

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
    if (overlay) {
      onClose?.();
      return;
    }
    setOpen(false);
    // A box opened by a click keeps focus on Esc (the combobox pattern);
    // one opened by `/` hands it back.
    if (openerRef.current) returnFocus();
  }

  function pick(item: SearchItem) {
    setValue("");
    setCursor(-1);
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
    if (overlay) return;
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
  }, [overlay]);

  // Desktop, §7.2: a pointerdown anywhere outside the box closes the
  // dropdown (audit defect 8: it never closed on an outside tap).
  useEffect(() => {
    if (overlay || !open) return;
    const onDown = (e: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
        setCursor(-1);
        openerRef.current = null;
      }
    };
    document.addEventListener("pointerdown", onDown, true);
    return () => document.removeEventListener("pointerdown", onDown, true);
  }, [overlay, open]);

  const comboProps = {
    ref: inputRef,
    role: "combobox",
    "aria-label": SEARCH_LABEL,
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
      // z-40: above every other layer on the map (the selection panel and
      // package legend are z-10, the coverage popover z-30), so nothing
      // covers the dropdown's edges (§4.8, audit defect 8).
      className="absolute left-2.5 top-2.5 z-40 w-[340px] max-w-[calc(100%-20px)]"
      onBlur={(e) => {
        if (open && !rootRef.current?.contains(e.relatedTarget as Node | null)) {
          setOpen(false);
          setCursor(-1);
        }
      }}
    >
      <input
        {...comboProps}
        type="text"
        onFocus={() => setOpen(true)}
        className="flex h-10 w-full rounded-md border border-[var(--rule)] bg-[var(--chrome)] px-3 py-1.5 text-small text-[var(--on)] shadow-sm outline-none placeholder:text-[var(--dim)] focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
      />
      {open && (
        <div
          data-search-dropdown
          className="mt-1 max-h-[min(460px,calc(100vh-140px))] overflow-y-auto rounded-md border border-[var(--rule)] bg-[var(--chrome)] pb-1 shadow-lg"
          style={{ overscrollBehavior: "contain" }}
        >
          {list}
        </div>
      )}
    </div>
  );
}

function Marked({ text, marks }: { text: string; marks: readonly Mark[] }) {
  return (
    <>
      {segments(text, marks).map((s, n) =>
        s.hit ? (
          <mark key={n} className="bg-transparent font-bold text-[var(--on)]">
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
  results: SearchResults;
  value: string;
  cursor: number;
  optionId(n: number): string;
  variant: "float" | "overlay";
  onPick(item: SearchItem): void;
  onHover?(n: number): void;
}

/** The listbox: group headers with counts, then rows. Names wrap rather
 * than truncate (§7.2: nothing important only in a `title`); paths break
 * anywhere, so a long one wraps instead of widening the row. */
function ResultList({ id, results, value, cursor, optionId, variant, onPick, onHover }: ResultListProps) {
  const phone = variant === "overlay";
  let n = -1;
  return (
    <>
      {!results.query && (
        <p data-search-state="empty" className={`text-[var(--dim)] ${phone ? "px-5 pt-4 text-small" : "px-3 pt-2.5 text-meta"}`}>
          Type a district, file, class or function name.
        </p>
      )}
      {results.query && results.flat.length === 0 && (
        <p data-search-state="no-results" role="status" className={`text-[var(--dim)] ${phone ? "px-5 py-7 text-body" : "px-3 py-3 text-small"}`}>
          No district, file or symbol matches “{value.trim()}”.
        </p>
      )}
      <div role="listbox" id={id} aria-label="Results">
        {results.groups.map((g) => (
          <div key={g.kind + g.label} role="group" aria-label={`${g.label}, ${g.items.length}`} data-search-group={g.kind}>
            <div
              aria-hidden="true"
              data-search-group-header
              className={`flex items-baseline justify-between gap-3 text-label uppercase text-[var(--dim)] ${phone ? "px-5 pb-1.5 pt-3.5" : "px-3 pb-1 pt-2.5"}`}
            >
              <span>{g.label}</span>
              <span className="font-mono tabular-nums">{g.items.length}</span>
            </div>
            {g.items.map((it) => {
              n++;
              const at = n;
              const active = at === cursor;
              const Icon = ICONS[it.pick.kind];
              const mono = it.pick.kind !== "district";
              return (
                <button
                  type="button"
                  role="option"
                  id={optionId(at)}
                  key={it.key}
                  aria-selected={active}
                  tabIndex={-1}
                  data-search-option={it.pick.kind}
                  data-search-key={it.key}
                  // Keep focus in the input (the combobox) on desktop.
                  onMouseDown={(e) => e.preventDefault()}
                  onMouseMove={onHover ? () => at !== cursor && onHover(at) : undefined}
                  onClick={() => onPick(it)}
                  className={`flex w-full items-center text-left text-[var(--on)] outline-none ${
                    phone ? "min-h-[56px] gap-3 border-b border-[var(--rule)] px-5 py-2" : "min-h-[40px] gap-2.5 px-3 py-1.5"
                  }`}
                  style={active ? { background: "color-mix(in srgb, var(--accent) 14%, transparent)" } : undefined}
                >
                  <span className="shrink-0 text-[var(--dim)]">
                    <Icon size={phone ? 20 : 16} />
                  </span>
                  <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                    <span
                      data-search-name
                      className={`[overflow-wrap:anywhere] ${mono ? `font-mono ${phone ? "text-[15px] leading-[21px]" : "text-small"}` : phone ? "text-body" : "text-small"}`}
                    >
                      <Marked text={it.name} marks={it.nameMarks} />
                    </span>
                    {it.detail && (
                      <span data-search-detail className="font-mono text-meta text-[var(--dim)] [overflow-wrap:anywhere]">
                        <Marked text={it.detail} marks={it.detailMarks} />
                      </span>
                    )}
                  </span>
                  {it.aside && <span className="shrink-0 font-mono text-meta text-[var(--dim)]">{it.aside}</span>}
                </button>
              );
            })}
          </div>
        ))}
      </div>
    </>
  );
}
