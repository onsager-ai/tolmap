import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { useJobProgress } from "@/api/useJobProgress";
import { postIndexJob, cancelJob, ApiRequestError } from "@/api/client";
import type { JobSnapshot } from "@/types";
import type { ProgressValue } from "@bindings/ProgressValue";
import type { StageId } from "@bindings/StageId";
import { ThemeToggle } from "@/components/ThemeToggle";
import { useIsNarrow } from "@/hooks/useIsNarrow";
import {
  foundSoFar,
  formatDuration,
  formatEtaRange,
  formatStartsIn,
  formatStepCount,
  ordinal,
  skippedStages,
  stagePosition,
  summarisePhases,
  type PhaseView,
} from "@/lib/indexPhases";
import { classifyFailure, reportIssueUrl, type FailureAction, type FailureView } from "@/lib/jobFailure";

// docs/UX.md §6: the indexing, queue and failure pages. The owner called the
// page this replaces "quite poorly designed" (2026-09-27): 22 stage rows in
// pipeline vocabulary, the ETA in small print at the bottom, and "try again"
// offered on refusals that would only repeat. This one leads with how long
// (§6.1), shows four plain phases (§6.2; the mapping is lib/indexPhases.ts),
// keeps the stage list behind "Technical details" (principle 9), and picks a
// failure page by error code (§6.5, lib/jobFailure.ts).

/** Ticks once a second while `active`, so "Running 1m 12s" and "waiting
 * 0m 40s" keep moving through a quiet stage (the service updates
 * `elapsed_s` only on worker events). */
function useTick(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [active]);
  return now;
}

// ---------- icons ----------

const ICON = { fill: "none", stroke: "currentColor", strokeLinecap: "round" as const, strokeLinejoin: "round" as const };

function ChevronLeft() {
  return (
    <svg width="20" height="20" viewBox="0 0 20 20" {...ICON} strokeWidth={2} aria-hidden>
      <path d="M12.5 4.5 7 10l5.5 5.5" />
    </svg>
  );
}

/** A disclosure's state, turned by its `details` (a `group`) opening. */
function Chevron() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" {...ICON} strokeWidth={1.8} aria-hidden className="flex-none text-[var(--dim)] transition-transform group-open:rotate-180">
      <path d="m4 6 4 4 4-4" />
    </svg>
  );
}

function CopyIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 18 18" {...ICON} strokeWidth={1.6} aria-hidden>
      <rect x="6" y="6" width="9" height="9" rx="2" />
      <path d="M12 3.5H5A1.5 1.5 0 0 0 3.5 5v7" />
    </svg>
  );
}

function PhaseIcon({ state }: { state: PhaseView["state"] }) {
  const box = "inline-flex h-7 w-7 flex-none items-center justify-center rounded-full";
  if (state === "done") {
    return (
      <span aria-hidden className={`${box} bg-[var(--accent)] text-[var(--on-accent)]`}>
        <svg width="14" height="14" viewBox="0 0 14 14" {...ICON} strokeWidth={2.2}>
          <path d="m3 7.2 2.6 2.6L11 4.4" />
        </svg>
      </span>
    );
  }
  if (state === "failed") {
    return (
      <span aria-hidden className={`${box} bg-[var(--link-out)] text-[var(--on-accent)]`}>
        <svg width="12" height="12" viewBox="0 0 12 12" {...ICON} strokeWidth={2.2}>
          <path d="m3 3 6 6M9 3 3 9" />
        </svg>
      </span>
    );
  }
  if (state === "running") {
    return (
      <span aria-hidden className={`${box} relative border-2 border-[var(--rule)]`}>
        <span className="tolmap-spin absolute -inset-[2px] rounded-full border-2 border-transparent border-t-[var(--accent)]" />
      </span>
    );
  }
  return <span aria-hidden className={`${box} border-2 border-[var(--rule)]`} />;
}

