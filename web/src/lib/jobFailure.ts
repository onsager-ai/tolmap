// docs/UX.md §6.5: a failed job's page, chosen by `error_code`. Pure, so the
// classification, the copy and the actions are unit-checked without a browser
// (scripts/check-index-page.ts). The service's own message is never replaced:
// the page shows it verbatim inside "What tolmap saw".
import type { JobSnapshot } from "@bindings/JobSnapshot";
import { phaseOf, phaseTitle } from "./indexPhases";

/** §6.5's classes. Only "deterministic" is a promise about the future: the
 * same repository at the same commit gives the same answer, so trying again
 * is never offered (issue #162, the owner's hindsight report). */
export type FailureClass = "deterministic" | "user" | "input" | "transient" | "bug";

export type FailureAction =
  /** Home, to map another repository. */
  | "another"
  /** A new GitHub issue with the slug and the error filled in. */
  | "report"
  /** POST the same repository again. */
  | "retry"
  /** POST the same repository again, after a cancel. */
  | "again"
  /** Home, to correct the repository name. */
  | "check-name"
  | "home";

export interface FailureView {
  code: string;
  cls: FailureClass;
  title: string;
  /** One or two short paragraphs. */
  explanation: string[];
  /** In order; the first is the primary button. */
  actions: FailureAction[];
}

const LANGUAGE: Record<string, string> = { py: "Python", go: "Go", ts: "TypeScript", rs: "Rust", rust: "Rust" };

/** "It found 972 Python files" from the detection evidence's own head
 * ("py at . (972 files, low confidence) -- ..."), or null when the evidence
 * does not start that way. Only what the service said, never a guess. */
export function foundInEvidence(error: string | null): string | null {
  if (!error) return null;
  const m = /^\s*([a-z]+) at \S+ \((\d[\d,]*) files?\b/.exec(error);
  if (!m) return null;
  const lang = LANGUAGE[m[1]];
  const n = Number(m[2].replace(/,/g, ""));
  if (!lang || !Number.isFinite(n)) return null;
  return `It found ${n.toLocaleString("en-US")} ${lang} ${n === 1 ? "file" : "files"}`;
}

/** The phase the job was in when it stopped, for `worker_crashed`: the
 * furthest stage it actually reached -- failed first, then running, then the
 * last finished -- since a crash can be observed before the stage's own
 * `failed` transition lands. Null for a job with no stage rows started. */
export function stoppedPhase(job: Pick<JobSnapshot, "stages">): string | null {
  const reversed = [...job.stages].reverse();
  const stage =
    reversed.find((s) => s.state === "failed") ?? reversed.find((s) => s.state === "running") ?? reversed.find((s) => s.state === "done");
  return stage ? phaseTitle(phaseOf(stage.id)) : null;
}

const TRY_AGAIN_WONT = "Trying again won't change the result.";

export function classifyFailure(job: Pick<JobSnapshot, "error_code" | "error" | "stages">): FailureView {
  const code = job.error_code ?? "";
  switch (code) {
    case "detection_uncertain": {
      const found = foundInEvidence(job.error);
      return {
        code,
        cls: "deterministic",
        title: "tolmap couldn't tell where this repository's code lives",
        explanation: [
          `${found ?? "It found source files"} but no package it could name for certain. A wrong guess draws a convincing but wrong map, so it stopped instead of guessing.`,
          `${TRY_AGAIN_WONT} This layout needs support in tolmap itself.`,
        ],
        actions: ["another", "report"],
      };
    }
    case "detection_failed":
      return {
        code,
        cls: "deterministic",
        title: "tolmap found no Python, Go, TypeScript or Rust source here",
        explanation: [
          "tolmap maps code written in those four languages, and it found none of them it could read in this repository.",
          TRY_AGAIN_WONT,
        ],
        actions: ["another"],
      };
    case "cancelled":
      return {
        code,
        cls: "user",
        title: "Mapping was cancelled",
        explanation: ["The job was stopped before the map was ready, so nothing was saved."],
        actions: ["again", "home"],
      };
    case "clone_failed":
      return {
        code,
        cls: "input",
        title: "tolmap couldn't download this repository",
        explanation: [
          "It may be private, renamed or deleted, or the host didn't answer. Check the name; if it's right, trying again may work.",
        ],
        actions: ["retry", "check-name"],
      };
    case "worker_crashed": {
      const phase = stoppedPhase(job);
      return {
        code,
        cls: "transient",
        title: phase ? `The mapping job stopped during ${phase}` : "The mapping job stopped",
        explanation: [
          "The process mapping it ended before it finished. A very large repository can run out of memory on this server; trying again may work.",
        ],
        actions: ["retry"],
      };
    }
    case "busy":
    case "server_stopping":
    case "rate_limited":
      return {
        code,
        cls: "transient",
        title: "tolmap is busy right now",
        explanation: [
          code === "server_stopping"
            ? "The server restarted while this repository was being mapped. Try again in a moment."
            : "Too many repositories are being mapped at once. Try again in a moment.",
        ],
        actions: ["retry"],
      };
    default:
      // index_failed, internal_error, and any code this page doesn't know
      // (docs/API.md: "render unknown error codes as a generic failure").
      return {
        code,
        cls: "bug",
        title: "Something went wrong while mapping",
        explanation: ["This looks like a bug in tolmap, not a problem with the repository. Trying again may work; reporting it helps fix it."],
        actions: ["retry", "report"],
      };
  }
}

/** A deterministic refusal never offers trying again (§6.5). */
export function offersRetry(view: FailureView): boolean {
  return view.actions.includes("retry") || view.actions.includes("again");
}

export const ISSUES_URL = "https://github.com/onsager-ai/tolmap/issues/new";

/** "Report this repository": a plain link to a new issue, the slug and the
 * error filled in through the URL. Nothing is submitted for the person. */
export function reportIssueUrl(job: Pick<JobSnapshot, "slug" | "commit" | "error_code" | "error" | "job_id">): string {
  const title = `Couldn't map ${job.slug} (${job.error_code ?? "unknown error"})`;
  const body = [
    job.slug.startsWith("local/") ? `Repository: ${job.slug}` : `Repository: https://github.com/${job.slug}`,
    job.commit ? `Commit: ${job.commit}` : null,
    `Error code: ${job.error_code ?? "(none)"}`,
    `Job: ${job.job_id}`,
    "",
    "What tolmap saw:",
    "```",
    (job.error ?? "(no message)").slice(0, 2000),
    "```",
  ]
    .filter((line): line is string => line != null)
    .join("\n");
  const params = new URLSearchParams({ title, body });
  return `${ISSUES_URL}?${params.toString()}`;
}
