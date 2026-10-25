import { useQuery } from "@tanstack/react-query";
import { getSnapshotFreshness } from "@/api/client";
import type { SnapshotFreshness } from "@bindings/SnapshotFreshness";
import { DESKTOP_PANEL_LEFT_PX, DESKTOP_PANEL_TOP_PX, DESKTOP_PANEL_WIDTH_PX } from "@/map/layoutProfile";

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

export function SnapshotFreshnessNote({ owner, repo, commit, narrow, mapLeft = "env(safe-area-inset-left, 0px)", inline = false }: {
  owner: string; repo: string; commit?: string; narrow: boolean;
  mapLeft?: string; inline?: boolean;
}) {
  const { data } = useQuery({
    queryKey: ["snapshot-freshness", owner, repo, commit],
    queryFn: () => getSnapshotFreshness(owner, repo, commit!),
    enabled: !!commit, retry: false, refetchInterval: 5000,
  });
  if (!data) return null;
  const desktopLeft = DESKTOP_PANEL_LEFT_PX + DESKTOP_PANEL_WIDTH_PX + 12;
  const position = inline ? undefined : narrow ? {
    left: `calc(${mapLeft} + 12px)`, right: "calc(68px + env(safe-area-inset-right, 0px))",
    top: "calc(122px + env(safe-area-inset-top, 0px))",
  } : {
    left: `calc(${desktopLeft}px + env(safe-area-inset-left, 0px))`,
    top: `calc(${DESKTOP_PANEL_TOP_PX}px + env(safe-area-inset-top, 0px))`,
    width: `min(380px, calc(100% - ${desktopLeft + 12}px - env(safe-area-inset-left, 0px) - env(safe-area-inset-right, 0px)))`,
  };
  return <div data-snapshot-freshness-note className={`pointer-events-auto min-w-0 rounded-md border border-[var(--rule)] bg-[var(--chrome2)] p-2 text-small text-[var(--on)] ${inline ? "" : "absolute z-20"}`}
    style={position}>
    <FreshnessStatus value={data} />
  </div>;
}
