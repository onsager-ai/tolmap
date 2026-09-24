// A tiny keyed store for "how is this big fetch going", read by
// useLoadProgress (src/hooks/useLoadProgress.ts) via useSyncExternalStore.
// It exists because the progress itself is produced deep inside a
// TanStack Query queryFn (src/api/streaming.ts's fetchJsonTracked, used by
// both the service and static map/symbols fetches in src/data/queries.ts)
// -- a queryFn's return value is the final parsed document, not a place to
// thread intermediate byte counts through, and re-rendering on every chunk
// via query state would fight the query cache's own memoisation. This
// store is the side channel: the fetch reports into it by key, and any
// component showing a loading indicator for that key subscribes
// independently of the query itself.
export type LoadPhase = "downloading" | "parsing";

export interface LoadProgress {
  phase: LoadPhase;
  receivedBytes: number;
  /** Null when Content-Length is absent, or when the response is
   * compressed (docs/API.md: responses under /api/* may be gzip-encoded;
   * the header then names the wire size, not the decoded size this reads
   * out, so showing it as a total would overshoot 100%). */
  totalBytes: number | null;
}

const state = new Map<string, LoadProgress>();
const listeners = new Map<string, Set<() => void>>();

export function setLoadProgress(key: string, progress: LoadProgress): void {
  state.set(key, progress);
  for (const listener of listeners.get(key) ?? []) listener();
}

export function clearLoadProgress(key: string): void {
  if (!state.has(key)) return;
  state.delete(key);
  for (const listener of listeners.get(key) ?? []) listener();
}

export function getLoadProgress(key: string): LoadProgress | null {
  return state.get(key) ?? null;
}

export function subscribeLoadProgress(key: string, onChange: () => void): () => void {
  let set = listeners.get(key);
  if (!set) {
    set = new Set();
    listeners.set(key, set);
  }
  set.add(onChange);
  return () => {
    set.delete(onChange);
    if (set.size === 0) listeners.delete(key);
  };
}
