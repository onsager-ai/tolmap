import { useQuery } from "@tanstack/react-query";
import { getSnapshotFreshness } from "@/api/client";
import type { SnapshotFreshness } from "@bindings/SnapshotFreshness";

export function FreshnessStatus({ value }: { value: SnapshotFreshness }) {
  const indexed = value.indexed_commit?.slice(0, 10);
  const requested = value.requested_commit?.slice(0, 10);
  return <div role="status" data-snapshot-state={value.state}>
    <span>{indexed ? `Viewing ${indexed}. ` : "No complete snapshot. "}</span>
    {value.state === "queued" && <span>{requested} is queued. The saved snapshot stays available.</span>}
    {value.state === "indexing" && <span>Indexing {requested}. The saved snapshot stays available.</span>}
    {value.state === "failed" && <span>Indexing {requested} failed. {value.failure_reason} </span>}
    {value.state === "stale" && <span>A newer saved or requested snapshot is available.</span>}
    {value.retryable && <a className="underline" href="/">Retry indexing</a>}
  </div>;
}

export function SnapshotFreshnessNote({ owner, repo, commit, narrow }: {
  owner: string; repo: string; commit?: string; narrow: boolean;
}) {
  const { data } = useQuery({
    queryKey: ["snapshot-freshness", owner, repo, commit],
    queryFn: () => getSnapshotFreshness(owner, repo, commit!),
    enabled: !!commit, retry: false, refetchInterval: 5000,
  });
  if (!data) return null;
  return <div className="pointer-events-auto absolute z-20 max-w-[min(380px,calc(100%-24px))] rounded-md border border-[var(--rule)] bg-[var(--chrome2)] p-2 text-small text-[var(--on)]"
    style={{ left: 12, top: narrow ? "calc(122px + env(safe-area-inset-top, 0px))" : 70 }}>
    <FreshnessStatus value={data} />
  </div>;
}
