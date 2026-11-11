import { useCallback, useEffect, useRef, useState } from "react";
import { Link, useParams } from "@tanstack/react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { API_BASE, type IndexResponse } from "@/api/client";
import { useJobProgress } from "@/api/useJobProgress";
import { IndexedTime, RepositoryState } from "@/components/RepositoryMeta";
import { ThemeToggle } from "@/components/ThemeToggle";
import { ProgressPage } from "@/routes/IndexJobView";
import type { RepositoryAction, RepositoryHistory, RepositorySnapshot } from "@/types";

const button = "inline-flex min-h-11 items-center justify-center rounded-[12px] border border-[var(--rule)] px-4 text-small font-semibold";

function actionPath(action: RepositoryAction) {
  if (!action.path.startsWith("/") || action.path.startsWith("//") || action.path.includes("\\")) throw new Error("Repository actions must stay on this origin");
  return action.path;
}

function OpenAction({ action, children }: { action: RepositoryAction; children: React.ReactNode }) {
  return action.method === "GET" ? <a href={actionPath(action)} className={button}>{children}</a> : null;
}

function RunningJob({ id, onFinished }: { id: string; onFinished: () => void }) {
  const { job, seen, error } = useJobProgress(id);
  useEffect(() => {
    if (job?.status === "done" || job?.status === "failed") onFinished();
  }, [job?.status, onFinished]);
  if (error) return <p role="alert" className="text-small">{error}</p>;
  if (!job) return <p role="status">Loading indexing progress…</p>;
  return <ProgressPage job={job} seen={seen} narrow inline />;
}

function SnapshotRow({ snapshot, currentRevision, onDelete }: { snapshot: RepositorySnapshot; currentRevision: string | null; onDelete: () => void }) {
  const content = <>
    <span className="font-mono text-row">{snapshot.commit.slice(0, 8)}</span>
    <span className="text-meta text-[var(--dim)]"><IndexedTime value={snapshot.indexed_at} /> · <span className="font-mono">{snapshot.files.toLocaleString()}</span> files · <span className="font-mono">{snapshot.districts.toLocaleString()}</span> districts</span>
    {snapshot.core_revision && currentRevision && snapshot.core_revision !== currentRevision && <span className="text-meta text-[var(--dim)]">Mapped with a different indexer · <span className="font-mono">{snapshot.core_revision.slice(0, 8)}</span></span>}
  </>;
  const row = "flex min-h-16 min-w-0 flex-1 flex-col justify-center gap-1 px-4 py-3";
  return <li data-snapshot-row className="flex items-center border-b border-[var(--rule)] last:border-b-0">
    {snapshot.reopen?.method === "GET" ? <a className={`${row} hover:bg-[var(--chrome)]`} href={actionPath(snapshot.reopen)}>{content}</a> : <div className={row}>{content}</div>}
    {snapshot.delete_snapshot && <button type="button" className="min-h-11 shrink-0 px-3 text-small text-[var(--link-out)]" aria-label={`Delete snapshot ${snapshot.commit.slice(0, 8)}`} onClick={onDelete}>Delete</button>}
  </li>;
}

