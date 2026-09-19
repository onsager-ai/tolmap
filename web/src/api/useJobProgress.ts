import { useEffect, useState } from "react";
import { getJob, jobEventsUrl, type JobStatus } from "./client";

export type JobConnection = "connecting" | "sse" | "poll";

function isTerminal(s: JobStatus | null): boolean {
  return s?.status === "done" || s?.status === "failed";
}

/** Drives the progress view: GET the current status once (so a job that's
 * already done/failed resolves immediately, e.g. after a page refresh),
 * then GET /api/jobs/{id}/events over SSE for live updates, falling back to
 * polling GET /api/jobs/{id} every 2s if the stream errors or never opens
 * (milestone brief, "A progress view"). SSE is closed rather than left to
 * auto-reconnect on error, so the poll fallback doesn't race a resurrected
 * stream for the same update. */
export function useJobProgress(jobId: string | undefined) {
  const [job, setJob] = useState<JobStatus | null>(null);
  const [connection, setConnection] = useState<JobConnection>("connecting");
  const [fatalError, setFatalError] = useState<string | null>(null);

  useEffect(() => {
    if (!jobId) return;
    const id = jobId; // narrowed once, closed over below instead of `jobId`
    let cancelled = false;
    let pollTimer: ReturnType<typeof setInterval> | undefined;
    setJob(null);
    setConnection("connecting");
    setFatalError(null);

    function stopPolling() {
      if (pollTimer) {
        clearInterval(pollTimer);
        pollTimer = undefined;
      }
    }

    function startPolling() {
      if (pollTimer) return;
      setConnection("poll");
      const tick = async () => {
        try {
          const s = await getJob(id);
          if (cancelled) return;
          setJob(s);
          if (isTerminal(s)) stopPolling();
        } catch (err) {
          if (!cancelled) setFatalError(err instanceof Error ? err.message : String(err));
        }
      };
      void tick();
      pollTimer = setInterval(tick, 2000);
    }

    getJob(id)
      .then((s) => {
        if (!cancelled) setJob(s);
      })
      .catch((err) => {
        if (!cancelled) setFatalError(err instanceof Error ? err.message : String(err));
      });

    let es: EventSource | undefined;
    if (typeof EventSource !== "undefined") {
      es = new EventSource(jobEventsUrl(id));
      es.onopen = () => {
        if (!cancelled) setConnection("sse");
      };
      es.onmessage = (ev) => {
        if (cancelled) return;
        try {
          const s = JSON.parse(ev.data) as JobStatus;
          setJob(s);
          if (isTerminal(s)) es?.close();
        } catch {
          // A malformed frame is not fatal - keep listening (or polling).
        }
      };
      es.onerror = () => {
        es?.close();
        if (!cancelled) startPolling();
      };
    } else {
      startPolling();
    }

    return () => {
      cancelled = true;
      es?.close();
      stopPolling();
    };
  }, [jobId]);

  return { job, connection, error: fatalError };
}
