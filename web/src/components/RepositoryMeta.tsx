import "./repository.css";
import type { RepositoryStatus } from "@/types";

export function IndexedTime({ value }: { value: string }) {
  const at = Date.parse(value);
  if (!Number.isFinite(at)) return null;
  const elapsed = Math.max(0, Date.now() - at);
  const days = Math.floor(elapsed / 86_400_000);
  const hours = Math.floor(elapsed / 3_600_000);
  const minutes = Math.floor(elapsed / 60_000);
  const label = days > 0 ? `${days}d ago` : hours > 0 ? `${hours}h ago` : minutes > 0 ? `${minutes}m ago` : "just now";
  return <time dateTime={value} title={new Date(at).toLocaleString()} className="font-mono">{label}</time>;
}

export function RepositoryState({ status }: { status?: RepositoryStatus | null }) {
  if (!status) return null;
  const tone = status.kind === "failed" ? "var(--link-out)" : status.kind === "reconnect_needed" ? "var(--warn)" : "var(--accent)";
  return <span data-repository-status={status.kind} className="inline-flex w-fit shrink-0 items-center gap-1 h-5 rounded-[4px] px-1.5 text-[12px] leading-none" style={{ color: tone, background: `color-mix(in srgb, ${tone} 12%, transparent)` }} title={status.kind === "failed" ? status.message : undefined}>
    <span aria-hidden="true" className="h-1.5 w-1.5 rounded-full bg-current" />
    {status.kind === "indexing" ? `Indexing · ${status.phase}` : status.kind === "failed" ? "Failed" : "Reconnect needed"}
  </span>;
}
