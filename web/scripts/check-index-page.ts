#!/usr/bin/env -S npx tsx
// Unit checks for the indexing page's pure parts (docs/UX.md §6, phase 4):
//   - lib/indexPhases.ts: §6.2's stage -> phase table, each phase's state,
//     share done and current step (including the indeterminate state and the
//     hidden pending `index_*` stages), "Found so far" (§6.3), and the ETA and
//     queue wording (§6.1);
//   - lib/jobFailure.ts: §6.5's error-code table -- class, title, actions, and
//     that a deterministic refusal never offers trying again.
// No browser: the same standalone-script pattern as check-phone-shell.ts (no
// test runner exists in this project -- web/README.md). CI runs it in the
// `web build and lint` job; check-view-stability.mjs drives the real page.
//
// Run: npx tsx web/scripts/check-index-page.ts

import { readFileSync } from "node:fs";
import type { JobSnapshot } from "../../bindings/JobSnapshot";
import type { ProgressValue } from "../../bindings/ProgressValue";
import type { StageId } from "../../bindings/StageId";
import type { StageSnapshot } from "../../bindings/StageSnapshot";
import {
  PHASES,
  STEP_TEXT,
  foundSoFar,
  formatDuration,
  formatEtaRange,
  formatStartsIn,
  formatStepCount,
  ordinal,
  phaseOf,
  skippedStages,
  stagePosition,
  summarisePhases,
  type PhaseView,
} from "../src/lib/indexPhases";
import { ISSUES_URL, classifyFailure, foundInEvidence, offersRetry, reportIssueUrl, stoppedPhase } from "../src/lib/jobFailure";

let failures = 0;
let checks = 0;

function report(ok: boolean, label: string, detail?: string) {
  checks++;
  if (ok) console.log(`  ok    ${label}`);
  else {
    failures++;
    console.log(`  FAIL  ${label}${detail ? " -- " + detail : ""}`);
  }
}
const eq = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

// StageId::ALL, read from the generated binding so a stage added in Rust
// fails here until the table covers it.
const STAGE_IDS = [...readFileSync(new URL("../../bindings/StageId.ts", import.meta.url), "utf8").matchAll(/"([a-z_]+)"/g)].map(
  (m) => m[1] as StageId,
);

type Row = [StageId, StageSnapshot["state"], number?];
/** A snapshot with every stage pending except `rows`. */
function job(status: JobSnapshot["status"], rows: Row[], progress: Partial<ProgressValue> | null = null): JobSnapshot {
  const states = new Map(rows.map(([id, state, d]) => [id, { state, d }]));
  return {
    job_id: "job",
    slug: "owner/repo",
    commit: "c0ffee",
    status,
    stage: "stage text",
    queue_position: null,
    started_at: "2026-09-27T07:08:00Z",
    finished_at: null,
    error: null,
    error_code: null,
    progress: progress
      ? ({ stage_index: 6, stage_count: 22, label: "", unit: "files", done: 0, total: null, rate_per_s: null, ...progress } as ProgressValue)
      : null,
    eta: null,
    eta_start_s: null,
    elapsed_s: 72,
    stages: STAGE_IDS.map((id) => ({
      id,
      label: id,
      state: states.get(id)?.state ?? "pending",
      started_at: null,
      duration_s: states.get(id)?.d ?? null,
    })),
    map_ready: false,
  };
}
const byId = (phases: PhaseView[]) => Object.fromEntries(phases.map((p) => [p.id, p]));
const FETCHED: Row[] = [
  ["clone", "done", 1],
  ["clone_objects", "done", 9],
  ["clone_deltas", "done", 3],
  ["clone_checkout", "done", 2],
];

