import { http, HttpResponse, delay } from "msw";
import { setupWorker } from "msw/browser";
import type { JobSnapshot, MapDocument } from "../src/types";
import type { StageId } from "../../bindings/StageId";

export const commit = "1111111111111111111111111111111111111111";
export const slug = "onsager-ai/tolmap";
const worker = setupWorker();
let started: Promise<unknown> | undefined;
let map: MapDocument;
const nativeFetch = window.fetch.bind(window);
const asset = (name: string) => new URL(`fixtures/${name}`, location.href).href;
const stageIds: StageId[] = ["clone", "clone_objects", "clone_deltas", "clone_checkout", "detect", "parse", "resolve", "install", "index_go", "index_py", "index_ts", "history", "blend_prune", "partition", "neighbourhoods", "naming", "regions", "footprints", "write_map", "symbols", "symbol_cards", "write"];
const phases: Record<string, StageId> = { fetch: "clone_objects", read: "parse", map: "partition", detail: "symbols", early: "symbols" };

function snapshot(state: string): JobSnapshot {
  const active = phases[state];
  const failed = !active && state !== "queued";
  const index = stageIds.indexOf(active ?? "parse");
  return {
    job_id: "fixture", slug, commit, status: state === "queued" ? "queued" : failed ? "failed" : state === "fetch" ? "cloning" : "indexing",
    stage: active ?? state, queue_position: state === "queued" ? 2 : null,
    started_at: new Date(Date.now() - 72_000).toISOString(), finished_at: failed ? new Date().toISOString() : null,
    error: failed ? "Synthetic qualification message. No repository or provider was contacted." : null,
    error_code: failed ? state : null, eta: active ? { low_s: 180, high_s: 300, basis: "model" } : null,
    eta_start_s: state === "queued" ? 130 : null, elapsed_s: 72, map_ready: state === "early",
    progress: active ? { stage: active, stage_index: index + 1, stage_count: stageIds.length, label: active, unit: "files", done: 72n, total: 120n, rate_per_s: 4 } : null,
    stages: stageIds.map((id, i) => ({ id, label: id, state: state === "queued" ? "pending" : i < index ? "done" : i === index ? failed ? "failed" : "running" : "pending", started_at: null, duration_s: i < index ? 3 : null })),
  };
}
const json = (value: unknown, status = 200) => new HttpResponse(JSON.stringify(value, (_key, value) => typeof value === "bigint" ? Number(value) : value), { status, headers: { "Content-Type": "application/json" } });

export async function installFixture(state: string) {
  if (!map) map = await (await nativeFetch(asset("map.json"))).json() as MapDocument;
  let job = snapshot(state);
  const streams = new Set<ReadableStreamDefaultController<Uint8Array>>();
  const emit = () => new TextEncoder().encode(`data: ${JSON.stringify(job, (_key, value) => typeof value === "bigint" ? Number(value) : value)}\n\n`);
  const entry = { slug, owner: "onsager-ai", repo: "tolmap", label: "acme/platform", files: map.F.length, districts: Object.keys(map.districts).length, lang: "rs", modularity: map.q, commit, indexed_at: "2026-10-19T08:00:00Z" };
  const handler = async ({ request }: { request: Request }) => {
    const url = new URL(request.url), path = url.pathname;
    if (path === "/api/healthz") return json({ status: "ok" });
    if (path === "/api/maps" || path === "/maps/index.json") {
      if (state === "loading") { await delay("infinite"); return json([]); }
      return state === "unavailable" ? json({ error: "unavailable" }, 503) : json(state === "empty" ? [] : [entry]);
    }
    if (path.endsWith("/events")) {
      const body = new ReadableStream<Uint8Array>({
        start(controller) { streams.add(controller); controller.enqueue(emit()); },
        cancel() { /* Closed by the browser when the story unmounts. */ },
      });
      return new HttpResponse(body, { headers: { "Content-Type": "text/event-stream" } });
    }
    if (path.endsWith("/cancel")) {
      job = snapshot("cancelled");
      for (const stream of streams) stream.enqueue(emit());
      return json(job);
    }
    if (path.startsWith("/api/jobs/")) return json(job);
    if (path === "/api/index") return json({ job_id: "fixture", slug, status: "queued" }, 202);
    if (path.endsWith("/provenance")) {
      if (state === "source-loading") { await delay("infinite"); }
      return state === "source-unavailable" ? json({}, 403) : json({ version: 1, repository: slug, commit });
    }
    if (path.endsWith("/source")) {
      const status = state.replace("source-", "");
      const text = status === 'long-line' ? `// Synthetic long-line fixture\n${'x'.repeat(20_001)}` : "// Synthetic source fixture; not the repository's contents.\nexport function navigateMap() {\n  return 'districts';\n}\n";
      return json({ version: 1, repository: slug, commit, path: url.searchParams.get("path"), status: ["ready", "long-line", "search", "search-empty"].includes(status) ? "ready" : status, text, bytes: new TextEncoder().encode(text).length, long_line: status === "long-line" });
    }
    if (path.endsWith("/symbols")) {
      const response = await nativeFetch(asset(`tolmap.symbols/${url.searchParams.get("district")}.json`));
      return response.ok ? new HttpResponse(await response.text(), { headers: { "Content-Type": "application/json" } }) : json({}, 404);
    }
    if (path.startsWith("/api/maps/") || /^\/maps\/.+\.json$/.test(path)) {
      if (state === "map-loading") { await delay("infinite"); }
      return state === "map-unavailable" ? json({ error: "not_found" }, 404) : json(map);
    }
    return json({ error: "unhandled_fixture_request", path }, 500);
  };
  worker.resetHandlers(http.all("/api/*", handler), http.all("/maps/*", handler));
  started ??= worker.start({ quiet: true, serviceWorker: { url: new URL("mockServiceWorker.js", location.href).pathname }, onUnhandledRequest(request, print) {
    if (new URL(request.url).pathname.startsWith("/api/")) print.error();
  } });
  await started;
  return map;
}

export function resolveRoute(route: string, doc: MapDocument): string {
  const district = Object.keys(doc.districts).sort((a, b) => Number(a) - Number(b))[0];
  const symbolFile = Object.keys(doc.S).sort((a, b) => Number(a) - Number(b)).find(key => doc.S[key]?.length);
  if (route === "symbol" && symbolFile == null) throw new Error("The committed Storybook map fixture has no symbol-backed file for the symbol-selection story");
  const file = route === "symbol" && symbolFile != null ? doc.F[Number(symbolFile)] : doc.F.find(path => path.endsWith(".rs") && path.includes("/")) ?? doc.F[0];
  const params = new URLSearchParams({ geo: "r", layer: "d" });
  if (route === "district") params.set("d", district);
  if (["file", "symbol", "source"].includes(route)) params.set("file", file);
  if (route === "symbol") params.set("sym", "0");
  if (route === "folder") params.set("dir", file.split("/").slice(0, -1).join("/"));
  if (route === "source") { params.set("read", "true"); params.set("commit", commit); params.set("line", "2"); params.set("end", "3"); }
  return ["district", "file", "symbol", "folder", "source"].includes(route) ? `/${slug}?${params}` : route;
}
