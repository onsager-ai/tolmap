import { useLoadProgress } from "@/hooks/useLoadProgress";

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** The map's own loading state (spec item 3): shows bytes received (and the
 * total, when known and not gzip-obscured -- src/api/streaming.ts) while the
 * body streams in, then "parsing…" while the worker does the blocking
 * JSON.parse off the main thread. `label` names what's loading ("owner/repo"
 * for the map, nothing extra needed for a district's symbols since the
 * caller already says "loading symbols…" around it). */
export function LoadProgressIndicator({ progressKey, label }: { progressKey: string; label: string }) {
  const progress = useLoadProgress(progressKey);
  if (!progress) return <p data-load-progress="idle">loading {label}…</p>;
  if (progress.phase === "parsing") {
    return (
      <p data-load-progress="parsing">
        parsing {label}… ({formatBytes(progress.receivedBytes)})
      </p>
    );
  }
  return (
    <p data-load-progress="downloading">
      loading {label}… {formatBytes(progress.receivedBytes)}
      {progress.totalBytes != null ? ` / ${formatBytes(progress.totalBytes)}` : ""}
    </p>
  );
}