// ---------------------------------------------------------------- the table
console.log("\nstage -> phase table (§6.2)");
{
  const flat = PHASES.flatMap((p) => p.stages);
  report(STAGE_IDS.length === 22, "the binding lists 22 stages", String(STAGE_IDS.length));
  report(eq(flat, STAGE_IDS), "the four phases cover every stage once, in pipeline order", JSON.stringify(flat));
  report(STAGE_IDS.every((id) => typeof STEP_TEXT[id] === "string" && STEP_TEXT[id].length > 0), "every stage has plain step text");
  report(eq(PHASES.map((p) => p.title), ["Fetch", "Read", "Map", "Detail"]), "phases are Fetch, Read, Map, Detail");
  const expect: [StageId, string][] = [
    ["clone", "fetch"],
    ["clone_checkout", "fetch"],
    ["detect", "read"],
    ["install", "read"],
    ["index_ts", "read"],
    ["history", "read"],
    ["blend_prune", "map"],
    ["write_map", "map"],
    ["symbols", "detail"],
    ["write", "detail"],
  ];
  report(expect.every(([s, p]) => phaseOf(s) === p), "phaseOf follows the table", JSON.stringify(expect.map(([s]) => [s, phaseOf(s)])));
  report(STEP_TEXT.clone_objects === "Downloading" && STEP_TEXT.write_map === "Saving the map" && STEP_TEXT.symbols === "Reading classes and functions", "step text matches §6.2's wording");
}

// ---------------------------------------------------------------- states
console.log("\nphase states, shares and the current step");
{
  const queued = summarisePhases(job("queued", []));
  report(queued.every((p) => p.state === "pending" && p.fraction === 0 && p.current == null), "queued: four pending phases, nothing done");

  const parsing = byId(
    summarisePhases(job("indexing", [...FETCHED, ["detect", "done", 1], ["parse", "running"]], { stage: "parse", done: 3912, total: 6347, rate_per_s: 410 })),
  );
  report(parsing.fetch.state === "done" && parsing.fetch.fraction === 1, "a finished Fetch is done");
  report(parsing.fetch.durationS === 15, "a finished phase's duration is the sum of its stages' (1+9+3+2 = 15 s)", String(parsing.fetch.durationS));
  report(parsing.read.state === "running" && parsing.map.state === "pending" && parsing.detail.state === "pending", "Read runs; Map and Detail wait");
  const step = parsing.read.current;
  report(step?.stage === "parse" && step.text === "Parsing files" && !step.indeterminate, "the current step is parse, in plain words, determinate", JSON.stringify(step));
  report(step?.done === 3912 && step.total === 6347 && step.rate === 410, "the step carries the live count and rate", JSON.stringify(step));
  // Read's shown rows on a hand job: detect, parse, resolve, install,
  // history (the three pending index_* are hidden).
  report(parsing.read.stages.length === 5 && !parsing.read.stages.some((s) => s.id.startsWith("index_")), "pending index_* stages are hidden", JSON.stringify(parsing.read.stages.map((s) => s.id)));
  const expected = (1 + 3912 / 6347) / 5;
  report(Math.abs(parsing.read.fraction - expected) < 1e-9, "a phase's share: stages done plus the running stage's done/total", `${parsing.read.fraction} vs ${expected}`);
  report(formatStepCount(step!) === "3,912 / 6,347 files · 410 files/s", "the step's count reads as in the design", String(formatStepCount(step!)));

  const detecting = byId(summarisePhases(job("detecting", [["clone", "done", 2], ["detect", "running"]], { stage: "detect", done: 1, total: null, unit: "steps" })));
  report(detecting.fetch.state === "done", "clone sub-stages that never ran (a cached clone) do not hold Fetch open");
  report(detecting.read.current?.indeterminate === true, "a step with no known total is indeterminate");
  report(detecting.read.fraction === 0, "an indeterminate step adds nothing to its phase's share", String(detecting.read.fraction));

  const hand = byId(
    summarisePhases(
      job("indexing", [...FETCHED, ["detect", "done"], ["parse", "done"], ["resolve", "done"], ["history", "running"]], { stage: "history", done: 10, total: 20, unit: "commits" }),
    ),
  );
  report(skippedStages(job("indexing", [["history", "running"]]).stages).has("install"), "install is skipped once a later stage has started");
  report(Math.abs(hand.read.fraction - (4 + 0.5) / 5) < 1e-9, "a skipped install counts as settled in Read's share", String(hand.read.fraction));

  const scip = byId(summarisePhases(job("indexing", [...FETCHED, ["detect", "done"], ["parse", "done"], ["resolve", "done"], ["index_py", "running"]])));
  report(scip.read.stages.some((s) => s.id === "index_py") && scip.read.current?.text === "Indexing Python", "a SCIP job's indexing stage shows once it runs");

  const between = byId(summarisePhases(job("indexing", [...FETCHED, ["history", "done"], ["regions", "done"], ["footprints", "done"]])));
  report(between.map.state === "running" && between.map.current?.stage === "write_map" && between.map.current.done == null, "between two stages the next step shows, without a count yet", JSON.stringify(between.map.current));

  const done = summarisePhases(job("done", STAGE_IDS.filter((id) => !id.startsWith("index_") && id !== "install").map((id) => [id, "done", 1] as Row)));
  report(done.every((p) => p.state === "done" && p.fraction === 1), "a done job: four done phases");

  const failed = byId(summarisePhases(job("failed", [...FETCHED, ["detect", "done"], ["parse", "failed"]])));
  report(failed.read.state === "failed" && failed.fetch.state === "done" && failed.map.state === "pending", "a failure marks its own phase only");
}

