#!/usr/bin/env node
// A local stub of the job/index service's contract (docs/API.md), used to
// drive the submit -> progress -> map path end to end during development and
// Playwright verification, without a real Rust service or a network clone.
// Not built, not shipped, not referenced by any production code path --
// vite.config.ts's dev proxy points at it only when TOLMAP_API_PROXY_TARGET
// names it.
//
// Issue #97 (live job progress, ETA, cancel): this now emits the real
// JobSnapshot shape (see ../../bindings/JobSnapshot.ts) -- stages[], progress,
// eta, eta_start_s, elapsed_s -- not just the four-phase status string the
// pre-#97 stub used, and answers POST /api/jobs/{id}/cancel. It also models
// the service's max_concurrent_jobs=1 admission (docs/API.md): only one job
// runs at a time, so a second submission while one is in flight comes back
// queued with a real queue_position/eta_start_s, which is what
// check-view-stability.mjs's queued-state check and screenshots.mjs's
// queued-state frame exercise.
//
// Usage: node scripts/mock-api-server.mjs [port]
import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";

const PORT = Number(process.argv[2] ?? process.env.PORT ?? 8787);

// Real MapDocument shape (repo, q, names, districts, F, N, E, L, S, U,
// roads, lang, P) is nontrivial - see ../../bindings/MapDocument.ts - so
// this stub reuses one of the committed acceptance fixtures rather than
// hand-rolling a fake one, to actually exercise the viewer end to end
// instead of just its loading state.
const FIXTURE_PATH = path.resolve(fileURLToPath(new URL("..", import.meta.url)), "..", "data", "httpx.json");
const FIXTURE_DOC = JSON.parse(readFileSync(FIXTURE_PATH, "utf8"));

// In-memory catalogue: one already-indexed repo (so /api/maps and the
// "already cached" 200-on-POST path have something to exercise), keyed by
// slug.
const owner = "mockorg";
const repo = "already-indexed";
const cachedSlug = `${owner}/${repo}`;
const cachedDoc = { ...FIXTURE_DOC, repo };

// StageId::ALL, in pipeline order (src/progress.rs) -- label/unit copied
// verbatim so the mock's stage rows read exactly like the real service's.
const STAGE_DEFS = [
  { id: "clone", label: "Clone or fetch", unit: "objects" },
  { id: "clone_objects", label: "Receiving objects", unit: "objects" },
  { id: "clone_deltas", label: "Resolving deltas", unit: "objects" },
  { id: "clone_checkout", label: "Updating files", unit: "files" },
  { id: "detect", label: "Detecting source", unit: "steps" },
  { id: "parse", label: "Parsing files", unit: "files" },
  { id: "resolve", label: "Resolving imports", unit: "files" },
  { id: "history", label: "Reading history", unit: "commits" },
  { id: "blend_prune", label: "Blending and pruning", unit: "steps" },
  { id: "partition", label: "Partitioning districts", unit: "steps" },
  { id: "neighbourhoods", label: "Partitioning neighbourhoods", unit: "districts" },
  { id: "naming", label: "Naming districts", unit: "districts" },
  { id: "regions", label: "Drawing regions", unit: "districts" },
  { id: "footprints", label: "Drawing footprints", unit: "districts" },
  { id: "write_map", label: "Writing map", unit: "bytes" },
  { id: "symbols", label: "Extracting symbols", unit: "files" },
  { id: "symbol_cards", label: "Drawing symbol cards", unit: "files" },
  { id: "write", label: "Writing symbols", unit: "bytes" },
];
const TICKS_PER_STAGE = 3;
const TICK_MS = 220;

function statusForStage(stageId) {
  if (stageId.startsWith("clone")) return "cloning";
  if (stageId === "detect") return "detecting";
  return "indexing";
}

// A slug containing "crashes" fails partway through as worker_crashed (the
// IndexJobView stage-naming message); one containing "fails" fails for an
// ordinary reason instead. "toolong"/"huge" are kept as aliases of "fails"
// -- docs/API.md: repo_too_large is no longer emitted, so there is no
// distinct too-large state to model any more, just an ordinary failure.
function classify(slug) {
  if (/crashes/i.test(slug)) return "worker_crashed";
  if (/fails|toolong|huge/i.test(slug)) return "generic";
  return null;
}

