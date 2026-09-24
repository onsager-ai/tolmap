# Job service API

`tolmap serve` runs an axum service that accepts a repository, indexes it as
a background job, streams progress, and serves the resulting map. It binds
to `127.0.0.1` by default -- see `docs/ARCHITECTURE.md`'s "Limits" section
for why. Binding off-box was out of scope until 2026-09-20, when the Fly.io
deployment made it an explicit opt-in: set `TOLMAP_BIND_ADDR` to widen it. A
developer who sets nothing still gets loopback-only, unchanged.

**Default listen address is `127.0.0.1:8787`** (the frontend's dev proxy
defaults to the same address, overridable there via
`TOLMAP_API_PROXY_TARGET`). The port is configurable service-side via the
`TOLMAP_PORT` environment variable; the host via `TOLMAP_BIND_ADDR` -- see
`src/service/config.rs`.

Optionally, the service can also serve the built web bundle itself, so one
origin answers both the API and the site: set `TOLMAP_STATIC_DIR` to the
built `web/dist` directory. Unset (the default), the router registers only
`/api/*`, exactly as before this existed -- local development keeps running
the Vite dev server separately and proxying `/api` to this service
(`web/vite.config.ts`). When set, everything under `/api/` keeps its own
JSON 404s; every other path falls back to `index.html` so the client-side
router can own it (`GET /django/django` on a cold load, for instance) --
see `src/service/http.rs::router`.

Responses under `/api/*` and files served from `TOLMAP_STATIC_DIR` may use
`Content-Encoding: gzip` when the request accepts gzip. Clients that do not
send `Accept-Encoding: gzip` receive the original bytes. SSE job events stay
uncompressed so frames can stream as they are produced.

This document is the contract. It is written and committed before the
implementation so the frontend and the service can be built in parallel
against the same shape; the implementation must not drift from it without
updating this file in the same change.

## Configuration

Everything below is read once, at startup, from the environment; an unset
or unparsable variable falls back to the default rather than failing
startup. There is no config file for the service (`.tolmap/config.toml` is
a per-repository thing, not a deployment thing -- docs/ARCHITECTURE.md).

| variable | default | what |
|---|---|---|
| `TOLMAP_PORT` | `8787` | listen port |
| `TOLMAP_BIND_ADDR` | `127.0.0.1` | listen host -- loopback unless explicitly widened (2026-09-20) |
| `TOLMAP_STATIC_DIR` | unset | serve the built web bundle from this directory alongside the API when set (see above) |
| `TOLMAP_PRUNE_VARIANT` | `node-relative` | blend/prune route: `absolute`, `percentile`, `node-relative`, or `pre-rescale`; unset or invalid uses `node-relative` |
| `TOLMAP_DB_PATH` | `<TOLMAP_CACHE_DIR>/tolmap.sqlite3` | the SQLite store |
| `TOLMAP_CACHE_DIR` | system temp dir `/tolmap-cache` | clone cache + indexed map files |
| `TOLMAP_MAX_FILES` | `5000` | reject a repo with more source files than this after detection |
| `TOLMAP_MAX_CLONE_BYTES` | `2147483648` (2 GiB) | reject a clone whose working tree + `.git` exceeds this |
| `TOLMAP_MAX_HISTORY_COMMITS` | `200000` | reject a repo whose `HEAD` history has more commits than this -- does not bound indexing cost (`extract.rs` caps its own history read at 4000 regardless of depth); see `service::config::Limits::max_history_commits`'s doc comment for what it actually guards on each of `service::clone::materialize`'s two request paths (pre-clone on a local path, a narrower post-clone refusal on a remote one). Default was 20000 until 2026-09-20, which rejected django (34942 commits) |
| `TOLMAP_MAX_JOB_SECONDS` | `900` | wall-clock budget for one job before it fails as `index_failed` |
| `TOLMAP_MAX_CONCURRENT_JOBS` | `1` | maximum blocking index jobs running at once; zero is treated as one |
| `TOLMAP_MAX_QUEUED_JOBS` | `16` | pending jobs allowed beyond running jobs; zero disables waiting |
| `TOLMAP_RATE_LIMIT_PER_IP` | `30` | requests per window, per source IP, under `/api/` |
| `TOLMAP_RATE_LIMIT_WINDOW_SECONDS` | `60` | window for the per-IP limit |
| `TOLMAP_RATE_LIMIT_PER_REPO` | `3` | `POST /api/index` requests per window, per slug |
| `TOLMAP_RATE_LIMIT_PER_REPO_WINDOW_SECONDS` | `300` | window for the per-repo limit |
| `TOLMAP_RETAIN_COMMITS_PER_REPO` | `20` | indexed commits kept per slug before older ones are pruned (see "Store" below) |

