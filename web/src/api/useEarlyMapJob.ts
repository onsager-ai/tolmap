import { useState } from "react";
import { useJobProgress } from "./useJobProgress";

/** What the map view needs to know about the job whose map it opened early
 * (docs/UX.md §12, owner decision "Open early", 2026-09-27): the indexing
 * page hands over as soon as the service serves the map (`map_ready`), with
 * `?job=<id>` on the map's URL, and the job's Detail phase is still running.
 */
export interface EarlyMapJob {
  /** The job's commit. Sticky for the life of the view, so the map and its
   * symbols stay pinned to it after the `job` param is dropped. */
  commit: string | undefined;
  /** A `job` param is set and its first snapshot has not arrived: the map
   * fetch waits, since without the commit it would fetch the latest map,
   * which may be an older commit's. */
  waiting: boolean;
  /** The job is still running its Detail phase: symbols are not served yet. */
  detailPending: boolean;
  /** The job ended without its symbols; the service's message. */
  detailFailed: string | null;
  /** The job ended (done or failed); the view drops its `job` param. */
  settled: boolean;
}

export function useEarlyMapJob(jobId: string | undefined): EarlyMapJob {
  // useJobProgress keeps its last snapshot when `jobId` goes away, which is
  // what lets the outcome outlive the `job` param below.
  const { job, error } = useJobProgress(jobId);
  // The first commit seen is kept (state adjusted while rendering, React's
  // pattern for state derived from a changing input).
  const [commit, setCommit] = useState<string | undefined>(undefined);
  if (commit === undefined && job?.commit) setCommit(job.commit);

  const terminal = job?.status === "done" || job?.status === "failed";
  return {
    commit: commit ?? job?.commit ?? undefined,
    // A job the service no longer knows (an old link) is not waited on: the
    // latest map is then the right one to show.
    waiting: !!jobId && !job && !error,
    detailPending: !!job && !terminal,
    detailFailed: job?.status === "failed" ? (job.error ?? "the job failed") : null,
    settled: !!job && terminal,
  };
}