/** The failure page's tile (§6.5): a map with a question mark for a refusal
 * that will repeat, a stop for a cancel, a clock for a failure that may pass,
 * a warning for a bug. */
function FailureIcon({ cls }: { cls: FailureView["cls"] }) {
  const tone =
    cls === "deterministic" ? "var(--warn)" : cls === "user" ? "var(--dim)" : cls === "bug" ? "var(--link-out)" : "var(--accent)";
  return (
    <span
      aria-hidden
      data-failure-icon
      className="inline-flex h-14 w-14 items-center justify-center rounded-[14px]"
      style={{ color: tone, background: `color-mix(in srgb, ${tone} 16%, transparent)` }}
    >
      <svg width="28" height="28" viewBox="0 0 28 28" {...ICON} strokeWidth={2}>
        {cls === "deterministic" ? (
          <>
            <path d="M4 7.5 10 5l8 3 6-2.5v15L18 23l-8-3-6 2.5z" />
            <path d="M12.2 11.3a2.4 2.4 0 1 1 3.3 2.2c-.8.3-1.3.9-1.3 1.7" />
            <path d="M14.2 18.1h.01" />
          </>
        ) : cls === "user" ? (
          <>
            <circle cx="14" cy="14" r="9" />
            <path d="M10.5 10.5h7v7h-7z" />
          </>
        ) : cls === "bug" ? (
          <>
            <path d="M14 4.5 25 23H3z" />
            <path d="M14 11v5.5M14 19.6h.01" />
          </>
        ) : (
          <>
            <circle cx="14" cy="14" r="9" />
            <path d="M14 9v5l3.5 2" />
          </>
        )}
      </svg>
    </span>
  );
}

// ---------- page frame ----------

/** The bar across the top: "‹ tolmap" back to Home on a phone (artboards
 * "Indexing · running" and "Couldn't map"), the brand and the theme button
 * on desktop (artboard "Desktop indexing"). */
function PageFrame({ narrow, children }: { narrow: boolean; children: ReactNode }) {
  return (
    <div className="h-full overflow-y-auto bg-[var(--chrome)] text-[var(--on)]" data-index-page>
      {narrow ? (
        <div className="px-3 pt-[max(env(safe-area-inset-top),12px)]">
          <Link to="/" className="inline-flex h-11 items-center gap-1.5 rounded-[12px] pl-1 pr-3 text-row text-[var(--on)]" aria-label="tolmap home">
            <ChevronLeft />
            tolmap
          </Link>
        </div>
      ) : (
        <header className="flex h-14 items-center justify-between border-b border-[var(--rule)] px-5">
          <Link to="/" className="text-row text-[var(--on)]">
            tolmap
          </Link>
          <ThemeToggle />
        </header>
      )}
      {children}
    </div>
  );
}

function Slug({ slug }: { slug: string | undefined }) {
  if (!slug) return null;
  return <p className="break-all font-mono text-body text-[var(--dim)]">{slug}</p>;
}

// ---------- the running and queued page ----------

