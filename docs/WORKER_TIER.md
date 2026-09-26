# Production worker tier (#97)

**Status: specification, not implemented.** Owner timing ruling on [#97](https://github.com/onsager-ai/tolmap/issues/97) (AskUserQuestion, session `16030105`, transcript line 6521, 2026-09-25T13:51:51Z): **"After P2a merges."** P2a ([#127](https://github.com/onsager-ai/tolmap/pull/127)) merged 2026-09-25T18:07Z and the P1c sandbox ([#129](https://github.com/onsager-ai/tolmap/pull/129)) 17:48Z, so the design starts here. That ruling expected P2a to make SCIP the default; the owner then kept `hand` as the default (finding 45, "The default did not flip", 16:56Z). This matters below, because most hosted jobs are therefore small.

Owner rulings this document builds on, all on #97: **workers never touch the database**; they dial the master over WebSocket or another RPC channel, so the database is never exposed (2026-09-24). **No caps**: no file-count, clone-size, history-depth or job-time admission caps. **Cancel + queue ETA**: every queued job shows its ETA, including the jobs ahead of it (transcript line 2157, 2026-09-24T01:20:31Z).

Reserved to the owner, and listed in §10 rather than decided here: hosting spend (worker classes, autoscaling bounds, object storage), credentials (worker tokens, any platform API token), and making the worker endpoint publicly reachable. This document is provider-neutral on purpose. Machine names, prices and deploy configuration belong in the private hosting repository, not here.

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
- **Results are paths on a shared filesystem.** The child's `result` names `map_path`, `symbols_path`, `symbols_dir` and `names_cache` inside its output directory; the service renames them into the store. That cannot cross a network and must not: a remote worker's path must never be opened on the master (§5.3).
- **All job state is in memory.** The `JobRegistry` queue, cancel set and snapshots live in the serving process. A master restart fails every job (`server_stopping`), and the private deploy notes record the resulting risk when a platform stops an idle machine.
- **The cost model predicts time, not memory.** `src/service/eta.rs` predicts per-stage seconds from repository features (finding 38) and refits from completed jobs. Nothing predicts peak memory, which class selection needs.
- **The queue ETA assumes one slot.** `refresh_queue_etas` adds up the remaining time of every running job, then each queued job in turn. With `TOLMAP_MAX_CONCURRENT_JOBS` above 1 that overstates the wait, because a queued job starts when the first slot frees, not after all of them.
- **What jobs cost.** With `--refs hand`, the default, peak RSS tracks file count (finding 18, r = 0.937 on log–log): median 21 MB, 114 MB, 520 MB and 1.8 GB for the small, medium, large and ultra bands, p90 5.3 GB in ultra. There are two outliers: msgraph-sdk-python at 13.8 GB, and msgraph-sdk-go, which OOMed at 23.3 GB. With `--refs scip`, peaks are 0.4–3.9 GB on the nine small fixtures (finding 45) and 4.9–9.1 GB at corpus scale (finding 44). A sandboxed install adds up to 1.8 GB (finding 46: dify 8.65 GB, n8n 8.43 GB).

## 1. Goals and non-goals

**Goals**

1. **Scale out large reports.** Run jobs on more than one machine at once, and put each job on a machine with enough memory for it, so a 13 GB job does not need every machine to be 16 GB and a 20 MB job does not wait behind it.
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

### 2.1 Class selection

The master picks a class for each job from a **predicted peak RSS**, compared with each agent's advertised usable memory (total minus a reserve for the agent and the OS, 1 GiB by default).

The prediction uses, in order:

1. **This slug's own last measurement**, scaled by the change in file count. The same repository at a nearby commit is the best predictor there is.
2. **The reference mode**, known at admission: `scip` jobs start from findings 44–46's per-indexer peaks, and `hand` jobs from finding 18's per-band figures.
3. **Features reported mid-job.** The job child already sends `features` after clone and after detection (file and byte counts per language), and the agent forwards them. If the master's prediction from those features exceeds the agent's class, it sends `cancel` with `reason: "reroute"`, and on `released` it rebinds the job to the larger class, at the head of that queue. The memory model stays on the master; agents never need it. Clone and detection cost seconds, and the second attempt starts with known features instead of a guess.
4. **An OOM kill**, the last resort: re-queue on the next larger class (§6).

Memory needs a model next to the time model. Each finished job records its peak RSS alongside its stage durations: the agent (or the master in local mode) reads it from `getrusage(RUSAGE_CHILDREN)` after reaping the job child, which gives the largest single process in the tree, the same measure findings 41–46 report. The model fits log(peak) against log(files) per reference mode and language, seeded from the findings above, and uses an upper quantile. It errs high on purpose: underestimating memory costs an OOM and a rerun, while overestimating costs a larger machine for one job. This does not conflict with the rule that numbers must be a lower bound, which governs what a map reports, not internal scheduling estimates.

A job whose prediction exceeds every class is bound to the largest class anyway. No caps means best effort, never a rejection.

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
- **The commit is pinned.** The master resolves the commit before admission and the cache key is `(slug, commit)`; the agent checks out that commit, not whatever the branch points to by then. Today the child resolves HEAD itself after cloning, which can differ from the admitted commit if the branch moved in between. Remote mode closes that gap; local mode keeps its current behaviour unless phase 1 fixes both.

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
   {"name": "symbols_dir", "sha256": "<hex>", "bytes": <n>},
   {"name": "names", "sha256": "<hex>", "bytes": <n>}
 ]}
