# Production worker tier (#97)

**Status: specification; phase 0, phase 1 (loopback agents) and phase 2 (durable jobs, leases, restart survival, resume, and worker classes with rerouting and out-of-memory escalation) are implemented (§8); phases 3 and 4 are not. The owner's decisions on it are recorded in §10.** Owner timing ruling on [#97](https://github.com/onsager-ai/tolmap/issues/97) (AskUserQuestion, session `16030105`, transcript line 6521, 2026-09-25T13:51:51Z): **"After P2a merges."** P2a ([#127](https://github.com/onsager-ai/tolmap/pull/127)) merged 2026-09-25T18:07Z and the P1c sandbox ([#129](https://github.com/onsager-ai/tolmap/pull/129)) 17:48Z, so the design starts here. That ruling expected P2a to make SCIP the default; the owner then kept `hand` as the default (finding 45, "The default did not flip", 16:56Z). This matters below, because most hosted jobs are therefore small.

Owner rulings this document builds on, all on #97: **workers never touch the database**; they dial the master over WebSocket or another RPC channel, so the database is never exposed (2026-09-24). **No caps**: no file-count, clone-size, history-depth or job-time admission caps. **Cancel + queue ETA**: every queued job shows its ETA, including the jobs ahead of it (transcript line 2157, 2026-09-24T01:20:31Z).