function PhaseRow({ phase }: { phase: PhaseView }) {
  const running = phase.state === "running";
  const step = phase.current;
  const pct = step && !step.indeterminate && step.done != null && step.total ? Math.min(100, (step.done / step.total) * 100) : null;
  const count = step ? formatStepCount(step) : null;
  return (
    <li
      data-phase-row
      data-phase-id={phase.id}
      data-phase-state={phase.state}
      className={`flex gap-3.5 px-4 py-3.5 [&:not(:first-child)]:border-t [&:not(:first-child)]:border-[var(--rule)] ${
        running ? "bg-[color-mix(in_srgb,var(--accent)_7%,transparent)]" : ""
      }`}
    >
      <PhaseIcon state={phase.state} />
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-3">
          <p className={`text-row ${phase.state === "pending" ? "text-[var(--dim)]" : "text-[var(--on)]"}`}>{phase.title}</p>
          <span className="ml-auto font-mono text-meta tabular-nums text-[var(--dim)]">
            {phase.state === "done" && phase.durationS != null ? formatDuration(phase.durationS) : null}
            {phase.state === "failed" ? "stopped" : null}
          </span>
        </div>
        <p className="text-small text-[var(--dim)]">{phase.meaning}</p>
        {running && step && (
          <div className="mt-3" data-current-step data-stage-id={step.stage}>
            <div
              className="h-1.5 w-full overflow-hidden rounded-full bg-[var(--rule)]"
              role="progressbar"
              aria-label={`${step.text} progress`}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={pct ?? undefined}
            >
              <div
                data-progress-fill
                data-progress-pct={pct == null ? undefined : pct.toFixed(1)}
                className={
                  pct == null
                    ? "tolmap-indeterminate h-full w-1/3 rounded-full bg-[var(--accent)]"
                    : "h-full rounded-full bg-[var(--accent)] transition-[width] duration-300"
                }
                style={pct == null ? undefined : { width: `${pct}%` }}
              />
            </div>
            <div className="mt-2 flex flex-wrap items-baseline justify-between gap-x-3 gap-y-0.5">
              <span className="text-small text-[var(--on)]">{step.text}</span>
              {count && <span className="font-mono text-meta tabular-nums text-[var(--dim)]">{count}</span>}
            </div>
          </div>
        )}
      </div>
    </li>
  );
}

/** §6.4: the full stage list grouped under the four phases, with durations,
 * "stage 6 of 22" and the raw `stage` string. Open on desktop, closed on
 * phones. */
function TechnicalDetails({ job, phases, open, card }: { job: JobSnapshot; phases: PhaseView[]; open: boolean; card: boolean }) {
  const position = stagePosition(job);
  const skipped = skippedStages(job.stages);
  return (
    <details
      open={open}
      data-tech-details
      className={`group ${card ? "rounded-[14px] border border-[var(--rule)] bg-[var(--chrome2)]" : "border-t border-[var(--rule)]"}`}
    >
      <summary className={`flex min-h-11 cursor-pointer select-none items-center justify-between gap-3 ${card ? "border-b border-[var(--rule)] px-5 py-3" : "py-3"}`}>
        <span className="text-row">Technical details</span>
        <span className="flex items-center gap-2.5">
          {position && (
            <span className="font-mono text-meta text-[var(--dim)]" data-stage-position>
              stage {position.index} of {position.count}
            </span>
          )}
          <Chevron />
        </span>
      </summary>
      <div className={card ? "px-5 py-4" : "pb-4"}>
        <p className="mb-3 break-words font-mono text-meta text-[var(--dim)]" data-raw-stage>
          {job.stage}
        </p>
        {phases.map((phase) => (
          <section key={phase.id} className="mb-3 last:mb-0">
            <h3 className="text-label uppercase text-[var(--dim)]">{phase.title}</h3>
            <ol className="mt-1" data-stage-timeline>
              {phase.stages.map((stage) => (
                <li
                  key={stage.id}
                  data-stage-row
                  data-stage-id={stage.id}
                  data-stage-state={stage.state}
                  className="flex items-baseline justify-between gap-3 py-0.5 text-small"
                >
                  <span
                    className={
                      stage.state === "running"
                        ? "text-[var(--accent)]"
                        : stage.state === "failed"
                          ? "text-[var(--link-out)]"
                          : stage.state === "done"
                            ? "text-[var(--on)]"
                            : "text-[var(--dim)]"
                    }
                  >
                    {stage.label}
                  </span>
                  <span className="font-mono text-meta tabular-nums text-[var(--dim)]">
                    {stage.state === "done" && stage.duration_s != null
                      ? formatDuration(stage.duration_s)
                      : stage.state === "running"
                        ? "running"
                        : stage.state === "failed"
                          ? "failed"
                          : skipped.has(stage.id)
                            ? "skipped"
                            : ""}
                  </span>
                </li>
              ))}
            </ol>
          </section>
        ))}
      </div>
    </details>
  );
}