```

`symbols_dir` is uploaded as one uncompressed tar with entries in sorted order and zeroed metadata, so its digest is reproducible. The master unpacks it into the store. In remote mode the master refuses a `result` whose path fields are anything but `artifact:` names.

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

## 4. Transport

### 4.1 Control channel: WebSocket or gRPC

| | WebSocket | gRPC bidirectional streaming |
|---|---|---|
| server dependency | axum's `ws` feature (brings `tokio-tungstenite`); axum is already the HTTP stack | `tonic`, `prost`, `h2` and a code generator (`prost-build` needs `protoc`, or a pure-Rust substitute); a second server stack beside axum |
| client dependency | `tokio-tungstenite` with rustls | `tonic` client |
| schema | the serde types already in `src/worker.rs`, as JSON text frames; the v1 events cross unchanged | a `.proto` file: a second hand-written definition of the worker events next to the Rust types, which `CLAUDE.md` rules out ("two hand-written definitions will drift"), or opaque JSON bytes inside protobuf, which throws away what gRPC offers |
| proxies and load balancers | an HTTP/1.1 Upgrade: passes nearly every reverse proxy and platform edge; needs keepalive traffic against idle timeouts, which the 15 s heartbeat provides | needs HTTP/2 end to end; some platform edges terminate HTTP/2 and speak HTTP/1.1 to the backend unless configured otherwise, and some corporate proxies strip trailers |
| backpressure | TCP only; the application bounds its send queue and coalesces progress (§3.5) | per-stream HTTP/2 flow control, deadlines and keepalive built in |
| debuggability | readable frames; `websocat` can play a fake worker | needs `grpcurl` and the `.proto` |
| payload efficiency | JSON, about four frames a second per job at most | protobuf; irrelevant at this rate |

**Recommendation: WebSocket.** It reuses the serde types that already define the seam, adds one small dependency family to a stack that is already axum and tokio, and crosses more proxies. gRPC's advantages, flow control and binary framing, matter for high-rate or large streams, and this design moves the only large payloads off the channel (§4.2). Deadlines and keepalive are replaced by the lease and heartbeat, which are needed anyway.

### 4.2 Artifacts: over the channel, through the master, or presigned object storage

A map plus its symbols document and per-district files is small for most repositories (the nine committed fixtures are 0.1–0.6 MB each) but can reach tens of MB on the largest ones. That figure is the issue's estimate: the largest corpus map's size was not measured for this document. Artifacts flow both ways: results up, and warm-start maps and names caches down.

| | A. streamed over the WebSocket | B. HTTPS PUT and GET to the master, lease-scoped | C. presigned object-storage URLs |
|---|---|---|---|
| how | chunked binary frames on the control channel | `PUT /workers/artifacts/{job}/{epoch}/{name}` with the worker token; the master checks the lease and streams the body to disk while hashing | the master signs short-lived URLs that expire with the lease; workers talk to the object store directly |
| control traffic | heartbeats and cancels queue behind artifact chunks unless frames are interleaved by hand | unaffected | unaffected |
| resume after a drop | needs chunk offsets and acknowledgement logic | retry the PUT; content-addressed, so a repeat is harmless | retry the PUT |
| master load | bytes pass through the master's memory and bandwidth | bytes pass through the master's bandwidth and disk, streamed, not buffered | none; the master only signs |
| new infrastructure | none | none | an object store, its credentials on the master, and a retention policy (spend and credentials, §10) |
| worker credentials | the channel token | the channel token, checked against the lease | none standing: URLs die with the lease |

**Recommendation: B now, with the protocol carrying URLs so C needs no protocol change.** `assign` gives the agent URLs, and the agent does not care whether they point at the master or an object store. B needs no new infrastructure and keeps artifacts off the control channel. C becomes worth it when the master tier has more than one API node, or when artifact traffic starts to matter for the master's size. That is phase 4 and an owner decision.

Integrity is the same in every option. Each upload carries its SHA-256, the master recomputes it while streaming, and a mismatch refuses the upload. Stored artifacts are content-addressed, so a duplicate upload is a no-op.

### 4.3 Frame and message bounds

These bound the protocol, not jobs: the master rejects a control frame over 1 MiB (the largest legitimate frame is a `features` or `log` event), limits each agent's frame rate, and refuses an artifact whose declared size disagrees with its body. None of these limits a repository's size.

## 5. Security

### 5.1 Worker authentication

The agent presents a **per-worker bearer token** in the `Authorization` header of the WebSocket upgrade and of every artifact request, never in a URL, where it would be logged. Non-loopback connections require TLS (`wss`, `https`); the agent refuses plain `ws` to anything but loopback.

Options for the credential itself are in §10.4. The recommendation is random 256-bit tokens, one per worker, stored on the master only as SHA-256 hashes in a file named by configuration, each line binding a hash to a `worker_id`. Revoking a worker means deleting its line; rotating means adding a new line, moving the worker to the new token, then deleting the old line. A small `tolmap worker-token new --id <id>` command would print the token once and the line to add. **Issuing, storing and rotating these tokens is a credential decision reserved to the owner.** Nothing in phases 0–2 needs one: loopback agents get an ephemeral token the master generates in memory at start-up (§8).

On the worker host the token sits in a root-only file (mode 0600) read by the agent. The job child runs as uid 10001 with an empty environment, so it can read neither the file nor the agent's memory.

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
| make the master open a file it names | in remote mode the master never interprets a worker-supplied string as a filesystem path: results name artifacts (§3.4), and the master chooses every path it writes. Local mode confines child-reported result paths to the job's output directory (phase 0) |
| mint tokens, see other workers' tokens, or reach other workers | tokens are issued out of band and stored hashed; workers dial the master only |
| obtain service secrets | none are on the host (§5.2) |

### 5.4 Checking results

Registration checks each artifact's SHA-256 and size, parses the map as a `MapDocument` and the symbols document against its schema, and checks that the result's `commit` is the job's pinned commit and `files` and `districts` match the document. Because tolmap's output is deterministic, the master can also re-run a sample of jobs on a second worker and compare digests. A mismatch is either a determinism bug or a lying worker, and both are worth an alert. The sampling rate is an operational setting; one job in fifty costs 2% more compute. Two results for the same `(slug, commit)` from different jobs should always be byte-identical, and the master logs any that are not.

### 5.5 Where the nsjail sandbox goes

The install sandbox moves with the executor: it runs on the worker host, started by the root agent, exactly as the root service starts it today (`docs/API.md` "Dependency installs"). Policy, mounts, egress proxy, self-test and the 20 min / 20 GB fallback bound are unchanged. `docs/SCIP_SANDBOX.md` §4.2 sets out why this ends the accepted risk: a kernel exploit from the jail now becomes root on a worker host that holds one token and no master data, instead of root on the machine holding the store, every map and the service's secrets. The in-VM jail still matters there, because it keeps install code away from the agent's token and the host's network.

Two consequences follow. In remote mode the master no longer needs root, since it spawns no children and starts no jails, so it can run unprivileged. And the indexers, which run as the worker uid outside the jail (`docs/API.md` "Not covered"), now do so on a host with nothing of the master's to reach.

Production runs `TOLMAP_REFS=hand` today, so the install path is not exercised in hosting at all. The kernel-exploit exposure the sandbox carries becomes live only when hosting enables `scip` with installs. Moving the sandbox is therefore tied to the first remote worker (phase 3), not urgent before it (§10.5).

### 5.6 Exposing the worker endpoint

The worker endpoint is a separate listener (`TOLMAP_WORKER_BIND`, default unset, meaning off), not a route on the public site's port. This keeps it out from under the `/api` per-IP rate limit and the static-site fallback, and lets a deployment put it on a private interface when workers share a private network with the master. Loopback mode binds it to `127.0.0.1`. **Making it publicly reachable is reserved to the owner**, and is part of the decision in §10.4.

## 6. Failure semantics under the no-caps ruling

No failure below is an admission cap: nothing is refused for size or time. Jobs fail only when the machine cannot finish them, and then with the codes clients already know. `worker_crashed` keeps its meaning, with a message that says what happened, since clients render unknown codes as a generic failure anyway (`docs/API.md` "Errors").

| event | detected by | master action | job outcome |
|---|---|---|---|
| **job child OOM-killed** | agent: child killed by SIGKILL with the memory cgroup's `oom_kill` count raised (or, without a cgroup, SIGKILL not sent by the agent) | agent sends `released` `oom` with the observed peak; master records the peak, rebinds the job to the next larger class, puts it at the head of that class's queue, epoch + 1 | continues on a larger worker; on the largest class, fails `worker_crashed` ("out of memory on the largest worker class") |
| **job child crashes otherwise** | agent: exit without `result` or `error` | forwarded as today | fails `worker_crashed` with exit status and last stage, as today |
| **worker host dies, or its OOM takes the agent too** | lease expiry | attempt + 1, epoch + 1, head of the same class's queue; if that host died of memory (last heartbeat's `rss_bytes` near its class), rebind to the next class | continues; after the retry bound (§10.6), fails `worker_crashed` ("lost N workers") |
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

With one class, which is every deployment until phase 3, this is exactly today's single FIFO.

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

The ETA is only as good as its inputs. Finding 38 measured a cold-start n8n underestimate of 42 s at 10% of the run. With several workers, the start estimate of a queued job adds up several such errors, so the range widens with queue depth. That is honest, and the UI already shows a range.

### 7.3 Fairness

Admission already bounds request frequency per IP (30 a minute) and index requests per repository (3 per 5 minutes), and the queue length (`TOLMAP_MAX_QUEUED_JOBS`, 16). Within a class, strict FIFO keeps every queued job's ETA stable: no later job can jump it, which is what makes "the jobs ahead of it" a promise rather than a guess. Shortest-job-first would finish more jobs sooner but make every queued ETA unstable, so it is not proposed. If a single requester floods a class within the rate limits, a round-robin across requesters inside each class queue is the next step. It changes FIFO positions, so it is left until measurement shows a need.

With several classes, `TOLMAP_MAX_QUEUED_JOBS` applies per class, so a backlog of large jobs cannot fill the queue that small jobs need. A job that joins a full class queue gets `503 busy`, as today.

### 7.4 Autoscaling

The master exposes desired capacity per class: running plus queued jobs bound to that class, clamped to the configured minimum and maximum per class, and dropping back after a configurable idle period. A provider-specific scaler acts on it. The first implementation is none: a static fleet, which is all phases 1–3 need. A scaler that starts and stops worker machines needs a platform API credential or a platform autostart feature, and its bounds are spend. Both are owner decisions (§10.1), and its configuration lives in the private hosting repository. Capacity an autoscaler may start is counted in the ETA simulation at its measured cold-start time, which is refitted like a stage duration.

## 8. Migration plan

Every phase ships on its own, keeps single-process mode unchanged and on by default, and passes CI without real hosting. Phases 0–2 need no new hosting spend and no owner credential.

| phase | ships | new spend or credential |
|---|---|---|
| 0 | memory measurement and model, one-queue-per-class scheduler with the ETA simulation (one class in practice), result-path confinement in local mode | none |
| 1 | the executor split, channel protocol `proto` 1, `tolmap worker --connect`, the worker listener, `TOLMAP_WORKERS=loopback:N` | none |
| 2 | durable jobs and leases in the store, resume, restart survival, rerouting and OOM escalation, loopback agents advertising configurable classes | none |
| 3 | the first remote worker host; the sandbox moves there; the master can drop to a small class and run unprivileged | **yes**: worker class, token, public worker endpoint (§10) |
| 4 | autoscaling, presigned object storage, Postgres with several API nodes and internal SSE fan-out | **yes**: bounds, object store, database |

**Phase 0: measure and schedule, in today's process.**
- The master records the job child's peak RSS (`getrusage(RUSAGE_CHILDREN)` after reaping) next to its stage durations in `job_timings`, and `eta.rs` gains a peak-memory prediction seeded from findings 18 and 44–46. It changes nothing about a map, so the determinism and parity gates are untouched.
- `refresh_queue_etas` is replaced by the §7.2 simulation over a scheduler with per-class queues. Local mode has one class with `TOLMAP_MAX_CONCURRENT_JOBS` slots, so single-slot behaviour is identical and multi-slot ETAs become correct.
- Child-reported result paths are confined to the job's output directory: resolved without following symlinks and refused otherwise. This is the local-mode half of §5.3's rule and a hardening worth having regardless of this design.

**Phase 1: the network protocol, over loopback.**
- `run_blocking` splits into prepare, execute and register (§2). Local mode calls execute in-process and its behaviour is byte-for-byte unchanged.
- The channel protocol lands in `src/worker.rs` beside `WorkerEvent`, as serde types. `tolmap worker --connect <url> --token-file <path>` runs the agent, and the master gets the worker listener and artifact endpoints.
- `TOLMAP_WORKERS` selects the mode: unset or `local` is today's behaviour; `loopback:N` makes `tolmap serve` start N agents on its own host that dial `127.0.0.1` with an ephemeral token and their own cache directories under `cache_dir/agents/<n>` (never the store). On today's single production machine this runs the whole network protocol with no new hosting, the same memory and the same root-started sandbox. It is a switch the owner can turn on or off by configuration.
- State is still in memory in this phase, so a restart still fails jobs, now through the agents.

**Phase 2: durability and classes.**
- A `jobs` table in the master's store holds status, class, attempt, epoch, lease holder and deadline, and the last persisted snapshot. The in-memory registry becomes a cache of it in remote and loopback modes. Local mode keeps its in-memory registry and shutdown contract.
- Resume after a dropped channel, restart survival with the lease-deadline extension, reroute after `features`, OOM escalation and the retry bound (§10.6).
- Loopback agents take a configured `class` override, so CI can run a "small" and a "large" loopback agent on one runner and exercise class selection, spill-down and escalation for real.
- `docs/API.md` gains the remote-mode shutdown paragraph and any additive snapshot fields.

**Phase 3: the first remote worker (owner decisions §10.1, §10.4, §10.5).**
- One worker host of the chosen class runs the same image as `tolmap worker --connect wss://<master>/…` with its token. The worker endpoint becomes reachable from it: publicly with TLS, or over a private network if the platform offers one.
- The sandbox runs there; `TOLMAP_SCIP_INSTALL=sandbox` is set on the worker host, not the master. The master can drop to a small class and run as a non-root user.
- Loopback agents can stay as a fallback class on the master's host, or be switched off, which is what finally removes untrusted indexing from the master.
- The hosting configuration lives in the private hosting repository.

