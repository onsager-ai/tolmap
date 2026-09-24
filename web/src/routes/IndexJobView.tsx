import { useEffect, useState, type ReactNode } from "react";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { useJobProgress } from "@/api/useJobProgress";
import { postIndexJob, cancelJob, ApiRequestError } from "@/api/client";
import type { JobSnapshot, ProgressValue, StageSnapshot } from "@/types";
import { Button } from "@/components/ui/button";

function isTerminalStatus(status: JobSnapshot["status"]): boolean {
  return status === "done" || status === "failed";
}

function formatSeconds(totalSeconds: number): string {
  const s = Math.max(0, Math.round(totalSeconds));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const rem = s % 60;
  return rem === 0 ? `${m}m` : `${m}m ${rem}s`;
}

/** Spec item 2: "Overall: elapsed time and ETA shown as a range ('about
 * 3-5 min left')." Rounds each bound to whole minutes independently rather
 * than the midpoint, so the displayed range still brackets the model's own
 * low_s/high_s rather than looking falsely precise. */
function formatEtaRange(lowS: number, highS: number): string {
  const lowMin = Math.max(0, Math.round(lowS / 60));
  const highMin = Math.max(lowMin, Math.round(highS / 60));
  if (lowMin === 0 && highMin === 0) return "less than a minute left";
  if (lowMin === highMin) return `about ${lowMin} min left`;
  return `about ${lowMin}–${highMin} min left`;
}

/** Spec item 2: "While queued: 'starts in about N min - #k in queue', from
 * eta_start_s and queue_position." Below 30s this would round to "0 min",
 * which reads as broken rather than imminent -- "about a minute" covers
 * that floor without inventing false precision for a short wait. */
function formatStartsIn(etaStartS: number): string {
  if (etaStartS < 30) return "starts in about a minute";
  const min = Math.round(etaStartS / 60);
  return `starts in about ${min} min`;
}

function formatUnitValue(value: bigint, unit: string): string {
  const n = Number(value);
  if (unit === "bytes") return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return n.toLocaleString();
}

function formatDoneTotal(progress: ProgressValue): string {
  const unitWord = progress.unit === "bytes" ? "" : ` ${progress.unit}`;
  const done = formatUnitValue(progress.done, progress.unit);
  if (progress.total != null) {
    return `${done} / ${formatUnitValue(progress.total, progress.unit)}${unitWord}`;
  }
  return `${done}${unitWord}`;
}

function formatRate(progress: ProgressValue): string | null {
  if (progress.rate_per_s == null) return null;
  if (progress.unit === "bytes") return `${(progress.rate_per_s / (1024 * 1024)).toFixed(1)} MB/s`;
  return `${progress.rate_per_s.toFixed(1)} ${progress.unit}/s`;
}

/** Spec item 2's `worker_crashed` wording names "the last stage" -- the
 * furthest one this job actually reached, not necessarily the one the
 * terminal snapshot still calls "running" (a crash can be observed before
 * the stage's own `failed` transition lands). Checked in that order:
 * explicitly failed, then still running, then the last one finished, with
 * the free-text `stage` field as a last resort for a snapshot with no
 * stage rows at all (shouldn't happen, but this must never throw). */
function lastActiveStageLabel(job: JobSnapshot): string {
  const reversed = [...job.stages].reverse();
  const failed = reversed.find((s) => s.state === "failed");
  if (failed) return failed.label;
  const running = reversed.find((s) => s.state === "running");
  if (running) return running.label;
  const done = reversed.find((s) => s.state === "done");
  if (done) return done.label;
  return job.stage;
}