function CopyLink({ narrow }: { narrow: boolean }) {
  const [copied, setCopied] = useState<"yes" | "failed" | null>(null);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(null), 2500);
    return () => clearTimeout(timer);
  }, [copied]);
  async function copy() {
    try {
      await navigator.clipboard.writeText(window.location.href);
      setCopied("yes");
    } catch {
      setCopied("failed");
    }
  }
  return (
    <div className={narrow ? "flex flex-col gap-3" : "flex min-w-0 items-center gap-5"}>
      <button
        type="button"
        data-copy-link
        onClick={() => void copy()}
        className={`inline-flex flex-none items-center justify-center gap-2.5 rounded-[14px] border border-[var(--rule)] bg-[var(--chrome2)] text-row text-[var(--on)] hover:bg-[color-mix(in_srgb,var(--on)_6%,var(--chrome2))] ${
          narrow ? "h-[52px] w-full" : "h-11 px-5"
        }`}
      >
        <CopyIcon />
        <span aria-live="polite">{copied === "yes" ? "Link copied" : copied === "failed" ? "Copy the address bar" : "Copy link"}</span>
      </button>
      <p className={`text-small text-[var(--dim)] ${narrow ? "text-center" : ""}`} data-copy-note>
        You can close this page. The link keeps working, and the map opens here when it's ready.
      </p>
    </div>
  );
}