**Phase 4: scale.** A scaler per §7.4 within owner-set bounds, presigned object storage for artifacts (§4.2, §10.3), and several API nodes on Postgres with the SSE fan-out kept on the master tier's private network (LISTEN/NOTIFY or equivalent), never reachable by workers.

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
- **Authentication and confinement.** No token, a wrong token, a revoked token, a token for another `worker_id`, a result for a job the agent does not hold, an artifact with the wrong digest or size, an over-sized frame, and a remote result naming a filesystem path: each is refused and none reaches the store. A test asserts that no `WorkerSpec` or `JobSpec` an agent receives contains the store's path. The phase 0 path-confinement test sends a result naming a path outside the output directory, and a symlink inside it.
- **Bindings.** Any field added to `JobSnapshot` goes through the existing generated-bindings check.

**What CI cannot prove**

- Real network partitions and latency between hosts, and the platform edge's handling of long-lived WebSockets and idle timeouts.
- Worker cold-start times, image pull times, and the autoscaler against a real provider.
- The sandbox on the production kernel and cgroup setup (finding 46 "What was not exercised" still applies, now on the worker host).
- Memory predictions for repositories the corpus lacks. The model refits from production jobs; its accuracy there has to be watched, not assumed.

These need a staging worker host, which is phase 3 and spend.

