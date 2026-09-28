// docs/UX.md §6.1-§6.3: the indexing page's pure half. Twenty-two pipeline
// stages (src/progress.rs `StageId::ALL`) become four plain-language phases,
// each with a state, a share done and, while it runs, the current step in
// plain words with its live count. Everything here is a function of the job
// snapshot (plus, for "Found so far", the progress values the page has
// already seen), so it is unit-checked without a browser in
// scripts/check-index-page.ts.
//
// Type-only imports: the check script runs this file under tsx, where the
// app's path aliases are not needed for anything that is erased.
import type { JobSnapshot } from "@bindings/JobSnapshot";
import type { ProgressValue } from "@bindings/ProgressValue";
import type { StageId } from "@bindings/StageId";
import type { StageSnapshot } from "@bindings/StageSnapshot";

export type PhaseId = "fetch" | "read" | "map" | "detail";

export interface PhaseDef {
  id: PhaseId;
  title: string;
  meaning: string;
  stages: readonly StageId[];
}

/** §6.2's table, in pipeline order. Every `StageId` appears exactly once
 * (checked by scripts/check-index-page.ts against the binding's union). */
export const PHASES: readonly PhaseDef[] = [
  {
    id: "fetch",
    title: "Fetch",
    meaning: "Download the repository",
    stages: ["clone", "clone_objects", "clone_deltas", "clone_checkout"],
  },
  {
    id: "read",
    title: "Read",
    meaning: "Find the source, parse files, follow imports, read history",
    stages: ["detect", "parse", "resolve", "index_go", "index_py", "install", "index_ts", "history"],
  },
  {
    id: "map",
    title: "Map",
    meaning: "Group files into districts and draw them",
    stages: ["blend_prune", "partition", "neighbourhoods", "naming", "regions", "footprints", "write_map"],
  },
  {
    id: "detail",
    title: "Detail",
    meaning: "Classes and functions for close zoom",
    stages: ["symbols", "symbol_cards", "write"],
  },
];

/** §6.2: the step text shown while a stage runs. The stage's own `label`
 * (src/progress.rs) stays for the technical details. */
export const STEP_TEXT: Record<StageId, string> = {
  clone: "Contacting the host",
  clone_objects: "Downloading",
  clone_deltas: "Unpacking",
  clone_checkout: "Writing files",
  detect: "Finding the source",
  parse: "Parsing files",
  resolve: "Following imports",
  index_go: "Indexing Go",
  index_py: "Indexing Python",
  install: "Installing dependencies",
  index_ts: "Indexing TypeScript",
  history: "Reading history",
  blend_prune: "Weighing links",
  partition: "Finding districts",
  neighbourhoods: "Finding neighborhoods",
  naming: "Naming districts",
  regions: "Drawing districts",
  footprints: "Sizing files",
  write_map: "Saving the map",
  symbols: "Reading classes and functions",
  symbol_cards: "Laying out symbols",
  write: "Saving details",
};

const PHASE_OF = new Map<StageId, PhaseId>(PHASES.flatMap((p) => p.stages.map((s) => [s, p.id] as const)));

export function phaseOf(stage: StageId): PhaseId {
  return PHASE_OF.get(stage) ?? "read";
}

export function phaseTitle(id: PhaseId): string {
  return PHASES.find((p) => p.id === id)!.title;
}

/** Issue #110: the three `index_*` stages run only for a `--refs scip` job,
 * one per indexed language. A hand-written job never starts them, so a
 * pending one is hidden rather than listed as work still to come; a SCIP
 * job's rows appear as each language's indexer starts. */
export function shownStage(stage: Pick<StageSnapshot, "id" | "state">): boolean {
  return !(stage.id.startsWith("index_") && stage.state === "pending");
}

/** A pending stage the job has already gone past: some later stage (in
 * pipeline order, which is the order of `stages`) has started. `install`
 * on a hand-written job, the clone sub-stages of a clone already in the
 * cache. It counts as done for its phase's share, and never holds a phase
 * open. */
export function skippedStages(stages: readonly Pick<StageSnapshot, "id" | "state">[]): Set<StageId> {
  const skipped = new Set<StageId>();
  let laterStarted = false;
  for (let i = stages.length - 1; i >= 0; i--) {
    const stage = stages[i];
    if (stage.state === "pending") {
      if (laterStarted) skipped.add(stage.id);
    } else {
      laterStarted = true;
    }
  }
  return skipped;
}