console.log("\nstage position and found so far (§6.3, §6.4)");
{
  report(eq(stagePosition(job("indexing", [["parse", "running"]], { stage: "parse", stage_index: 6, stage_count: 22 })), { index: 6, count: 22 }), "stage N of 22 from progress");
  report(eq(stagePosition(job("indexing", [...FETCHED, ["detect", "running"]])), { index: 5, count: 22 }), "stage N of 22 from the running row without progress");
  report(stagePosition(job("queued", [])) == null, "no position before anything starts");
  report(eq(foundSoFar({}), []), "nothing found before the first counts");
  report(
    eq(foundSoFar({ parse: { done: 1, total: 6347 }, history: { done: 4000, total: null }, regions: { done: 19, total: 19 } } as never), [
      "6,347 source files",
      "4,000 commits read",
      "19 districts",
    ]),
    "parse total, history count, district count",
    JSON.stringify(foundSoFar({ parse: { done: 1, total: 6347 }, history: { done: 4000, total: null }, regions: { done: 19, total: 19 } } as never)),
  );
  report(eq(foundSoFar({ naming: { done: 1, total: 1 } } as never), ["1 district"]), "naming's total stands in before regions, singular for one");
}

console.log("\nETA and queue wording (§6.1)");
{
  report(formatEtaRange(180, 300) === "About 3–5 min left", "3-5 min", formatEtaRange(180, 300));
  report(formatEtaRange(150, 170) === "About 3 min left", "bounds rounding to one number", formatEtaRange(150, 170));
  report(formatEtaRange(10, 20) === "Less than a minute left", "under half a minute", formatEtaRange(10, 20));
  report(formatEtaRange(10, 200) === "Up to 3 min left", "a range starting at zero", formatEtaRange(10, 200));
  report(formatStartsIn(120) === "Starts in about 2 min" && formatStartsIn(10) === "Starts in about a minute", "starts in about N min");
  report(eq([1, 2, 3, 4, 11, 12, 13, 21, 22, 101, 111].map(ordinal), ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "101st", "111th"]), "ordinals");
  report(formatDuration(72, true) === "1m 12s" && formatDuration(40, true) === "0m 40s" && formatDuration(15) === "15s", "durations");
  report(formatStepCount({ done: 5 * 1048576, total: null, rate: 1048576, unit: "bytes" }) === "5.0 MB · 1.0 MB/s", "bytes read as MB", String(formatStepCount({ done: 5 * 1048576, total: null, rate: 1048576, unit: "bytes" })));
}

