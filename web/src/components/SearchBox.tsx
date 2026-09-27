import { useMemo, useRef, useState } from "react";
import type { MapDocument } from "@/types";
import { searchHits, type SearchHit } from "@/map/search";
import { KIND, KCOL } from "@/map/constants";
import { Input } from "@/components/ui/input";

interface SearchBoxProps {
  doc: MapDocument;
  onPick(hit: SearchHit): void;
  /** "overlay": the phone's full-screen search (docs/UX.md §4.8's layer,
   * phase 2's interim form: this same box at 16 px with a back button;
   * the grouped-results redesign is phase 3). "float" (default): desktop. */
  variant?: "float" | "overlay";
  /** Overlay only: the back button and Esc. */
  onClose?(): void;
}

/** File-and-symbol search with fly-to. Ranking lives in map/search.ts
 * (shared, pure) — this component is just the input, the dropdown and
 * keyboard nav, ported from the reference's #q/#sug pair. */
export function SearchBox({ doc, onPick, variant = "float", onClose }: SearchBoxProps) {
  const [value, setValue] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [open, setOpen] = useState(false);
  const [cursor, setCursor] = useState(-1);
  const inputRef = useRef<HTMLInputElement>(null);

  const districtName = useMemo(() => doc.names, [doc]);

  function onChange(v: string) {
    setValue(v);
    const q = v.trim().toLowerCase();
    if (!q) {
      setOpen(false);
      return;
    }
    setHits(searchHits(doc, q));
    setCursor(-1);
    setOpen(true);
  }

  function pick(hit: SearchHit) {
    setOpen(false);
    setValue("");
    onPick(hit);
    inputRef.current?.blur();
  }

  if (variant === "overlay") {
    return (
      <div className="flex h-full min-h-0 flex-col">
        <div
          className="flex items-center gap-1.5 border-b border-[var(--rule)] px-3 pb-2.5"
          style={{ paddingTop: "calc(12px + env(safe-area-inset-top, 0px))" }}
        >
          <button
            type="button"
            aria-label="Close search"
            onClick={onClose}
            className="flex h-11 w-11 shrink-0 items-center justify-center rounded-full text-[var(--on)]"
          >
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M15 5l-7 7 7 7" />
            </svg>
          </button>
          <Input
            ref={inputRef}
            value={value}
            autoFocus
            placeholder="Files, classes, functions…"
            autoComplete="off"
            aria-label="Search files"
            className="h-12 rounded-[24px] border-2 border-[var(--accent)] bg-[var(--chrome2)] px-4 text-body text-[var(--on)]"
            onChange={(e) => onChange(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                onClose?.();
                return;
              }
              if (!open || !hits.length) return;
              if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                setCursor((c) => (c + (e.key === "ArrowDown" ? 1 : hits.length - 1)) % hits.length);
              } else if (e.key === "Enter") {
                e.preventDefault();
                pick(hits[Math.max(0, cursor)]);
              }
            }}
          />
        </div>
        <div role="listbox" aria-label="Results" className="min-h-0 flex-1 overflow-y-auto" style={{ touchAction: "pan-y" }}>
          {open && hits.length === 0 && <div className="px-5 py-7 text-body text-[var(--dim)]">nothing matches</div>}
          {open &&
            hits.map((h, n) => {
              const sm = h.s != null ? doc.S?.[String(h.i)]?.[h.s] : null;
              return (
                <button
                  type="button"
                  role="option"
                  aria-selected={n === cursor}
                  key={`${h.i}:${h.s ?? "f"}`}
                  onClick={() => pick(h)}
                  className={`flex min-h-[56px] w-full flex-col justify-center gap-0.5 border-b border-[var(--rule)] px-5 py-2 text-left ${n === cursor ? "bg-[var(--chrome2)]" : ""}`}
                >
                  <span className="truncate font-mono text-body text-[var(--on)]">{sm ? sm[0] : doc.F[h.i].split("/").pop()}</span>
                  <span className="truncate font-mono text-meta text-[var(--dim)]">
                    {sm ? `${KIND[sm[1]]} · ${doc.F[h.i].split("/").pop()}:${sm[2]}` : doc.F[h.i]}
                  </span>
                </button>
              );
            })}
        </div>
      </div>
    );
  }

  return (
    <div className="absolute left-2.5 top-2.5 w-[268px] max-[820px]:left-2.5 max-[820px]:right-2.5 max-[820px]:w-auto">
      <Input
        ref={inputRef}
        value={value}
        placeholder="search files, classes, functions…"
        autoComplete="off"
        aria-label="Search files"
        className="bg-[var(--chrome)] text-[var(--on)]"
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (!open || !hits.length) return;
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            setCursor((c) => (c + (e.key === "ArrowDown" ? 1 : hits.length - 1)) % hits.length);
          } else if (e.key === "Enter") {
            e.preventDefault();
            pick(hits[Math.max(0, cursor)]);
          } else if (e.key === "Escape") {
            setOpen(false);
          }
        }}
      />
      {open && (
        <div className="mt-1 max-h-[238px] overflow-y-auto rounded-md border border-[var(--rule)] bg-[var(--chrome)] shadow-lg">
          {hits.length === 0 && <div className="px-2.5 py-1.5 text-meta italic text-[var(--dim)]">nothing matches</div>}
          {hits.map((h, n) => {
            const sm = h.s != null ? doc.S?.[String(h.i)]?.[h.s] : null;
            const refs = sm ? (doc.U?.[`${h.i}:${h.s}`] ?? []).length : 0;
            return (
              <div
                key={`${h.i}:${h.s ?? "f"}`}
                onClick={() => pick(h)}
                className={`cursor-pointer break-all px-2.5 py-1.5 font-mono text-meta hover:bg-[var(--chrome2)] ${n === cursor ? "bg-[var(--chrome2)]" : ""}`}
              >
                {sm ? (
                  <>
                    <b style={{ color: KCOL[sm[1]], fontWeight: 400 }}>{sm[0]}</b>{" "}
                    <em className="not-italic text-[var(--dim)]">
                      {KIND[sm[1]]} · {doc.F[h.i].split("/").pop()}
                      {refs ? ` · ${refs} refs` : ""}
                    </em>
                  </>
                ) : (
                  <>
                    {doc.F[h.i].split("/").pop()}{" "}
                    <em className="not-italic text-[var(--dim)]">{districtName[String(doc.N[h.i][0])]}</em>
                  </>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