const jobs = new Map();
// One running slot, matching the service's default TOLMAP_MAX_CONCURRENT_JOBS=1.
let activeJobId = null;
const queue = [];
// Map documents by slug, seeded with the one pre-cached repo. A job that
// reaches "done" registers its own slug here too.
const docs = new Map([[cachedSlug, cachedDoc]]);

function newStages() {
  return STAGE_DEFS.map((def) => ({ id: def.id, label: def.label, state: "pending", started_at: null, duration_s: null }));
}

function estimateRemainingS(job) {
  const stageIndex = STAGE_DEFS.findIndex((d) => d.id === job._currentStage);
  const remainingStages = stageIndex < 0 ? STAGE_DEFS.length : STAGE_DEFS.length - stageIndex - 1;
  const perStageS = (TICKS_PER_STAGE * TICK_MS) / 1000;
  return Math.max(0, remainingStages) * perStageS;
}

function snapshot(job) {
  const elapsed_s = job._startedMs != null ? (job._finishedMs ?? Date.now() - job._startedMs) / 1000 : 0;
  return {
    job_id: job.job_id,
    slug: job.slug,
    commit: job.commit,
    status: job.status,
    stage: job.stageText,
    queue_position: job.queue_position,
    started_at: job.started_at,
    finished_at: job.finished_at,
    error: job.error,
    error_code: job.error_code,
    progress: job.progress,
    eta: job.eta,
    eta_start_s: job.eta_start_s,
    elapsed_s,
    stages: job.stages,
  };
}

function publish(job) {
  const payload = JSON.stringify(snapshot(job));
  for (const res of job._subscribers) res.write(`data: ${payload}\n\n`);
}

function closeSubscribers(job) {
  for (const res of job._subscribers) res.end();
  job._subscribers.clear();
}

function refreshQueuePositions() {
  queue.forEach((job, index) => {
    job.queue_position = index + 1;
    const activeJob = activeJobId != null ? jobs.get(activeJobId) : null;
    const activeRemaining = activeJob ? estimateRemainingS(activeJob) : 0;
    const aheadS = queue.slice(0, index).reduce((sum) => sum + (TICKS_PER_STAGE * TICK_MS * STAGE_DEFS.length) / 1000, 0);
    job.eta_start_s = activeRemaining + aheadS;
    job.eta = { low_s: job.eta_start_s, high_s: job.eta_start_s * 1.6 + 1, basis: "model" };
    publish(job);
  });
}

function finishJob(job, { status, error = null, error_code = null }) {
  job.status = status;
  job.stageText = status;
  job.finished_at = new Date().toISOString();
  // A job cancelled while still queued never started (_startedMs is null,
  // and snapshot()'s own elapsed_s is gated on that same check) -- 0 rather
  // than NaN, for anything that reads _finishedMs directly.
  job._finishedMs = job._startedMs != null ? Date.now() - job._startedMs : 0;
  job.error = error;
  job.error_code = error_code;
  job.eta = null;
  job.eta_start_s = null;
  job.queue_position = null;
  for (const stage of job.stages) {
    if (stage.state === "running") stage.state = "failed";
  }
  if (job._timer) clearTimeout(job._timer);
  job._timer = null;
  if (status === "done") {
    job.commit = `mock${Date.now().toString(36)}`;
    docs.set(job.slug, { ...FIXTURE_DOC, repo: job.slug.split("/")[1] });
  }
  publish(job);
  closeSubscribers(job);
  if (activeJobId === job.job_id) {
    activeJobId = null;
    promoteQueued();
  }
}

function promoteQueued() {
  if (activeJobId != null) return;
  const next = queue.shift();
  if (!next) return;
  activeJobId = next.job_id;
  next.queue_position = null;
  next.eta_start_s = null;
  next.started_at = new Date().toISOString();
  next._startedMs = Date.now();
  refreshQueuePositions();
  runStages(next, 0);
}

