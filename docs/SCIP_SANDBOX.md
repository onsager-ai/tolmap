# Sandboxed dependency installs and worker sizing for SCIP (#110 P1c)

**Status: §4.1's install layer is implemented (issue #110 P1c, PR #129): the npm-only install policy, nsjail as the worker's uid without user namespaces, the registry-only CONNECT proxy, the per-install self-test, the 20 min / 20 GB bound with fallback, and `coverage.references.ts.install` (`src/indexers.rs`, `docs/API.md` "Dependency installs", finding 45). Not implemented from §4.1: jailing the indexers themselves, the start-up self-test (it runs before each install instead), and a memory cgroup by default (`TOLMAP_INSTALL_MEMORY_MAX` is opt-in, since cgroup v1 on the production host is unverified). The owner's decisions on #117 are recorded there.** The design text below is unchanged from review. Owner decisions (AskUserQuestion, session `16030105`): line 4359, 2026-09-24T06:15:43Z, **"Bigger production machine"** and **"Installs in a sandbox"**; line 4376, 06:26:42Z, **"Go: P1a+P1b now, P1c design first"**. Everything under "Open decisions" is reserved to the owner: it covers hosting spend and credentials.

Claims about Fly that come from Fly's docs or staff are marked *documented*. Claims we have not observed on a Fly Machine ourselves are marked **unverified**. (Fly pricing and machine-sizing figures moved to the private hosting repo's `docs/SIZING.md` -- see §5 below.)

## Summary

- **Install as little as possible.** Finding 41 shows that only TypeScript/JavaScript monorepos gain from installs: dify's TypeScript went from indexing nothing to 3,622 cross-package pairs. Python gained 33 in-repo pairs out of 11,453 and Go gained 2, while their installs cost up to 10.5 GB and 296 s. So the design installs only JS/TS packages, from the lockfile, with lifecycle scripts and pnpmfiles off. Python and Go never install.
- **Sandbox every step that reads the checkout with a tool that could execute something.** That covers the install and the indexers. Each runs in nsjail, started by the root-owned worker. It runs as an unprivileged uid with an empty environment, sees only its job's workspace plus a read-only image, and has its own network namespace whose only exit is an allowlist proxy to `registry.npmjs.org`. A memory cgroup and a fixed-size workspace bound it. This works inside the current single Fly Machine because the service is root in its own VM, so it does not depend on unprivileged user namespaces. Those are unverified on Fly.
- **Fail soft.** If the sandbox cannot be set up, or the install fails or exceeds its limits, that language falls back to indexing without installs. Only if that also fails does it fall back to today's hand-written resolver, and `coverage` records which path ran. No job fails because of the sandbox.
- **Machine sized for SCIP indexing.** Every P0 repository fits under the recommended install policy; the largest is n8n at 8.84 GB. Python roots beyond roughly 5k files and TypeScript beyond roughly 20–25k files are expected to OOM. That is a two-point extrapolation, not a measurement. The exact machine class and its cost are recorded in the private hosting repo's `docs/SIZING.md` (§5 below), not here.
- **Later, with #97's master/worker split**, the same sandbox runs inside a separate worker Machine. That Machine has no volume holding master data and no secrets beyond its own worker token, and it stops when idle. The master drops back to a smaller class. Cost at a few hundred jobs a month is also in `docs/SIZING.md`.

## 1. What an install buys

From `docs/FINDINGS.md` finding 41 (standard 4-vCPU/16 GB runners; times vary between runs):

| repo, language | without install | with install | install cost |
|---|---|---|---|
| dify TS | **fails**: every tsconfig `extends` an uninstalled workspace package | 4,350/4,358 files, recall 1.000, 3,622 cross-package pairs (hand-written: 45); 251.7 s, 5.67 GB | pnpm `--frozen-lockfile --ignore-scripts`, +23.0 s |
| n8n TS | recall 0.514, 85 cross-package pairs vs 11,690 hand-written; 314.5 s, 8.84 GB | not measured | — |
| dify Py (`api/`) | 11,453 in-repo pairs; 236.1 s, 7.17 GB | 11,486 pairs (+33); 285.5 s, **10.53 GB** | `uv sync --frozen`, +24.5 s |
| prometheus Go | 5,436 pairs; 3.0 s, 0.80 GB | 5,438 pairs (+2); 299.2 s, 2.98 GB | `go mod download`, +25.9 s |
| django Py, vue TS, prometheus UI TS | near-superset of hand-written without installs | not needed | — |

