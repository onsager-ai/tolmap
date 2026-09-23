import { useMemo } from "react";
import type { DistrictSymbols, MapDocument } from "@/types";
import { D_, symbolsOf } from "@/map/geometry";
import { ancestorsOf, decodeDistrictSymbols, rowName } from "@/map/symbolCards";

interface Props {
  doc: MapDocument;
  sel: number | null;
  selSym: number | null;
  selD: number | null;
  /** Issue #82 C2: a GLOBAL hierarchical-symbol index (search.ts's `hsym`),
   * distinct from `selSym` above (the map's own flat `S` list). Extends the
   * breadcrumb to repo › district › file › class › method -- one segment per
   * level of NESTING, not just one flat "symbol" segment. */
  selHSym?: number | null;
  /** The selected file's district symbols, undocded here (cheap, and this is
   * the only place in the sidebar header that needs the ancestor CHAIN
   * specifically -- SelectionPanel's outline tree decodes its own copy for
   * its own, larger, purpose). `undefined` while unfetched, or for a map
   * with no symbols sibling -- degrades to the same view a plain `sym`
   * selection already gets. */
  symbolsDoc?: DistrictSymbols;
  /** Jumps straight to "nothing selected" -- not a step back, the repo
   * segment is always the top of the path. */
  onSelectRepo(): void;
  /** Selects a district WITHOUT moving the view (issue #82 A1 scope item 3:
   * "Clicking a segment selects that level without moving the view") --
   * callers pass MapView's plain, pan-free `selectDistrict`, the same
   * function the map's own district-polygon tap and the panel's internal
   * "near" links already use. */
  onSelectDistrict(d: number): void;
  /** Drops the symbol, keeping the SAME file selected -- also guaranteed not
   * to move the view, since the file was already selected (and so already
   * wherever it's going to be) before this segment existed to click. */
  onSelectFile(i: number): void;
  /** Selects an ANCESTOR of the current hierarchical-symbol selection
   * (a class segment clicked while its method is the deepest level) --
   * pan-free for the same reason onSelectFile is: the file, and so the
   * card, is already exactly where it's going to be. */
  onSelectHierSymbol?(global: number): void;
}

/** Issue #82 A1 scope item 3 (extended by #82 C2): repo › district › file ›
 * class › method, clickable up to (but not including) whichever segment is
 * the CURRENT deepest selection -- clicking the current level would just
 * reselect it, so it renders as plain bold text instead of a button, the
 * common breadcrumb convention. Shared between the desktop card and the
 * phone sheet: both render SelectionPanel's header, which is where this
 * mounts, so there is only one implementation to keep the two in sync. */
export function Breadcrumb({ doc, sel, selSym, selD, selHSym, symbolsDoc, onSelectRepo, onSelectDistrict, onSelectFile, onSelectHierSymbol }: Props) {
  const decoded = useMemo(() => (symbolsDoc ? decodeDistrictSymbols(symbolsDoc) : null), [symbolsDoc]);
  const hierChain = useMemo(() => {
    if (!decoded || selHSym == null) return null;
    const local = decoded.globalToLocal.get(selHSym);
    if (local == null) return null;
    const chain = [...ancestorsOf(decoded, local)].reverse().map((a) => ({
      global: decoded.raw.symbol_indices[a],
      name: rowName(decoded.raw.symbols[a]),
    }));
    chain.push({ global: selHSym, name: rowName(decoded.raw.symbols[local]) });
    return chain;
  }, [decoded, selHSym]);

  if (sel == null && selD == null) return null;
  const d = selD ?? D_(doc, sel!);
  const sy = sel != null ? symbolsOf(doc, sel) : [];
  const sm = sel != null && selSym != null ? sy[selSym] : null;
  const districtIsCurrent = sel == null; // selD-only state: district is the deepest level
  const fileIsCurrent = sel != null && sm == null && !hierChain;

  return (
    <div
      data-breadcrumb
      // Stops a tap on a segment from ALSO toggling the phone card's
      // collapse/expand -- the header row above this (SelectionPanel.tsx)
      // has its own onClick={onToggleOpen} that this would otherwise bubble
      // into, exactly like the existing "Zoom to district" button already
      // guards against with its own stopPropagation.
      onClick={(e) => e.stopPropagation()}
      className="mb-1 flex flex-wrap items-center gap-x-1 gap-y-0.5 text-[9.5px] text-[var(--dim)]"
    >
      <button type="button" className="hover:text-[var(--on)]" onClick={onSelectRepo}>
        {doc.repo}
      </button>
      <span aria-hidden="true">›</span>
      {districtIsCurrent ? (
        <span className="font-semibold text-[var(--on)]">{doc.names[d] ?? `district ${d}`}</span>
      ) : (
        <button type="button" className="hover:text-[var(--on)]" onClick={() => onSelectDistrict(d)}>
          {doc.names[d] ?? `district ${d}`}
        </button>
      )}
      {sel != null && (
        <>
          <span aria-hidden="true">›</span>
          {fileIsCurrent ? (
            <span className="font-semibold text-[var(--on)]">{doc.F[sel].split("/").pop()}</span>
          ) : (
            <button type="button" className="hover:text-[var(--on)]" onClick={() => onSelectFile(sel)}>
              {doc.F[sel].split("/").pop()}
            </button>
          )}
        </>
      )}
      {sm && (
        <>
          <span aria-hidden="true">›</span>
          <span className="font-semibold text-[var(--on)]">{sm[0]}</span>
        </>
      )}
      {hierChain?.map((entry, index) => {
        const isLast = index === hierChain.length - 1;
        return (
          <span key={entry.global} className="contents">
            <span aria-hidden="true">›</span>
            {isLast ? (
              <span className="font-semibold text-[var(--on)]">{entry.name}</span>
            ) : (
              <button type="button" className="hover:text-[var(--on)]" onClick={() => onSelectHierSymbol?.(entry.global)}>
                {entry.name}
              </button>
            )}
          </span>
        );
      })}
    </div>
  );
}