function runStages(job, stageIndex) {
  if (job.status === "failed" || job.status === "done") return;
  if (stageIndex >= STAGE_DEFS.length) {
    finishJob(job, { status: "done" });
    return;
  }
  const def = STAGE_DEFS[stageIndex];
  const stageRow = job.stages[stageIndex];
  stageRow.state = "running";
  stageRow.started_at = new Date().toISOString();
  job._currentStage = def.id;
  job.status = statusForStage(def.id);
  job.stageText = `${def.label.toLowerCase()}`;
  const total = 40 + stageIndex * 17;
  const stageStartedMs = Date.now();

  const failure = classify(job.slug);
  // Fails partway through a representative early stage (parse, index 5),
  // late enough that the timeline and progress bar have something to show
  // first.
  if (failure && stageIndex === 5) {
    job._timer = setTimeout(() => {
      if (failure === "worker_crashed") {
        finishJob(job, {
          status: "failed",
          error: "worker exited unexpectedly (signal 9)",
          error_code: "worker_crashed",
        });
      } else {
        finishJob(job, {
          status: "failed",
          error: "indexing failed: unexpected token in some/file.py",
          error_code: "index_failed",
        });
      }
    }, TICK_MS);
    return;
  }

  let tick = 0;
  const step = () => {
    tick += 1;
    const done = Math.min(total, Math.round((total * tick) / TICKS_PER_STAGE));
    const elapsedStageS = (Date.now() - stageStartedMs) / 1000;
    job.progress = {
      stage: def.id,
      stage_index: stageIndex + 1,
      stage_count: STAGE_DEFS.length,
      label: def.label,
      unit: def.unit,
      done,
      // clone/detect steps model an indeterminate total, same as the real
      // worker before it has seen enough of the source to know file counts.
      total: stageIndex <= 1 ? null : total,
      rate_per_s: elapsedStageS > 0 ? done / elapsedStageS : null,
    };
    const remainingS = estimateRemainingS(job);
    job.eta = {
      low_s: Math.max(0, remainingS * 0.7),
      high_s: remainingS * 1.4 + 1,
      basis: stageIndex <= 2 ? "model" : stageIndex <= 8 ? "blend" : "rate",
    };
    publish(job);
    if (tick < TICKS_PER_STAGE) {
      job._timer = setTimeout(step, TICK_MS);
    } else {
      stageRow.state = "done";
      stageRow.duration_s = (Date.now() - stageStartedMs) / 1000;
      job._timer = setTimeout(() => runStages(job, stageIndex + 1), TICK_MS);
    }
  };
  job._timer = setTimeout(step, TICK_MS);
}

function startJob(slug) {
  const job_id = `job_${randomUUID()}`;
  const job = {
    job_id,
    slug,
    commit: null,
    status: "queued",
    stageText: "queued",
    queue_position: null,
    started_at: new Date().toISOString(),
    finished_at: null,
    error: null,
    error_code: null,
    progress: null,
    eta: null,
    eta_start_s: null,
    stages: newStages(),
    _startedMs: null,
    _finishedMs: null,
    _currentStage: null,
    _timer: null,
    _subscribers: new Set(),
  };
  jobs.set(job_id, job);
  if (activeJobId == null) {
    activeJobId = job_id;
    job.started_at = new Date().toISOString();
    job._startedMs = Date.now();
    runStages(job, 0);
  } else {
    queue.push(job);
    refreshQueuePositions();
  }
  return job;
}

function send(res, status, body) {
  res.writeHead(status, { "Content-Type": "application/json", "Access-Control-Allow-Origin": "*" });
  res.end(JSON.stringify(body));
}

