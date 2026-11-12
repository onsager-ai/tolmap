import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { hostProfile } from "@/host";
import { hostRequest, HostRequestError } from "@/api/onboarding";
import { HostActionLink } from "@/components/HostActions";
import type { OfferedRepository } from "@bindings/OfferedRepository";
import type { BatchIndexRequest } from "@bindings/BatchIndexRequest";
import type { BatchIndexResponse } from "@bindings/BatchIndexResponse";

export function AddRepositorySheet({ onClose }: { onClose: () => void }) {
  const onboarding = hostProfile!.onboarding!;
  const navigate = useNavigate();
  const dialog = useRef<HTMLDialogElement>(null);
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [pending, setPending] = useState(false);
  const [submitError, setSubmitError] = useState<string>();
  const offered = useQuery({ queryKey: ["offered-repositories", onboarding.offered_repositories_path], queryFn: () => hostRequest<OfferedRepository[]>(onboarding.offered_repositories_path), retry: false, staleTime: 0, refetchOnMount: "always" });
  useEffect(() => { dialog.current?.showModal(); }, []);
  const repositories = offered.data ?? [];
  const selectedRepositories = repositories.filter(repo => !repo.connected_repository_id && selected.has(repo.provider_id));
  const visible = repositories.filter(repo => repo.full_name.toLowerCase().includes(search.trim().toLowerCase()));
  const expired = offered.error instanceof HostRequestError && [401, 403].includes(offered.error.status);
  const state = offered.isLoading ? "loading" : offered.isError ? expired ? "expired" : "unavailable" : repositories.length === 0 ? "empty" : visible.length === 0 ? "search-miss" : "list";
  async function submit() {
    if (pending || selectedRepositories.length === 0) return;
    setPending(true); setSubmitError(undefined);
    try {
      const body: BatchIndexRequest = { provider_ids: selectedRepositories.map(repo => repo.provider_id) };
      const result = await hostRequest<BatchIndexResponse>(onboarding.batch_index_path, body);
      if (result.jobs.length === 0) throw new Error("No mapping jobs were started. Refresh the repository list and try again.");
      onClose();
      if (result.jobs.length === 1) await navigate({ to: "/new", search: { job: result.jobs[0].job_id } });
      else await navigate({ to: "/new/batches/$batchId", params: { batchId: result.batch_id } });
    } catch (error) { setSubmitError(error instanceof Error ? error.message : "Repositories could not be mapped."); }
    finally { setPending(false); }
  }
  return <dialog ref={dialog} data-add-repository data-add-state={state} className="add-repository-dialog" aria-labelledby="add-repository-title" onCancel={onClose} onClose={onClose} onClick={event => { if (event.target === event.currentTarget) { const rect = event.currentTarget.getBoundingClientRect(); if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) onClose(); } }}>
    <div className="flex items-center justify-between gap-3"><h2 id="add-repository-title" className="text-section-title">Add repository</h2><button type="button" className="repo-control repo-control-ghost repo-control-icon" onClick={onClose} aria-label="Close add repository">×</button></div>
    <label className="text-small" htmlFor="offered-search">Search repositories</label>
    <input id="offered-search" type="search" value={search} onChange={event => setSearch(event.target.value)} className="offered-search" placeholder="owner/repository" />
    <div className="offered-repositories" aria-live="polite">
      {state === "loading" && <p role="status">Loading repositories…</p>}
      {state === "empty" && <p>No repositories shared with tolmap yet.</p>}
      {state === "search-miss" && <p>No repositories match “{search}”.</p>}
      {offered.isError && <div role="alert"><p>{offered.error.message}</p>{!expired && <button type="button" className="repo-control mt-3" onClick={() => void offered.refetch()}>Retry</button>}</div>}
      {visible.map(repo => <label key={repo.provider_id} data-offered-repository className="offered-repository">
        <input type="checkbox" disabled={!!repo.connected_repository_id || pending} checked={selected.has(repo.provider_id)} onChange={event => setSelected(previous => { const next = new Set(previous); if (event.target.checked) next.add(repo.provider_id); else next.delete(repo.provider_id); return next; })} />
        <span className="min-w-0 flex-1 break-words font-mono text-small">{repo.full_name}</span><span className="offered-badge">{repo.visibility}</span>{repo.connected_repository_id && <span className="offered-badge">Mapped</span>}
      </label>)}
    </div>
    {onboarding.manage_access && <HostActionLink action={onboarding.manage_access} className="text-small text-[var(--dim)] underline underline-offset-2">Don't see it? Manage GitHub access ↗</HostActionLink>}
    {submitError && <p role="alert" className="text-small text-[var(--link-out)]">{submitError}</p>}
    <div className="flex justify-end gap-3 py-1"><button type="button" className="repo-control" onClick={onClose}>Cancel</button><button type="button" className="repo-control repo-control-primary" disabled={pending || selectedRepositories.length === 0 || offered.isError || offered.isLoading} onClick={() => void submit()}>{pending ? "Starting…" : `Map ${selectedRepositories.length} ${selectedRepositories.length === 1 ? "repository" : "repositories"}`}</button></div>
  </dialog>;
}