type Confirmation = { label: string; action: RepositoryAction; typed?: string; leave?: boolean };
function ConfirmAction({ confirmation, pending, error, onConfirm, onClose }: { confirmation: Confirmation; pending: boolean; error: string | null; onConfirm: () => void; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [value, setValue] = useState("");
  useEffect(() => { dialog.current?.showModal(); }, []);
  return <dialog ref={dialog} onCancel={event => { event.preventDefault(); if (!pending) onClose(); }} aria-labelledby="repository-confirm-title" className="w-[calc(100%-32px)] max-w-[440px] rounded-[14px] border border-[var(--rule)] bg-[var(--chrome2)] p-5 text-[var(--on)] backdrop:bg-black/40">
    <h2 id="repository-confirm-title" className="text-section-title">{confirmation.label}?</h2>
    <p className="mt-3 text-small text-[var(--dim)]">{confirmation.leave ? "This removes the repository and its saved snapshots." : "This action affects the selected repository or snapshot."}</p>
    {confirmation.typed && <label className="mt-4 block text-small">Type <span className="font-mono">{confirmation.typed}</span> to confirm<input autoFocus value={value} onChange={event => setValue(event.target.value)} className="mt-2 min-h-11 w-full rounded-[12px] border border-[var(--rule)] bg-[var(--chrome)] px-3 font-mono" /></label>}
    {error && <p role="alert" className="mt-3 text-small text-[var(--link-out)]">{error}</p>}
    <div className="mt-5 flex flex-wrap justify-end gap-2"><button type="button" className={button} disabled={pending} onClick={onClose}>Cancel</button><button data-confirm-action type="button" className={`${button} text-[var(--link-out)] disabled:opacity-50`} disabled={pending || (!!confirmation.typed && value !== confirmation.typed)} onClick={onConfirm}>{pending ? "Working…" : confirmation.label}</button></div>
  </dialog>;
}

export function RepositoryPage() {
  const { repositoryId } = useParams({ from: "/repositories/$repositoryId" });
  const cache = useQueryClient();
  const history = useQuery({
    queryKey: ["repository-history", repositoryId],
    queryFn: async () => {
      const response = await fetch(`${API_BASE}/repositories/${encodeURIComponent(repositoryId)}/history`);
      if (!response.ok) throw new Error("Repository history couldn't be reached.");
      return response.json() as Promise<RepositoryHistory>;
    },
    retry: false,
  });
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [jobId, setJobId] = useState<string>();
  const data = history.data;
  const runningId = jobId ?? (data?.status?.kind === "indexing" ? data.status.job_id ?? undefined : undefined);
  const refresh = useCallback(async () => {
    await cache.invalidateQueries({ queryKey: ["catalogue"] });
    await cache.invalidateQueries({ queryKey: ["repository-history", repositoryId] });
  }, [cache, repositoryId]);
  const finished = useCallback(() => { setJobId(undefined); void refresh(); }, [refresh]);
  async function perform(action: RepositoryAction, leave = false) {
    setPending(true); setError(null);
    try {
      const response = await fetch(actionPath(action), { method: action.method, headers: { Accept: "application/json" } });
      if (!response.ok) throw new Error(`Action failed (${response.status}). Please retry.`);
      if (response.headers.get("content-type")?.includes("application/json")) {
        const result = await response.json() as Partial<IndexResponse>;
        if (result.job_id) setJobId(result.job_id);
        else if (result.status === "done" && result.slug) { window.location.assign(`/${result.slug}`); return; }
      }
      setConfirmation(null);
      if (leave) window.location.assign("/");
      else await refresh();
    } catch (cause) { setError(cause instanceof Error ? cause.message : "Action failed."); }
    finally { setPending(false); }
  }
  function confirm(value: Confirmation) { setError(null); setConfirmation(value); }
  return <div className="h-full overflow-y-auto bg-[var(--chrome)] text-[var(--on)]"><main className="mx-auto flex max-w-[760px] flex-col gap-6 px-5 pb-10 pt-7">
    <nav className="flex items-center justify-between"><Link to="/" className="flex min-h-11 items-center text-small text-[var(--accent)]">‹ Repositories</Link><ThemeToggle className="h-11 w-11 rounded-full" /></nav>
    {history.isLoading && <p>Loading repository…</p>}
    {history.isError && <div role="alert"><p>Repository history couldn't be reached.</p><button className={button} onClick={() => void history.refetch()}>Retry</button></div>}
    {data && <>
      <header data-repository-header className="flex flex-col gap-3"><h1 className="break-words text-title">{data.name}</h1><RepositoryState status={data.status} />
        <div className="flex flex-wrap items-center gap-2">
          {data.actions.reopen && <OpenAction action={data.actions.reopen}>Open latest map</OpenAction>}
          {data.actions.reindex && <button type="button" className={`${button} disabled:opacity-50`} disabled={pending} onClick={() => void perform(data.actions.reindex!)}>Re-index</button>}
          {(data.actions.disconnect || data.actions.delete_repository) && <details className="relative"><summary aria-label="Repository actions" className={`${button} cursor-pointer list-none`}>More ···</summary><div className="absolute right-0 z-10 mt-2 flex w-52 flex-col rounded-[12px] border border-[var(--rule)] bg-[var(--chrome2)] p-1 shadow-lg">
            {data.actions.disconnect && <button className="min-h-11 px-3 text-left text-small" onClick={() => confirm({ label: "Disconnect", action: data.actions.disconnect! })}>Disconnect</button>}
            {data.actions.delete_repository && <button className="min-h-11 px-3 text-left text-small text-[var(--link-out)]" onClick={() => confirm({ label: "Delete repository", action: data.actions.delete_repository!, typed: data.name, leave: true })}>Delete repository</button>}
          </div></details>}
        </div>
      </header>
      {error && !confirmation && <p role="alert" className="text-small text-[var(--link-out)]">{error}</p>}
      <section className="flex flex-col gap-3"><h2 className="text-section-title">Snapshot history <span className="font-mono text-[var(--dim)]">{data.snapshots.length}</span></h2>
        {runningId && <RunningJob key={runningId} id={runningId} onFinished={finished} />}
        {data.snapshots.length === 0 ? <p data-history-empty className="text-small text-[var(--dim)]">No snapshots yet. Your first map will appear here.</p> : <ol className="overflow-hidden rounded-[14px] border border-[var(--rule)]">{data.snapshots.map(snapshot => <SnapshotRow key={snapshot.id} snapshot={snapshot} currentRevision={data.current_core_revision} onDelete={() => confirm({ label: "Delete snapshot", action: snapshot.delete_snapshot! })} />)}</ol>}
      </section>
    </>}
    {confirmation && <ConfirmAction confirmation={confirmation} pending={pending} error={error} onClose={() => setConfirmation(null)} onConfirm={() => void perform(confirmation.action, confirmation.leave)} />}
  </main></div>;
}
