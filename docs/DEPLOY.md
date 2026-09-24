# Deployment

Two targets, picked for opposite properties rather than out of indecision.

| | production | staging |
|---|---|---|
| platform | Fly.io, app `tolmap` (org core-digital) | Railway |
| config | `fly.toml` | `railway.json` + `deploy/railway.staging.env` |
| deploys on | a pushed `v*` tag, or a manual dispatch | every commit on `main`, automatically, unfiltered |
| shape | one machine, `performance-2x` / 16 GB, stops when idle, volume at `/data` | one replica, volume at `/data` |
| pricing model | per-second while running (`$0.176`/h active) + about `$2`/month rootfs and volume | metered |

Sizing and the auto-stop shape changed 2026-09-24 (issue #110, owner decision via AskUserQuestion, session `16030105`, transcript line 4551, `2026-09-24T06:51:34Z`: "performance-2x / 16 GB, stopping when idle"). **This takes effect only on the owner's next `v*` tag deploy** (see "Cutting a production release" below) — it is not live on the current machine until that deploy happens. Price and its per-second/per-GB breakdown: [docs.fly.io/about/pricing](https://docs.fly.io/about/pricing/), fetched 2026-09-24.

## Why both

Indexing is bursty and CPU-heavy, and a running job lives entirely in the serving process's memory — the `JobRegistry` and the SSE `watch::Sender` in `src/service/jobs.rs`. (There is also no wall-clock cap on a job's own runtime any more; #97/#105 removed the file-count/clone-size/history-depth/job-time admission caps that used to bound this — see `docs/API.md`.) None of that state is persisted anywhere a stopped-then-restarted machine could recover it from.

Until 2026-09-24 this meant production ran one machine 24×7 (`auto_stop_machines = false`, `min_machines_running = 1`): the only way to guarantee a job in flight was never interrupted by the platform itself. The owner has since explicitly chosen the opposite trade for cost — `auto_stop_machines = "stop"`, `min_machines_running = 0` — accepting that a job nobody is actively watching (no open poll or SSE connection) can now be stopped mid-run by Fly's own idle-machine policy, with no failure event, no SSE frame, and no `error`/`error_code` written to the job's row; the client is left polling a job id that never moves again. `fly.toml`'s `[http_service]` comment has the full account of what was checked — Fly's proxy-driven idle detection, and why `kill_signal`/`kill_timeout` don't close the gap without a code change this PR does not make — and the honest conclusion is **not mitigated**. `TOLMAP_MAX_CONCURRENT_JOBS = "1"` bounds the blast radius to one job per incident, not a service-wide outage.

Staging inverts every one of those facts. It is idle almost all the time, nobody is watching it, and a dropped job costs nothing. Metered pricing is the right shape for that, and Railway's native push-to-deploy removes the step a human otherwise has to remember. Fly has no equivalent — `flyctl deploy` from CI is the whole mechanism — so splitting the two puts the automation where the risk is lowest. Fly's machine sizing and auto-stop settings live in `fly.toml`-specific sections (`[[vm]]`, `[http_service]`), not `[env]` keys, so they have no Railway counterpart and carry no parity requirement with staging — `scripts/check_deploy_env_parity.py` only compares `[env]` key sets, which this change does not touch (`12 shared keys` before and after). Railway staging keeps its own metered per-replica pricing regardless of production's machine shape.

## What staging does not reproduce

Say this out loud before trusting a staging result:

- **Process lifecycle.** Railway restarts and re-provisions containers more readily than a Fly machine. Because job state is in-memory, staging will lose in-flight index jobs in situations where production does not — and, as of 2026-09-24, production can now lose one too, when nobody is watching it closely enough to keep Fly's proxy from treating the machine as idle (see "Why both" above). Read job-state loss as a standing reminder that state is unpersisted, not as a staging-only quirk.
- **Machine size.** Production is `performance-2x` / 16 GB as of issue #110 (previously `shared-cpu-1x` / 1 GB). This is no longer an unmeasured guess: `docs/SCIP_SANDBOX.md` (#117, design for owner review) §5.1 has real peak-RSS figures per repository from finding 41's runs, and §5.4 sizes the machine against them. It is still not a precise ceiling — the doc itself calls the OOM boundary (roughly 5k files for a Python root, 20–25k for TypeScript) "a two-point extrapolation, not a measurement." If staging is provisioned differently, an OOM on one side still says nothing about the other.
- **Everything else is held identical on purpose.** The limits in `deploy/railway.staging.env` match `fly.toml`'s `[env]` value for value, so a repository that is accepted or rejected here is accepted or rejected there. `scripts/check_deploy_env_parity.py` runs in CI and fails the build when the two files stop describing the same set of knobs.

## First-time setup

Both steps need credentials and are the principal's to run; nothing in this repo mints or stores a token.

**Fly deploy token** — once, so the workflow can deploy:

```sh
fly tokens create deploy -a tolmap        # copy the output, including the FlyV1 prefix
gh secret set FLY_API_TOKEN --repo onsager-ai/tolmap
```

Optionally create a `production` environment in the repo's settings with yourself as a required reviewer; `.github/workflows/fly-deploy.yml` already names it, so that alone turns production deploys into an approval gate.

**Railway staging service** — once:

1. New project → Deploy from GitHub repo → `onsager-ai/tolmap`. In the service's **Settings → Source**, set the branch to `main` and leave **Automatic Deploys** enabled — that is the whole mechanism, and it is Railway-side state, not something `railway.json` can express. `railway.json` covers only what it can: Dockerfile build, healthcheck on `/api/healthz`, restart policy.

   Every commit on `main` deploys, including docs-only ones. That is deliberate — a staging environment that skips commits is a staging environment you cannot reason about. Railway's **Watch Paths** could filter them and should be left empty.

   Leave **Wait for CI** off unless you want staging gated on the Rust gate finishing (~minutes). Off means staging reflects `main` immediately and can briefly run a commit CI later calls red; that is the correct trade for a box whose job is to be looked at, but it is a choice, so make it knowingly.
2. Add a volume mounted at `/data` (5 GB is the initial staging size). The clone cache evicts older clones above 1 GiB via `TOLMAP_CLONE_CACHE_BYTES`, but an active clone can exceed the budget and fill this volume. Do this **before** the first successful deploy, or the healthcheck passes against a store that a redeploy then throws away.
3. Apply the variables: `railway variables --set-from-file deploy/railway.staging.env`, or paste that file into the dashboard's raw editor.
4. Note that the generated `*.up.railway.app` URL is public. This service clones and indexes arbitrary public repositories on request; the rate limits in the env file are the only thing in front of it. If staging should not be an open compute endpoint, put Railway's access protection in front of it at this point.

## Cutting a production release

```sh
git tag -a v1.1.0 -m "what changed"
git push origin v1.1.0
```

That fires `.github/workflows/fly-deploy.yml`, which builds on Fly's remote builder and waits for the healthcheck. `flyctl releases -a tolmap` at the end of the run reports what is actually live.

**A deploy replaces the machine, and any index job running at that moment dies with it** — no failure event, no SSE frame, no `error_code` in the job's row; a client polling `GET /api/jobs/{id}` just waits forever. Blue-green would not help: the `/data` volume pins the app to a single machine. Until job state is persisted, deploy when the box is quiet.

A deploy with no tag to cut — a rollback, or shipping a fix that is already on `main` — is the same workflow run by hand: Actions → *fly deploy (production)* → Run workflow.

## Changing a limit

Change it in `fly.toml` and in `deploy/railway.staging.env`, in the same PR, with the arithmetic in `fly.toml`'s comment. CI's `deploy env parity` job fails the build if only one side moves. A deliberate one-sided knob goes in `EXPECTED_ONLY_IN_*` in `scripts/check_deploy_env_parity.py` with the reason written next to it.


`TOLMAP_PRUNE_VARIANT` accepts `absolute`, `percentile`, `node-relative`, or `pre-rescale` and defaults to `node-relative` when unset or invalid. Both staging and production set the owner-approved default explicitly so a deployment cannot retain the former absolute default through stale configuration.
