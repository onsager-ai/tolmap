import { useSyncExternalStore } from "react";
import { getLoadProgress, subscribeLoadProgress, type LoadProgress } from "@/api/loadProgress";

/** Reactive read of src/api/loadProgress.ts's keyed store. Returns null once
 * the fetch this key names has finished (or hasn't started) -- callers show
 * their own default/loading text in that case. */
export function useLoadProgress(key: string): LoadProgress | null {
  return useSyncExternalStore(
    (onChange) => subscribeLoadProgress(key, onChange),
    () => getLoadProgress(key),
    () => null,
  );
}