## 10. Open decisions for the owner

Each has options and a recommendation. Provider-specific sizing and cost for 10.1 and 10.3 go to the private hosting repository.

**10.1 Worker classes and autoscaling bounds (spend).** Options: (a) one class matching today's production machine size (16 GB, 2 vCPU), a fleet of at most one worker, stopped when idle and started when a job is queued for it; (b) two classes, small (about 4 GB) for hand jobs and large (16 GB) for SCIP and ultra-band repositories, each at most one; (c) (b) plus an on-demand 32 GB class for the msgraph-sized outliers. **Recommendation: (a) for phase 3.** It adds no sizing risk, because it is the size production already runs. With `hand` the default, nearly every job would fit a smaller class, but on a per-second machine that stops when idle the difference is small. Add the small class once queue-wait measurements show large jobs delaying small ones, and the 32 GB class only if an outlier is actually requested. How a stopped worker gets started is part of this decision: the platform's own start-on-request feature where it has one (no credential), or a platform API credential held by the master (a credential decision).

**10.2 Control channel.** Options: (a) WebSocket on axum; (b) gRPC bidirectional streaming via tonic. **Recommendation: (a)**, for the reasons in §4.1: no second schema definition, one small dependency family, better proxy compatibility, and nothing large on the channel.

**10.3 Artifact path and retention (spend).** Options: (a) HTTPS PUT and GET through the master, stored on the master's disk as today; (b) presigned object-storage URLs; (c) streamed over the WebSocket. Retention: keep today's rule (`TOLMAP_RETAIN_COMMITS_PER_REPO` newest commits per slug, never the newest row, 5 in production) wherever artifacts live, plus deletion of uploads for jobs that never registered after 24 h. **Recommendation: (a) now, (b) in phase 4** if the master tier grows past one node. The protocol carries URLs, so the switch needs no worker change. Retention unchanged, with the 24 h orphan sweep.

