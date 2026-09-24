import { useEffect, useState } from "react";
import type { JobSnapshot } from "@/types";
import { getJob, jobEventsUrl } from "./client";

export type JobConnection = "connecting" | "sse" | "poll";

function isTerminal(s: JobSnapshot | null): boolean {
  return s?.status === "done" || s?.status === "failed";
}

// Bounded backoff before giving up on SSE and falling back to polling —
// three quick retries (0.5s/1.5s/4s) covers a blip; anything that still
// won't reconnect after that is polling's job, not a tight retry loop's.
const SSE_RECONNECT_DELAYS_MS = [500, 1500, 4000];

/** Drives the progress view: GET the current status once (so a job that's
 * already done/failed resolves immediately, e.g. after a page refresh),
 * then GET /api/jobs/{id}/events over SSE for live updates, falling back to
 * polling GET /api/jobs/{id} every 2s once SSE reconnection is exhausted
 * (milestone brief, "A progress view"; docs/API.md's SSE section).
 *
 * Reconnect, per docs/API.md: "A client that reconnects after a drop should
 * GET /api/jobs/{id} first to catch up, since SSE here does not replay
 * frames sent before the connection opened." Every (re)connect attempt below
 * — the first one and every retry after a drop — goes through that catch-up
 * GET before (re)opening the stream, so a frame missed while disconnected is
 * never silently lost between two SSE frames. */
export function useJobProgress(jobId: string | undefined) {
  const [job, setJob] = useState<JobSnapshot | null>(null);
  const [connection, setConnection] = useState<JobConnection>("connecting");
  const [fatalError, setFatalError] = useState<string | null>(null);

  useEffect(() => {
    if (!jobId) return;
    const id = jobId; // narrowed once, closed over below instead of `jobId`
    let cancelled = false;
    let pollTimer: ReturnType<typeof setInterval> | undefined;
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
    let es: EventSource | undefined;
    let sseAttempts = 0;
    // React state (`job`) is only readable via the render that scheduled
    // this effect, not the latest one — this mirrors it synchronously so
    // onerror below can tell "the job already finished" from "we lost the
    // stream" without a stale closure.
    let lastSnapshot: JobSnapshot | null = null;
    setJob(null);
    setConnection("connecting");
    setFatalError(null);

    function applySnapshot(s: JobSnapshot) {
      lastSnapshot = s;
      setJob(s);
    }

    function stopPolling() {
      if (pollTimer) {
        clearInterval(pollTimer);
        pollTimer = undefined;
      }
    }

    function startPolling() {
      stopReconnecting();
      if (pollTimer) return;
      setConnection("poll");
      const tick = async () => {
        try {
          const s = await getJob(id);
          if (cancelled) return;
          applySnapshot(s);
          if (isTerminal(s)) stopPolling();
        } catch (err) {
          if (!cancelled) setFatalError(err instanceof Error ? err.message : String(err));
        }
      };
      void tick();
      pollTimer = setInterval(tick, 2000);
    }

    function stopReconnecting() {
      if (reconnectTimer) {
        clearTimeout(reconnectTimer);
        reconnectTimer = undefined;
      }
    }

    function catchUpThenConnect() {
      getJob(id)
        .then((s) => {
          if (cancelled) return;
          applySnapshot(s);
          if (isTerminal(s)) return;
          openSse();
        })
        .catch((err) => {
          if (cancelled) return;
          setFatalError(err instanceof Error ? err.message : String(err));
          startPolling();
        });
    }

    function openSse() {
      if (typeof EventSource === "undefined") {
        startPolling();
        return;
      }
      es = new EventSource(jobEventsUrl(id));
      es.onopen = () => {
        if (cancelled) return;
        sseAttempts = 0;
        setConnection("sse");
      };
      es.onmessage = (ev) => {
        if (cancelled) return;
        try {
          const s = JSON.parse(ev.data) as JobSnapshot;
          applySnapshot(s);
          if (isTerminal(s)) es?.close();
        } catch {
          // A malformed frame is not fatal - keep listening (or polling).
        }
      };
      es.onerror = () => {
        es?.close();
        if (cancelled || isTerminal(lastSnapshot)) return;
        if (sseAttempts < SSE_RECONNECT_DELAYS_MS.length) {
          const delay = SSE_RECONNECT_DELAYS_MS[sseAttempts];
          sseAttempts += 1;
          setConnection("connecting");
          reconnectTimer = setTimeout(catchUpThenConnect, delay);
        } else {
          startPolling();
        }
      };
    }

    catchUpThenConnect();

    return () => {
      cancelled = true;
      es?.close();
      stopPolling();
      stopReconnecting();
    };
  }, [jobId]);

  return { job, connection, error: fatalError };
}