All limit/rate-limit variables map directly to `service::config::Limits`'s
fields, which is the single place their defaults are documented and the
only place that reads them from the environment -- request-handling code
only ever sees a resolved `Limits` value, never `std::env::var` itself.

## Repository identifiers

A request names a repository three ways:

- `{"repo": "owner/name"}` -- resolved against `https://github.com/<owner>/<name>.git`.
- `{"repo": "<https url>"}` -- cloned directly; owner/repo are parsed from
  the last two path segments (`.git` suffix stripped).
- `{"path": "<local path>"}` -- an already-checked-out local directory, used
  for tests and fixtures without hitting the network. Its `slug` is
  `local/<basename>`, so `/tmp/tolmap-fixtures/flask` becomes `local/flask`.

Every repository has a canonical `slug` of the form `owner/repo`.
`owner`/`repo` are lowercased on input (GitHub treats them
case-insensitively, so `Owner/Repo` and `owner/repo` must resolve to the
same cache entry rather than being cloned and indexed twice -- see
`service::clone::canonicalize`); the originally submitted casing is not
retained anywhere. `GET /api/maps/{owner}/{repo}` lowercases its path
parameters the same way before looking a slug up. The slug, not the input
string, is the cache key's non-commit half and the path segment used
everywhere else in this API.

## Endpoints

### `GET /api/maps/{owner}/{repo}/symbols?district=<id>[&commit=<sha>]`

