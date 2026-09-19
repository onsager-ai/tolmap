import { useMemo, useRef, useState } from "react";
import type { MapDocument } from "@/types";
import { searchHits, type SearchHit } from "@/map/search";
import { KIND, KCOL } from "@/map/constants";
import { Input } from "@/components/ui/input";

interface SearchBoxProps {
  doc: MapDocument;
  onPick(hit: SearchHit): void;
}

/** File-and-symbol search with fly-to. Ranking lives in map/search.ts
 * (shared, pure) — this component is just the input, the dropdown and
 * keyboard nav, ported from the reference's #q/#sug pair. */
export function SearchBox({ doc, onPick }: SearchBoxProps) {
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
          {hits.length === 0 && <div className="px-2.5 py-1.5 text-[10.5px] italic text-[var(--dim)]">nothing matches</div>}
          {hits.map((h, n) => {
            const sm = h.s != null ? doc.S?.[String(h.i)]?.[h.s] : null;
            const refs = sm ? (doc.U?.[`${h.i}:${h.s}`] ?? []).length : 0;
            return (
              <div
                key={`${h.i}:${h.s ?? "f"}`}
                onClick={() => pick(h)}
                className={`cursor-pointer break-all px-2.5 py-1.5 text-[10.5px] hover:bg-[var(--chrome2)] ${n === cursor ? "bg-[var(--chrome2)]" : ""}`}
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
