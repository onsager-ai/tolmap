import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { API_BASE, type IndexResponse } from "@/api/client";
import { hostPath } from "@/host";
import type { RepositoryHistory } from "@/types";

/** Fetch capabilities when the row menu opens; the host still owns re-indexing. */
export function RepositoryRowActions({ repositoryId, name }: { repositoryId: string; name: string }) {
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  const navigate = useNavigate();
  const history = useQuery({ queryKey: ["repository-history", repositoryId], enabled: open,
    queryFn: async () => {
      const response = await fetch(`${API_BASE}/repositories/${encodeURIComponent(repositoryId)}/history`);
      if (!response.ok) throw new Error("Repository actions couldn't be reached.");
      return response.json() as Promise<RepositoryHistory>;
    }, retry: false,
  });
  async function reindex() {
    const action = history.data?.actions.reindex;
    if (!action || pending) return;
    setPending(true); setError(undefined);
    try {
      const response = await fetch(hostPath(action.path), { method: action.method, headers: { Accept: "application/json" } });
      if (!response.ok) throw new Error("Re-indexing couldn't be started. Try again shortly.");
      const result = await response.json() as IndexResponse;
      if (result.status === "done") window.location.assign(`/${result.slug}?commit=${encodeURIComponent(result.commit)}`);
      else await navigate({ to: "/new", search: { job: result.job_id, slug: result.slug } });
    } catch (cause) { setError(cause instanceof Error ? cause.message : "Re-indexing failed."); }
    finally { setPending(false); }
  }
  return <details className="relative mr-2" onToggle={event => setOpen(event.currentTarget.open)}><summary className="repo-control repo-control-ghost repo-control-icon list-none" aria-label={`Actions for ${name}`}>⋯</summary><div className="repo-menu">
    {history.isLoading && <p className="px-2 py-1 text-meta">Loading actions…</p>}
    {history.data?.actions.reindex && <button type="button" className="repo-control repo-control-ghost" disabled={pending} onClick={() => void reindex()}>{pending ? "Starting…" : "Re-index"}</button>}
    {history.data && !history.data.actions.reindex && <p className="px-2 py-1 text-meta text-[var(--dim)]">No actions available</p>}
    {(error || history.error) && <p role="alert" className="px-2 py-1 text-meta text-[var(--link-out)]">{error ?? history.error?.message}</p>}
  </div></details>;
}