const server = createServer((req, res) => {
  const url = new URL(req.url, `http://localhost:${PORT}`);

  if (req.method === "GET" && url.pathname === "/api/healthz") {
    return send(res, 200, { status: "ok" });
  }

  if (req.method === "POST" && url.pathname === "/api/index") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      let parsed;
      try {
        parsed = JSON.parse(body || "{}");
      } catch {
        return send(res, 400, { error: "invalid_request", message: "body is not valid JSON" });
      }
      const raw = parsed.repo ?? parsed.path;
      if (!raw || typeof raw !== "string") {
        return send(res, 400, { error: "invalid_request", message: "expected {\"repo\": ...} or {\"path\": ...}" });
      }
      const slugMatch = raw.match(/github\.com\/([^/]+)\/([^/]+?)(?:\.git)?\/?$/) ?? raw.match(/^([^/]+)\/([^/]+)$/);
      if (!slugMatch) {
        return send(res, 400, { error: "invalid_request", message: `couldn't parse a repository from "${raw}"` });
      }
      const slug = `${slugMatch[1]}/${slugMatch[2]}`;
      if (slug === cachedSlug) {
        return send(res, 200, { job_id: null, slug, status: "done", commit: "cached123" });
      }
      const job = startJob(slug);
      return send(res, 202, { job_id: job.job_id, slug: job.slug, status: "queued" });
    });
    return;
  }

  const jobMatch = url.pathname.match(/^\/api\/jobs\/([^/]+)$/);
  if (req.method === "GET" && jobMatch) {
    const job = jobs.get(jobMatch[1]);
    if (!job) return send(res, 404, { error: "not_found", message: "no such job" });
    return send(res, 200, snapshot(job));
  }

  const cancelMatch = url.pathname.match(/^\/api\/jobs\/([^/]+)\/cancel$/);
  if (req.method === "POST" && cancelMatch) {
    const job = jobs.get(cancelMatch[1]);
    if (!job) return send(res, 404, { error: "not_found", message: "no such job" });
    if (job.status === "done" || job.status === "failed") return send(res, 200, snapshot(job));
    const queuedIndex = queue.findIndex((j) => j.job_id === job.job_id);
    if (queuedIndex >= 0) queue.splice(queuedIndex, 1);
    finishJob(job, { status: "failed", error: "job cancelled", error_code: "cancelled" });
    refreshQueuePositions();
    return send(res, 200, snapshot(job));
  }

  const eventsMatch = url.pathname.match(/^\/api\/jobs\/([^/]+)\/events$/);
  if (req.method === "GET" && eventsMatch) {
    const job = jobs.get(eventsMatch[1]);
    if (!job) return send(res, 404, { error: "not_found", message: "no such job" });
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
      "Access-Control-Allow-Origin": "*",
    });
    res.write(`data: ${JSON.stringify(snapshot(job))}\n\n`);
    job._subscribers.add(res);
    req.on("close", () => job._subscribers.delete(res));
    if (job.status === "done" || job.status === "failed") res.end();
    return;
  }

  if (req.method === "GET" && url.pathname === "/api/maps") {
    return send(res, 200, [
      {
        slug: cachedSlug,
        owner,
        repo,
        lang: "py",
        files: 2,
        districts: 1,
        modularity: 0.42,
        commit: "cached123",
        indexed_at: new Date().toISOString(),
      },
    ]);
  }

  const mapMatch = url.pathname.match(/^\/api\/maps\/([^/]+)\/([^/]+)$/);
  if (req.method === "GET" && mapMatch) {
    const [, o, r] = mapMatch;
    const doc = docs.get(`${o}/${r}`);
    if (doc) return send(res, 200, doc);
    return send(res, 404, { error: "not_found", message: "not indexed" });
  }

  send(res, 404, { error: "not_found", message: `no route for ${req.method} ${url.pathname}` });
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`[mock-api-server] listening on http://127.0.0.1:${PORT}`);
  console.log(`[mock-api-server] cached map: ${cachedSlug} · submit e.g. "octocat/hello" to run a job`);
  console.log(`[mock-api-server] one job runs at a time -- submit a second slug while one is running to see the queued state`);
  console.log(`[mock-api-server] submit a slug containing "crashes" to see the worker_crashed stage-naming message`);
  console.log(`[mock-api-server] submit a slug containing "fails" to see a generic failure`);
});
