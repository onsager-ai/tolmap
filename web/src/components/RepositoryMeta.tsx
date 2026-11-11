import type { RepositoryStatus } from "@/types";

export function IndexedTime({ value }: { value: string }) {
  const at = Date.parse(value);
  if (!Number.isFinite(at)) return null;
  const days = Math.floor((Date.now() - at) / 86_400_000);
  const hours = Math.floor((Date.now() - at) / 3_600_000);
  const label = days > 0 ? `${days}d ago` : hours > 0 ? `${hours}h ago` : "just now";
  return <time dateTime={value} title={new Date(at).toLocaleString()} className="font-mono">{label}</time>;
}

export function RepositoryState({ status }: { status?: RepositoryStatus | null }) {
  if (!status) return null;
  return <span data-repository-status={status.kind} className="text-meta text-[var(--dim)]" title={status.kind === "failed" ? status.message : undefined}>
    {status.kind === "indexing" ? `Indexing · ${status.phase}` : status.kind === "failed" ? "Failed" : "Reconnect needed"}
  </span>;
}