Installs change in-repo edges for TS monorepos only. For Python and Go they add external references, which tolmap drops because only in-repo definitions count, and they cost time and memory. **The design therefore limits installs to JS/TS.** That shrinks the attack surface to one ecosystem and one registry.

## 2. Threat model

tolmap indexes arbitrary public repositories on request, and the endpoint is public (`docs/ARCHITECTURE.md`, "What a public endpoint forces"). Assume an attacker controls every byte of the repository, including its lockfile, `.npmrc`, `pnpm-workspace.yaml`, `pyproject.toml`, `go.mod` and tsconfigs, and can submit it as often as the rate limits allow.

### What is exposed today

The current code and deployment matter here, because the sandbox has to close these gaps:

- `src/service/jobs.rs` `process_worker_exe` spawns `tolmap worker` with the **parent's full environment**: no `env_clear`. It runs **as the same user**, which is root, since the `Dockerfile` runtime stage has no `USER`. The child therefore holds `OPENROUTER_API_KEY` (`src/naming.rs`) and anything else set as a Fly secret.
- The worker spec gives the child `cache_dir` and `output_dir` on the `/data` volume. That volume also holds `/data/tolmap.sqlite3`, every stored map, the names cache, and **the other repositories' clones** (the production deploy config's `[mounts]`, `TOLMAP_DB_PATH`, `TOLMAP_CACHE_DIR`).
- The machine has unrestricted outbound network. That includes Fly's private 6PN network: *documented*, "every machine in an organization can reach every other app's machines by default", over `fdaa::/16`, with DNS at `fdaa::3`. That puts every other app in org `core-digital` within reach, along with `_api.internal:4280` (the Machines API, which needs a token).
- A root process in a Fly Machine can use the `/.fly/api` Unix socket. *Documented*: it "exports a subset of the Fly Machines API to privileged processes in the Machine", including minting OIDC tokens for `org:app:machine`.
- One machine runs one job at a time (`TOLMAP_MAX_CONCURRENT_JOBS = "1"`) with no time or size caps (owner decision, #97). A job that exhausts memory or disk takes down the API, the store writes and the queue with it.

### Where repository-controlled code can run

| step | repo-controlled execution | notes |
|---|---|---|
| `git clone --filter=blob:none` | none by default | Hooks are not cloned, submodules are not recursed, no LFS. Unchanged from today. |
| tolmap parse/resolve/build | none | tree-sitter plus Rust, in-process. It handles untrusted *data*, not code. |
| npm / pnpm / yarn install | **lifecycle scripts** (`preinstall`, `install`, `postinstall`, `prepare`) of the repo's packages and every dependency; **`.pnpmfile.cjs`** hooks (pnpm executes it from the repo root even with `--ignore-scripts`); **yarn berry `yarnPath`** (executes a repo-committed JS file as yarn itself); `packageManager` makes corepack download a version the repo chose | Lockfile `resolution.tarball` URLs and `.npmrc` `registry=` can point anywhere, which is exfiltration and SSRF. |
| pip / uv install | **sdist builds** run `setup.py` or PEP 517 backends. Wheels run nothing at install time, but a wheel can drop a **`.pth` file** that runs when any later Python process starts in that environment. That includes scip-python, which asks the environment's `pip` for packages (finding 41's install variant). `uv sync` also builds the project itself unless told not to. | `[tool.uv]` / `requirements.txt` can set index URLs. |
| `go mod download` | no build, no `go generate` | A `toolchain` directive with `GOTOOLCHAIN=auto` downloads a Go toolchain. `GOPROXY=...,direct` fetches over VCS from arbitrary hosts. |
| scip-typescript | none known. It runs the TS compiler API and does not load tsconfig `plugins` or transformers (**unverified**). | It parses attacker input: DoS by memory or time. |
| scip-python (Pyright) | runs a Python interpreter to discover search paths. Repo config (`pyrightconfig.json`, `[tool.pyright]` `venvPath`/`venv`) chooses which environment. Whether that can make it execute a repo-committed file is **unverified**. | 5–10.5 GB peak RSS. |
| scip-go | runs `go list`/`go/packages`. With cgo files this can invoke the C toolchain and `#cgo pkg-config:` directives (**unverified** for scip-go's exact load mode). | |

### Assets and what must hold

1. **Service filesystem.** A job must not read or write SQLite, maps, the names cache, or other clones.
2. **Credentials.** A job must not see `OPENROUTER_API_KEY`, any Fly secret, `/.fly/api`, or (later) the worker's own master token.
3. **Network.** A job must not reach 6PN (`fdaa::/16`, `*.internal`), link-local or metadata addresses (`169.254.0.0/16`), host loopback services (the API on `:8787`), or arbitrary internet hosts. The last would allow exfiltration, SSRF, abuse traffic from our IP, and fetching second-stage payloads.
4. **CPU, memory and disk.** A job's excess must kill that job's sandboxed step, not the service or the next job.

The residual risk after namespaces is a **Linux kernel exploit**: namespaces share the VM's kernel, so a kernel bug turns sandbox code into root in the VM. On today's single machine that means the whole service. That is why the later step moves jobs to a separate VM (§4.2).

## 3. Options

### 3.1 Reduce execution

| mechanism | stops | does not stop |
|---|---|---|
| `pnpm install --frozen-lockfile --ignore-scripts --ignore-pnpmfile`; `npm ci --ignore-scripts`; yarn classic `--frozen-lockfile --ignore-scripts` | lifecycle scripts and pnpmfile hooks, which are the npm-ecosystem code execution path; version drift from the lockfile | yarn berry `yarnPath` (so: **no install for yarn berry**, fall back). The lockfile is still attacker-written, so tarball URLs and registries need the egress allowlist. Package-manager bugs. |
| package manager from the image, pinned; `COREPACK_ENABLE_STRICT=0`, corepack never downloads | a repo choosing which package-manager build runs | — |
| `pip install --only-binary=:all:` / `uv sync --frozen --no-build --no-install-workspace` | sdist `setup.py` and PEP 517 builds, including building the project itself | `.pth` execution at later interpreter start-up. Some dependency sets have no wheel for some package, so the install fails. |
| **no Python install at all** (recommended) | all of the above | nothing lost in-repo (finding 41: +33 pairs of 11,453) |
| `go mod download` with `GOPROXY=https://proxy.golang.org` (no `direct`), `GOTOOLCHAIN=local` | VCS fetches from arbitrary hosts, toolchain download | cgo tooling at index time |
| **no Go download** (`GOPROXY=off`, as in P0; recommended) | all module fetching | nothing lost in-repo (finding 41: +2 pairs) |

Reducing execution is necessary but not sufficient. It removes the documented execution paths, but the sandbox still has to cover the ones we have not found: package-manager config quirks, indexer behaviour, and parser bugs.

### 3.2 Isolate

"Works on Fly" means inside a Fly Machine, which is a Firecracker microVM where the service is root. *Documented*: staff state that Machines are "full virtual machines each running an independent Linux kernel, in which you get root". Staff also state (2023) that "we currently don't support nested virtualisation".

| option | mechanics | stops | does not stop | works on a Fly Machine? |
|---|---|---|---|---|
| **nsjail** (recommended in-VM layer) | Run by the root worker. New user, mount, pid, net, ipc and uts namespaces. `--user/--group` map the jail's uid to a real unprivileged uid, which root can do without unprivileged userns. Tmpfs root with `-R` read-only binds of `/usr` and the tool directories, `-B` for the job workspace. cgroup v1 or v2 memory and pids limits, `-t` wall time, optional seccomp policy. The environment is empty unless passed with `-E`. | filesystem, credential and network access (§2 assets 1–3); memory and pids; runaway time | kernel exploits (shared kernel) | **Likely, unverified.** It needs root, which we have, not unprivileged userns. Fly Machines run hybrid cgroup v1/v2. Creating a v1 cgroup as root worked in a Fly community thread (Nov 2025), with the memory controller mounted as v1 (**unverified by us**). nsjail's default rlimits must be lifted: address space 4 GiB, file size 1 MiB, 32 files, 600 s CPU. V8 and Pyright need far more address space than they use, so the cgroup, not `RLIMIT_AS`, is the memory control. |
| bubblewrap | Same namespace model, built for unprivileged use. `--unshare-all` gives a fresh net namespace with loopback. `--ro-bind` / `--bind`. No cgroups of its own. | as nsjail, minus resource limits | kernel exploits. Run as root, `--uid` needs `--unshare-user`, which maps the jail uid back to real root outside. Dropping to a real unprivileged uid first requires unprivileged user namespaces. | **Unverified.** Whether Fly's kernel allows unprivileged user namespaces is not documented anywhere we found. Resource limits would need a separate cgroup step. Viable if a start-up probe (`bwrap --unshare-all true` as the unprivileged user) succeeds. |
| gVisor (`runsc`) | A user-space kernel intercepts syscalls. The systrap platform needs no KVM. | kernel attack surface as well as the above | — | **Unverified, with a reported failure.** A Fly community thread (Nov 2025) shows `runsc` failing at cgroup setup on Fly's hybrid cgroups ("cannot set up cgroup for root"), with no resolution. `--ignore-cgroups` might get past it, but that would drop resource limits (untested). Heavier to ship. A candidate for later, not first. |
| Firecracker microVM inside the Machine | nested microVM | everything, including kernel exploits | — | **No**: no nested virtualisation or `/dev/kvm` on Fly (staff, 2023). |
| rootless podman / docker | OCI containers | as nsjail | kernel exploits | **Poor fit.** A 2025 thread shows podman limits failing on Fly's hybrid cgroups ("Using cgroups-v1 which is deprecated", CPU limits not applied). Needs a daemon or userns plumbing for no gain over nsjail. |
| **separate throwaway Fly Machine per job** (Machines API) | The master calls `POST /v1/apps/{app}/machines` (internal `http://_api.internal:4280`, *documented*) with `auto_destroy: true` and `restart.policy: "no"`. The worker runs one job and exits. | kernel exploits cannot reach the master's VM, volume or secrets. A job's OOM or disk exhaustion is its own VM's. | Inside the job VM, install code can still reach that VM's worker token, `/.fly/api` and 6PN, so the in-VM nsjail layer still applies. | **Yes, documented, not tested by us.** Needs a Machines API token held by the master: a credential, reserved to the owner. A deploy token (`fly tokens deploy`) is scoped to one app. Constraints (*documented*): rootfs writes are limited to **8 MiB/s and 2000 IOPS**, with an ~8 GB rootfs cap reported in the community, so a workspace needs a volume (one volume per Machine). Large Machines "might need to retry" at capacity. A new shared-CPU Machine starts with a **5 s** CPU burst balance, so per-job Machines must be `performance`. Boot plus image pull latency for a multi-GB toolchain image is **unmeasured**. |
| standing worker Machine, stopped when idle | as above, but one pre-created Machine and volume, started per job. Started by the Machines API, or by a Flycast request with `auto_start_machines` (*documented* for private Flycast services), which needs no API token. | as the per-job Machine, except jobs share one VM over time | cross-job persistence on that VM if the sandbox is escaped | **Yes, documented, not tested by us.** |
| Fly Sprites | Fly's hosted sandbox VMs with an allowlist network policy (DNS-based) and checkpoints | as a separate VM | — | Separate product and plan. *Documented*: default memory "up to 8GB", 16 GB on request to support (June 2026). That is too small for n8n (8.84 GB) and tight for dify Python (7.2–7.9 GB) without the request. |

### 3.3 Constrain

| control | mechanism | Fly |
|---|---|---|
| memory | cgroup memory limit on the jail, set **below machine RAM**, so the kernel OOM-kills the indexer and not `tolmap serve`. Set `NODE_OPTIONS=--max-old-space-size` inside it for scip-typescript. | cgroup v1 memory controller present (community report, **unverified**) |
| pids | cgroup `pids.max` (fork bombs) | same |
| time | nsjail `-t` on the install step only (see §4.3 on the no-caps ruling) | yes |
| disk | fixed-size workspace per job: a sparse ext4 image, loop-mounted `nodev,nosuid`, on `/data`; ENOSPC ends the job at its budget. Alternative: a `tmpfs` with `size=`, which is charged to the job's memory cgroup (costs RAM). Fallback: the worker polls `du` and kills. | loop devices in a Fly Machine **unverified**; tmpfs certainly works |
| credentials | the jail gets an explicit environment (`HOME`, `PATH`, proxy variables, tool settings) and nothing else. `/data`, `/.fly` and other clones are never mounted. The worker child itself should also be spawned with `env_clear()` plus an allowlist, and `OPENROUTER_API_KEY` passed only to the naming step. | yes |
| identity | a dedicated uid/gid per job slot (e.g. 20000+slot), owning only that job's workspace | yes |

### 3.4 Egress

The jail gets its own network namespace with only loopback. Its one exit is a Unix socket bind-mounted from the worker's side, which `socat` inside the jail exposes as `127.0.0.1:3128`, with `HTTPS_PROXY` pointing there. npm, pnpm, yarn, pip, uv and go all honour `HTTPS_PROXY`. The proxy runs outside the jail, in the trusted worker, and:

- accepts only `CONNECT <host>:443`, where `<host>` exactly matches the allowlist: no wildcards, no suffix match, no IP literals;
- resolves the name itself and refuses any address that is not globally routable. That covers `169.254.0.0/16` (metadata), RFC 1918, loopback, and `fc00::/7`, which includes Fly's `fdaa::/16` 6PN. It also refuses `*.internal`;
- logs host, bytes and duration per connection, which feeds `coverage` and abuse review.

Since TLS runs end to end through CONNECT, the proxy never holds certificates. Allowlist per policy:

| ecosystem | hosts | needed under the recommended policy |
|---|---|---|
| npm / pnpm / yarn classic | `registry.npmjs.org` (`registry.yarnpkg.com` is an alias yarn classic uses by default) | **yes** |
| pip / uv | `pypi.org`, `files.pythonhosted.org` | no (Python not installed) |
| Go | `proxy.golang.org`, `sum.golang.org` | no (Go not downloaded) |
| git dependencies | `codeload.github.com`, `github.com` | **no**. Allowing them opens an exfiltration channel through attacker-chosen repository URLs. A lockfile that needs them fails its install and falls back. |

Even the registry is a covert channel: data can be encoded in request paths. The allowlist bounds where traffic goes, and with scripts disabled no repository code runs to use that channel. The proxy exists for the execution paths we have not found.

The proxy could be tinyproxy or squid with a filter, but a small tokio CONNECT proxy inside `tolmap` (about 150 lines) keeps the policy in one reviewed place and needs no extra daemon.

## 4. Recommendation

### 4.1 First: on the current single-machine deployment

1. **Policy (code only).** Install only for JS/TS repositories where a no-install index is known to lose edges: a workspace manifest (`pnpm-workspace.yaml`, `package.json` `workspaces`) or a tsconfig `extends` into a workspace package. Also require a lockfile the image's package manager can honour frozen: pnpm or npm, or yarn classic. Never yarn berry. Never install Python or Go (`GOPROXY=off`, `GOTOOLCHAIN=local`, as in P0).
2. **In-VM sandbox (nsjail).** The worker child (still root, as now) runs each untrusted step through nsjail:
   - `install` (new stage) and each `scip-*` indexer run under the job's uid;
   - the mounts are: tmpfs root; read-only `/usr`, `/etc` (image copy, no secrets) and the toolchain prefix; read-write the job workspace only;
   - the environment is empty except explicit variables;
   - networking is a new net namespace. The install step gets the proxy bridge with the npm allowlist. Indexers get **no** egress at all: they need none.
   - The memory cgroup is set to machine RAM minus a reserve for `tolmap serve` (about 2 GB on a 16 GB machine); pids.max is 4096.
   - Each job gets its own workspace image under `/data/jobs/<id>`, deleted when the job ends. The clone is copied or `git worktree`-checked-out into it, so the shared clone cache is never writable from a jail.
3. **Start-up self-test, fail closed.** At service start and before each install, run a trivial jail that checks four things: uid ≠ 0, `/data` not visible, direct egress fails, and proxied egress to a non-allowlisted host is refused. If the self-test fails, installs and SCIP indexers are disabled for the process's life. Jobs then run on the hand-written path, and the job's `coverage` says why.
4. **Worker hygiene.** Spawn `tolmap worker` with `env_clear()` and an allowlist of variables. `OPENROUTER_API_KEY` goes only to the naming step, which runs outside the jail and never touches untrusted execution.
5. **Machine:** see §5.

This layer protects the service's files, credentials and network from install scripts, pnpmfiles, `.pth` files and whatever we missed. It does not protect against a kernel exploit. Given that no repository code is meant to run at all once §3.1 is applied, the owner may judge that acceptable for the MVP; that call is open decision 1.

### 4.2 Later: with #97's master/worker split

#97's production design already says workers dial out to the master over one authenticated channel and never touch the database. The sandbox slots in unchanged:

- **Worker VM.** A separate Fly app (e.g. `tolmap-worker`), ideally on a **custom private network** (*documented* feature) so its 6PN cannot reach other `core-digital` apps. It reaches the master only through the master's public `wss://…/workers` endpoint. It has no `/data`, no `OPENROUTER_API_KEY` (naming moves to the master or goes through it), and only its own worker token.
- **In-VM nsjail stays.** It keeps install code away from the worker token, `/.fly/api` and the VM's network.
- **Lifecycle.** Start with one standing worker Machine plus volume, stopped when idle and started per job. Flycast autostart needs no Machines API token; the Machines API does need one, which is open decision 4. Move to a throwaway Machine per job (`auto_destroy`) when concurrency or isolation between consecutive jobs matters more than start-up latency. Either way the master can then return to `shared-cpu-1x`/1 GB, because the heavy work leaves it.
- **Routing.** #97's worker `hello` already advertises its memory class, so jobs predicted over 16 GB can go to a larger class when one exists.

### 4.3 Failure semantics and the no-caps ruling

The owner ruled "no file-count, clone-size, history-depth or wall-time admission caps" (#97, `docs/ARCHITECTURE.md` "Capacity policy"). The sandbox's limits are not admission caps: they never reject or fail a job. On any sandbox outcome other than success, the language falls back:

1. **Install fails, times out or hits its disk budget.** The indexer runs without installs.
2. **Indexer OOM-killed or fails.** That language uses the hand-written resolver.

`coverage` records which path each language took. Whether the install step gets a wall-time bound at all, and how large, is open decision 3.

## 5. Machine sizing and cost

Fly prices, machine-class options and the billing-shape comparison that
used to live here moved to the private hosting repo (`tolmap-infra`'s
`docs/SIZING.md`) 2026-09-24, scope "hosting config only" (owner decision,
session 16030105) -- they describe this deployment's hosting spend, not
the open-source service. What stays here is the design: install as little
as possible (§1), sandbox what does run (§§2–4), and fail soft rather than
reject a job (§4.3). The current machine is sized against the same
per-repository peak-RSS measurements (finding 41) that motivated this
document; the exact class and its cost are recorded privately because they
are a hosting decision, not a statement about what the software requires.

## 6. What to verify before the code merges

This change ran no experiments. A CI probe of the mechanisms was prepared for a standard runner and not run: pushing it was denied by this session's permission settings. Before P1c code merges, run the following on a standard runner and then on a Fly Machine in staging:

- nsjail as root: jail uid is not 0; empty environment; `/data` and `/.fly` absent; read-only root; ENOSPC at the workspace budget; OOM-kill at the cgroup limit (cgroup **v1** memory on Fly); wall-time kill.
- egress: direct HTTPS fails, and so do an IP literal, `127.0.0.1:8787` on the host and `[fdaa::3]`. The proxy allows `registry.npmjs.org` and refuses `example.com`, `registry.npmjs.org.example.com`, `169.254.169.254` and plain HTTP.
- installs through the jail: vue and dify with pnpm `--ignore-scripts --ignore-pnpmfile`, with time, peak RSS, `node_modules` size, and which hosts the proxy saw.
- on Fly only: whether unprivileged user namespaces are enabled (decides whether bubblewrap is an alternative); loop devices; whether `/.fly/api` is root-only (*documented* as such in a community answer); cold-start time for shapes B and C.

## 7. Open decisions for the owner

1. **Sandbox layer for now.** (a) In-VM nsjail on the single Machine (§4.1), accepting that a kernel exploit reaches the whole service until #97. (b) Ship installs only together with a separate worker VM (§4.2), which delays TS monorepo installs until #97's worker channel exists. (c) Add gVisor on top of (a), after the Fly cgroup issue is tested.
2. **Machine size and billing shape.** Options and their cost are in the private hosting repo's `docs/SIZING.md` (moved there 2026-09-24, scope "hosting config only"): performance-2x/16 GB always-on, the same Machine auto-stopping, shared-cpu-8x/16 GB (throttled), or performance-4x/32 GB for the large Python SDK monorepos. The change lands only on a release the owner deploys via that repo's `fly-deploy.yml` (#110).
3. **Install limits under the no-caps ruling.** Whether the install step may have a wall-time bound and a disk budget that fall back to no-install indexing rather than failing the job (§4.3), and their values (proposed: 20 minutes, 20 GB).
4. **Egress policy.** `registry.npmjs.org` only (proposed), or also PyPI/Go proxies (only if Python or Go installs are ever enabled), or also GitHub for git dependencies (opens an exfiltration channel). Separately, for the later worker VM: a Machines API token held by the master (credential) versus Flycast autostart (no token); and a custom private network for the worker app.

## Sources

Fetched 2026-09-24. Fly pricing, CPU-quota and machine-sizing sources moved
to the private hosting repo's `docs/SIZING.md` along with the figures they
support.

- Rootfs 8 MiB/s / 2000 IOPS; volume limits per preset; one volume per Machine: https://docs.fly.io/volumes/overview/
- Rootfs 8 GB uncompressed limit (community): https://community.fly.io/t/rootfs-8gb-uncompressed-limit-exceeded-error/18202
- Autostop/autostart, and apps that stop themselves: https://docs.fly.io/launch/autostop-autostart/
- Suspend not recommended above 2 GB: https://docs.fly.io/reference/suspend-resume/
- Flycast autostart for private apps: https://fly.io/docs/blueprints/autostart-internal-apps/
- Machines API resource (`auto_destroy`, `restart.policy`, guest): https://docs.fly.io/machines/api/machines-resource/
- Machines API internal endpoint `_api.internal:4280`, deploy tokens: https://docs.fly.io/machines/api/working-with-machines-api/
- Private networking (6PN default reachability, `fdaa::3`, custom private networks): https://docs.fly.io/networking/private-networking/
- `/.fly/api` exports Machines API subset to privileged processes: https://fly.io/blog/oidc-cloud-roles/ and https://docs.fly.io/security/openid-connect/
- `/.fly/api` non-root access is plain Unix permissions (community): https://community.fly.io/t/non-root-access-to-fly-api-oidc-socket-expected-or-is-there-a-workaround/28662
- Root in a full VM (staff): https://community.fly.io/t/why-are-fly-machines-running-as-firecracker-microvms-configured-with-privileged-true/26380
- No nested virtualisation (staff, 2023): https://community.fly.io/t/nested-virtualization-on-fly-io/11778
- Hybrid cgroups; gVisor and podman limit failures (Nov 2025): https://community.fly.io/t/podman-and-gvisor/26529
- Sprites: default memory "up to 8GB", 16 GB on request: https://community.fly.io/t/16gb-ram-advertised-for-sprites-but-not-actually-available/28123 (pricing moved to `docs/SIZING.md`)
