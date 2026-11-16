import { useEffect, useRef, useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import type { OfferedRepository } from "@bindings/OfferedRepository";
import type { BatchIndexResponse } from "@bindings/BatchIndexResponse";

async function personalRequest<T>(path: string, body?: unknown): Promise<T> {
  const response = await fetch(path, body === undefined ? undefined : { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
  if (!response.ok) { const error = await response.json() as { message?: string }; throw new Error(error.message ?? "Repository request failed."); }
  return response.json() as Promise<T>;
}

/** The shared add dialog, adapted to sources available on this machine. */
export function PersonalRepositorySheet({ onClose }: { onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const navigate = useNavigate();
  const [source, setSource] = useState("");
  const [roots, setRoots] = useState<OfferedRepository[]>();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => { dialog.current?.showModal(); }, []);
  async function scan() {
    setPending(true); setError(undefined);
    try {
      const found = await personalRequest<OfferedRepository[]>(`/api/personal/scan?path=${encodeURIComponent(source)}`);
      setRoots(found); setSelected(new Set(found.filter(repo => !repo.connected_repository_id).map(repo => repo.provider_id)));
    } catch (cause) { setError(cause instanceof Error ? cause.message : "Folder could not be scanned."); }
    finally { setPending(false); }
  }
  async function submit() {
    const sources = roots ? roots.filter(repo => selected.has(repo.provider_id)).map(repo => repo.provider_id) : [source.trim()];
    if (!sources.length || !sources[0]) return;
    setPending(true); setError(undefined);
    try {
      const result = await personalRequest<BatchIndexResponse>("/api/personal/index", { provider_ids: sources });
      onClose();
      if (result.jobs.length === 1) await navigate({ to: "/new", search: { job: result.jobs[0].job_id } });
      else await navigate({ to: "/new/batches/$batchId", params: { batchId: result.batch_id } });
    } catch (cause) { setError(cause instanceof Error ? cause.message : "Repositories could not be added."); }
    finally { setPending(false); }
  }
  return <dialog ref={dialog} data-personal-add className="add-repository-dialog" aria-labelledby="personal-add-title" onCancel={onClose}>
    <div className="flex items-center justify-between gap-3"><h2 id="personal-add-title" className="text-section-title">Add repository</h2><button type="button" aria-label="Close add repository" className="repo-control repo-control-icon" disabled={pending} onClick={onClose}>×</button></div>
    <label className="text-small">Local checkout, parent folder, or public GitHub repository<input autoFocus className="offered-search mt-2 w-full" value={source} disabled={pending} placeholder="/path/to/checkout or owner/repository" onChange={event => { setSource(event.target.value); setRoots(undefined); }} /></label>
    <p className="text-meta text-[var(--dim)]">Only committed source is indexed. Original checkouts stay on disk when removed from tolmap. Public repositories are acquired anonymously. Private GitHub access is not available yet.</p>
    <button type="button" className="repo-control self-start" disabled={pending || !source.trim()} onClick={() => void scan()}>Scan folder</button>
    {roots && <div className="offered-repositories">{roots.length === 0 && <p>No checkouts found in this folder.</p>}{roots.map(repo => <label key={repo.provider_id} className="offered-repository"><input type="checkbox" checked={selected.has(repo.provider_id)} disabled={pending || !!repo.connected_repository_id} onChange={event => setSelected(previous => { const next = new Set(previous); if (event.target.checked) next.add(repo.provider_id); else next.delete(repo.provider_id); return next; })} /><span className="min-w-0 flex-1 break-all font-mono text-small">{repo.full_name}</span>{repo.connected_repository_id && <span className="offered-badge">Mapped</span>}</label>)}</div>}
    {error && <p role="alert" className="text-small text-[var(--link-out)]">{error}</p>}
    <div className="flex justify-end gap-2"><button type="button" className="repo-control" disabled={pending} onClick={onClose}>Cancel</button><button type="button" className="repo-control repo-control-primary" disabled={pending || !source.trim() || (roots !== undefined && selected.size === 0)} onClick={() => void submit()}>{pending ? "Starting…" : roots ? `Map ${selected.size} ${selected.size === 1 ? "repository" : "repositories"}` : "Add and map"}</button></div>
  </dialog>;
}
