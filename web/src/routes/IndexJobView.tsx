import { useEffect, useState, type ReactNode } from "react";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { useJobProgress } from "@/api/useJobProgress";
import { postIndexJob, ApiRequestError, type JobStage } from "@/api/client";
import { Button } from "@/components/ui/button";

const STEPS: { stage: JobStage; label: string }[] = [
  { stage: "queued", label: "queued" },
  { stage: "cloning", label: "cloning" },
  { stage: "detecting", label: "detecting language & source root" },
  { stage: "indexing", label: "indexing" },
  { stage: "done", label: "done" },
];

function stepIndex(stage: JobStage | undefined): number {
  return STEPS.findIndex((s) => s.stage === stage);
}

/** A repository refused by the hosted size limits (docs/ARCHITECTURE.md,
 * "Limits") is not a distinct job status — it is `status: "failed"` carrying
 * `error_code: "repo_too_large"`. Branch on the code, never on the message:
 * the message names which limit tripped and its value, so it is display text
 * and will be reworded. The regex below is a fallback for a service older
 * than the `error_code` field. */
function looksTooLarge(job: { error?: string | null; error_code?: string | null }): boolean {
  if (job.error_code) return job.error_code === "repo_too_large";
  return !!job.error && /too large|size limit|exceeds.*(limit|cap)/i.test(job.error);
}

/** The /new progress view: watches one indexing job via useJobProgress and
 * routes to the map on completion. This is most of the first-run
 * experience (django takes minutes), so every state names the stage,
 * shows the human `stage` text from the job, and never falls back to a
 * meaningless spinner. */
export function IndexJobView() {
  const search = useSearch({ strict: false }) as { job?: string; slug?: string };
  const navigate = useNavigate();
  const { job, connection, error: transportError } = useJobProgress(search.job);
  const [retrying, setRetrying] = useState(false);
  const [retryError, setRetryError] = useState<string | null>(null);

  const slug = job?.slug ?? search.slug;
  const status = job?.status;

  useEffect(() => {
    if (status !== "done" || !job) return;
    const [owner, repo] = job.slug.split("/");
    if (!owner || !repo) return;
    navigate({
      to: "/$owner/$repo",
      params: { owner, repo },
      search: { geo: "r", layer: "d" },
      replace: true,
    });
  }, [status, job, navigate]);

  async function retry() {
    if (!slug || retrying) return;
    setRetrying(true);
    setRetryError(null);
    try {
      const res = await postIndexJob({ repo: slug });
      if (res.status === "done") {
        const [owner, repo] = res.slug.split("/");
        await navigate({ to: "/$owner/$repo", params: { owner, repo }, search: { geo: "r", layer: "d" } });
      } else {
        await navigate({ to: "/new", search: { job: res.job_id, slug: res.slug }, replace: true });
      }
    } catch (err) {
      setRetryError(err instanceof ApiRequestError && err.code === "busy"
        ? "The index queue is full. Please try again shortly."
        : err instanceof Error ? err.message : String(err));
    } finally {
      setRetrying(false);
    }
  }

  if (!search.job) {
    return (
      <Centered>
        <p className="text-sm text-[var(--dim)]">No indexing job to show.</p>
        <Link to="/" className="text-sm text-[#6FB39F] underline underline-offset-2">
          back to the map index
        </Link>
      </Centered>
    );
  }

  if (transportError && !job) {
    return (
      <Centered>
        <p className="text-sm text-[var(--hot)]">couldn't reach the indexing service: {transportError}</p>
        <Link to="/" className="text-sm text-[#6FB39F] underline underline-offset-2">
          back to the map index
        </Link>
      </Centered>
    );
  }

  if (!job) {
    return (
      <Centered>
        <p className="text-sm text-[var(--dim)]">connecting to job {search.job}…</p>
      </Centered>
    );
  }

  if (job.status === "failed") {
    const tooLarge = looksTooLarge(job);
    return (
      <Centered>
        <h1 className="font-sans text-lg font-semibold text-[var(--on)]">{slug}</h1>
        <div
          className={`max-w-md rounded-md border px-4 py-3 text-sm ${
            tooLarge
              ? "border-[#8A6B1C] bg-[#8A6B1C1a] text-[var(--on)]"
              : "border-[var(--hot)] bg-[color-mix(in_srgb,var(--hot)_12%,transparent)] text-[var(--hot)]"
          }`}
        >
          {tooLarge ? (
            <>
              <p className="font-semibold">too large for the hosted index</p>
              <p className="mt-1 text-[var(--dim)]">{job.error}</p>
            </>
          ) : (
            <p>{job.error ?? "indexing failed."}</p>
          )}
        </div>
        {retryError && <p className="text-xs text-[var(--hot)]">retry failed: {retryError}</p>}
        <div className="flex gap-2">
          <Button size="sm" variant="outline" onClick={() => void retry()} disabled={retrying || tooLarge}>
            {retrying ? "retrying…" : "try again"}
          </Button>
          <Link to="/">
            <Button size="sm" variant="ghost">
              back to the map index
            </Button>
          </Link>
        </div>
      </Centered>
    );
  }

  const idx = stepIndex(job.status);

  return (
    <Centered>
      <h1 className="font-sans text-lg font-semibold text-[var(--on)]">{slug}</h1>
      <p className="text-sm text-[var(--dim)]">{job.queue_position != null ? `queued (#${job.queue_position})` : job.stage}</p>

      <ol className="flex w-full max-w-sm flex-col gap-1.5">
        {STEPS.slice(0, -1).map((s, i) => {
          const state = i < idx ? "done" : i === idx ? "current" : "pending";
          return (
            <li key={s.stage} className="flex items-center gap-2 text-sm">
              <span
                className={`inline-flex h-4 w-4 flex-none items-center justify-center rounded-full text-[10px] ${
                  state === "done"
                    ? "bg-[#6FB39F] text-[#0a1410]"
                    : state === "current"
                      ? "border-2 border-[#6FB39F] text-[#6FB39F]"
                      : "border border-[var(--rule)] text-[var(--dim)]"
                }`}
                aria-hidden
              >
                {state === "done" ? "✓" : ""}
              </span>
              <span className={state === "pending" ? "text-[var(--dim)]" : "text-[var(--on)]"}>{s.label}</span>
            </li>
          );
        })}
      </ol>

      <p className="text-[11px] text-[var(--dim)]">
        {connection === "poll" ? "polling for updates" : connection === "sse" ? "live updates" : "connecting…"}
        {" · "}started {new Date(job.started_at).toLocaleTimeString()}
      </p>
      <p className="max-w-sm text-center text-[11px] text-[var(--dim)]">
        A large repository (django-sized) can take several minutes — this page updates as each stage completes.
      </p>
    </Centered>
  );
}

function Centered({ children }: { children: ReactNode }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 bg-[var(--chrome)] p-8 text-center text-[var(--on)]">
      {children}
    </div>
  );
}
