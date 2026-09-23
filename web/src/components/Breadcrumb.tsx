import type { MapDocument } from "@/types";
import { D_, symbolsOf } from "@/map/geometry";

interface Props {
  doc: MapDocument;
  sel: number | null;
  selSym: number | null;
  selD: number | null;
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
}

/** Issue #82 A1 scope item 3: repo › district › file › symbol, clickable up
 * to (but not including) whichever segment is the CURRENT deepest
 * selection -- clicking the current level would just reselect it, so it
 * renders as plain bold text instead of a button, the common breadcrumb
 * convention. Shared between the desktop card and the phone sheet: both
 * render SelectionPanel's header, which is where this mounts, so there is
 * only one implementation to keep the two in sync. */
export function Breadcrumb({ doc, sel, selSym, selD, onSelectRepo, onSelectDistrict, onSelectFile }: Props) {
  if (sel == null && selD == null) return null;
  const d = selD ?? D_(doc, sel!);
  const sy = sel != null ? symbolsOf(doc, sel) : [];
  const sm = sel != null && selSym != null ? sy[selSym] : null;
  const districtIsCurrent = sel == null; // selD-only state: district is the deepest level
  const fileIsCurrent = sel != null && sm == null;

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
    </div>
  );
}
