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
| `TOLMAP_REFS` | `hand` | reference graph for job maps: `hand` (tree-sitter resolver) or `scip` (issue #110: SCIP indexers on the worker's `PATH`, per-language fallback to `hand`, recorded in the map's `coverage.references`); unset or invalid uses `hand` |
| `TOLMAP_DB_PATH` | `<TOLMAP_CACHE_DIR>/tolmap.sqlite3` | the SQLite store |
| `TOLMAP_CACHE_DIR` | system temp dir `/tolmap-cache` | clone cache + indexed map files |
| `TOLMAP_CLONE_CACHE_BYTES` | `2147483648` (2 GiB) | total clone-cache LRU eviction budget; never rejects or evicts the active clone |
| `TOLMAP_MAX_CONCURRENT_JOBS` | `1` | maximum blocking index jobs running at once; zero is treated as one |
| `TOLMAP_MAX_QUEUED_JOBS` | `16` | pending jobs allowed beyond running jobs; zero disables waiting |
| `TOLMAP_RATE_LIMIT_PER_IP` | `30` | requests per window, per source IP, under `/api/` |
| `TOLMAP_RATE_LIMIT_WINDOW_SECONDS` | `60` | window for the per-IP limit |
| `TOLMAP_RATE_LIMIT_PER_REPO` | `3` | `POST /api/index` requests per window, per slug |
| `TOLMAP_RATE_LIMIT_PER_REPO_WINDOW_SECONDS` | `300` | window for the per-repo limit |
| `TOLMAP_RETAIN_COMMITS_PER_REPO` | `20` | indexed commits kept per slug before older ones are pruned (see "Store" below) |
| `TOLMAP_SCIP_TYPESCRIPT` | `scip-typescript` on `PATH` | path to the scip-typescript binary (issue #110 P1a); the runtime image's `Dockerfile` sets this to an absolute path, overridable for a different install |
| `TOLMAP_SCIP_PYTHON` | `scip-python` on `PATH` | path to the scip-python binary, same pattern |
| `TOLMAP_SCIP_GO` | `scip-go` on `PATH` | path to the scip-go binary, same pattern |
| `TOLMAP_SCIP_INSTALL` | `off` | with `TOLMAP_REFS=scip` (issue #110 P1c): `sandbox` installs a TypeScript workspace's npm dependencies (pnpm or npm lockfile) before scip-typescript, inside nsjail -- see "Worker isolation" below; `off` never installs. Anything short of a finished install falls back to indexing without it and is recorded in the map's `coverage.references.ts.install`; no job fails because of it. Any value but `sandbox` means `off` |
| `TOLMAP_NSJAIL` | `nsjail` on `PATH` | path to nsjail; the runtime image sets `/usr/local/bin/nsjail` |
| `TOLMAP_INSTALL_NODE_PREFIX` | prefix of `node` on `PATH` | the Node.js prefix mounted read-only in the install sandbox, whose `bin` has `node`, `npm` and `pnpm`; the runtime image sets `/opt/node` |
| `TOLMAP_INSTALL_TIME_LIMIT_S` | `1200` (20 min) | wall-time bound of one install, in seconds (fractions allowed); past it the install is killed and falls back (`install_timeout`) |
| `TOLMAP_INSTALL_DISK_BUDGET_BYTES` | `20000000000` (20 GB) | bytes one install may add to the checkout and its sandbox home; past it the install is killed and falls back (`install_disk_budget`) |
| `TOLMAP_INSTALL_MEMORY_MAX` | unset (off) | bytes: puts the install in a memory cgroup, with a 4,096-pid limit. Off by default because nsjail's cgroups on the production host's cgroup v1 are unverified, and a cgroup that cannot be created makes every install fall back |

These queue, cache and rate settings map to `service::config::Limits`. There are no file-count, clone-size, history-depth or job-time admission caps. The co-change algorithm still reads at most 4000 commits per build; that horizon does not reject a repository with deeper history.

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
is `[file, name, kind, start_line, end_line, parent, code_lines, abstract]`; `parent`
and every edge endpoint are **global symbol indices**. Kinds are 0 class,
1 function, 2 method, 3 nested function, 4 interface, 5 type, 6 constant.
An edge is `[source, target, occurrences, kind]`. `kind` indexes the response's
`kinds` legend: 0 unknown, 1 call, 2 extends, 3 implements, 4 overrides,
5 annotation, 6 decorator, 7 value, 8 possible_implementation. One pair can
have several rows, with occurrence counts kept separately. The unknown kind
is used when loading an older three-column edge. An older seven-column
symbol row loads with `abstract = false`. Go `possible_implementation` means
names and parameter/result counts match; without type checking, it does not
prove Go interface satisfaction. An edge crossing districts is
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
`calls_resolved`, `inherited_calls_resolved`, `possible_implementations`,
and `unresolved` counts by reason). Coverage lives there to keep the initial map
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

While the service is shutting down (a SIGINT/SIGTERM graceful-stop is in
progress -- see "Graceful shutdown" below), every `POST /api/index` is
rejected with:

```
503 Service Unavailable
{"error": "server_stopping", "message": "the service is shutting down and is not accepting new jobs"}
```

instead of being queued. This is permanent for the life of the process --
unlike `busy`, no `Retry-After` is sent, since the caller should retry
against a different instance or later, not against this one.

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
    "stage": "parse", "stage_index": 6, "stage_count": 18,
    "label": "Parsing files", "unit": "files", "done": 123,
    "total": 500 | null, "rate_per_s": 23.5 | null,
    "transfer_bytes": 1048576,             // optional, git transfer only
    "transfer_rate_bytes_per_s": 524288.0 // optional, git transfer only
  } | null,
  "eta": {"low_s": 15.0, "high_s": 32.0, "basis": "model" | "rate" | "blend"} | null,
  "eta_start_s": 44.0 | null,
  "elapsed_s": 12.3,
  "stages": [
    {"id": "parse", "label": "Parsing files", "state": "pending" | "running" | "done" | "failed",
     "started_at": "<RFC3339>" | null, "duration_s": 1.5 | null}
  ]
}
```

`commit` is the commit resolved before admission. `queue_position` is a
one-based FIFO position while waiting, updated when jobs ahead start. It is
`null` once running and in terminal states. For queued jobs, `eta_start_s` is the sum of the estimated remaining time of the running job and the estimated runtimes of jobs ahead in FIFO order. It updates as jobs ahead progress; it is null once running or terminal. `eta` is a remaining-time range for the job itself. The model starts from finding 36 and corpus timings, refits from completed job stages in SQLite, and blends current-stage progress with an EWMA rate. The interval widens when file count exceeds observed training sizes. Either field may be null when unknown. No wall-time timeout is imposed. `elapsed_s`
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

### `POST /api/jobs/{job_id}/cancel`

Returns the current `JobSnapshot` with `200 OK`. Cancelling a queued job removes it from the FIFO queue and updates later positions and start estimates. Cancelling a running job kills the worker process group, including git children, and releases the slot only after the child exits. Both finish as `status: "failed"` with `error_code: "cancelled"`; the existing status enum stays compatible with older clients. Repeating the request returns the same terminal snapshot. Unknown IDs return 404. The endpoint has the same per-IP and per-repo request rate limits as `POST /api/index`.

### Graceful shutdown

On SIGINT or SIGTERM, `tolmap serve` stops admitting new jobs (see
`POST /api/index` above) and fails every job currently queued or running --
killing any worker process group already started, the same way cancelling
that job would -- with `status: "failed"` and `error_code: "server_stopping"`
(`error: "the service is shutting down"`), before the process exits. This is
what makes an operator-initiated stop (a deploy, a resize, a platform
auto-stop, if your host has one) observable rather than silent: a client
watching `GET /api/jobs/{job_id}/events` gets a final SSE frame carrying the
terminal snapshot before its connection closes, and a client that only
polls `GET /api/jobs/{job_id}` sees the same terminal state on its next
request instead of the job hanging forever.

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
`stage_started`, `progress`, `stage_finished`, `features`, `log`, `result`,
`error`, or `install_request`. A spec with `install: "sandbox"` (issue #110
P1c) keeps the worker's stdin open: just before scip-typescript, a worker
whose checkout the install policy accepts sends `install_request` and blocks
until the service writes one `InstallCoverage` JSON line back (the same
record the map carries). The request has no parameters; the service reads
the policy from the job's checkout itself.
`stage_finished` has `duration_s` and `success`. `result` carries the map,
symbols sibling, district symbols directory, names cache paths, commit and
branch, plus language, file count, district count and modularity. `error`
carries a machine `code` and human `message`. The Rust definitions in
`src/worker.rs` and generated `bindings/WorkerEvent.ts` are authoritative.

The service owns its SQLite store and queue. It gives the child clone source,
clone-cache eviction budget, output directory, previous map candidates and a names cache file. The worker emits `features` after clone (clone bytes and history count) and after detection (source file counts and bytes per language).
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

## Worker isolation

The `tolmap worker` child every index job spawns (`src/service/jobs.rs::process_worker_exe`) runs as an unprivileged `tolmap-worker` user (fixed uid/gid `10001:10001`, baked into the runtime image as `TOLMAP_WORKER_UID`/`TOLMAP_WORKER_GID` — see the Dockerfile's runtime stage), with an empty environment plus a short explicit allowlist (`PATH`, `LANG`, the `TOLMAP_NAMER_*` budget/ledger knobs, the `TOLMAP_SCIP_*` indexer locations in the configuration table above, and `OPENROUTER_API_KEY` only when `TOLMAP_NAMER=model`), and its own per-job directory instead of the shared `cache_dir` — see that function's doc comments for the full reasoning. The shared clone cache under `cache_dir/repos` is still fetched/updated and reused across jobs for the same repo; the service (at its own uid) does that clone/fetch itself and then hands the worker a fresh, non-hardlinked local-clone copy in its own job directory (`service::jobs::run_blocking`, `service::clone::local_clone_into`) rather than the shared cache directory itself. The uid switch only applies when the service itself runs as root, which is the runtime image's case (no `USER` in the Dockerfile, deliberately); locally and in CI the service is not root, so the worker runs as the current user.

### Dependency installs (issue #110 P1c)

A `scip` job may install a TypeScript workspace's npm dependencies before scip-typescript (`TOLMAP_SCIP_INSTALL=sandbox`; off by default); the design is `docs/SCIP_SANDBOX.md` and the code `src/indexers.rs`. The worker cannot start the sandbox, so it asks the service over the worker protocol and the service, as root, runs the install:

- **Policy.** Only a workspace (`pnpm-workspace.yaml`, or `package.json` `workspaces`) with a pnpm or npm lockfile at the repository root, and no `node_modules` there yet. yarn and bun lockfiles are never installed. Python and Go are never installed.
- **Execution.** `pnpm install --frozen-lockfile --ignore-scripts --ignore-pnpmfile` or `npm ci --ignore-scripts`, with the image's own pnpm/npm; pnpm's `packageManager` handling is off, so a repository cannot choose the package-manager build. A runtime a manifest pins (`node@runtime:`) is fetched from outside the registry, which the proxy refuses, so such an install falls back unless pnpm can satisfy it otherwise.
- **Sandbox.** nsjail started by root with `--disable_clone_newuser`: the install runs as the worker's uid (`TOLMAP_WORKER_UID`) with every capability dropped, an empty environment plus an explicit list, a tmpfs root holding read-only `/usr`, `/etc` and the Node prefix, the job's checkout read-write at its own path with its `.git` read-only on top, and a scratch home under `cache_dir/install/<job>`. Nothing else of the service is mounted: not the store, not the stored maps, not other jobs.
- **Network.** A new network namespace with only loopback. Its one way out is an HTTP CONNECT proxy in the service, reached through a Unix socket bridged to `127.0.0.1:3128` in the jail. It allows `CONNECT registry.npmjs.org:443` and nothing else (exact host, no suffix, no IP literal, no other port, no plain HTTP), resolves the name itself and dials only globally routable addresses, so metadata endpoints (`169.254.0.0/16`), private and unique-local ranges (including `fdaa::/16`) and the host's own services stay unreachable.
- **Bounds and fallback.** 20 minutes and 20 GB (`TOLMAP_INSTALL_TIME_LIMIT_S`, `TOLMAP_INSTALL_DISK_BUDGET_BYTES`). A self-test runs in the jail first (not root, no service paths, no credential-like variables, no direct egress, the proxy refusing another host). If the jail cannot start (a non-root service, a container that forbids namespaces, no nsjail), the self-test fails, or the install fails or passes a bound, every `node_modules` it created is removed and scip-typescript runs without it. The map records which in `coverage.references.ts.install`; the job never fails for it. Cancelling a job, or a graceful stop, kills the jail with it; if the service process itself dies, nsjail's own time limit (the bound plus a minute) ends the jail.
- **Not covered.** The indexers themselves still run as the worker, outside the jail. A kernel exploit from inside the jail reaches the whole machine until the worker moves to its own VM (#97); that risk was accepted on #117.

Nothing here needs a migration step for an existing cache volume. The per-job directories are made fresh and torn down by the service on every job, never a static layout an operator provisions, so there is nothing to pre-create, `chown`, or backfill.

## Errors

Every non-2xx response is JSON:

```
{"error": "<machine code>", "message": "<human text>"}
```

| status | `error` | when |
|---|---|---|
| 400 | `invalid_request` | body does not match the contract above |
| 404 | `not_found` | unknown job id, or unknown slug/commit for `GET /api/maps/{owner}/{repo}` |
| 429 | `rate_limited` | per-IP or per-repo rate limit tripped -- `message` says which |
| 503 | `busy` | all workers and pending queue slots are occupied; `Retry-After: 30` seconds |
| 503 | `server_stopping` | the service received a shutdown signal and is not accepting new jobs (see "Graceful shutdown" above); no `Retry-After` |
| 422 | `detection_failed` | `detect::detect` (issue #4) found no supported source at all -- an empty or non-source repository, not a size or confidence problem |
| 422 | `detection_uncertain` | detection succeeded but at `Confidence::Low` -- `message` is the chosen candidate's evidence text. Never indexed silently: a wrong source root produces a plausible-looking wrong map (finding 7) |
| 502 | `clone_failed` | git clone/fetch failed (bad URL, network, repo does not exist) |
| 500 | `index_failed` | the indexing pipeline itself errored on an otherwise-valid repository |
| 500 | `internal_error` | a bug, not a caller or repository problem |

A job that fails carries `error` and `error_code` in its terminal snapshot,
with `status: "failed"`, because admission already returned `202`. A worker
that crashes or is killed by the machine (including out-of-memory) reports
`worker_crashed`. Very large repositories may take a long time or fail this
way on the current worker; a full volume can also make cloning fail with
`clone_failed`. This is the owner's accepted MVP trade-off;
[issue #97](https://github.com/onsager-ai/tolmap/issues/97) tracks routing
large reports to appropriate worker classes. Existing clients should render
unknown error codes as a generic failure; `repo_too_large` is no longer emitted.

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