function StageIcon({ state }: { state: StageSnapshot["state"] }) {
  if (state === "done") {
    return (
      <span
        aria-hidden
        className="inline-flex h-4 w-4 flex-none items-center justify-center rounded-full bg-[#6FB39F] text-[10px] text-[#0a1410]"
      >
        ✓
      </span>
    );
  }
  if (state === "failed") {
    return (
      <span
        aria-hidden
        className="inline-flex h-4 w-4 flex-none items-center justify-center rounded-full bg-[var(--hot)] text-[10px] text-white"
      >
        ×
      </span>
    );
  }
  if (state === "running") {
    return <span aria-hidden className="inline-flex h-4 w-4 flex-none items-center justify-center rounded-full border-2 border-[#6FB39F]" />;
  }
  return <span aria-hidden className="inline-flex h-4 w-4 flex-none items-center justify-center rounded-full border border-[var(--rule)]" />;
}

/** The running stage's own progress bar (spec item 2): a determinate bar
 * with done/total in its unit and a rate when `total` is known, otherwise an
 * indeterminate bar plus the job's overall elapsed time (there is no
 * per-stage elapsed field on the wire -- `progress.rate_per_s` is already
 * null in the indeterminate case, since it's computed from `done`, and the
 * job's own elapsed time is the honest thing to show instead of nothing). */
function StageProgressBar({ progress, elapsedS }: { progress: ProgressValue; elapsedS: number }) {
  const pct = progress.total != null && progress.total > 0n ? Math.min(100, (Number(progress.done) / Number(progress.total)) * 100) : null;
  return (
    <div className="ml-6 mt-0.5" data-stage-progress>
      <div
        className="h-1.5 w-full overflow-hidden rounded-full bg-[var(--rule)]"
        role="progressbar"
        aria-valuenow={pct ?? undefined}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={`${progress.label} progress`}
      >
        <div
          data-progress-fill
          data-progress-pct={pct ?? undefined}
          className={pct == null ? "h-full w-1/3 animate-pulse rounded-full bg-[#6FB39F]" : "h-full rounded-full bg-[#6FB39F] transition-[width] duration-300"}
          style={pct == null ? undefined : { width: `${pct}%` }}
        />
      </div>
      <p className="mt-0.5 text-[10.5px] text-[var(--dim)]">
        {pct == null ? `${formatDoneTotal(progress)} · elapsed ${formatSeconds(elapsedS)}` : formatDoneTotal(progress)}
        {formatRate(progress) ? ` · ${formatRate(progress)}` : ""}
      </p>
    </div>
  );
}

function StageRow({ stage, progress, elapsedS }: { stage: StageSnapshot; progress: ProgressValue | null; elapsedS: number }) {
  const showProgress = stage.state === "running" && progress != null && progress.stage === stage.id;
  return (
    <li data-stage-row data-stage-id={stage.id} data-stage-state={stage.state} className="flex flex-col py-0.5">
      <div className="flex items-center gap-2 text-sm">
        <StageIcon state={stage.state} />
        <span className={stage.state === "pending" ? "text-[var(--dim)]" : "text-[var(--on)]"}>{stage.label}</span>
        <span className="ml-auto text-[10.5px] text-[var(--dim)]">
          {stage.state === "done" && stage.duration_s != null ? formatSeconds(stage.duration_s) : null}
          {stage.state === "failed" ? "failed" : null}
        </span>
      </div>
      {showProgress && <StageProgressBar progress={progress} elapsedS={elapsedS} />}
    </li>
  );
}

/** The /new progress view: watches one indexing job via useJobProgress and
 * routes to the map on completion. This is most of the first-run
 * experience (django takes minutes, and now shows exactly why: a real
 * per-stage timeline, a progress bar for the stage in flight, and an ETA
 * range instead of a four-step skeleton), and every state names what's
 * happening rather than falling back to a meaningless spinner. */