// ---------------------------------------------------------------- failures
console.log("\nfailure pages by error code (§6.5)");
{
  const fail = (code: string | null, error: string | null = "message", rows: Row[] = []) => ({ ...job("failed", rows), error_code: code, error });
  const table: [string | null, string, string, string[]][] = [
    ["detection_uncertain", "deterministic", "tolmap couldn't tell where this repository's code lives", ["another", "report"]],
    ["detection_failed", "deterministic", "tolmap found no Python, Go, TypeScript or Rust source here", ["another"]],
    ["cancelled", "user", "Mapping was cancelled", ["again", "home"]],
    ["clone_failed", "input", "tolmap couldn't download this repository", ["retry", "check-name"]],
    ["worker_crashed", "transient", "The mapping job stopped", ["retry"]],
    ["busy", "transient", "tolmap is busy right now", ["retry"]],
    ["server_stopping", "transient", "tolmap is busy right now", ["retry"]],
    ["rate_limited", "transient", "tolmap is busy right now", ["retry"]],
    ["index_failed", "bug", "Something went wrong while mapping", ["retry", "report"]],
    ["internal_error", "bug", "Something went wrong while mapping", ["retry", "report"]],
    ["some_future_code", "bug", "Something went wrong while mapping", ["retry", "report"]],
    [null, "bug", "Something went wrong while mapping", ["retry", "report"]],
  ];
  for (const [code, cls, title, actions] of table) {
    const view = classifyFailure(fail(code));
    report(view.cls === cls && view.title === title && eq(view.actions, actions), `${code ?? "(no code)"}: ${cls}, "${title}", ${actions.join(" + ")}`, JSON.stringify(view));
    report(view.explanation.length >= 1 && view.explanation.length <= 2, `${code ?? "(no code)"}: one or two sentences of explanation`);
  }
  for (const code of ["detection_uncertain", "detection_failed"]) {
    const view = classifyFailure(fail(code));
    report(!offersRetry(view), `${code}: a deterministic refusal never offers trying again`);
    report(view.explanation.join(" ").includes("Trying again won't change the result."), `${code}: the copy says trying again won't change it`);
  }
  report(offersRetry(classifyFailure(fail("clone_failed"))) && offersRetry(classifyFailure(fail("cancelled"))), "transient and user failures keep a retry");

  const crashed = classifyFailure(fail("worker_crashed", "worker exited unexpectedly (signal 9)", [...FETCHED, ["detect", "done"], ["parse", "running"]]));
  report(crashed.title === "The mapping job stopped during Read", "worker_crashed names the phase it stopped in", crashed.title);
  report(stoppedPhase(job("failed", [...FETCHED, ["symbols", "failed"]])) === "Detail", "a failed stage wins over later finished ones");

  const evidence = "py at . (972 files, low confidence) -- no pyproject.toml/setup.py package match and no directory with __init__.py; mapping the repository root directly";
  report(foundInEvidence(evidence) === "It found 972 Python files", "detection evidence gives the file count and language", String(foundInEvidence(evidence)));
  report(foundInEvidence("something else") == null, "evidence of another shape is not guessed from");
  report(classifyFailure(fail("detection_uncertain", evidence)).explanation[0].startsWith("It found 972 Python files but no package"), "the refusal's explanation uses it");

  const url = reportIssueUrl({ ...fail("detection_uncertain", evidence), slug: "vectorize-io/hindsight" });
  const parsed = new URL(url);
  report(url.startsWith(`${ISSUES_URL}?`), "report links to a new GitHub issue", url.slice(0, 80));
  report(parsed.searchParams.get("title") === "Couldn't map vectorize-io/hindsight (detection_uncertain)", "the issue title names the slug and code", String(parsed.searchParams.get("title")));
  const body = parsed.searchParams.get("body") ?? "";
  report(body.includes("https://github.com/vectorize-io/hindsight") && body.includes(evidence) && body.includes("c0ffee"), "the body carries the repository, commit and what tolmap saw", body);
}

console.log(`\n${checks - failures}/${checks} checks passed`);
if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exitCode = 1;
}
