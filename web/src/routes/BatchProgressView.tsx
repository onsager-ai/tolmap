import { Link, useParams } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { hostProfile } from "@/host";
import { hostRequest } from "@/api/onboarding";
import { AppShell } from "@/components/AppShell";
import { summarisePhases } from "@/lib/indexPhases";
import type { BatchProgress } from "@bindings/BatchProgress";

export function BatchProgressView() {
  const { batchId } = useParams({ from: "/new/batches/$batchId" });
  const path = hostProfile?.onboarding?.batch_progress_path;
  const batch = useQuery({ queryKey: ["batch-progress", path, batchId], enabled: !!path,
    queryFn: () => hostRequest<BatchProgress>(path!.replace("{batch_id}", encodeURIComponent(batchId))), retry: false,
    refetchInterval: query => query.state.data?.jobs.every(job => job.status === "done" || job.status === "failed") ? false : 2000,
  });
  return <div className="h-full overflow-y-auto bg-[var(--chrome)] text-[var(--on)]"><main className="mx-auto flex max-w-[760px] flex-col gap-6 px-5 pb-10 pt-7"><AppShell /><h1 className="text-title">Mapping repositories</h1>
    {!path && <p role="alert">Batch progress is not available from this host.</p>}
    {path && batch.isLoading && <p role="status">Loading mapping progress…</p>}
    {batch.isError && <div role="alert"><p>{batch.error.message}</p><button type="button" className="repo-control mt-3" onClick={() => void batch.refetch()}>Retry</button></div>}
    {batch.data && <ol data-batch-progress className="overflow-hidden rounded-[10px] border border-[var(--rule)]">{batch.data.jobs.map(job => {
      const phases = summarisePhases(job);
      const phase = phases.find(item => item.state === "running" || item.state === "failed");
      const complete = job.status === "done";
      // Stage position is a lower-bound overview; within-stage work never implies
      // a precise estimate of total indexing time.
      const progress = complete ? 100 : job.progress ? Math.max(0, Math.min(99, ((job.progress.stage_index - 1) / job.progress.stage_count) * 100)) : undefined;
      return <li key={job.job_id} className="border-b border-[var(--rule)] last:border-0"><Link to="/new" search={{ job: job.job_id, slug: job.slug }} data-batch-job className="flex flex-col gap-2 px-4 py-3 hover:bg-[var(--chrome2)]">
        <span className="flex items-baseline justify-between gap-3"><span className="min-w-0 break-words font-mono text-small">{job.slug}</span><span className="offered-badge">{complete ? "Ready" : job.status === "failed" ? "Failed" : job.status === "queued" ? "Queued" : phase?.title ?? "Mapping"}</span></span>
        <progress aria-label={`Mapping ${job.slug}`} max={100} value={progress} className="batch-progress-bar" />
        <span className="text-meta text-[var(--dim)]">{job.status === "failed" ? job.error ?? "Mapping failed. Open details to retry." : complete ? "Open progress to view the map" : phase?.meaning ?? "Waiting to start"}</span>
      </Link></li>;
    })}</ol>}
    {batch.data?.jobs.length === 0 && <p>No repositories in this batch.</p>}
  </main></div>;
}