export function IndexJobView() {
  const search = useSearch({ strict: false }) as { job?: string; slug?: string };
  const navigate = useNavigate();
  const { job, connection, error: transportError } = useJobProgress(search.job);
  const [retrying, setRetrying] = useState(false);
  const [retryError, setRetryError] = useState<string | null>(null);
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [cancelError, setCancelError] = useState<string | null>(null);

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

  // A job that finishes (on its own, or via a cancel this view itself just
  // issued) on the next snapshot no longer needs a pending confirmation.
  useEffect(() => {
    if (job && isTerminalStatus(job.status)) setConfirmCancel(false);
  }, [job]);

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

  async function doCancel() {
    if (!job || cancelling) return;
    setCancelling(true);
    setCancelError(null);
    try {
      // The SSE/poll loop in useJobProgress delivers the resulting terminal
      // snapshot right behind this; nothing else to do with the response.
      await cancelJob(job.job_id);
    } catch (err) {
      setCancelError(err instanceof Error ? err.message : String(err));
    } finally {
      setCancelling(false);
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
    const cancelled = job.error_code === "cancelled";
    // docs/API.md: "Existing clients should render unknown error codes as a
    // generic failure" -- so everything that isn't the two codes this view
    // gives its own wording to (cancelled, worker_crashed) falls through to
    // job.error, the server's own human text, unchanged.
    const crashed = job.error_code === "worker_crashed";
    const message = cancelled
      ? "Cancelled"
      : crashed
        ? `The indexer stopped during ${lastActiveStageLabel(job)} — the repository may be too large for this server.`
        : (job.error ?? "indexing failed.");
    return (
      <Centered>
        <h1 className="font-sans text-lg font-semibold text-[var(--on)]">{slug}</h1>
        <div
          data-job-failure
          data-job-failure-code={job.error_code ?? ""}
          className={`max-w-md rounded-md border px-4 py-3 text-sm ${
            cancelled
              ? "border-[var(--rule)] bg-[var(--chrome2)] text-[var(--on)]"
              : "border-[var(--hot)] bg-[color-mix(in_srgb,var(--hot)_12%,transparent)] text-[var(--hot)]"
          }`}
        >
          <p>{message}</p>
        </div>
        {retryError && <p className="text-xs text-[var(--hot)]">retry failed: {retryError}</p>}
        <div className="flex gap-2">
          <Button size="sm" variant="outline" onClick={() => void retry()} disabled={retrying}>
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

  return (
    <Centered>
      <h1 className="font-sans text-lg font-semibold text-[var(--on)]">{slug}</h1>

      {job.status === "queued" ? (
        <p className="text-sm text-[var(--dim)]" data-queued-text>
          {job.eta_start_s != null ? formatStartsIn(job.eta_start_s) : "queued"}
          {job.queue_position != null ? ` · #${job.queue_position} in queue` : ""}
        </p>
      ) : (
        <>
          <p className="text-sm text-[var(--dim)]">{job.stage}</p>
          <ol className="flex w-full max-w-sm flex-col gap-0.5" data-stage-timeline>
            {job.stages.map((stage) => (
              <StageRow key={stage.id} stage={stage} progress={job.progress} elapsedS={job.elapsed_s} />
            ))}
          </ol>
          <div className="flex flex-col items-center gap-0.5">
            <p className="text-[11px] text-[var(--dim)]">elapsed {formatSeconds(job.elapsed_s)}</p>
            {job.eta && (
              <p className="text-[11px] text-[var(--on)]" data-eta-range>
                {formatEtaRange(job.eta.low_s, job.eta.high_s)}
                <span className="ml-1 text-[10px] text-[var(--dim)]">
                  ({job.eta.basis === "model" ? "rough estimate" : "estimate"})
                </span>
              </p>
            )}
          </div>
        </>
      )}

      <div className="flex flex-col items-center gap-1.5" data-cancel-area>
        {confirmCancel ? (
          <div className="flex items-center gap-2 text-sm">
            <span className="text-[var(--dim)]">cancel this job?</span>
            <Button size="sm" variant="outline" onClick={() => void doCancel()} disabled={cancelling}>
              {cancelling ? "cancelling…" : "yes, cancel"}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirmCancel(false)} disabled={cancelling}>
              no
            </Button>
          </div>
        ) : (
          <Button size="sm" variant="ghost" onClick={() => setConfirmCancel(true)}>
            cancel
          </Button>
        )}
        {cancelError && <p className="text-xs text-[var(--hot)]">cancel failed: {cancelError}</p>}
      </div>

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
