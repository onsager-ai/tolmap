#!/usr/bin/env node
// A local stub of the job/index service's contract (see the milestone
// brief's "The API contract" and docs/ARCHITECTURE.md's MVP section), used
// only to drive the submit -> SSE -> map path end to end during
// development and manual/Playwright verification. Not built, not shipped,
// not referenced by any production code path — vite.config.ts's dev proxy
// points at it only when TOLMAP_API_PROXY_TARGET names it.
//
// Usage: node scripts/mock-api-server.mjs [port]
import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

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

const jobs = new Map();
// Map documents by slug, seeded with the one pre-cached repo. A job that
// reaches "done" registers its own slug here too, so GET
// /api/maps/{owner}/{repo} has something to return for a freshly indexed
// repo, not just the pre-cached one.
const docs = new Map([[cachedSlug, cachedDoc]]);
const STAGES = [
  { status: "queued", stage: "queued" },
  { status: "cloning", stage: "cloning repository (shallow, blob-less)" },
  { status: "detecting", stage: "detecting language and source root" },
  { status: "indexing", stage: "indexing (extract, blend, partition, layout)" },
  { status: "done", stage: "done" },
];

// A slug containing "toolong" or "huge" fails fast as a size-limit refusal,
// so the "too large" state (docs/ARCHITECTURE.md's Limits paragraph) is
// reachable without waiting through the whole stage sequence. A slug
// containing "fails" fails for an ordinary reason instead.
function classify(slug) {
  if (/toolong|huge/i.test(slug)) return "too_large";
  if (/fails/i.test(slug)) return "generic";
  return null;
}

function startJob(slug) {
  const job_id = `job_${Math.random().toString(36).slice(2, 10)}`;
  const failure = classify(slug);
  const job = {
    job_id,
    slug,
    commit: null,
    status: "queued",
    stage: STAGES[0].stage,
    started_at: new Date().toISOString(),
    finished_at: null,
    error: null,
    _stepAt: 0,
    _failure: failure,
    _subscribers: new Set(),
  };
  jobs.set(job_id, job);
  advance(job);
  return job;
}

function publish(job) {
  const payload = JSON.stringify(snapshot(job));
  for (const res of job._subscribers) res.write(`data: ${payload}\n\n`);
}

function snapshot(job) {
  const { job_id, slug, commit, status, stage, started_at, finished_at, error } = job;
  return { job_id, slug, commit, status, stage, started_at, finished_at, error };
}

function advance(job) {
  const delay = 600 + Math.random() * 500;
  setTimeout(() => {
    // Fail partway through cloning for a "too large" or generic failure,
    // so both cases are reachable well before the full sequence completes.
    if (job._failure && job._stepAt === 1) {
      job.status = "failed";
      job.stage = "failed";
      job.finished_at = new Date().toISOString();
      job.error =
        job._failure === "too_large"
          ? "repository exceeds the hosted index's 200,000-file / 4GB clone size limit"
          : "clone failed: repository not found or not public";
      publish(job);
      closeSubscribers(job);
      return;
    }
    job._stepAt += 1;
    const step = STAGES[Math.min(job._stepAt, STAGES.length - 1)];
    job.status = step.status;
    job.stage = step.stage;
    if (job.status === "done") {
      job.commit = "abc1234";
      job.finished_at = new Date().toISOString();
      docs.set(job.slug, { ...FIXTURE_DOC, repo: job.slug.split("/")[1] });
      publish(job);
      closeSubscribers(job);
      return;
    }
    publish(job);
    advance(job);
  }, delay);
}

function closeSubscribers(job) {
  for (const res of job._subscribers) res.end();
  job._subscribers.clear();
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
        return send(res, 400, { error: "bad_request", message: "body is not valid JSON" });
      }
      const raw = parsed.repo ?? parsed.path;
      if (!raw || typeof raw !== "string") {
        return send(res, 400, { error: "bad_request", message: "expected {\"repo\": ...} or {\"path\": ...}" });
      }
      const slugMatch = raw.match(/github\.com\/([^/]+)\/([^/]+?)(?:\.git)?\/?$/) ?? raw.match(/^([^/]+)\/([^/]+)$/);
      if (!slugMatch) {
        return send(res, 400, { error: "bad_request", message: `couldn't parse a repository from "${raw}"` });
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
  console.log(`[mock-api-server] submit a slug containing "toolong" or "huge" to see the too-large state`);
  console.log(`[mock-api-server] submit a slug containing "fails" to see a generic failure`);
});
