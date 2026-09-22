# Deployment

Two targets, picked for opposite properties rather than out of indecision.

| | production | staging |
|---|---|---|
| platform | Fly.io, app `tolmap` (org core-digital) | Railway |
| config | `fly.toml` | `railway.json` + `deploy/railway.staging.env` |
| deploys on | a pushed `v*` tag, or a manual dispatch | every commit on `main`, automatically, unfiltered |
| shape | one always-on machine, 1 GB, volume at `/data` | one replica, volume at `/data` |
| pricing model | fixed provisioned machine | metered |

## Why both

Indexing is bursty and CPU-heavy, and a running job lives entirely in the serving process's memory — the `JobRegistry`, the SSE `watch::Sender` and the wall-clock budget in `src/service/jobs.rs`. Production therefore needs a machine that is never suspended between requests, which is what `auto_stop_machines = false` and `min_machines_running = 1` buy, and a fixed provisioned shape beats metering every idle vCPU-second for a box that must stay up regardless.

Staging inverts every one of those facts. It is idle almost all the time, nobody is watching it, and a dropped job costs nothing. Metered pricing is the right shape for that, and Railway's native push-to-deploy removes the step a human otherwise has to remember. Fly has no equivalent — `flyctl deploy` from CI is the whole mechanism — so splitting the two puts the automation where the risk is lowest.

## What staging does not reproduce

Say this out loud before trusting a staging result:

- **Process lifecycle.** Railway restarts and re-provisions containers more readily than a pinned always-on Fly machine. Because job state is in-memory, staging will lose in-flight index jobs in situations where production does not. Read that as a standing reminder that job state is unpersisted, not as a staging-only quirk.
- **Machine size.** Production is `shared-cpu-1x` / 1 GB, a figure that was never measured against a real peak-RSS number for a django-sized index (see the `[[vm]]` comment in `fly.toml`). If staging is provisioned differently, an OOM on one side says nothing about the other.
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
2. Add a volume mounted at `/data` (5 GB is ample — the clone cache is capped at 1 GiB by `TOLMAP_MAX_CLONE_BYTES`, and maps are small). Do this **before** the first successful deploy, or the healthcheck passes against a store that a redeploy then throws away.
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

`TOLMAP_TERRAIN` is a feature setting, not a deployment limit. It accepts `false`, `auto`, or `true` and defaults to `false` when unset or invalid. By the owner's 2026-09-22 decision, staging sets it to `auto`, which enables terrain only above 2,000 mapped source files. Production deliberately leaves it unset until the owner has looked at staging. This one-sided setting is recorded in `scripts/check_deploy_env_parity.py`.

`TOLMAP_PRUNE_VARIANT` accepts `absolute`, `percentile`, `node-relative`, or `pre-rescale` and defaults to `node-relative` when unset or invalid. Both staging and production set the owner-approved default explicitly so a deployment cannot retain the former absolute default through stale configuration.