**10.4 Worker tokens and the public worker endpoint (credentials, public reachability).** Options: (a) per-worker random bearer tokens, stored hashed on the master, issued and rotated by hand with a `tolmap worker-token` helper; (b) mutual TLS with a private CA; (c) platform workload identity (OIDC tokens the worker's platform mints), verified by the master. **Recommendation: (a)**, reachable only over TLS on a separate listener, exposed publicly only if the platform has no private network between master and workers. (b) adds a CA to run for no gain at one to three workers, and (c) ties the protocol to one provider. Phases 0–2 need no token at all.

**10.5 When the sandbox moves.** Options: (a) with the first remote worker (phase 3), keeping the in-VM jail on the master until then; (b) earlier, by standing up a worker host before the protocol exists, which cannot work because the host has no way to receive jobs; (c) keep installs off in hosting until phase 3. **Recommendation: (a), which in practice is also (c) today**: production runs `TOLMAP_REFS=hand`, so neither installs nor indexers run in hosting, and the kernel-exploit risk accepted on #117 stays latent until the owner enables `scip` there. If `scip` with installs is wanted in hosting before phase 3, the accepted risk applies as ruled on #117.

**10.6 A retry bound for jobs that kill their workers (touches the no-caps ruling).** A job that repeatedly takes down its worker host (lease lost with no result) would otherwise re-queue forever, killing a machine each time. Options: (a) no bound: re-queue indefinitely; (b) a bound on lost-worker retries per class, after which the job fails `worker_crashed`, for example two per class plus one escalation. **Recommendation: (b).** It limits how many machines one job may destroy, not what may be admitted. Nothing is refused for its size or duration, and a job that finishes, however long it takes, is never affected. This is still the owner's call, because it bounds a job's outcome.