function CancelArea({ job, queued }: { job: JobSnapshot; queued: boolean }) {
  const [confirm, setConfirm] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function doCancel() {
    if (cancelling) return;
    setCancelling(true);
    setError(null);
    try {
      // The SSE/poll loop in useJobProgress delivers the resulting terminal
      // snapshot right behind this; nothing else to do with the response.
      await cancelJob(job.job_id);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setCancelling(false);
    }
  }
  return (
    <div className="flex flex-none flex-col items-center gap-2" data-cancel-area>
      {confirm ? (
        <div className="flex flex-wrap items-center justify-center gap-2 text-small" data-cancel-confirm>
          <span className="text-[var(--dim)]">{queued ? "Leave the queue?" : "Stop mapping?"}</span>
          <button
            type="button"
            onClick={() => void doCancel()}
            disabled={cancelling}
            className="h-11 rounded-[12px] border border-[var(--link-out)] px-4 text-[var(--link-out)] disabled:opacity-50"
          >
            {cancelling ? "Stopping…" : queued ? "Yes, leave" : "Yes, cancel"}
          </button>
          <button
            type="button"
            onClick={() => setConfirm(false)}
            disabled={cancelling}
            className="h-11 rounded-[12px] px-4 text-[var(--on)] hover:bg-[color-mix(in_srgb,var(--on)_8%,transparent)]"
          >
            {queued ? "Stay" : "Keep mapping"}
          </button>
        </div>
      ) : (
        <button type="button" onClick={() => setConfirm(true)} className="h-11 px-4 text-body text-[var(--link-out)]" data-cancel-button>
          {queued ? "Leave the queue" : "Cancel mapping"}
        </button>
      )}
      {error && <p className="text-meta text-[var(--link-out)]">Couldn't cancel: {error}</p>}
    </div>
  );
}

function ProgressPage({
  job,
  seen,
  narrow,
}: {
  job: JobSnapshot;
  seen: Partial<Record<StageId, ProgressValue>>;
  narrow: boolean;
}) {
  const queued = job.status === "queued";
  const now = useTick(true);
  // The tick at which the snapshot on screen arrived, so the elapsed time
  // runs on from the service's `elapsed_s` between snapshots.
  const [received, setReceived] = useState({ job, at: now });
  if (received.job !== job) setReceived({ job, at: now });
  const receivedAt = received.job === job ? received.at : now;
  const phases = useMemo(() => summarisePhases(job), [job]);
  // §6.3: from the last progress value seen for each stage
  // (useJobProgress keeps them, per snapshot received).
  const found = useMemo(() => foundSoFar(seen), [seen]);

  const elapsedS = job.elapsed_s + Math.max(0, (now - receivedAt) / 1000);
  const waitingS = Math.max(0, (now - Date.parse(job.started_at)) / 1000);
  const basis = job.eta?.basis === "model" ? "rough estimate" : "estimate";
  const title = queued
    ? job.eta_start_s != null
      ? formatStartsIn(job.eta_start_s)
      : "Waiting to start"
    : job.eta
      ? formatEtaRange(job.eta.low_s, job.eta.high_s)
      : "Working out how long";
  const startedAt = new Date(job.started_at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

  const header = (
    <>
      <Slug slug={job.slug} />
      <h1 className="mt-1 text-title tabular-nums text-[var(--on)]" {...(queued ? { "data-queued-title": "" } : { "data-eta-range": "" })}>
        {title}
      </h1>
      {queued ? (
        <p className="mt-2 text-small text-[var(--dim)]" data-queued-text>
          {job.queue_position != null && <span className="text-[var(--on)]">{ordinal(job.queue_position)} in line · </span>}
          waiting <span className="font-mono text-[var(--on)]">{formatDuration(waitingS, true)}</span>
          {job.eta_start_s != null && ` · ${basis}`}
        </p>
      ) : (
        <p className="mt-2 text-small text-[var(--dim)]" data-running-text>
          Running <span className="font-mono text-[var(--on)]">{formatDuration(elapsedS, true)}</span> · started {startedAt}
          {job.eta && ` · ${basis}`}
        </p>
      )}
      <div className="mt-6 grid grid-cols-4 gap-2" data-phase-bar aria-hidden>
        {phases.map((phase) => (
          <div key={phase.id} className="h-1.5 overflow-hidden rounded-full bg-[var(--rule)]" data-phase-segment={phase.id}>
            <div
              className={`h-full rounded-full ${phase.state === "failed" ? "bg-[var(--link-out)]" : "bg-[var(--accent)]"} transition-[width] duration-300`}
              style={{ width: `${Math.round(phase.fraction * 100)}%` }}
            />
          </div>
        ))}
      </div>
    </>
  );

  const phaseList = (
    <ol className="mt-5 overflow-hidden rounded-[14px] border border-[var(--rule)]" data-phase-list>
      {phases.map((phase) => (
        <PhaseRow key={phase.id} phase={phase} />
      ))}
    </ol>
  );

  const foundLine =
    found.length > 0 ? (
      <p className="mt-5 text-small text-[var(--dim)]" data-found-so-far>
        Found so far:{" "}
        {found.map((fact, i) => (
          <span key={fact}>
            {i > 0 && " · "}
            <span className="text-[var(--on)]">{fact}</span>
          </span>
        ))}
      </p>
    ) : null;

  if (narrow) {
    return (
      <main className="flex min-h-[calc(100%-56px)] flex-col px-5 pb-[max(env(safe-area-inset-bottom),16px)] pt-4" data-job-state={job.status}>
        {header}
        {phaseList}
        {foundLine}
        <div className="mt-6">
          <CopyLink narrow />
        </div>
        {!queued && (
          <div className="mt-6">
            <TechnicalDetails job={job} phases={phases} open={false} card={false} />
          </div>
        )}
        <div className="mt-auto pt-8">
          <CancelArea job={job} queued={queued} />
        </div>
      </main>
    );
  }
  return (
    <main className="mx-auto grid w-full max-w-[1180px] grid-cols-[minmax(0,1fr)_minmax(300px,380px)] gap-12 px-8 py-12" data-job-state={job.status}>
      <div className="min-w-0">
        {header}
        {phaseList}
        {foundLine}
        <div className="mt-6 flex items-center justify-between gap-6">
          <CopyLink narrow={false} />
          <CancelArea job={job} queued={queued} />
        </div>
      </div>
      <div className="min-w-0">{!queued && <TechnicalDetails job={job} phases={phases} open card />}</div>
    </main>
  );
}

// ---------- the failure page ----------

function FailurePage({ job, slug, narrow }: { job: JobSnapshot; slug: string | undefined; narrow: boolean }) {
  const navigate = useNavigate();
  const view = classifyFailure(job);
  const [retrying, setRetrying] = useState(false);
  const [retryError, setRetryError] = useState<string | null>(null);
  const [waitUntil, setWaitUntil] = useState<number | null>(null);
  const now = useTick(waitUntil != null);
  const waitS = waitUntil != null ? Math.max(0, Math.ceil((waitUntil - now) / 1000)) : 0;
  const target = job.slug || slug;

  async function retry() {
    if (!target || retrying || waitS > 0) return;
    setRetrying(true);
    setRetryError(null);
    try {
      const res = await postIndexJob({ repo: target });
      if (res.status === "done") {
        const [owner, repo] = res.slug.split("/");
        await navigate({ to: "/$owner/$repo", params: { owner, repo }, search: { geo: "r", layer: "d" } });
      } else {
        await navigate({ to: "/new", search: { job: res.job_id, slug: res.slug }, replace: true });
      }
    } catch (err) {
      // §6.5: "Try again (after Retry-After when present)".
      if (err instanceof ApiRequestError && err.retryAfterS != null) setWaitUntil(Date.now() + err.retryAfterS * 1000);
      setRetryError(
        err instanceof ApiRequestError && (err.code === "busy" || err.code === "rate_limited")
          ? "tolmap is still busy."
          : err instanceof Error
            ? err.message
            : String(err),
      );
    } finally {
      setRetrying(false);
    }
  }

  const primary = "bg-[var(--accent)] text-[var(--on-accent)] hover:opacity-90 disabled:opacity-60";
  const secondary =
    "border border-[var(--rule)] bg-[var(--chrome2)] text-[var(--on)] hover:bg-[color-mix(in_srgb,var(--on)_6%,var(--chrome2))] disabled:opacity-60";
  const base = `inline-flex items-center justify-center rounded-[14px] text-row ${narrow ? "h-[52px] w-full" : "h-12 px-6"}`;

  function action(kind: FailureAction, index: number) {
    const className = `${base} ${index === 0 ? primary : secondary}`;
    switch (kind) {
      case "retry":
      case "again":
        return (
          <button key={kind} type="button" data-failure-action={kind} className={className} onClick={() => void retry()} disabled={retrying || waitS > 0}>
            {retrying ? "Starting…" : waitS > 0 ? `Try again in ${waitS}s` : kind === "again" ? "Map it again" : "Try again"}
          </button>
        );
      case "report":
        return (
          <a key={kind} data-failure-action={kind} className={className} href={reportIssueUrl(job)} target="_blank" rel="noopener noreferrer">
            Report this repository
          </a>
        );
      case "another":
        return (
          <Link key={kind} data-failure-action={kind} className={className} to="/">
            Map another repository
          </Link>
        );
      case "check-name":
        return (
          <Link key={kind} data-failure-action={kind} className={className} to="/">
            Check the name
          </Link>
        );
      case "home":
        return (
          <Link key={kind} data-failure-action={kind} className={className} to="/">
            Home
          </Link>
        );
    }
  }

  return (
    <main
      className={narrow ? "flex min-h-[calc(100%-56px)] flex-col px-5 pb-[max(env(safe-area-inset-bottom),20px)] pt-6" : "mx-auto w-full max-w-[680px] px-8 py-14"}
      data-job-failure
      data-job-failure-code={job.error_code ?? ""}
      data-failure-class={view.cls}
    >
      <FailureIcon cls={view.cls} />
      <div className="mt-6">
        <Slug slug={target} />
      </div>
      <h1 className="mt-1 text-title text-[var(--on)]" data-failure-title>
        {view.title}
      </h1>
      <div className="mt-4 flex flex-col gap-3 text-body text-[var(--dim)]" data-job-failure-explanation>
        {view.explanation.map((paragraph) => (
          <p key={paragraph}>{paragraph}</p>
        ))}
      </div>
      {job.error && (
        <details open className="group mt-6 overflow-hidden rounded-[14px] border border-[var(--rule)] bg-[var(--chrome2)]" data-job-failure-evidence>
          <summary className="flex min-h-12 cursor-pointer select-none items-center justify-between gap-3 px-4 text-row">
            What tolmap saw
            <Chevron />
          </summary>
          {/* Evidence names paths with no spaces to break at; let them wrap
              anywhere rather than push a phone into sideways scrolling. */}
          <pre className="whitespace-pre-wrap border-t border-[var(--rule)] bg-[var(--chrome)] px-4 py-3 font-mono text-small text-[var(--on)] [overflow-wrap:anywhere]">
            {job.error}
          </pre>
        </details>
      )}
      {retryError && (
        <p className="mt-4 text-small text-[var(--link-out)]" data-retry-error>
          {retryError}
        </p>
      )}
      <div className={narrow ? "mt-auto flex flex-col gap-3 pt-10" : "mt-8 flex flex-wrap gap-3"} data-failure-actions>
        {view.actions.map((kind, index) => action(kind, index))}
      </div>
    </main>
  );
}

// ---------- the route ----------

/** /new?job=<id>: watches one indexing job via useJobProgress and hands over
 * to the map as soon as the map is served (`map_ready`, docs/UX.md §12), or
 * when the job is done. */
export function IndexJobView() {
  const search = useSearch({ strict: false }) as { job?: string; slug?: string };
  const navigate = useNavigate();
  const narrow = useIsNarrow();
  const { job, error: transportError, seen } = useJobProgress(search.job);

  const status = job?.status;
  const mapReady = job?.map_ready === true;

  // docs/UX.md §12 ("Open early", owner 2026-09-27): the map opens when the
  // Map phase finishes; the map view keeps watching the job (its `job`
  // search param) and fills in classes and functions when Detail lands.
  useEffect(() => {
    if (!job || !status) return;
    if (status !== "done" && !(mapReady && status !== "failed")) return;
    const [owner, repo] = job.slug.split("/");
    if (!owner || !repo) return;
    navigate({
      to: "/$owner/$repo",
      params: { owner, repo },
      search: status === "done" ? { geo: "r", layer: "d" } : { geo: "r", layer: "d", job: job.job_id },
      replace: true,
    });
  }, [status, mapReady, job, navigate]);

  if (!search.job) {
    return (
      <PageFrame narrow={narrow}>
        <Notice>
          <p className="text-body text-[var(--dim)]">No mapping job to show.</p>
          <Link to="/" className="text-body text-[var(--accent)] underline underline-offset-2">
            Map a repository
          </Link>
        </Notice>
      </PageFrame>
    );
  }

  if (transportError && !job) {
    return (
      <PageFrame narrow={narrow}>
        <Notice>
          <Slug slug={search.slug} />
          <p className="text-body text-[var(--link-out)]">Couldn't reach the mapping service: {transportError}</p>
          <Link to="/" className="text-body text-[var(--accent)] underline underline-offset-2">
            Home
          </Link>
        </Notice>
      </PageFrame>
    );
  }

  if (!job) {
    return (
      <PageFrame narrow={narrow}>
        <Notice>
          <Slug slug={search.slug} />
          <p className="text-body text-[var(--dim)]">Connecting…</p>
        </Notice>
      </PageFrame>
    );
  }

  return (
    <PageFrame narrow={narrow}>
      {job.status === "failed" ? (
        <FailurePage job={job} slug={search.slug} narrow={narrow} />
      ) : (
        <ProgressPage job={job} seen={seen} narrow={narrow} />
      )}
    </PageFrame>
  );
}

function Notice({ children }: { children: ReactNode }) {
  return <main className="mx-auto flex w-full max-w-[680px] flex-col gap-3 px-5 py-10">{children}</main>;
}