export type PhaseState = "pending" | "running" | "done" | "failed";

export interface CurrentStep {
  stage: StageId;
  text: string;
  /** Null when the snapshot's `progress` is not this stage's yet. */
  done: number | null;
  total: number | null;
  rate: number | null;
  unit: string | null;
  /** No known total: the bar is indeterminate (§6.2). */
  indeterminate: boolean;
}

export interface PhaseView {
  id: PhaseId;
  title: string;
  meaning: string;
  state: PhaseState;
  /** Share of the phase done, 0..1: its shown stages done or skipped, plus
   * the running stage's `done/total` when the total is known. */
  fraction: number;
  /** Sum of the phase's stage durations; shown once the phase is done. */
  durationS: number | null;
  /** The running step, only while the phase runs. */
  current: CurrentStep | null;
  /** The phase's stages as the technical details list them. */
  stages: StageSnapshot[];
}

type PhaseInput = Pick<JobSnapshot, "status" | "stages" | "progress">;

/** §6.2 in one pass over the snapshot. A queued job shows four pending
 * phases whatever its (reset) stage rows say. */
export function summarisePhases(job: PhaseInput): PhaseView[] {
  const byId = new Map(job.stages.map((s) => [s.id, s]));
  const skipped = skippedStages(job.stages);
  const failedJob = job.status === "failed";
  return PHASES.map((def) => {
    const rows = def.stages.map((id) => byId.get(id)).filter((s): s is StageSnapshot => !!s && shownStage(s));
    // Hidden rows (a pending `index_*`) are not work still to come either.
    const counted = rows;
    const running = [...rows].reverse().find((s) => s.state === "running");
    const failed = rows.some((s) => s.state === "failed");
    const allSettled = rows.length > 0 && rows.every((s) => s.state === "done" || skipped.has(s.id));
    const anyStarted = rows.some((s) => s.state !== "pending");
    let state: PhaseState;
    if (job.status === "queued") state = "pending";
    else if (failed) state = "failed";
    else if (running) state = failedJob ? "failed" : "running";
    else if (allSettled && rows.some((s) => s.state === "done")) state = "done";
    else if (anyStarted) state = failedJob ? "failed" : job.status === "done" ? "done" : "running";
    else state = job.status === "done" ? "done" : "pending";

    let current: CurrentStep | null = null;
    let partial = 0;
    if (state === "running") {
      // Between two stages nothing is `running` for a moment: the step is
      // then the next pending one, not yet started.
      const step = running ?? rows.find((s) => s.state === "pending" && !skipped.has(s.id)) ?? null;
      if (step) {
        const p = job.progress && job.progress.stage === step.id ? job.progress : null;
        const done = p ? Number(p.done) : null;
        const total = p && p.total != null ? Number(p.total) : null;
        if (step.state === "running" && done != null && total != null && total > 0) partial = Math.min(1, done / total);
        current = {
          stage: step.id,
          text: STEP_TEXT[step.id],
          done,
          total,
          rate: p?.rate_per_s ?? null,
          unit: p?.unit ?? null,
          indeterminate: total == null || total <= 0,
        };
      }
    }
    const settled = counted.filter((s) => s.state === "done" || skipped.has(s.id)).length;
    const fraction =
      state === "done" ? 1 : state === "pending" || counted.length === 0 ? 0 : Math.min(1, (settled + partial) / counted.length);
    const durations = rows.map((s) => s.duration_s).filter((d): d is number => d != null);
    return {
      id: def.id,
      title: def.title,
      meaning: def.meaning,
      state,
      fraction,
      durationS: durations.length ? durations.reduce((a, b) => a + b, 0) : null,
      current,
      stages: rows,
    };
  });
}

/** "stage 6 of 22" (§6.4): from `progress` while it names the running
 * stage, else the running (or last started) stage row's place in the list. */