The owner decided the spend, credential and exposure questions this design raised on 2026-09-26 (§10, recorded on #97): one 16 GB worker class with at most one worker, WebSocket, artifacts over HTTPS through the master, per-worker tokens on a private network with nothing public, the sandbox moving with the first remote worker, and bounded retries. Any change to those, and any platform API credential, stays reserved to the owner. This document is provider-neutral on purpose. Machine names, prices and deploy configuration belong in the private hosting repository, not here.

## Terms

| term | meaning |
|---|---|
| master | `tolmap serve` with the store: HTTP API, SSE, queue, leases, artifact registry. The only process that opens the database. |
| agent | `tolmap worker --connect <url>`: a long-lived process on a worker host that dials the master, takes jobs and runs each one in a job child. It plays the part `tolmap serve` plays for its child today. |
| job child | `tolmap worker` with no arguments: today's one-job process, reading a `WorkerSpec` on stdin and writing v1 `WorkerEvent` lines on stdout. Unchanged. |
| executor | the code that prepares a checkout, spawns the job child, relays its events, answers `install_request` and kills it on cancel. Today it lives inside `run_blocking` and `process_worker_exe` in `src/service/jobs.rs`; this design moves it into a module that both the master (local mode) and the agent call. |
| class | a worker's advertised usable memory and CPU count. Classes are numbers, not names. |
| lease | the master's record that one agent holds one job until a deadline, renewed by heartbeats. |
| epoch | a per-job counter the master increments on every assignment. It fences stale workers: anything carrying an old epoch is refused. |

## 0. Where this starts

The MVP seam is already most of the way there. What this design keeps and what it has to change:

- **One child per job, JSON lines.** `src/worker.rs` defines `WorkerSpec` and `WorkerEvent` (`stage_started`, `progress`, `stage_finished`, `features`, `log`, `result`, `error`, `install_request`), each carrying `v: 1`. `docs/API.md` "Worker protocol (v1)" is the contract. The child never opens the database.
- **The service does the trusted work around the child.** It clones into its own LRU cache, copies a fresh non-hardlinked checkout into a per-job directory, drops the child to uid 10001 with an empty environment plus an allowlist, and answers `install_request` by running the nsjail install as root (`run_blocking`, `process_worker_exe`, `ServiceInstall`).
- **Results are paths on a shared filesystem.** The child's `result` names `map_path`, `symbols_path`, `symbols_dir` and `names_cache`; the service moves them into the store. Paths cannot cross a network, so in remote mode results name artifacts instead (§3.4).
- **All job state is in memory.** The `JobRegistry` queue, cancel set and snapshots live in the serving process. A master restart fails every job (`server_stopping`), and the private deploy notes record the resulting risk when a platform stops an idle machine.
- **The cost model predicts time, not memory.** `src/service/eta.rs` predicts per-stage seconds from repository features (finding 38) and refits from completed jobs. Nothing predicts peak memory, which class selection needs.
- **The queue ETA assumes one slot.** `refresh_queue_etas` adds up the remaining time of every running job, then each queued job in turn. With `TOLMAP_MAX_CONCURRENT_JOBS` above 1 that overstates the wait, because a queued job starts when the first slot frees, not after all of them.
- **What jobs cost.** With `--refs hand`, the default, peak RSS tracks file count (finding 18, r = 0.937 on log–log): median 21 MB, 114 MB, 520 MB and 1.8 GB for the small, medium, large and ultra bands, p90 5.3 GB in ultra. There are two outliers: msgraph-sdk-python at 13.8 GB, and msgraph-sdk-go, which OOMed at 23.3 GB. With `--refs scip`, peaks are 0.4–3.9 GB on the nine small fixtures (finding 45) and 4.9–9.1 GB at corpus scale (finding 44). A sandboxed install adds up to 1.8 GB (finding 46: dify 8.65 GB, n8n 8.43 GB).

## 1. Goals and non-goals

**Goals**

1. **Scale out large reports.** Move indexing onto worker machines sized for it, and make the master able to run several workers and several classes, placing each job on one with enough memory. The first fleet is one 16 GB worker (§10.1); adding workers or classes later is configuration plus an owner spend decision, not a redesign.
2. **Isolate untrusted indexing from the master.** Repository-controlled bytes (clone, parse, indexers, installs) run on worker hosts that hold no database, no stored maps, no service secrets and nothing but their own worker token. A sandbox escape on a worker then reaches one worker, not the service. This ends the in-VM sandbox's accepted kernel-exploit risk (#117, `docs/SCIP_SANDBOX.md` §2 and §4.2).
3. **Keep the no-caps ruling.** No admission caps. A job that exceeds its worker's memory moves to a larger class; it fails only when no class can hold it.
4. **Honest per-job ETA with several workers.** Extend the owner's "cancel + queue ETA" to many workers and several classes, with the ETA computed by the same rule the dispatcher uses.
5. **One worker binary.** The same `tolmap` binary and image run the job child, the agent and the master. The job child and its v1 events are unchanged, so local and remote jobs run the same code on the same inputs and produce byte-identical maps.
6. **Survive a master restart** once the tier is on: queued and running jobs persist in the master's store.

**Non-goals**

- **Changing single-process mode.** With no worker configuration, `tolmap serve` behaves exactly as today: it spawns a local child per job, keeps the in-memory registry and fails jobs with `server_stopping` on shutdown. Self-hosters need nothing new.
- **Workers reaching the database** in any form, including through a query API. Workers get job specs and lease-scoped artifact URLs, nothing else.
- **Private repositories.** They are out of MVP scope (`docs/ARCHITECTURE.md`). The protocol leaves room for per-job clone credentials, but none are designed here.
- **Model naming on remote workers.** `OPENROUTER_API_KEY` never reaches a worker host. Hosting uses the default IDF namer. A later `naming_request` round-trip, shaped like `install_request`, could run model naming on the master.
- **Multi-master and cross-region scheduling.** Several API nodes sharing one store are covered in phase 4 (SSE fan-out stays internal to the master tier). A distributed scheduler is not.
- **Wall-time limits.** Consistent with the no-caps ruling, a hung job keeps its lease for as long as its agent heartbeats, and the user can cancel it.

## 2. Architecture

```mermaid
flowchart LR
  Browser["Browser"] -->|"HTTPS: /api, SSE"| API
  subgraph MT["master tier: private network"]
    API["master: tolmap serve<br/>API, SSE, scheduler, leases"]
    DB[("store: SQLite now, Postgres hosted")]
    FS[("artifacts: map files, later object storage")]
    API --- DB
    API --- FS
  end
  subgraph WH["worker host: one per class instance"]
    AG["agent: tolmap worker connect mode<br/>root, holds the worker token"]
    CH["job child: tolmap worker<br/>uid 10001, empty env"]
    NJ["nsjail: dependency install"]
    AG -->|"stdin spec, stdout v1 events"| CH
    AG --> NJ
  end
  AG -->|"wss control channel, dialled out"| API
  AG -->|"HTTPS artifact PUT and GET, lease-scoped"| API
  AG -->|"git clone"| GH["GitHub"]
```

**The master owns all state.** The job table, queue, leases, epochs, cancel flags, snapshots, timing rows and the artifact registry live in the master's store. Workers never see a database path, a connection string or a store file. The agent asks for nothing: the master pushes a job spec and a set of URLs that are valid only for that job and epoch.

**Workers dial out.** An agent opens one authenticated WebSocket to the master's worker endpoint and keeps it open. Workers need no inbound ports, and nothing new is exposed except that endpoint. Artifacts move over plain HTTPS requests to URLs the master hands out (§4.2), so a 50 MB upload never sits in front of a heartbeat.

**The executor moves; the job child does not change.** `run_blocking` splits into three parts:

1. **prepare** (master): read warm-start candidates and the names cache from the store, choose the class, build a portable `JobSpec`;
2. **execute** (master in local mode, agent in remote mode): materialise the clone in its own cache, copy it into a fresh job directory, harden it, fetch inputs, spawn the job child with a v1 `WorkerSpec` whose paths are all local to that host, relay events, answer `install_request` with the local nsjail, kill the process group on cancel;
3. **register** (master): check the returned artifacts, move them into the store, insert the map row, save names, prune, record timings.

In local mode all three run in `tolmap serve`, exactly as now. In remote mode, execute runs in the agent, and the only things crossing the network are the `JobSpec`, v1 events, and artifact bytes.

**Implemented in local mode** (#97 phase 1): `jobs::prepare` and `jobs::register` in `src/service/jobs.rs`, and `executor::execute` in `src/service/executor.rs`. The executor takes no `AppState`, `Store` or `ServeConfig`: it gets the `JobSpec`, a `JobInputs` (the names cache by value, previous-map files by path), an `ExecEnv` naming this host's cache, job and install directories, worker binary and uid, and two callbacks, an `EventSink` for the child's events and a `CancelProbe` for cancellation. The master implements both against its registry exactly as before; an agent will implement them against the channel. The executor checks out the pinned `JobSpec.commit` in the job's own copy (§3.3), as of #97 phase 2, so local mode and remote mode share this behaviour rather than local mode building whatever the clone resolves HEAD to. `tests/service_byte_identity.rs` holds `tolmap build` and `tolmap serve` to byte-identical map and symbols documents (§9).

### 2.1 Class selection

The master picks a class for each job from a **predicted peak RSS**, compared with each agent's advertised usable memory (total minus a reserve for the agent and the OS, 1 GiB by default).

The prediction uses, in order:

1. **This slug's own last measurement**, scaled by the change in file count. The same repository at a nearby commit is the best predictor there is.
2. **The reference mode**, known at admission: `scip` jobs start from findings 44–46's per-indexer peaks, and `hand` jobs from finding 18's per-band figures.
3. **Features reported mid-job.** The job child already sends `features` after clone and after detection (file and byte counts per language), and the agent forwards them. If the master's prediction from those features exceeds the agent's class, it sends `cancel` with `reason: "reroute"`, and on `released` it rebinds the job to the larger class, at the head of that queue. The memory model stays on the master; agents never need it. Clone and detection cost seconds, and the second attempt starts with known features instead of a guess.
4. **An OOM kill**, the last resort: re-queue on the next larger class (§6).

Memory needs a model next to the time model. Each finished job records its peak RSS alongside its stage durations: the agent (or the master in local mode) reads it from `wait4`, not `getrusage(RUSAGE_CHILDREN)` as an earlier draft of this document said. `RUSAGE_CHILDREN` is the maximum over *every* child the reaping process has ever reaped, so a small job run after a large one would report the large one's peak, and the service's own git clone and the root-run nsjail install would leak into the figure. `wait4` on the job child's own pid returns that child's own usage, which on Linux also covers whatever grandchildren it reaps itself (the SCIP indexers a `--refs scip` job spawns), and excludes the service's clone and install children — the same measure findings 41–46 report. The model fits log(peak) against log(files) per reference mode and language, seeded from the findings above, and uses an upper quantile. It errs high on purpose: underestimating memory costs an OOM and a rerun, while overestimating costs a larger machine for one job. This does not conflict with the rule that numbers must be a lower bound, which governs what a map reports, not internal scheduling estimates.

A job whose prediction exceeds every class is bound to the largest class anyway. No caps means best effort, never a rejection.

With the decided fleet of one class (§10.1), every job binds to that class, and rerouting (step 3) and OOM escalation (step 4) have nowhere to go: an OOM there fails the job (§6). The memory model is still worth building in phase 0. It records what jobs actually need, which is the evidence for any later class decision, and it makes class selection work the day a second class exists.

### 2.2 Job and lease states

```mermaid
stateDiagram-v2
  [*] --> queued: admitted
  queued --> leased: assign, epoch n
  leased --> running: first job_event
  running --> running: heartbeat renews lease
  leased --> queued: lease expired, attempt counted
  running --> queued: lease expired, attempt counted
  running --> queued: released for stop or reroute, not counted
  running --> done: result verified and registered
  running --> failed: error event, or attempts exhausted
  queued --> failed: cancelled
  leased --> failed: cancelled
  running --> failed: cancelled
  done --> [*]
  failed --> [*]
```

These are the master's internal states, stored per job. The public `JobSnapshot.status` enum (`queued`, `cloning`, `detecting`, `indexing`, `done`, `failed`) is unchanged. `leased` shows as `queued` until the first event arrives. A re-queued job goes back to `queued` with its stages reset, and a note in `stage` saying why ("worker lost, retrying on another worker").

Defaults, all configurable: heartbeat every **15 s** (the same interval as the SSE heartbeat, well inside common proxy idle timeouts), lease TTL **60 s**. Any `job_event` also counts as a heartbeat. A heartbeat comes from the agent, not the job child, so a stage that runs for ten minutes without a progress event still keeps its lease, and so does the blocking `install_request` round-trip, which never leaves the worker host.

### 2.3 Claim, assign, progress, result

```mermaid
sequenceDiagram
  autonumber
  participant B as Browser
  participant M as Master
  participant A as Agent
  participant C as Job child
  A->>M: WebSocket upgrade with bearer token
  A->>M: hello with proto range, build, class, slots, features
  M-->>A: welcome with proto, heartbeat_s, lease_ttl_s
  B->>M: POST /api/index
  M-->>B: 202 with job_id, queued, eta_start_s
  A->>M: ready with one free slot
  M->>M: pick the first job that fits, epoch 1, store the lease
  M-->>A: assign with job_id, epoch 1, JobSpec, input and output URLs
  A->>A: clone into own cache, copy into job dir, GET warm-start map and names
  A->>C: spawn as uid 10001, write v1 WorkerSpec on stdin
  C-->>A: stage_started, progress, features
  A->>M: job_event with job_id, epoch 1, seq, v1 event
  M-->>B: SSE snapshot
  loop every heartbeat_s
    A->>M: heartbeat with job_id, epoch 1, last seq
    M-->>A: lease_renewed with expiry and acked seq
  end
  C-->>A: install_request
  A->>A: run the nsjail install on this host
  A-->>C: InstallCoverage line on stdin
  C-->>A: result naming local paths
  A->>M: PUT each artifact with its sha256
  A->>M: job_event result naming artifacts, not paths
  M->>M: check digests and schema, register, insert map row, prune
  M-->>A: result_accepted
  M-->>B: SSE frame with status done
  A->>M: ready with one free slot
```

### 2.4 Cancel

```mermaid
sequenceDiagram
  autonumber
  participant B as Browser
  participant M as Master
  participant A as Agent
  participant C as Job child
  B->>M: POST /api/jobs/id/cancel
  M->>M: mark cancelled in the store, terminal snapshot
  M-->>B: 200 with failed and error_code cancelled
  M-->>A: cancel with job_id, epoch 1
  A->>C: SIGKILL the process group, jail included
  A->>M: released with job_id, epoch 1, reason cancelled
  M->>M: slot on this agent is free again
  Note over M,A: events for epoch 1 that arrive after the cancel are dropped
  alt result was already in flight
    A->>M: job_event result for epoch 1
    M-->>A: result_rejected, reason cancelled
    Note over M: artifacts for a cancelled job are never registered
  end
```

As today, the cancel is terminal the moment the master records it, and a repeated cancel returns the same snapshot. The master counts the agent's memory as in use until `released` arrives or the lease expires, so it never assigns a second job onto memory the killed one may still hold.

### 2.5 Lease expiry and re-queue

```mermaid
sequenceDiagram
  autonumber
  participant M as Master
  participant A as Agent 1
  participant A2 as Agent 2
  participant C as Job child
  A->>M: job_event for epoch 1, seq 40
  Note over A,M: the channel drops
  A->>A: keep running, buffer events, reconnect with backoff and jitter
  alt reconnect within the lease TTL
    A->>M: hello with resume for job_id, epoch 1, last seq 57
    M-->>A: welcome, resume continue, acked seq 40
    A->>M: replay seq 41 to 57, then carry on
  else lease expired first
    M->>M: attempt 2, epoch 2, job to the head of its class queue
    M-->>A2: assign with job_id, epoch 2
    A->>M: hello with resume for job_id, epoch 1
    M-->>A: welcome, resume cancel, reason lease_lost
    A->>C: SIGKILL the process group
  end
```

## 3. Protocol

### 3.1 Two layers

The network channel carries **the same v1 job events** as the child's stdout, wrapped in an envelope, plus a small set of session messages. There are two version numbers:

- **`v`** on every job event: the existing worker protocol version, currently 1, unchanged. The job child keeps emitting exactly what it emits today.
- **`proto`**: the channel protocol version, negotiated once in `hello`/`welcome`. It covers the envelope and session messages.

The agent reads the child's stdout line by line, exactly as `process_worker_exe` does, and forwards each event inside a `job_event` envelope. Two events are handled on the worker host instead of being forwarded as-is:

- `install_request` is answered by the agent with its local nsjail (§5.5). The install never crosses the network. The resulting `stage_started`/`stage_finished` for `install` are forwarded as usual.
- `result` names local paths. The agent uploads each named file (§4.2) and forwards the event with the four path fields replaced by artifact names and a new `artifacts` list (§3.4).

Session messages are JSON text frames with a `type` tag, defined in Rust next to `WorkerEvent` with serde, like everything else on this seam.

### 3.2 Message catalogue

Worker → master:

| type | fields | when |
|---|---|---|
| `hello` | `proto_min`, `proto_max`, `worker_id`, `build` (crate version, commit, indexer versions), `class` (`memory_bytes`, `cpus`), `slots`, `features[]`, `resume[]` (`job_id`, `epoch`, `last_seq`) | first frame after the upgrade |
| `ready` | `slots_free` | willing to take work; repeated after each job |
| `job_event` | `job_id`, `epoch`, `seq`, `event` (a v1 `WorkerEvent`) | every child event, in order |
| `heartbeat` | `jobs[]` (`job_id`, `epoch`, `last_seq`), optional `rss_bytes` | every `heartbeat_s` |
| `released` | `job_id`, `epoch`, `reason` (`cancelled`, `lease_lost`, `server_stopping`, `reroute`, `worker_stopping`, `oom`), optional `peak_rss_bytes` observed | the agent has stopped a job and freed its memory |
| `draining` | none | the host is stopping: take no new work (§6) |

Master → worker:

| type | fields | when |
|---|---|---|
| `welcome` | `proto`, `heartbeat_s`, `lease_ttl_s`, `resume[]` (`job_id`, `action`: `continue` or `cancel`, `acked_seq`) | answer to `hello` |
| `assign` | `job_id`, `epoch`, `lease_ttl_s`, `job` (a `JobSpec`, §3.3), `inputs` (URLs), `outputs` (URL base) | a job for a free slot |
| `lease_renewed` | `job_id`, `epoch`, `expires_in_s`, `acked_seq` | answer to a heartbeat |
| `cancel` | `job_id`, `epoch`, `reason` (`cancelled`, `lease_lost`, `server_stopping`, `reroute`) | stop this job now |
| `result_accepted` / `result_rejected` | `job_id`, `epoch`, `reason` | after registering, or refusing, a result; the agent may then delete its job directory |
| `shutdown` | `mode` (`drain`: finish current jobs, then exit; `now`: release and exit), `reason` | rolling upgrades, scale-down |
| `error` | `code`, `message` | a protocol violation; the master closes the channel after sending it |

The issue's `claim` is `ready`: work is pulled by agents with free slots and assigned by the master, never taken by a worker on its own initiative.

### 3.3 JobSpec, and the v1 WorkerSpec the agent derives from it

`WorkerSpec` today mixes what the job is (slug, source, refs, prune variant) with where things are on disk (`cache_dir`, `output_dir`, `previous_maps[].path`, `names_cache`). Only the first half can cross the network. `assign` carries a portable **`JobSpec`**:

```json
{
  "slug": "django/django", "owner": "django", "repo": "django",
  "source": "https://github.com/django/django.git",
  "commit": "<sha pinned at admission>",
  "all_sources": false, "prune_variant": "node-relative",
  "namer": "idf", "refs": "hand", "install": null
}
```

plus input URLs (`names_cache`, and `previous_maps[]` as `{branch, url}`) and an output URL base. The agent clones and checks out the commit, learns the branch, downloads the one previous map the job child would choose (same branch, else newest), and writes an ordinary v1 `WorkerSpec` for the child with every path local to its host. The job child cannot tell whether it runs under `tolmap serve` or an agent.

Two rules keep this safe and deterministic:

- **`source` is always a public `https` URL in remote mode.** `{"path": ...}` jobs (`local/<name>` slugs) are only assigned to agents that advertise the `local_paths` feature, which only loopback agents on the master's own host do.
- **The commit is pinned.** The master resolves the commit before admission and the cache key is `(slug, commit)`; the executor checks out that commit in the job's own copy, not whatever the branch points to by then, keeping the branch name (`git checkout -B <branch> <commit>`) so the job child still learns one. **Implemented in #97 phase 2** (`executor::execute`'s `pin_commit`): local mode and remote mode share this one executor, so both got it from the same change, and the gap the earlier draft of this section described (child resolves whatever HEAD the clone left it) is closed in both. A commit no longer reachable in the job's clone (force-pushed away between admission and this checkout) fails the job `clone_failed`, naming the commit, rather than silently building whatever HEAD resolved to.

### 3.4 Results and artifacts

The forwarded `result` event keeps every v1 field and adds one optional field, so v1 consumers still parse it (placeholders in angle brackets):

```text
{"type": "result", "v": 1,
 "map_path": "artifact:map", "symbols_path": "artifact:symbols",
 "symbols_dir": "artifact:symbols_dir", "names_cache": "artifact:names",
 "commit": "<sha>", "branch": "main", "lang": "py",
 "files": <n>, "districts": <n>, "modularity": <q>,
 "artifacts": [
   {"name": "map", "sha256": "<hex>", "bytes": <n>},
   {"name": "symbols", "sha256": "<hex>", "bytes": <n>},
   {"name": "symbols_dir/0.json", "sha256": "<hex>", "bytes": <n>},
   {"name": "symbols_dir/1.json", "sha256": "<hex>", "bytes": <n>},
   {"name": "names", "sha256": "<hex>", "bytes": <n>}
 ]}
```

Owner's change (phase 1): `symbols_dir` is not uploaded as a tar. Each entry the worker wrote into it (`<digits>.json`, see `src/service/worker_result.rs`) is its own artifact, named `symbols_dir/<file name>`, so one failed upload costs one district, not the whole result. An artifact name accepts only `map`, `symbols`, `names` and `symbols_dir/<digits>.json` (`worker::is_valid_artifact_name`); anything else, including one with an extra `/` or a `..` component, is refused. In remote mode the master refuses a `result` whose path fields are anything but `artifact:` names.

Each `PUT` requires its `Content-Length` to be within that artifact kind's
cap before the master reads the body; the streamed body must still match its
declared length and SHA-256. The map cap is the shared 256 MiB
`EARLY_MAP_MAX_BYTES` bound. The full `symbols` sibling is capped at 256 MiB:
the committed Dify symbols fixture is 10.1 MiB raw, leaving about 25×
headroom. Each `symbols_dir/<n>.json` is capped at 64 MiB because the viewer
fetches one complete district response at a time; Dify's largest shipped
district fixture is 1.7 MiB raw, leaving about 38× headroom. `names` keeps its
existing 16 MiB adoption bound; it is one short cache entry per district and
real caches are a few kilobytes.

A lease may retain at most 1 GiB of unique artifact blobs and 10,000 distinct
artifact names, including names currently uploading. Its byte reservations
include every in-flight declared length, so concurrent `PUT`s cannot spend
the same remaining capacity. Byte and artifact-name limit refusals return
HTTP 413 before that request's body is read. Failed uploads release their
reservation. Replacing
an existing name consumes no extra artifact slot; once its replacement is
complete, the old blob is removed if no other name in the lease references
it. Re-uploading identical content remains a content-addressed no-op.

The 1 GiB limit is per lease; this decision does not impose a global disk
cap. A lease may also have at most 8 in-flight artifact uploads. A further
`PUT` receives HTTP 429 before its body is read. This bounds how many
`spawn_blocking` writers one lease can hold while still allowing a small
batch of artifact writes. If no body bytes arrive for 30 seconds, the master
aborts that upload; the existing incomplete-body response is HTTP 400, and
the reservation and temporary file are released. The timeout runs while the
async handler is reading the body, so closing its channel also releases the
blocking writer.

The route's `DefaultBodyLimit` is defence in depth. `Content-Length` is
required, and every per-kind cap is at most the route ceiling, so a declared
size above the route ceiling is refused by the earlier per-kind check. No
valid artifact PUT can currently reach `DefaultBodyLimit`'s limit.

### 3.5 Ordering, acknowledgement and resume

- `seq` starts at 1 for each `(job_id, epoch)` and rises by one per `job_event`. The master applies events in `seq` order and ignores any `seq` it has already applied.
- `lease_renewed.acked_seq` tells the agent what it may drop from its buffer.
- While disconnected, the agent keeps running its jobs and buffers events. Progress is coalesced to the latest value per stage. `stage_*`, `features`, `error` and `result` are kept in order. `log` lines are kept up to a bound, and a marker notes any dropped. The buffer is therefore bounded by the number of stages, not by the job's length.
- On reconnect, `hello.resume` lists each job the agent still holds. The master answers `continue` with its `acked_seq` when the lease is still valid and the epoch current, otherwise `cancel` with `lease_lost`.
- An agent that holds a finished result keeps it on disk and keeps retrying the connection for a configurable hold time (24 h by default) before discarding it.

The master persists a job's snapshot at stage boundaries and every few seconds, not at every progress event. After a master restart the snapshot is at most that far behind, and the next events bring it up to date.

### 3.6 Versioning and compatibility

- **Additive changes do not bump anything.** A new optional field uses `#[serde(default)]` and is skipped when unset, the pattern `WorkerSpec.refs` and `install` already follow.
- **New message types are gated by features, not versions.** A peer sends a message type only if the other side listed its feature in `hello` (for example `install_sandbox`, `resume`, `local_paths`). An unknown `type` from a peer that was not offered it is a protocol error.
- **Breaking changes bump `proto`.** The master accepts the current and previous `proto`, so a rolling upgrade can move the master first and the workers after. `welcome` picks the highest version both sides support. With no overlap, the master sends `error` `unsupported_proto` and closes.
- **The job child's `v` stays 1** until a job event changes incompatibly. An agent only spawns a job child from its own binary, so agent and child always agree.

**Build identity is a scheduling constraint, not only a version.** tolmap promises that the same repository at the same commit produces a byte-identical map. A worker running a different build, or different indexer versions under `--refs scip`, can produce a different map for the same `(slug, commit)`, and the cache would keep whichever finished first. The master therefore assigns jobs only to agents whose `build` matches its own. A mismatched agent stays connected but idle, and is visible in the master's worker list. A rolling upgrade drains old agents (`shutdown` `drain`) rather than letting both builds serve at once.

**Implemented in #97 phase 3 code prep** (`WorkerBuild` in `src/worker.rs`, `own_build`/`same_build` in `src/service/workers.rs`): `commit` is `TOLMAP_BUILD_COMMIT`, baked in at compile time by `build.rs` -- CI's and the Dockerfile's build arg if one was set, else `git rev-parse HEAD` run at compile time, else `"unknown"`. `"unknown"` matches nothing, including another `"unknown"`, in every comparison but a test that sets a matching identity explicitly; `own_build()` is memoized once per process, so every call inside one binary (a real agent, or one `cargo test` binary) already returns an identical value without that exception doing any work. `indexers` is read once at startup from a small `{"scip-python": "0.6.6", ...}` file (`TOLMAP_INDEXER_VERSIONS`, default `/usr/local/share/tolmap/indexers.json`) the image writes at build time, never by running the indexers themselves; no file is an empty map, as phase 1 always was. The master's own build is additive on `GET /api/healthz` (`docs/API.md`), in local mode too. Departure from this document: §8 phase 1 said a loopback agent's `commit` would be its executable's own SHA-256, since nothing else was available; it is now the same `TOLMAP_BUILD_COMMIT` mechanism a remote worker will use, which #153 could not yet build.

## 4. Transport

### 4.1 Control channel: WebSocket (decided, §10.2)

The control channel is a WebSocket served by axum, using its `ws` feature (which brings `tokio-tungstenite`) on the server and `tokio-tungstenite` with rustls in the agent. Frames are JSON text built from the serde types in `src/worker.rs`, so the protocol's types stay defined once in Rust and the v1 events cross unchanged. The upgrade is plain HTTP/1.1, which passes reverse proxies and platform edges. The 15 s heartbeat doubles as keepalive against idle timeouts. TCP provides the only transport-level backpressure, so the application bounds its send queue and coalesces progress (§3.5). That is enough, because the only large payloads travel outside the channel (§4.2). A fake worker can be played with `websocat` in tests and debugging.

*Considered: gRPC bidirectional streaming (tonic).* It offers per-stream flow control, deadlines and binary framing. But it needs a `.proto` file, which is a second hand-written definition of the worker events that `CLAUDE.md` rules out, or opaque JSON inside protobuf. It would also bring a second server stack beside axum and need HTTP/2 end to end, which some platform edges do not provide by default. At a few frames a second per job its advantages do not matter.

### 4.2 Artifacts: HTTPS through the master (decided, §10.3)

A map plus its symbols document and per-district files is small for most repositories (the nine committed fixtures are 0.1–0.6 MB each) but can reach tens of MB on the largest ones. That figure is the issue's estimate: the largest corpus map's size was not measured for this document. Artifacts flow both ways: results up, and warm-start maps and names caches down.

Artifacts move as plain HTTPS requests to the master, on the same private listener as the channel (§5.6):

- `PUT /workers/artifacts/{job}/{epoch}/{name}` uploads a result artifact, with the worker token and the artifact's SHA-256. The master checks that this worker holds the lease at that epoch. It streams the body to disk while hashing, never buffering it, and refuses the upload on a digest or size mismatch. Per-artifact and per-lease byte/name limits return 413 before reading an over-limit body; the in-flight upload limit returns 429. The exact caps and the body idle timeout are in §3.4.
- `GET` on the input URLs in `assign` fetches the previous map and the names cache, under the same lease check.
- A retry is harmless: stored artifacts are content-addressed, so a repeat upload is a no-op.
- Artifacts are stored on the master's disk, as maps are today.
- **Retention is unchanged:** the newest `TOLMAP_RETAIN_COMMITS_PER_REPO` commits per slug, never the newest row. In addition, uploads from jobs that never registered (failed, cancelled or superseded) are deleted after 24 h.

`assign` gives the agent URLs, and the agent does not care where they point. Moving to presigned object-storage URLs therefore needs no protocol or worker change. That move waits until the master tier has more than one API node (phase 4), and it needs an object store, which is an owner decision.

*Considered: streaming artifacts over the WebSocket.* It would put heartbeats and cancels behind 50 MB of chunks unless frames were interleaved by hand, and it needs its own chunk-resume logic. *Considered: presigned object storage now.* It adds an object store, its credentials and a retention policy, for no benefit while there is one master node.

### 4.3 Frame and message bounds

The master rejects a control frame over 1 MiB (the largest legitimate frame is a `features` or `log` event), limits each agent's frame rate, and refuses an artifact whose declared size disagrees with its body. Artifact upload size and count caps also bound the result data a lease may place on the master; they do not reject a repository at admission.

## 5. Security

### 5.1 Worker authentication (decided, §10.4)

Each worker has its own **bearer token**. The agent presents it in the `Authorization` header of the WebSocket upgrade and of every artifact request, never in a URL, where it would be logged. Every connection uses TLS (`wss`, `https`) except loopback.

Tokens are random 256-bit values. The master stores them only as SHA-256 hashes, in a file named by configuration, each line binding a hash to a `worker_id`. The master compares in constant time. Revoking a worker means deleting its line. Rotating means adding a new line, moving the worker to the new token, then deleting the old line. A small `tolmap worker-token new --id <id>` command prints a token once, together with the line to add. **The owner issues and rotates the tokens.** Nothing in phases 0–2 needs one: loopback agents get an ephemeral token that the master generates in memory at start-up (§8).

On the worker host the token sits in a root-only file (mode 0600) that the agent reads. The job child runs as uid 10001 with an empty environment, so it can read neither the file nor the agent's memory.

**Implemented in #97 phase 3, remote mode** (`TOLMAP_WORKERS=remote`, `src/service/workers.rs`'s `TokenFile`, `server_tls` and `TlsListener`, and `src/service/agent.rs`), as this section describes, with these specifics:

- **The token file** is `TOLMAP_WORKER_TOKENS`: `<sha256 hex> <worker_id>` lines, blank lines and `#` comments allowed. A worker id may have several lines (the rotation above); a hash may not. A worker id is 1 to 64 ASCII letters, digits, `.`, `_` or `-`. The master checks the file for a change (size, modification time, inode, mode) on every channel upgrade, artifact request and heartbeat, and re-reads it when it changed, so a deleted line refuses that worker's next connection (401) and closes its open channel at its next heartbeat (`error` `token_revoked`). It fails closed: a file that is missing, unreadable, malformed or writable by its group or others stops startup, and later refuses every token until it is fixed, rather than keeping the tokens it held before an edit meant to revoke one.
- **A token is one worker's.** `hello.worker_id` must be the id the token's line names, or the master answers `error` `worker_id_mismatch` and closes. That id is what the store records as a lease's holder, which is what lets a restarted master hand an adopted lease back (§8, remote mode).
- **TLS on the listener** (§5.6): `TOLMAP_WORKER_TLS_CERT` and `TOLMAP_WORKER_TLS_KEY`, PEM, rustls on the `ring` provider, TLS 1.3 only, no client certificates. Any listen address but loopback requires both, and there is no fallback to plaintext: a missing file stops startup. A loopback listener may run without TLS, which is how CI runs the protocol. The key file must be mode `0600` or stricter; startup stops with the fix otherwise.
- **The agent** dials `wss://` to any host, `ws://` only to loopback, and verifies the master against `--ca-file` alone when given (the owner's private CA), else the public webpki roots, for the channel and every artifact request alike. It refuses a `--token-file` its group or others can use, and it names its `--worker-id`.
- **`tolmap worker-token new --id <id>`** prints the token and the line, two lines and nothing else, and writes no file.

*Considered:* mutual TLS with a private CA, which means running a CA for no gain at one worker; and platform workload identity (OIDC), which ties the protocol to one provider.

### 5.2 What reaches a worker host, and what never does

Reaches it: the `JobSpec` (public repository URL, commit, build options), URLs valid for one job and epoch, the previous map and names cache for that slug (both already public through the API), and its own token.

Never reaches it: the database or any path or credential for it, other slugs' maps except through URLs issued for a job it holds, the master's filesystem, `OPENROUTER_API_KEY` or any other service secret, platform API tokens, and other workers' tokens. The master's store and its internal event fan-out (phase 4) sit on a network workers cannot address.

### 5.3 What a compromised worker can and cannot do

Assume an attacker controls a worker host completely, root included, for example after a kernel exploit from the install jail.

| can | bounded by |
|---|---|
| return a wrong map for jobs leased to it | results are accepted only for the current epoch of a job that worker holds; artifacts are schema-checked on registration; determinism spot-checks (§5.4) catch a forged map with the probability of the sampling rate; revoking the token ends it |
| hold jobs and do nothing | a held job keeps heartbeating, so it is never re-queued by lease expiry; the user can cancel; the operator can revoke the token, which drops the channel, expires the lease and re-queues the job |
| read the specs, warm-start maps and names caches of jobs assigned to it | the same data is public through the API |
| flood the master with frames or uploads | per-agent frame-rate and frame-size bounds (§4.3); uploads only to URLs issued for its own lease |
| use its token from elsewhere | the token is per worker and revocable; a stolen token gives exactly the list above |

| cannot | why |
|---|---|
| read or write the database | no route, no path, no credential ever reaches the host |
| read or overwrite another slug's stored maps | artifact URLs are scoped to a job and epoch the worker holds; the master writes stored maps only after registration, under content-addressed names it chooses |
| choose where the master writes | results name artifacts (§3.4), never paths; the master chooses every path it writes |
| mint tokens, see other workers' tokens, or reach other workers | tokens are issued out of band and stored hashed; workers dial the master only |
| obtain service secrets | none are on the host (§5.2) |

### 5.4 Checking results

Registration checks each artifact's SHA-256 and size, parses the map as a `MapDocument` and the symbols document against its schema, and checks that the result's `commit` is the job's pinned commit and `files` and `districts` match the document. Because tolmap's output is deterministic, the master can also re-run a sample of jobs on a second worker and compare digests. A mismatch is either a determinism bug or a lying worker, and both are worth an alert. The sampling rate is an operational setting; one job in fifty costs 2% more compute. Two results for the same `(slug, commit)` from different jobs should always be byte-identical, and the master logs any that are not.

### 5.5 Where the nsjail sandbox goes (decided, §10.5)

The install sandbox moves with the executor: it runs on the worker host, started by the root agent, exactly as the root service starts it today (`docs/API.md` "Dependency installs"). Policy, mounts, egress proxy, self-test and the 20 min / 20 GB fallback bound are unchanged. `docs/SCIP_SANDBOX.md` §4.2 sets out why this ends the accepted risk: a kernel exploit from the jail now becomes root on a worker host that holds one token and no master data, instead of root on the machine holding the store, every map and the service's secrets. The in-VM jail still matters there, because it keeps install code away from the agent's token and the host's network.

Two consequences follow. In remote mode the master no longer needs root, since it spawns no children and starts no jails, so it can run unprivileged. And the indexers, which run as the worker uid outside the jail (`docs/API.md` "Not covered"), now do so on a host with nothing of the master's to reach.

**Installs stay off in hosting until phase 3** (§10.5). Production runs `TOLMAP_REFS=hand` today, so neither installs nor indexers run in hosting, and the kernel-exploit risk accepted on #117 stays latent until then. In phase 3 the sandbox moves with the executor to the worker host, and `TOLMAP_SCIP_INSTALL=sandbox` may then be set there, never on the master.

### 5.6 The worker endpoint: a private listener (decided, §10.4)

The worker endpoint (the channel and the artifact URLs) is a separate listener (`TOLMAP_WORKER_LISTEN`; this section called it `TOLMAP_WORKER_BIND` before phase 1 named it; unused, so off, unless `TOLMAP_WORKERS` selects agents), not a route on the public site's port. **It is reachable only on the provider's private network between the master and its workers. Nothing about it is public.** Loopback mode binds it to `127.0.0.1`. Keeping it off the public port also keeps it out from under the `/api` per-IP rate limit and the static-site fallback. Workers still dial out to the master and need no inbound ports. TLS and tokens apply on the private network too, so a peer that reaches the network still cannot act as a worker without a token.

**Implemented in #97 phase 3, remote mode:** with `TOLMAP_WORKERS=remote`, `TOLMAP_WORKER_LISTEN` may name any address, and any but loopback needs TLS (§5.1). Binding it to the private network's address, and keeping that network private, is the owner's hosting configuration, not something the code can check. TLS handshakes run in tasks of their own, each dropped after 10 s, so a peer that connects and stalls holds nothing; a failed handshake is logged, which is how an operator finds an agent that does not trust the certificate. The artifact URLs in an `assign` name the origin the agent dialled (its upgrade request's `Host`), since a remote master is dialled by a private-network name or address that its own listen address, often every interface, does not spell; the agent still sends its token only to the origin it dialled. §4.3's frame-rate bound applies to every agent channel, loopback included: `TOLMAP_WORKER_FRAME_RATE` (200 a second) and `TOLMAP_WORKER_FRAME_BURST` (1,000), past which the channel is closed with `error` `rate_limited`.

## 6. Failure semantics under the no-caps ruling

No failure below is an admission cap: nothing is refused for size or time. Jobs fail only when the machine cannot finish them, and then with the codes clients already know. `worker_crashed` keeps its meaning, with a message that says what happened, since clients render unknown codes as a generic failure anyway (`docs/API.md` "Errors").

| event | detected by | master action | job outcome |
|---|---|---|---|
| **job child OOM-killed** | agent: child killed by SIGKILL with the memory cgroup's `oom_kill` count raised (or, without a cgroup, SIGKILL not sent by the agent) | agent sends `released` `oom` with the observed peak; master records the peak, rebinds the job to the next larger class, puts it at the head of that class's queue, epoch + 1 | continues on a larger worker if a larger class exists; otherwise fails `worker_crashed` ("out of memory on the largest worker class"). With the decided single class (§10.1), an OOM fails the job |
| **job child crashes otherwise** | agent: exit without `result` or `error` | forwarded as today | fails `worker_crashed` with exit status and last stage, as today |
| **worker host dies, or its OOM takes the agent too** | lease expiry | attempt + 1, epoch + 1, head of the same class's queue; if that host died of memory (last heartbeat's `rss_bytes` near its class), rebind to the next class | continues; after the retry bound (§10.6), fails `worker_crashed` ("lost N workers"), never refused at admission |
| **channel lost, worker alive** | both sides | nothing within the lease TTL; the agent resumes (§3.5) | uninterrupted |
| **channel lost past the lease TTL** | lease expiry | as "worker host dies"; the old agent learns `lease_lost` on reconnect and kills its child | continues elsewhere |
| **master restarts mid-job** (remote mode) | agents see the channel close | on start-up, jobs and leases load from the store and every lease's deadline is extended by one TTL, so agents have time to reconnect and resume | uninterrupted; SSE clients reconnect and catch up with `GET` first, as `docs/API.md` already asks |
| **duplicate or stale result** | epoch and job state | a result for a stale epoch, or for a job already terminal, is refused with `result_rejected` and never registered; a repeat for the current epoch with the same digests is acknowledged again | unaffected |
| **cancel races** | job state in the store is the only truth | cancel before `assign` is sent: the job never leaves the queue. Cancel after `assign` but before the first event: `cancel` follows `assign` on the same ordered channel. Cancel while `install_request` is running: the agent kills the jail with the process group, as the service does today. Result in flight when the cancel lands: refused (§2.4) | `cancelled`, always |
| **master graceful stop** (SIGTERM) | — | local mode: unchanged, every queued and running job fails `server_stopping`. Remote mode: admission stops with `503 server_stopping`, jobs stay in the store, agents keep running and reconnect when the master is back | local: `server_stopping`. Remote: uninterrupted |
| **worker graceful stop** (host SIGTERM, scale-down) | agent | agent sends `draining`; if a job finishes within the host's grace period its result is delivered, otherwise the agent kills it and sends `released` `worker_stopping`; the master re-queues it at the head of its class queue without counting an attempt | continues elsewhere |
| **master sends `shutdown`** | agent | `drain`: finish current jobs, then exit; `now`: release them as above | as above |
| **clone fails on the worker** | job child `error` | forwarded | fails `clone_failed`, as today |

Remote mode changes one documented behaviour: after a master restart, jobs continue instead of failing with `server_stopping`. That is an additive improvement for clients, which already have to handle a closed SSE stream, and `docs/API.md` "Graceful shutdown" gains a remote-mode paragraph when phase 2 ships. Local mode keeps today's contract word for word.

A known gap carries over. Cancelling during the clone does not kill the git children that `clone::materialize_with_progress` spawns (`materialize_job_repo`'s doc comment). The agent calls the same code, so a cancel during a remote clone also waits for git to finish before the slot is released. The fix, threading a cancel hook through the git plumbing, is independent of this design.

## 7. Scheduling

### 7.1 One queue per class, strict order within each

At admission each job is **bound to the smallest class that fits its predicted peak** (§2.1): the largest if none fits, and the only one if the fleet has only one. Each class has its own FIFO queue, in admission order.

When an agent sends `ready`, it takes the head of its own class's queue. If that queue is empty, it may take the head of a smaller class's queue ("spill down"). An agent never takes work from a larger class, and nothing is ever preempted.

This gives the property the owner asked for: **a large job does not block small ones.** A 13 GB job waits for a large worker while small jobs keep flowing through small workers. The reverse holds too. A large worker spills only when its own queue is empty, so a large job that arrives waits at most for the one small job that worker is already running. No reservation logic is needed.

With one class this is exactly today's single FIFO. That covers every deployment through phase 2 and the decided phase 3 fleet of one 16 GB worker (§10.1). With a single worker, a large job does delay the small jobs behind it, as it does today. The property above needs a second worker or class, which is a later owner spend decision, and the design is ready for it.

### 7.2 Queue ETA with several workers

The per-job ETA comes from simulating the dispatcher's rule, so the estimate and the scheduler cannot disagree:

```
free_at[w]  = now + remaining midpoint of w's current job   (now if idle)
            = lease deadline, for a worker whose job is being cancelled
            = now + cold-start estimate, for capacity an autoscaler may start (§7.4)
for each class c, largest first, and each queued job j bound to c, in order:
    w = the worker able to take j (class c, or larger and spilling) with the smallest free_at;
        ties broken by worker id, so the result is deterministic
    j.eta_start_s = free_at[w] - now
    free_at[w]  += j's predicted midpoint
```

With one worker this reduces to today's `refresh_queue_etas` sum exactly, and the phase 0 test pins that equivalence. With `TOLMAP_MAX_CONCURRENT_JOBS` above 1 in local mode it fixes the overestimate noted in §0. `queue_position` becomes the position within the job's class queue, which is the count of jobs that must start before it, as the field already means. An optional `eta_start` range (`low_s`, `high_s`, from the low and high ends of each job ahead) can be added next to `eta_start_s` without changing it.

Phase 0 checks the claim that the estimate and the scheduler cannot disagree instead of assuming it ([#148](https://github.com/onsager-ai/tolmap/pull/148)). `src/service/schedule.rs` replays the dispatch rule itself as events, where the worker that frees next takes whatever queue `next_for` names, and compares that replay with this pseudo-code on 5,000 generated queue shapes with up to three classes and four workers, ties included. They agree job for job and bit for bit. Two details of the pseudo-code carry that agreement, and an implementation that drops either one breaks it:

- **Classes are taken largest first.** Taking queued jobs in admission order across classes would let a large worker that frees first take an older small job while a large job waits in its own queue, which dispatch never does. By the time a smaller class is simulated, every larger job is placed, each no later than the time any larger worker is left free at, so a larger worker is only offered small jobs from the moment its own queues are empty.
- **The tie-break by worker id belongs to the dispatcher too.** Worker ids run smallest class first, and a job admitted while several eligible workers are idle starts on the lowest id. It therefore takes an idle worker of its own class before an idle larger one, in the simulation and in dispatch alike.

The implementation keeps times relative to now (`free_at - now` as a remaining time from the start), because `(now + a) - now` is not `a` in floating point, and the one-slot equivalence is exact, not within a rounding error.

The ETA is only as good as its inputs. Finding 38 measured a cold-start n8n underestimate of 42 s at 10% of the run. With several workers, the start estimate of a queued job adds up several such errors, so the range widens with queue depth. That is honest, and the UI already shows a range.

### 7.3 Fairness

Admission already bounds request frequency per IP (30 a minute) and index requests per repository (3 per 5 minutes), and the queue length (`TOLMAP_MAX_QUEUED_JOBS`, 16). Within a class, strict FIFO keeps every queued job's ETA stable: no later job can jump it, which is what makes "the jobs ahead of it" a promise rather than a guess. Shortest-job-first would finish more jobs sooner but make every queued ETA unstable, so it is not proposed. If a single requester floods a class within the rate limits, a round-robin across requesters inside each class queue is the next step. It changes FIFO positions, so it is left until measurement shows a need.

With several classes, `TOLMAP_MAX_QUEUED_JOBS` applies per class, so a backlog of large jobs cannot fill the queue that small jobs need. A job that joins a full class queue gets `503 busy`, as today.

### 7.4 Starting and stopping workers

The decided bounds are one class and **at most one worker, stopped when idle and started when a job is queued for it** (§10.1). The master exposes desired capacity per class: running plus queued jobs bound to that class, clamped to the configured maximum (1), dropping to zero after a configurable idle period. A provider-specific starter acts on it, and its configuration lives in the private hosting repository. If starting a worker needs a platform API credential, issuing that credential is a separate owner decision. A stopped worker is counted in the ETA simulation at its measured cold-start time, which is refitted like a stage duration. General autoscaling, meaning more than one worker per class, is phase 4 and needs new bounds from the owner.

**Implemented in #97 phase 3 code prep**: `GET /workers/capacity` (`docs/API.md`), on the worker listener only, authenticated the same way as `/workers/connect`. One row per configured class, present from the moment the listener opens rather than only after the first admission, since a provider-specific starter has to be able to poll it before any job exists. `desired` is `min(running + queued, TOLMAP_WORKER_MAX_PER_CLASS)` while busy; once a class empties, `desired` holds at `min(1, TOLMAP_WORKER_MAX_PER_CLASS)` for `TOLMAP_WORKER_IDLE_S` before dropping to 0, and a class that has never been busy is `0` immediately, never held. Departure from this document: the idle-hold value is not specified above beyond "dropping to zero after an idle period" -- holding at one worker (rather than at whatever `running + queued` last was, which would already be zero) is what keeps a class from cycling a worker down and back up between two jobs that arrive moments apart, and is the plain reading of "stopped when idle" in this section's opening sentence. `TOLMAP_WORKER_MAX_PER_CLASS` is one number applied to every class, not a per-class table, matching §10.1's one decided bound rather than a table this design has not asked for.

## 8. Migration plan

Every phase ships on its own, keeps single-process mode unchanged and on by default, and passes CI without real hosting. Phases 0–2 need no new hosting spend and no owner credential.

| phase | ships | new spend or credential |
|---|---|---|
| 0 | memory measurement and model, one-queue-per-class scheduler with the ETA simulation (one class in practice) | none |
| 1 | the executor split, channel protocol `proto` 1, `tolmap worker --connect`, the worker listener, `TOLMAP_WORKERS=loopback:N` | none |
| 2 | durable jobs and leases in the store, resume, restart survival, rerouting and OOM escalation, loopback agents advertising configurable classes | none |
| 3 | one remote 16 GB worker on the private network, stopped when idle; the sandbox moves there; loopback agents off; the master can shrink and run unprivileged | decided (§10.1, §10.4, §10.5); tokens issued by the owner |
| 4 | more workers or classes, presigned object storage, Postgres with several API nodes and internal SSE fan-out | **yes**: new bounds, object store, database (not yet decided) |

**Phase 0: measure and schedule, in today's process.**
- The master records the job child's peak RSS (`wait4`, not `RUSAGE_CHILDREN` — see §2.1) after reaping, next to its stage durations in `job_timings`, and `eta.rs` gains a peak-memory prediction seeded from findings 18 and 44–46. It changes nothing about a map, so the determinism and parity gates are untouched. **Implemented in [#149](https://github.com/onsager-ai/tolmap/pull/149).**
- `refresh_queue_etas` is replaced by the §7.2 simulation over a scheduler with per-class queues. Local mode has one class with `TOLMAP_MAX_CONCURRENT_JOBS` slots, so single-slot behaviour is identical and multi-slot ETAs become correct. **Implemented in [#148](https://github.com/onsager-ai/tolmap/pull/148)** (`src/service/schedule.rs`): jobs bind to a class from the memory model's reference-mode prior (wired in [#149](https://github.com/onsager-ai/tolmap/pull/149)); local mode has one class, so every job still binds to it.

**Phase 1: the network protocol, over loopback.**
- `run_blocking` splits into prepare, execute and register (§2). Local mode calls execute in-process and its behaviour is byte-for-byte unchanged.
- The channel protocol lands in `src/worker.rs` beside `WorkerEvent`, as serde types. `tolmap worker --connect <url> --token-file <path>` runs the agent, and the master gets the worker listener and artifact endpoints.
- `TOLMAP_WORKERS` selects the mode: unset or `local` is today's behaviour; `loopback:N` makes `tolmap serve` start N agents on its own host that dial `127.0.0.1` with an ephemeral token and their own cache directories under `cache_dir/agents/<n>` (never the store). On today's single production machine this runs the whole network protocol with no new hosting, the same memory and the same root-started sandbox. It is a switch the owner can turn on or off by configuration.
- State is still in memory in this phase, so a restart still fails jobs, now through the agents.

**Phase 1 is implemented** in [#152](https://github.com/onsager-ai/tolmap/pull/152) (the split), [#151](https://github.com/onsager-ai/tolmap/pull/151) (the protocol types) and [#153](https://github.com/onsager-ai/tolmap/pull/153) (listener, agent, loopback mode): `src/service/workers.rs` is the master's side (listener, hub, leases, artifact endpoints, the loopback job runner, the agents' supervisor) and `src/service/agent.rs` the agent. `tests/service_byte_identity.rs` holds `tolmap build`, local mode and `loopback:2` to byte-identical map and symbols documents; `src/service/workers.rs`'s tests cover authentication, lease scoping, digests, frames, build identity, the cancel races and lost agents; the image workflow runs the SCIP install and warm-start e2e through `loopback:1`. Where the implementation differs from this document, plainly:

- **The listener variable is `TOLMAP_WORKER_LISTEN`**, not §5.6's `TOLMAP_WORKER_BIND`, defaulting to `127.0.0.1:0`. Phase 1 refuses a non-loopback address at startup.
- **A lost channel fails its job at once** with `worker_crashed` ("worker lost: the agent's channel closed"). §6 lets a channel drop for up to the lease TTL and resume; with no resume in phase 1, waiting would only delay the same failure. An agent that stays connected but goes quiet fails its job at lease expiry ("worker lost: lease expired"), and the master closes its channel. Nothing is re-queued or retried.
- **The agent dials plain `ws://` to a loopback master only**, and sends its token only to that master's `/workers/artifacts/` URLs, never through a proxy or a redirect. `wss://` and remote masters come with phase 3.
- **The agent uses `tungstenite`'s synchronous client** (re-exported by `tokio-tungstenite`): sending on the async stream needs the `futures` `Sink` trait, which no dependency exports. The agent is blocking by nature (executor, pipes, `ureq`), so one thread owns the socket.
- **`hello.build.commit` is the SHA-256 of the executable**, since no git commit is compiled in; `indexers` is empty, as a loopback agent is this binary on this host. Remote workers will need their indexer versions (§3.6).
- **`assign` offers the newest previous map per branch**, not the one map the child will choose: the agent learns its branch only after cloning. `PreviousMapUrl` gained `commit`, which names the child's copy and its `warm start from <commit>` log line.
- **`job_event` gained an optional `peak_rss_bytes`**, set on the terminal event, so the job child's `wait4` peak (§2.1) reaches the master's timing row and log as in local mode.
- **The executor's own clone crosses as a v1 `stage_started`/`stage_finished` pair for `clone`**, the first such pair for the job; the master turns it back into the two snapshot updates local mode makes, so the snapshots are the same.
- **The commit is pinned, as of phase 2** (§3.3): a loopback agent builds what its clone resolves HEAD to, as local mode did, was true only through phase 1. `executor::execute` now checks out `JobSpec.commit` in the job's own copy before the child ever runs, so a loopback agent's checkout is pinned exactly as local mode's is, and the master compares the reported commit against its own admission (`workers::check_result`), not the agent's checkout, next to the `files`/`districts`-vs-map-document check (`worker_result::check_counts`) -- both from #97 phase 2, ahead of the first remote worker.
- **Uploads of a job that did not register are deleted when its lease ends**, not after 24 hours (§4.2): with no resume there is nothing to keep them for.
- **Not in phase 1:** per-agent frame-rate limits (§4.3), more than one slot per agent (`slots` above 1 is treated as 1), and hashed token storage at rest (§5.1; loopback tokens are minted at startup and the master keeps only their digests in memory).

**Phase 2: durability and classes.**
- A `jobs` table in the master's store holds status, class, attempt, epoch, lease holder and deadline, and the last persisted snapshot. The in-memory registry becomes a cache of it in remote and loopback modes. Local mode keeps its in-memory registry and shutdown contract.
- Resume after a dropped channel, restart survival with the lease-deadline extension, reroute after `features`, OOM escalation and the retry bound (§10.6).
- Loopback agents take a configured `class` override, so CI can run a "small" and a "large" loopback agent on one runner and exercise class selection, spill-down and escalation for real.
- `docs/API.md` gains the remote-mode shutdown paragraph and any additive snapshot fields.

**Phase 2 is complete.** The plan on #97 has five steps. Step 1 (commit pinning and result checks, [#154](https://github.com/onsager-ai/tolmap/pull/154)), step 2 (durable jobs and leases, [#155](https://github.com/onsager-ai/tolmap/pull/155)), step 3 (resume, [#156](https://github.com/onsager-ai/tolmap/pull/156)), step 4 (worker classes, rerouting and out-of-memory escalation, [#157](https://github.com/onsager-ai/tolmap/pull/157)) and step 5 (the docs pass) are implemented, each below, step 2 with one sequencing change on purpose: **re-queue on lease expiry and the retry bound land with the durable state**, not with resume. Restart survival needs re-queue anyway (after a restart the loopback agents are new processes, so every job that was running has to go back to its queue), so step 3 is only resume: agent buffering, `seq` acknowledgement and `hello.resume` answered `continue` or `cancel`. What step 2 does, in loopback mode only (local mode is unchanged):

- **The `jobs` table** (`src/service/store.rs`, migration 5) holds, per job, the slug, commit and `JobSpec`, the class, the internal status of §2.2 (`queued`, `leased`, `running`, `done`, `failed`), the attempt, the epoch, the lease holder and deadline, the admission order and the last snapshot. A job's row is written before `POST /api/index` answers; then at the lease, at the first event, at every stage change and at most every 3 s between (never per progress event), at a re-queue and at the end. The registry and the hub's leases are caches of it; no SQLite I/O happens under the registry's lock or the hub's.
- **Epoch fencing.** An epoch is raised in the store when a runner starts looking for an agent, before the `assign` carrying it is sent, so no epoch is ever handed out twice, even across a crash. Events, heartbeats, uploads and results for another epoch are refused; a stale or duplicate result gets `result_rejected` and is never registered. Each epoch uploads into its own directory, removed when its lease ends.
- **Lease expiry** (agent died, channel lost, agent quiet) puts the job back at the head of its class queue: queues are kept in admission order and a re-queued job keeps its number, and a free agent goes to the waiting job with the lowest one. The snapshot shows `queued`, stages reset, `stage` "worker lost, retrying on another worker". This replaces phase 1's immediate `worker_crashed` on a lost channel: a lost channel now only detaches the lease, which runs out at its deadline.
- **Retry bound** (§10.6): `TOLMAP_WORKER_RETRIES`, default 2. The third lost worker fails the job `worker_crashed` ("lost 3 workers: …"). A release for a master or worker stop and a restart do not count.
- **Terminal is final.** No row leaves `done` or `failed`, and a terminal row is written from the same snapshot that then, normally, becomes the in-memory one -- durable first (#167: a run once let `done` reach `wait_done` over the API while the row still read `running`, caught by `tests/service_restart.rs`'s SIGKILL case). A write that fails is retried with backoff (one try, then 50 ms/200 ms/800 ms apart); if every retry still fails, it is logged loudly and the terminal state is published anyway, on the review's ruling that a client stuck watching a job that never reports `done`/`failed` is worse than a row that might re-run the job after a crash (harmless: the map is already stored, and the rebuild is byte-identical). A cancel is written before `POST /api/jobs/{id}/cancel` answers the same way, and a re-queue racing it never brings the job back.
- **Restart.** `jobs::restore` reloads live rows before the public listener opens and before any agent can connect. Queued jobs return to their queues in admission order, which rebuilds the dedup keys and the queue bounds with them. A leased or running job takes a slot again and its runner adopts the lease with its deadline one TTL from now; its agent is gone, so the lease runs out and the job re-queues without counting, `stage` "the service restarted; retrying the job on a worker". `GET /api/jobs/{id}` answers from the table for a job that ended before the restart. Finished rows are pruned to the newest 1,000 at startup.
- **Graceful stop (SIGTERM).** Admission stops with `503 server_stopping`, nothing fails, queued jobs stay queued, and a running job released on `shutdown now` goes back to `queued` without counting. Open SSE streams end without a terminal frame (axum's graceful drain would otherwise wait on them).

Where step 2 differs from this document, plainly (step 4 settles the third point below):

- **§6 "duplicate or stale result":** a repeat result for the current epoch is refused with `result_rejected` ("duplicate"), not acknowledged again, because the brief for this step asks for duplicates to be refused. The first result is the only one ever registered either way. *Reversed by step 3*, which follows §6: a repeat with the same artifacts is acknowledged again.
- **The epoch rises when the next assignment starts**, not at the moment the lease expires (§2.5's "attempt 2, epoch 2"). Same effect on the wire: the next `assign` carries the new epoch. This order is what makes an epoch safe across a crash.
- **One retry counter per job, not per class.** There is one class until step 4, when rerouting to another class will need the counter reset per class. *Settled by step 4*: the bound is per class.
- **An adopted lease has no channel**, so after a restart the job waits one TTL before it re-runs. Step 3 does not change this: an adopted lease is not resumable (below).
- **Unregistered uploads are still deleted when their lease ends**, not after 24 h (§4.2), as in phase 1.

`src/service/workers.rs`'s tests cover the re-queue on a lost agent and on a quiet one, the retry bound, stale and duplicate results, cancels while leased and while running, and an in-process restart; a small in-process TCP relay (§9) that pauses or cuts the channel carries a real agent in one of them, and step 3 will reuse it. `tests/service_restart.rs` runs real processes: an agent SIGKILLed mid-job with `loopback:2` (the job re-runs, one lost worker, map byte-identical to `tolmap build`), and the master SIGKILLed or SIGTERMed mid-job with `loopback:1` and restarted on the same store (every job completes, no attempt counted, queued jobs in order, a finished job still answers `GET`).

**Step 3: resume** ([#156](https://github.com/onsager-ai/tolmap/pull/156)), §2.5's first branch and §3.5, in loopback mode only:

- **A lost channel no longer kills the job.** Phase 1 killed it and exited, and step 2 kept that. The agent (`src/service/agent.rs`) now keeps running its job and redials with backoff and jitter: about 200 ms doubling, capped at a quarter of the lease TTL between 1 and 10 s, less up to half at random. It exits only on `shutdown` or on an error redialling cannot mend: the master refused its token (401), sent a protocol `error`, or nothing listens on its loopback master's port any more (the master that started it is gone, and a successor would have minted new tokens).
- **The agent's buffer** (`Outbox`) holds the held job's events until the master acknowledges them. `progress` is coalesced to the latest value per stage since the last stage boundary; `stage_*`, `features`, `error` and `result` are kept in order; `log` lines waiting for a channel are capped at 64, with one marker line counting the rest. It grows with the stages a job runs, not with its length. `lease_renewed.acked_seq` drops the acknowledged prefix.
- **`seq` is given when an event is written to a channel**, not when the child produces it. On `continue` with `acked_seq` A, the agent drops everything up to A and writes the rest again from A + 1, so the master sees one gapless sequence per job and epoch. The master applies `seq == next_seq`, ignores lower (a replay), and treats higher as a protocol violation, so A is exactly what it applied.
- **`hello.resume`** names the held job with its epoch and last written `seq`, and the agent advertises `resume`. The master refuses `hello.resume` from an agent that did not advertise it (`protocol_error`). It answers `continue`, with its own `acked_seq`, when the same token holds the lease at that epoch, the lease is neither cancelled nor run out, and the runner has not already found it expired. The lease then moves to the new channel with a fresh deadline, and an old channel still attached is closed. Anything else is answered `cancel`: `reason` `cancelled` if the user cancelled the job meanwhile, `lease_lost` otherwise. The agent kills the job's process group, discards its files and sends `released`, which the master accepts from the lease's agent on any of its channels.
- **A stalled channel is a lost one.** An agent holding a job that hears nothing from the master for a whole lease TTL (heartbeats are answered with `lease_renewed`) drops the channel and redials. That is how a paused or half-open connection ends up answered like a long drop.
- **Results.** A finished result's files stay on disk until the master's verdict, for at most `TOLMAP_WORKER_RESULT_HOLD_S` (24 h by default), and the result is replayed like any event. An upload that got no answer, or a 5xx, is sent again with backoff within the same hold; a 4xx is final. Uploads are content-addressed, so a repeat is a no-op. The master remembers the last 256 settled results with their artifacts and verdict. A repeat for the current epoch with the same artifacts gets the same verdict again, a different one `result_rejected`, and neither is registered. This reverses step 2's first departure. A resume of a settled job is answered `continue` and the verdict is sent again, since the agent lost it with the old channel.
- **Master restart.** Resume changes nothing there. Loopback agents are the master's children: after a master restart they are new processes and hold nothing, so an adopted lease runs out and re-queues uncounted, as in step 2. A master that dies leaves its agents with nothing to redial, and they kill their jobs and exit.

Where step 3 differs from this document, plainly:

- **An adopted lease is never resumed.** §6 "master restarts mid-job" extends every lease by one TTL so agents can come back. But an adopted lease records no holder the master could check a token against, and loopback tokens are minted anew at each start anyway. Resuming across a master restart needs a stable worker identity (phase 3's token file binds a token to a `worker_id`) persisted with the lease.
- **`welcome.resume[]` gained an optional `reason`** (`lease_lost` or `cancelled`) for `cancel` answers, as §2.5's diagram has it. §3.2's table lists none, so it is absent on `continue` and an agent that finds none reads `lease_lost`.
- **A resume of a job whose result already settled is answered `continue`, not `cancel`**, although the job is terminal (§3.5 says `cancel` unless the job is live). `cancel` would make the agent discard a result the store already holds, which is harmless, but it would never hear the verdict. `continue` plus the verdict again is §6's "acknowledged again".
- **A terminal `error` written on a live channel lets the job go at once.** The master sends no acknowledgement for an `error`: it ends the lease. If the channel dies between that write and the master's read, the error is lost and the lease runs out as for a lost worker, counted. An `error` produced while the channel is down is buffered and replayed like anything else.
- **The hold time starts when the job child finishes**, and bounds both the upload retries and the wait for a verdict. A job still running is kept however long the channel is down.
- **Settled results are remembered by count (256), not by time.** An agent asking after its entry is gone is answered `cancel` and discards a result the store already has.

`src/service/workers.rs`'s tests run the real agent behind the relay (§9): a cut shorter than the lease resumes without a re-run (one child start, epoch 1, attempt 1), with SSE progress that never decreases and runs on to the child's last value across the cut, the stage finished during the drop applied once, and the stored map byte-identical to what the child wrote. A cut the relay keeps refusing past the lease, and a pause past the lease, both end with the job finished on the other agent at epoch 2 and the old agent answered `cancel`, its child killed and the agent ready again. A cut the moment the result's uploads are in registers the result once and the agent hears a verdict. Scripted agents cover the master's answers: holder, other token, stale epoch, unknown job, replay ignored, verdict sent again on resume, repeat acknowledged, different result refused, and `hello.resume` without the feature. `src/service/agent.rs`'s tests pin the buffer: coalescing never crosses a stage boundary, the log bound and its marker, and a rewind's renumbering.

**Step 4: worker classes, rerouting and out-of-memory escalation** ([#157](https://github.com/onsager-ai/tolmap/pull/157)), §2.1, §6, §7 and §10.6, in loopback mode only (local mode keeps its one class, and an OOM there fails the job as it always has):

- **Configured classes.** `TOLMAP_LOOPBACK_CLASSES` (`<usable memory>:<agents>`, comma-separated, e.g. `2GiB:1,16GiB:1`; the counts must add up to `loopback:N`, and anything malformed stops startup) starts each loopback agent with `--class-memory`, which it advertises in `hello.class.memory_bytes` instead of its host's memory. The registry has one slot per agent, numbered smallest class first, and agents are numbered the same way. The hub places each connecting agent in the largest class its advertisement covers, and a runner asks for an agent of its slot's class, so a job spilled down to a large slot runs on a large agent. Unset, `loopback:N` has one class of unknown size, as before.
- **Prediction in §2.1's order.** Timing rows carry the slug now (`job_timings.slug`), and `MemoryModel::predict_for` predicts a job from its slug's newest finished peak (step 1), scaled along the hand curve's slope by the change in file count once the job has reported one, with a 25% margin; a failed or killed row newer than it is a floor the prediction never goes below. Without a finished row it is the reference-mode prior before `features` (step 2) and the features after (step 3), never below a floor. Admission binds with it.
- **Reroute after `features` (§2.1 step 3).** When a running job's `features` name files and the prediction exceeds the class the job runs on (its slot's, which is larger than the job's own after a spill) and a larger class exists, the master sends `cancel` `reroute`. On `released` `reroute` the job is rebound to the smallest class that holds the prediction (the largest if none does), epoch + 1, attempt not counted. It is asked once per lease; a job is never rerouted down, and since a rerouted job is bound to its new class and runs on it or a larger one, never twice to the same class. Unlike a user's cancel the lease stays live, so a result already on its way is still registered, and an agent whose channel dropped meanwhile hears the reroute from `resume`.
- **Out-of-memory kill (§6).** The agent reads the job child's exit: a SIGKILL that neither it nor its executor sent, and, where the memory cgroup's `oom_kill` count can be read (`memory.events` on cgroup v2, `memory.oom_control` on v1, found from `/proc/self/cgroup`), a count that rose during the job. It then sends `released` `oom` with the child's peak instead of an `error`. The master records the peak as a timing row of its own (`<job>/e<epoch>`, stages unfinished, so it is a floor for the slug and never narrows the population curve) and rebinds the job to the next larger class, epoch + 1, uncounted; on the largest class it fails `worker_crashed` "out of memory on the largest worker class".
- **Lost worker that died of memory (§6).** Heartbeats carry `rss_bytes`: the resident memory of the job child's process group, summed from `/proc/<pid>/stat` (`pgrp` and `rss`). The cgroup's own usage was not used: a loopback agent shares its cgroup with the master and the other agents. When a lease runs out and its last heartbeat had the job within 10% of the class's usable memory, the job is rebound to the next class, counted as the lost worker it is.
- **Retry bound per class (§10.6).** `jobs.class_lost` counts lost workers on the job's current class and is what `TOLMAP_WORKER_RETRIES` bounds; any move to another class resets it (`Store::rebind_job`). `attempt` still counts every lost worker over the job's life. Reroutes and escalations count in neither.
- **Spill-down only with an agent to spill with, and the ETA to match.** A slot takes its own class's jobs whatever its agents are doing (only that class's agents can run them anyway, so a class whose agent is restarting keeps its jobs and ETAs where they were), but spills down to a smaller class only while its own class has a connected agent no busy slot accounts for. `simulate_queue_etas` leaves out exactly the idle slots that may not spill, so a queued job is never quoted a start of 0 on a slot with nothing to run it, and a job no counted slot can take has no `eta_start_s`. The hub tells the registry when an agent connects or goes away, which dispatches and re-quotes.

Where step 4 differs from this document, plainly:

- **The class list is the configuration, not the advertisements.** The brief for this step had the master build its classes from what connected agents advertise. In loopback mode the master is what tells each agent what to advertise, so the configured list and the advertised one are the same by construction; the registry's classes, slots and queues are built once from the configuration, and each advertisement only places its agent in a class (and an agent smaller than every class is kept connected and idle, as a build mismatch is). A class whose agent is not connected still exists, so binding and the queue model stay put while an agent restarts. Remote workers (phase 3) will need classes that come from their `hello`s.
- **A rebound job goes into its new class's queue by admission order**, as a re-queued job does since #155, not strictly at the head (§2.1 "at the head of that queue", §6). It is the head unless a job admitted before it already waits in that class, which then keeps its turn.
- **§2.1 step 1 predicts from the slug's newest finished peak with a fixed 25% margin**, not the population curve's upper quantile (2.27×), which measures the spread between different repositories rather than one repository across commits. The spec names no margin.
- **Out-of-memory detection without a cgroup takes any SIGKILL the agent did not send**, as §6 allows. Another process on the host that SIGKILLs the job child is then read as an OOM: the job moves up a class, or, on the largest, fails with the out-of-memory message rather than a plain crash.
- **`rss_bytes` is the job child's process group**, so a dependency install (run by the agent in its own jail, outside that group) is not in it.

`src/service/workers.rs`'s tests run two classes on one runner: a job predicted large waits for the busy large agent while small jobs flow through the small one; the large agent spills down to a small job and a large job admitted meanwhile waits for that one job; queued ETAs match a hand-computed two-class schedule, with no start for a job no agent can take and none of 0 on an agentless slot; the queue bound holds per class; a scripted job whose `features` outgrow the small class is rerouted uncounted and finishes on the large one; a lost small agent whose last heartbeat was at 95% of its class moves the job up, counted; and with real agents a fake OOM (a job child that raises the `oom_kill` count in the agent's events file and SIGKILLs itself) escalates from the small agent and fails on the large one, while a SIGKILL that raised no count fails as a plain crash. `tests/service_classes.rs` runs `tolmap serve` with a 512 MiB and a 16 GiB agent: a generated 1,600-module project is admitted small, rerouted after detection, and its map is byte-identical to `tolmap build`'s (epoch 2, attempt 1). `store.rs`, `eta.rs` and `agent.rs` test the per-class bound, the slug prediction, and the OOM, cgroup and `/proc` parsing.

**Step 5, docs.** `docs/API.md` documents `TOLMAP_LOOPBACK_CLASSES`, the per-class retry bound and class moves in the worker-protocol paragraph; no `JobSnapshot` field was added. The loopback-mode graceful-shutdown and restart paragraph from #155 still reads right with classes: a released job goes back to its queue, and a rerouted or escalated job's class is in its row, so a restart keeps it.

**Phase 3: the first remote worker (decisions §10.1, §10.4, §10.5).**
- One 16 GB worker host runs the same image as `tolmap worker --connect wss://<master's private address>/…` with an owner-issued token. It is stopped when idle and started when a job is queued (§7.4).
- The worker listener is bound to the provider's private network only (§5.6). Nothing new is public.
- The sandbox runs on the worker host. Installs, off in hosting until now, may be enabled there with `TOLMAP_SCIP_INSTALL=sandbox`, never on the master.
- Loopback agents are switched off, which removes untrusted indexing from the master. The master can then shrink and run as a non-root user.
- The hosting configuration lives in the private hosting repository.

**Phase 3's code, item 1: remote mode.** `TOLMAP_WORKERS=remote` (or `remote:N` for N slots; the decided fleet is one worker, so `remote` is one): the master opens the worker listener with TLS and the owner's token file (§5.1, §5.6) and starts no agents. Its classes are one class of unknown size with N slots, as plain `loopback:N` has; classes built from remote workers' `hello`s wait for a fleet with more than one class. What else it changes:

- **An adopted lease is resumed by its worker** (lifting step 3's first departure). A lease's holder in the store is the worker id its token is bound to. A restarted master adopts each leased or running job's lease with that holder before the worker listener opens (`jobs::restore`, so an agent that redials at once finds it), and `resume` hands it back to an agent presenting a token for the same worker id at the same epoch: the job carries on from the master's `acked_seq` (0, since nothing of the new process has been applied), with no re-run and no attempt counted. Loopback mode is unchanged: a minted token names no one after a restart, so its adopted leases still run out and re-queue uncounted. A result whose artifacts were uploaded to the previous process is refused (`result_rejected`), since this process cleared its staging, and the job re-runs uncounted rather than failing for the restart.
- **Graceful stop** (§6 "master graceful stop", remote mode): no `shutdown` is sent. The master closes every channel and refuses new ones (503) until it exits; each running job's row stays `leased`/`running`, and its agent, still running it, resumes it with the next process.
- **A remote agent keeps redialling** (step 3's departure 7, lifted for remote agents): refused connections, TLS failures and 503s are retried with the same backoff and jitter, first dial included, and it exits only on `shutdown`, a 401 or a protocol `error`. An agent a loopback master starts is marked `--loopback` and keeps its old rule: nothing listening means its master is gone.
- **`local_paths`** is offered only by an agent whose master is on its own loopback, so a remote worker is never assigned a `local/<name>` job (§3.3).

Where it differs from this document, plainly:

- **`--ca-file` replaces the public roots rather than adding to them.** A certificate some public CA issued for the master's name is not the owner's master, so with a private CA configured only that CA is trusted.
- **`--worker-id` is a flag of the agent**, required for a remote agent: the token file on the worker host holds only the token, and the master checks the id against the token's line.
- **The listener speaks TLS 1.3 only.** Both ends are this binary.
- **The token file is checked for changes on every request rather than re-read on every connection:** a `stat` per request, a read only when it changed.
- **Unregistered uploads are still deleted when their lease ends, and at startup**, not after 24 h (§4.2).

`src/service/workers.rs`'s tests cover the token file (parsing, fail-closed reloads, revocation at the next dial, artifact request and heartbeat), the worker id check, the frame bound, TLS settings and the key's mode, and a restarted remote master handing an adopted lease back to its worker and to no other. `tests/service_remote.rs` runs real processes over TLS on `127.0.0.1` with the committed test-only certificate (`tests/fixtures/tls`): `worker-token new`'s output, the startup refusals (plaintext off loopback, a `0644` key, no token file), an agent without `--ca-file` retrying, one with it building a map byte-identical to `tolmap build`'s, and the master SIGKILLed or SIGTERMed mid-job and restarted, the job finishing at epoch 1, attempt 1.

### Running a remote worker

What the owner does to run one remote worker, provider-neutral (machine names, addresses and deploy configuration belong in the private hosting repository):

1. **A certificate for the worker listener.** Make a private CA and a server certificate it signs for the name or address the worker will dial on the private network (a `subjectAltName` for that DNS name or IP address), with its key in PEM (PKCS#8 is simplest). Any tool that makes an X.509 CA works, for example `openssl req -x509` for the CA and `openssl x509 -req -CA ...` for the server certificate. Keep the CA's key offline; only the CA's certificate goes to the worker host.
2. **A token for the worker.** Run `tolmap worker-token new --id <worker-id>` anywhere. The first line is the token and goes to the worker host only; the second line goes into the master's token file.
3. **On the master:** `TOLMAP_WORKERS=remote`, `TOLMAP_WORKER_LISTEN=<private address>:<port>`, `TOLMAP_WORKER_TLS_CERT` and `TOLMAP_WORKER_TLS_KEY` (the key at mode `0600`), and `TOLMAP_WORKER_TOKENS` naming the token file (the line from step 2, not writable by group or others). The listener must be reachable on the private network only.
4. **On the worker host:** the token in a `0600` root-owned file, the CA's certificate, and `tolmap worker --connect wss://<private name or address>:<port>/workers/connect --token-file <file> --worker-id <worker-id> --ca-file <ca.pem> --cache-dir <dir>` under a supervisor that restarts it. Installs may be enabled there with `TOLMAP_SCIP_INSTALL=sandbox` (§5.5), never on the master.
5. **Revoke** a worker by deleting its line from the token file (rename a new file over it); **rotate** by adding the new token's line, moving the worker to the new token, then deleting the old line.

**Phase 3's code, item 2: build identity and capacity.** It carries a real `WorkerBuild` (§3.6, `TOLMAP_BUILD_COMMIT` and `TOLMAP_INDEXER_VERSIONS`, additive on `GET /api/healthz`) and `GET /workers/capacity` (§7.4, `TOLMAP_WORKER_MAX_PER_CLASS` and `TOLMAP_WORKER_IDLE_S`). With item 1, the code for a remote worker is complete; starting one still needs the owner-issued token, the TLS certificate and the private hosting configuration this document keeps out of the public repository.

**Phase 4: scale.** More workers or classes within new owner-set bounds, presigned object storage once there is more than one API node (§4.2), and several API nodes on Postgres with the SSE fan-out kept on the master tier's private network (LISTEN/NOTIFY or equivalent), never reachable by workers.

## 9. Test plan

This repository's rule is that heavy builds and browser checks run on GitHub Actions, and everything below is designed for standard runners with no hosting.

**What CI proves**

- **Protocol shape.** Serde round-trip tests for every session message and the enveloped v1 events, like `worker.rs::protocol_round_trip`. A v1 `WorkerEvent` stream from an unmodified job child parses unchanged inside envelopes. Unknown optional fields are ignored and ungated message types are refused.
- **Byte identity across execution paths.** The same fixture through `tolmap build`, through `tolmap serve` in local mode and through `tolmap serve` with `loopback:2` produces byte-identical map and symbols documents. This is the determinism rule extended to the new seam, and the strongest single test here.
- **The single-slot ETA is unchanged.** The phase 0 simulation reproduces today's `refresh_queue_etas` values exactly for one slot, over a table of queue shapes. Multi-slot and multi-class cases are checked against hand-computed schedules.
- **Class selection and spill-down.** With a "small" and a "large" loopback agent, a large job waits for the large agent while small jobs run on the small one; a large job that arrives while the large agent spills starts after that one small job.
- **Failure injection**, with a small in-process TCP relay between agent and master that can pause, drop or cut the connection:
  - kill the agent mid-job: the lease expires, the job re-runs on the other agent and ends `done`, and the result is byte-identical;
  - drop the channel for less than the TTL: the job resumes without a re-run, and its snapshot has no gap or decrease in progress (the existing "progress never decreases" SSE test, extended);
  - SIGKILL and SIGTERM the master mid-job, then restart it on the same store: the job completes (phase 2);
  - a fake OOM (a test job child that kills itself with SIGKILL after raising a marker): the job escalates to the larger agent; on the largest it fails `worker_crashed`;
  - duplicate and stale results from a scripted fake agent: refused, never registered;
  - the cancel race matrix of §6, including cancel during `install_request` (the image workflow already runs the install through `tolmap serve` in a `--privileged` container, and gains a loopback variant);
  - worker `draining` and master `shutdown` in both modes; local-mode `server_stopping` tests stay green unchanged.
- **Authentication and scoping.** No token, a wrong token, a revoked token, a token for another `worker_id`, a result for a job the agent does not hold, an artifact with the wrong digest or size, an over-sized frame, and a remote result naming a filesystem path: each is refused and none reaches the store. A test asserts that no `WorkerSpec` or `JobSpec` an agent receives contains the store's path.
- **Bindings.** Any field added to `JobSnapshot` goes through the existing generated-bindings check.

**What CI cannot prove**

- Real network partitions and latency between hosts, and the platform edge's handling of long-lived WebSockets and idle timeouts.
- Worker cold-start times, image pull times, and the autoscaler against a real provider.
- The sandbox on the production kernel and cgroup setup (finding 46 "What was not exercised" still applies, now on the worker host).
- Memory predictions for repositories the corpus lacks. The model refits from production jobs; its accuracy there has to be watched, not assumed.

These need a staging worker host, which is phase 3 and spend.

## 10. Decisions

The owner decided all six questions this specification raised (AskUserQuestion, session `16030105`, 2026-09-26; recorded on [#97](https://github.com/onsager-ai/tolmap/issues/97), 2026-09-26T06:07Z). The alternatives considered are noted in the sections cited.

**10.1 Worker classes and bounds.** One 16 GB class, at most one worker, stopped when idle and started when a job is queued (AskUserQuestion, session `16030105`, 2026-09-26). This is the size production already runs. Considered: a small class for hand jobs next to it, and an on-demand 32 GB class for the msgraph-sized outliers. The phase 0 memory measurements are the evidence for revisiting this. See §2.1, §7.1 and §7.4.

**10.2 Control channel.** WebSocket on axum. The protocol's types stay defined once in Rust (AskUserQuestion, session `16030105`, 2026-09-26). Considered: gRPC bidirectional streaming. See §4.1.

**10.3 Artifacts and retention.** HTTPS PUT and GET through the master, scoped to the job's lease, stored on the master's disk. Today's retention stays, and uploads from jobs that never registered are deleted after 24 h. Object storage waits until there is more than one API node (AskUserQuestion, session `16030105`, 2026-09-26). See §4.2.

**10.4 Worker authentication and exposure.** One bearer token per worker, stored hashed on the master, issued and rotated by the owner, over TLS, on a separate listener reachable only on the provider's private network. Nothing public (AskUserQuestion, session `16030105`, 2026-09-26). Considered: mutual TLS, platform workload identity. See §5.1 and §5.6.

**10.5 Sandbox move.** The sandbox moves with the first remote worker (phase 3). Installs stay off in hosting until then (AskUserQuestion, session `16030105`, 2026-09-26). See §5.5 and §8.

**10.6 Retries.** Bounded. A job whose worker keeps dying fails with `worker_crashed` after a bounded number of lost-worker retries. There are no admission caps (AskUserQuestion, session `16030105`, 2026-09-26). The default bound is two lost-worker retries per class, configurable. A job released for a graceful worker stop or a reroute does not count against it, and neither does a finished job, however long it ran. See §6.