Returns the symbols for one district of the latest indexed commit, or the
specified commit. `district` is required and is the numeric district id in
the map's `N` rows. Unknown maps, commits, districts and older commits
without a symbols sibling return 404. The
response contains `district`, its `files` (indices into the map's `F`),
`symbol_indices`, `symbols`, `edges`, and `module_code_lines`. Each symbol row
is `[file, name, kind, start_line, end_line, parent, code_lines]`; `parent`
and every edge endpoint are **global symbol indices**. Kinds are 0 class,
1 function, 2 method, 3 nested function, 4 interface, 5 type, 6 constant.
An edge is `[source, target, occurrences]`. An edge crossing districts is
included from both sides, with both endpoint rows and their global indices
so a client can place the far endpoint without fetching another district.
`module_code_lines` maps file index to code lines outside top-level symbols.
`symbol_rings`, `module_rings`, and `header_rings` are optional card geometry.
Each contour is a flat integer list `[x₀, y₀, Δx₁, Δy₁, …]` in units of
10⁻¹¹ world coordinates. Accumulate the deltas, then divide by 10¹¹ to draw
it. A card's first contour is its exterior; later contours are holes under
even-odd fill. The same encoding appears in static district files.
The full sibling `<name>.symbols.json` also has `files`, `symbols`, `edges`,
`module_code_lines` and `coverage` (`calls_total` inside symbols,
`calls_resolved`, and `unresolved` counts by reason). Coverage lives there to keep the initial map
document small. The same sibling is copied to `/maps/<owner>/<repo>.symbols.json`
when a static map includes one.
`tolmap build` also writes `<name>.symbols/<district>.json` for every district.
The service reads that file directly for commits that have the directory,
without parsing the full map or symbols document. For older indexed commits
without the directory, it projects from their full symbols sibling. Each
file has the same JSON object returned by this endpoint for that district,
including any symbols at the far ends of crossing edges. Static map collection
copies this directory to `/maps/<owner>/<repo>.symbols/`, so a static client can
fetch one district at `/maps/<owner>/<repo>.symbols/<district>.json`. The full
symbols sibling remains available for older commits and determinism checks.

### `POST /api/index`

Body: `{"repo": "owner/name"}` or `{"repo": "<https url>"}` or `{"path": "<local path>"}`.

A fresh index is queued:

```
202 Accepted
{"job_id": "<uuid>", "slug": "owner/name", "status": "queued"}
```

If the same `(slug, commit)` is already queued or running, the request gets
that job's id with the same `202` body. The per-repo rate limit still applies.
When all workers and pending slots are occupied, a new job gets `503 busy`
with `Retry-After: 30` (seconds). A duplicate can still join a full queue.

The commit already has a cached map (see "Store" below):

```
200 OK
{"job_id": null, "slug": "owner/name", "status": "done", "commit": "<sha>"}
```

Determining "already cached" requires resolving HEAD, which is cheap (a
`git ls-remote` for a remote repo, a `git rev-parse` for a local path) and is
done synchronously before a job is created -- this is why the cache hit can
be answered in the same request rather than always queuing a job.

### `GET /api/jobs/{job_id}`

```
{
  "job_id": "<uuid>",
  "slug": "owner/name",
  "commit": "<sha>" | null,
  "status": "queued" | "cloning" | "detecting" | "indexing" | "done" | "failed",
  "stage": "<human-readable current step>",
  "queue_position": 1 | null,
  "started_at": "<RFC3339>",
  "finished_at": "<RFC3339>" | null,
  "error": "<human text>" | null,          // rendered directly by clients
  "error_code": "<machine code>" | null,   // branch on this, not on the text
  "progress": {
    "stage": "<stable stage id>", "stage_index": 1, "stage_count": 18,
    "label": "Parsing files", "unit": "files", "done": 123,
    "total": 500 | null, "rate_per_s": 23.5 | null,
    "transfer_bytes": 1048576,             // optional, git transfer only
    "transfer_rate_bytes_per_s": 524288.0 // optional, git transfer only
  } | null,
  "elapsed_s": 12.3,
  "stages": [
    {"id": "parse", "label": "Parsing files", "state": "pending" | "running" | "done" | "failed",
     "started_at": "<RFC3339>" | null, "duration_s": 1.5 | null}
  ]
}
```

`commit` is the commit resolved before admission. `queue_position` is a
one-based FIFO position while waiting, updated when jobs ahead start. It is
`null` once running and in terminal states. The 900-second job clock starts
when a worker starts the job, so queue waiting does not count. A timed-out
child can continue, and holds its worker slot until it exits. `elapsed_s`
is updated with each worker event and on completion. Progress counters never
decrease for a stage during a job, even when a multi-source build repeats
parsing and resolution. `total` may be unknown and can grow as another source
starts. Stage IDs are stable, in pipeline order; clone sub-stages may start
more than once during a fetch and checkout. Repeated stage durations are
summed in `stages`. A child that exits without a result or error event
fails the job with `worker_crashed`, including its exit status and last stage.
`stage` is a short
free-text description of what is happening right now (e.g. `"cloning
github.com/django/django"`, `"indexing (partition)"`) -- it is for display,
not for matching on; only `status` is a stable enum.

### `GET /api/jobs/{job_id}/events`

Server-Sent Events. The current snapshot is sent immediately, then changed
snapshots are sent at most about four times per second, each frame the same
JSON shape as `GET /api/jobs/{job_id}` above. The stream ends (the
connection closes) after the frame carrying `status: "done"` or
`status: "failed"` is sent. A client that reconnects after a drop should
`GET /api/jobs/{job_id}` first to catch up, since SSE here does not replay
frames sent before the connection opened. During quiet stages, an SSE
heartbeat comment is sent every 15 seconds.

## Worker protocol (v1)

`tolmap worker` reads one JSON `WorkerSpec` line from stdin and writes one
JSON event per stdout line. Events carry `v: 1` and a `type` of
`stage_started`, `progress`, `stage_finished`, `log`, `result`, or `error`.
`stage_finished` has `duration_s` and `success`. `result` carries the map,
symbols sibling, district symbols directory, names cache paths, commit and
branch, plus language, file count, district count and modularity. `error`
carries a machine `code` and human `message`. The Rust definitions in
`src/worker.rs` and generated `bindings/WorkerEvent.ts` are authoritative.

The service owns its SQLite store and queue. It gives the child clone source,
limits, output directory, previous map candidates and a names cache file.
The child chooses the newest previous map on the cloned branch, falling back
to the newest overall, then clones, detects and builds. The service registers
the returned artifacts only after a successful terminal result. The job spec
also accepts `all_sources: true` to union every detected source that clears
the detector's floor; the service currently sends `false` and retains its
existing single-source confidence check.

### `GET /api/maps`

```
200 OK
[
  {"slug","owner","repo","lang","files","districts","modularity","commit","indexed_at"},
  ...
]
```

One row per `(slug)`, showing its most recently indexed commit. Ordered by
`indexed_at` descending.

### `GET /api/maps/{owner}/{repo}`

The `MapDocument` (see `src/schema.rs`, `bindings/MapDocument.ts`) for the
most recently indexed commit of `owner/repo`. `?commit=<sha>` fetches a
specific previously-indexed commit instead of the latest. `404` if the
slug, or that commit of it, has never been indexed.

### `GET /api/healthz`

```
200 OK
{"status": "ok"}
```

## Errors

Every non-2xx response is JSON:

```
{"error": "<machine code>", "message": "<human text>"}
```

| status | `error` | when |
|---|---|---|
| 400 | `invalid_request` | body does not match the contract above |
| 404 | `not_found` | unknown job id, or unknown slug/commit for `GET /api/maps/{owner}/{repo}` |
| 413 | `repo_too_large` | a configured limit was tripped -- `message` names which one and its value (e.g. `"file count 1204 exceeds the configured limit of 1000"`). **Fixed exactly** (code and status), by agreement with the frontend, which detects this by exact match rather than a heuristic. |
| 429 | `rate_limited` | per-IP or per-repo rate limit tripped -- `message` says which |
| 503 | `busy` | all workers and pending queue slots are occupied; `Retry-After: 30` seconds |
| 422 | `detection_failed` | `detect::detect` (issue #4) found no supported source at all -- an empty or non-source repository, not a size or confidence problem |
| 422 | `detection_uncertain` | detection succeeded but at `Confidence::Low` -- `message` is the chosen candidate's evidence text. Never indexed silently: a wrong source root produces a plausible-looking wrong map (finding 7) |
| 502 | `clone_failed` | git clone/fetch failed (bad URL, network, repo does not exist) |
| 500 | `index_failed` | the indexing pipeline itself errored on an otherwise-valid repository |
| 500 | `internal_error` | a bug, not a caller or repository problem |

A job that fails carries the same `error`/`message` shape in its `error`
field (`GET /api/jobs/{job_id}` and the SSE stream), with `status: "failed"`
instead of an HTTP error status, since the job accepted successfully at
`202` and failed later. A job that fails with `repo_too_large` after
already being accepted still carries that exact code in its `error` field,
for the same reason the HTTP shape is fixed: the frontend renders it as its
own "too large for this index" state, retry disabled, rather than a generic
failure.

**A repository rejected for size must never present as a timeout.** Every
limit check that can run before the expensive stages (file count from
detection, clone size on disk, history depth) runs first and fails fast
with `repo_too_large` naming the limit, rather than letting indexing start
and time out.

## Store

SQLite. Cache key is `(slug, commit_sha)`: a second `POST /api/index` for a
commit already indexed returns the cached map immediately (see above), and
`GET /api/maps/{owner}/{repo}` without `?commit=` serves the most recent one
on record. See `src/service/store.rs` for the schema and why the map
document itself is kept as a content-addressed file next to the database
rather than a blob column.

**Pruned, not kept forever.** After a job finishes indexing, the store
keeps only the `TOLMAP_RETAIN_COMMITS_PER_REPO` most-recently-indexed
commits for that slug (default 20); older `(slug, commit_sha)` rows and
their map files are deleted. The single newest row for a slug is never
pruned, regardless of that setting -- docs/FINDINGS.md finding 4's warm
start reads the previous commit's district membership out of exactly this
row, so a policy that could evict it would silently degrade the highest-
leverage property the store has (see `service::store::Store::prune`).
