import { useState } from "react";

/** docs/UX.md §12: a map opened before its job's Detail phase finished says
 * so, once, in plain words -- zooming in shows no classes or functions until
 * they land, and without a word that reads as a broken map. It goes away by
 * itself when the Detail phase lands. A Detail phase that failed says that
 * instead, until dismissed: the map itself is fine.
 *
 * Transient chrome over the map, which §3 otherwise keeps clear: it exists
 * only for the minutes between the handover and the job's end. */
export function DetailStatusNote({ pending, failed, narrow }: { pending: boolean; failed: string | null; narrow: boolean }) {
  const [dismissed, setDismissed] = useState(false);
  if (!pending && (failed == null || dismissed)) return null;
  const position = narrow
    ? "left-3 right-[68px] top-[calc(max(env(safe-area-inset-top),0px)+72px)]"
    : "left-1/2 top-2.5 w-max max-w-[min(380px,max(200px,calc(100%-600px)))] -translate-x-1/2";
  return (
    <div
      role="status"
      data-detail-status={pending ? "pending" : "failed"}
      className={`pointer-events-auto absolute z-20 flex items-center gap-2.5 rounded-[12px] border border-[var(--rule)] bg-[var(--chrome2)] px-3 py-2 text-small text-[var(--on)] shadow-md ${position}`}
    >
      {pending ? (
        <>
          <span aria-hidden className="relative h-4 w-4 flex-none rounded-full border-2 border-[var(--rule)]">
            <span className="tolmap-spin absolute -inset-[2px] rounded-full border-2 border-transparent border-t-[var(--accent)]" />
          </span>
          <span>Classes and functions are still being read. They appear at close zoom when ready.</span>
        </>
      ) : (
        <>
          <span className="min-w-0">Classes and functions couldn't be read for this map.</span>
          <button
            type="button"
            onClick={() => setDismissed(true)}
            aria-label="Dismiss"
            className="-my-2 -mr-2 inline-flex h-11 w-11 flex-none items-center justify-center rounded-[10px] text-[var(--dim)] hover:bg-[color-mix(in_srgb,var(--on)_8%,transparent)]"
          >
            ×
          </button>
        </>
      )}
    </div>
  );
}
