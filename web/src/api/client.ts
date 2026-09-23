// The one module with the API base and every request the site makes to the
// job/index service. See docs/ARCHITECTURE.md, "MVP: a site that maps any
// public repository" and the API contract in the milestone brief.
//
// Base resolution: VITE_API_BASE overrides everything; the default is
// same-origin `/api`, with the dev server proxying that to the service on
// 127.0.0.1 (vite.config.ts). Nothing here ever points at a public origin —
// there is no third option.
import type { MapDocument } from "@/types";

export const API_BASE = (import.meta.env.VITE_API_BASE as string | undefined)?.replace(/\/$/, "") || "/api";

export type JobStage = "queued" | "cloning" | "detecting" | "indexing" | "done" | "failed";

export interface JobStatus {
  job_id: string;
  slug: string;
  commit: string | null;
  status: JobStage;
  stage: string;
  queue_position: number | null;
  started_at: string;
  finished_at: string | null;
  /** Human-readable failure text. Flat, not an object — it is rendered
   * directly, and an object here crashes React. */
  error: string | null;
  /** Machine-readable failure code (`repo_too_large`, `detection_failed`, …).
   * Branch on this rather than on `error`'s wording. */
  error_code: string | null;
}

export interface IndexAccepted {
  job_id: string;
  slug: string;
  status: "queued";
}

export interface IndexCached {
  job_id: null;
  slug: string;
  status: "done";
  commit: string;
}

export type IndexResponse = IndexAccepted | IndexCached;

export interface ApiError {
  error: string;
  message: string;
}

export interface ServiceCatalogueEntry {
  slug: string;
  owner: string;
  repo: string;
  lang: string;
  files: number;
  districts: number;
  modularity: number;
  commit: string;
  indexed_at: string;
}

/** Thrown for any non-2xx response. `code` is the contract's `error` field
 * when the body parsed as one; `tooLarge` is set when the service's own
 * size-limit refusal is recognisable (see the `isTooLarge` heuristic below)
 * so callers can show that as its own state rather than a generic failure
 * (docs/ARCHITECTURE.md's "Limits" paragraph, and the milestone brief). */
export class ApiRequestError extends Error {
  status: number;
  code: string | null;
  tooLarge: boolean;
  constructor(status: number, body: Partial<ApiError> | null, fallbackMessage: string) {
    super(body?.message ?? fallbackMessage);
    this.name = "ApiRequestError";
    this.status = status;
    this.code = body?.error ?? null;
    // The contract does not name the exact `error` code for a repository
    // that is over the hosted limits — recognise it by the common code
    // spellings and, failing that, by the response status a size refusal
    // would plausibly use (413 Payload Too Large) or the message itself
    // naming a limit. This is a heuristic, not part of the contract; noted
    // in the handback as something to firm up once the real service exists.
    const code = (body?.error ?? "").toLowerCase();
    const message = (body?.message ?? "").toLowerCase();
    this.tooLarge =
      code.includes("too_large") ||
      code.includes("too-large") ||
      code.includes("size_limit") ||
      status === 413 ||
      /too large|size limit|exceeds.*(limit|cap)/.test(message);
  }
}

async function parseErrorBody(res: Response): Promise<Partial<ApiError> | null> {
  try {
    const body = await res.json();
    if (body && typeof body === "object") return body as Partial<ApiError>;
  } catch {
    /* not JSON, or empty body */
  }
  return null;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, init);
  if (!res.ok) {
    const body = await parseErrorBody(res);
    throw new ApiRequestError(res.status, body, `${path}: ${res.status} ${res.statusText}`);
  }
  return res.json() as Promise<T>;
}

/** POST /api/index. `input` is exactly the contract's union — a bare
 * `owner/name`, an https URL, or (unused by this client; service-local
 * only) a filesystem path. */
export function postIndexJob(input: { repo: string }): Promise<IndexResponse> {
  return request<IndexResponse>("/index", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
}

export function getJob(jobId: string): Promise<JobStatus> {
  return request<JobStatus>(`/jobs/${encodeURIComponent(jobId)}`);
}

export function getServiceCatalogue(): Promise<ServiceCatalogueEntry[]> {
  return request<ServiceCatalogueEntry[]>("/maps");
}

export function getServiceMapDocument(owner: string, repo: string, commit?: string): Promise<MapDocument> {
  const qs = commit ? `?commit=${encodeURIComponent(commit)}` : "";
  return request<MapDocument>(`/maps/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}${qs}`);
}

/** Reachability probe used to pick a data source (see src/data/queries.ts).
 * A short timeout matters here: with no service running, same-origin `/api`
 * either 404s immediately (no proxy) or hangs until the proxy's own timeout
 * (proxy configured, nothing listening) — either way the UI should not sit
 * on a fallback decision for long. */
export async function pingService(timeoutMs = 2500): Promise<boolean> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const res = await fetch(`${API_BASE}/healthz`, { signal: controller.signal });
    return res.ok;
  } catch {
    return false;
  } finally {
    clearTimeout(timer);
  }
}

export function jobEventsUrl(jobId: string): string {
  return `${API_BASE}/jobs/${encodeURIComponent(jobId)}/events`;
}