export function stagePosition(job: Pick<JobSnapshot, "stages" | "progress">): { index: number; count: number } | null {
  if (job.progress) return { index: job.progress.stage_index, count: job.progress.stage_count };
  const count = job.stages.length;
  const last = (pred: (s: StageSnapshot) => boolean) => {
    for (let i = job.stages.length - 1; i >= 0; i--) if (pred(job.stages[i])) return i;
    return -1;
  };
  const running = last((s) => s.state === "running");
  const at = running >= 0 ? running : last((s) => s.state !== "pending");
  return at >= 0 ? { index: at + 1, count } : null;
}

/** §6.3: facts as they arrive, from the progress values the page has seen
 * (the snapshot only carries the current stage's, so the page keeps the last
 * one per stage). `parse`'s total is the source file count; `history`'s
 * count the commits read; `regions` (else `naming`) the district count. */
export function foundSoFar(seen: Partial<Record<StageId, Pick<ProgressValue, "done" | "total">>>): string[] {
  const facts: string[] = [];
  const parse = seen.parse;
  if (parse?.total != null && Number(parse.total) > 0) facts.push(`${formatCount(Number(parse.total))} source files`);
  const history = seen.history;
  if (history && Number(history.done) > 0) facts.push(`${formatCount(Number(history.done))} commits read`);
  const districts = seen.regions ?? seen.naming;
  if (districts?.total != null && Number(districts.total) > 0) {
    const n = Number(districts.total);
    facts.push(`${formatCount(n)} ${n === 1 ? "district" : "districts"}`);
  }
  return facts;
}

// ---------- formatting ----------

export function formatCount(n: number): string {
  return Math.round(n).toLocaleString("en-US");
}

/** "1m 12s", "40s", "0m 40s" with `padMinutes` (the queue's waiting time
 * reads as a clock, §6.1). */
export function formatDuration(totalSeconds: number, padMinutes = false): string {
  const s = Math.max(0, Math.round(totalSeconds));
  if (s < 60 && !padMinutes) return `${s}s`;
  const m = Math.floor(s / 60);
  const rem = s % 60;
  return `${m}m ${rem}s`;
}

/** §6.1: "About 3–5 min left", each bound rounded to whole minutes
 * independently rather than from the midpoint, so the range still brackets
 * the model's own low_s/high_s rather than looking falsely precise. */
export function formatEtaRange(lowS: number, highS: number): string {
  const lowMin = Math.max(0, Math.round(lowS / 60));
  const highMin = Math.max(lowMin, Math.round(highS / 60));
  if (lowMin === 0 && highMin === 0) return "Less than a minute left";
  if (lowMin === highMin) return `About ${lowMin} min left`;
  if (lowMin === 0) return `Up to ${highMin} min left`;
  return `About ${lowMin}–${highMin} min left`;
}

/** §6.1: "Starts in about 2 min". Below 30 s this would round to "0 min",
 * which reads as broken rather than imminent. */
export function formatStartsIn(etaStartS: number): string {
  if (etaStartS < 30) return "Starts in about a minute";
  return `Starts in about ${Math.round(etaStartS / 60)} min`;
}

/** "2nd in line". */
export function ordinal(n: number): string {
  const mod100 = n % 100;
  const suffix = mod100 >= 11 && mod100 <= 13 ? "th" : ({ 1: "st", 2: "nd", 3: "rd" } as Record<number, string>)[n % 10] ?? "th";
  return `${n}${suffix}`;
}

function formatQuantity(value: number, unit: string): string {
  if (unit === "bytes") return `${(value / (1024 * 1024)).toFixed(1)} MB`;
  return formatCount(value);
}

/** The running step's count: "3,912 / 6,347 files · 410 files/s". Bytes
 * read as MB. Null before the step has reported anything. */
export function formatStepCount(step: Pick<CurrentStep, "done" | "total" | "rate" | "unit">): string | null {
  if (step.done == null || step.unit == null) return null;
  const unit = step.unit;
  const word = unit === "bytes" ? "" : ` ${unit}`;
  const counted =
    step.total != null && step.total > 0
      ? `${formatQuantity(step.done, unit)} / ${formatQuantity(step.total, unit)}${word}`
      : `${formatQuantity(step.done, unit)}${word}`;
  if (step.rate == null || step.rate <= 0) return counted;
  const rate = unit === "bytes" ? `${(step.rate / (1024 * 1024)).toFixed(1)} MB/s` : `${formatCount(step.rate)} ${unit}/s`;
  return `${counted} · ${rate}`;
}
