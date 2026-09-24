# Fly.io deployment (2026-09-20): one image, one always-on machine, serving
# both the API and the built web bundle from a single origin -- see
# docs/API.md's TOLMAP_STATIC_DIR section and fly.toml.
#
# Five stages. The first two exist because of the one real native
# dependency this crate has (docs/ARCHITECTURE.md's "The one real risk:
# Leiden"): build.rs compiles native/leiden_bridge.cpp against
# libleidenalg + igraph and bakes `-Wl,-rpath,<LEIDEN_PREFIX>/lib` into the
# binary, so the runtime stage must carry those two shared libraries at the
# *exact* path the rust stage built against -- not just "somewhere on
# LD_LIBRARY_PATH". Get that path wrong and the binary does not start, and
# fails at dynamic-link time with no useful message (this file's own
# comments, not a guess: see build.rs).
#
# A fifth stage, `scip-tools` (issue #110 P1b), installs the pinned SCIP
# indexers -- scip-typescript, scip-python and scip-go -- plus the Node, Go
# and Python runtimes they need at index time, not just at their own build
# time. It is independent of the other four (no dependency on the Rust or
# web build) and is copied into `runtime` the same way `native`'s libs are.
# See docs/DEPLOY.md's "SCIP indexers" section for the env vars P1a's ingest
# reads to find these binaries, and docs/FINDINGS.md finding 41 for the
# indexer versions and what they were measured against.
#
# Image tags are pinned to a specific version everywhere, not `latest` --
# `docker.io/library/rust` and `docker.io/library/node`'s registry tag
# lists were checked (2026-09-20) to confirm the exact tags below exist.
# The SCIP indexer versions and the Go/Node download checksums below were
# checked the same way on 2026-09-24 (issue #110 P1b): scip-typescript
# 0.4.0, scip-python 0.6.6 and scip-go v0.2.7 are the same pins
# `.github/workflows/scip-spike.yml` measured for finding 41, and Go
# 1.26.8 matches that workflow's `go-version: "1.26.x"`.
#
# NOTE (2026-09-20, written by the change that added this file): this
# Dockerfile could not be built or smoke-tested here -- `docker` is not
# installed on this machine (`which docker` finds nothing) and installing
# it is out of scope for a change that must not deploy anything. Every
# individual piece (scripts/install-leiden.sh, the cc/rustc-link-arg rpath
# build.rs emits, the exact shared-library dependency set) was verified
# some other way instead -- see the PR body's verification table for what
# that was for each stage. Build and smoke-test this before the first real
# deploy: `docker build -t tolmap . && docker run --rm tolmap /app/tolmap
# --help` and `docker run --rm --entrypoint ldd tolmap /app/tolmap`
# (expect no "not found" lines).
#
# UPDATE (2026-09-24, issue #110 P1b): same constraint, same reason -- this
# change adds the `scip-tools` stage below without building or running it
# on the maintainer's machine (CLAUDE.md: no local docker build, indexers or
# analysis; the laptop's cooling is broken). `.github/workflows/
# scip-image-build.yml` builds this Dockerfile and smoke-tests the three
# indexers inside the built image on a standard GitHub-hosted runner
# instead -- see that workflow and the PR body for the image-size and
# build-time numbers it produced.

# ---------------------------------------------------------------------------
# Stage 1: native -- build libleidenalg + igraph from source with cmake.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS native

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        cmake \
        curl \
        ca-certificates \
        tar \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY scripts/install-leiden.sh scripts/install-leiden.sh
# Downloads igraph 1.0.0 and libleidenalg 0.12.0 from GitHub releases and
# installs both, -DBUILD_SHARED_LIBS=ON, to /opt/leiden -- this stage is
# the one that needs network access (it has it; the other three stages
# touch nothing but the build context and, for `web`, the pnpm registry).
RUN bash scripts/install-leiden.sh /opt/leiden

# ---------------------------------------------------------------------------
# Stage 2: rust -- compile the `tolmap` binary against the native libs above.
# ---------------------------------------------------------------------------
# Pinned to an explicit patch version, not `latest` -- confirmed present in
# docker.io/library/rust's tag list on 2026-09-20. `rust:<x>-bookworm` (not
# `-slim`) is a buildpack-deps image and already carries a C/C++ toolchain
# (gcc, g++, make), which two things here need without any extra apt step:
# `cc::Build` compiling native/leiden_bridge.cpp (build.rs) and
# `rusqlite`'s "bundled" feature compiling SQLite from source.
FROM rust:1.98.1-bookworm AS rust-builder

COPY --from=native /opt/leiden /opt/leiden
# The same prefix build.rs will be told to use for LEIDEN_PREFIX below --
# the include/ half is only needed here (compile time); the runtime stage
# copies just lib/ from the `native` stage directly, at this identical
# path, which is what makes the rpath baked in by this stage's build
# resolve there later.
ENV LEIDEN_PREFIX=/opt/leiden

WORKDIR /app
# The whole repo (minus .dockerignore's exclusions), not just Cargo.*/src/
# -- `cargo build` alone does not need data/, bindings/ or web/, but
# copying the full context here keeps this Dockerfile from having to
# enumerate every crate-relevant path by hand and stay in sync with it as
# the tree grows. .dockerignore is what keeps this from being enormous:
# target/, node_modules/, .git/, web/dist/, .maps/ and the frozen Python
# reference are excluded from the build context entirely, for every stage.
COPY . .
RUN cargo build --release --bin tolmap

# ---------------------------------------------------------------------------
# Stage 3: web -- build the React/TS bundle `tolmap serve` will host.
# ---------------------------------------------------------------------------
# Pinned to an explicit patch version -- confirmed present in
# docker.io/library/node's tag list on 2026-09-20. Node 24 is the current
# LTS line as of that date.
FROM node:24.21.0-bookworm-slim AS web-builder

# pnpm-lock.yaml is lockfileVersion 9.0 (pnpm 9's format); corepack ships
# with Node but its bundled pnpm version drifts from what the lockfile was
# written with, so pin explicitly rather than trust whatever corepack
# would otherwise resolve -- `pnpm install --frozen-lockfile` refuses to
# run at all if the lockfile doesn't match, and a pnpm major-version jump
# is exactly the kind of mismatch that trips.
RUN corepack enable && corepack prepare pnpm@9.15.9 --activate

WORKDIR /app
# Same whole-repo context as the rust stage, and for the same reason
# extract.rs/build.rs needed it there: `web/scripts/collect-maps.mjs`
# (predev/prebuild, see web/package.json) resolves `mapsDir` from
# web/maps.config.json (".maps", which does not exist in a fresh clone /
# fresh build context) and falls back to `../data` -- the nine committed
# fixture maps -- and Vite's `@bindings` alias resolves `../bindings`
# (web/vite.config.ts). Both are *outside* web/, so this stage's build
# context has to be the repository root, not `web/` alone, or the build
# either fails (no maps.config.json fallback dir found -- it would, since
# ../data still exists at repo root) or resolves @bindings to nothing.
COPY . .
WORKDIR /app/web
RUN pnpm install --frozen-lockfile
# Runs `prebuild` -> collect-maps.mjs -> `tsc -b && vite build` (see
# web/package.json). Output lands in web/dist (Vite's default), which
# .dockerignore excludes from every stage's context so a stale local build
# can never be silently reused here.
RUN pnpm build

# ---------------------------------------------------------------------------
# Stage 4: scip-tools -- pinned SCIP indexers (issue #110 P1b) plus the
# Node, Go and Python runtimes they need to run against a checkout, not
# just to install themselves.
# ---------------------------------------------------------------------------
# debian:bookworm-slim, same base as `runtime`, so the Go and Node tarballs
# extracted here (linux-amd64 official builds, glibc + libstdc++ only) are
# byte-identical to what `runtime` gets when this stage's /opt trees are
# copied over -- no separate base image's libc/OpenSSL assumptions to
# reconcile, the way `FROM node:...` or `FROM golang:...` here would have
# introduced.
FROM debian:bookworm-slim AS scip-tools

RUN apt-get update && apt-get install -y --no-install-recommends \
        curl \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# --- Go toolchain, checksum-verified. scip-go is not a static analyzer
# that merely happens to be written in Go: its own README says it "by
# default uses a few different `go` commands from the command line to gain
# information about the project and module" it is indexing (checked via
# github.com/scip-code/scip-go's README, 2026-09-24) -- so the *runtime*
# image needs the full toolchain (`go list`, `go build`, module resolution),
# not only the compiled scip-go binary. Version matches
# scip-spike.yml's `go-version: "1.26.x"` pin (finding 41).
ARG GO_VERSION=1.26.8
ARG GO_SHA256=d0f743b33e8d8945e6b1f432edd15785c70507121d6e2a723b21285eddf8b57b
RUN curl -fsSL -o /tmp/go.tar.gz "https://go.dev/dl/go${GO_VERSION}.linux-amd64.tar.gz" \
    && echo "${GO_SHA256}  /tmp/go.tar.gz" | sha256sum -c - \
    && tar -C /opt -xzf /tmp/go.tar.gz \
    && rm /tmp/go.tar.gz
ENV PATH="/opt/go/bin:${PATH}"
ENV GOTOOLCHAIN=local

# --- Node runtime, checksum-verified. scip-typescript and scip-python are
# both npm packages; the copy below brings node, npm and both packages'
# global installs over in one directory tree so every shebang and
# require() path baked in at install time (all under /opt/node) stays
# valid in `runtime`. Same version already pinned for `web-builder` above
# -- one Node version to track, not two.
ARG NODE_VERSION=24.21.0
ARG NODE_SHA256=6e1db87ef58b8819e5d5402eff1536491b18edd8eb7bee5ef7897876e88dc5ff
RUN curl -fsSL -o /tmp/node.tar.gz "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.gz" \
    && echo "${NODE_SHA256}  /tmp/node.tar.gz" | sha256sum -c - \
    && tar -C /opt -xzf /tmp/node.tar.gz \
    && mv "/opt/node-v${NODE_VERSION}-linux-x64" /opt/node \
    && rm /tmp/node.tar.gz
ENV PATH="/opt/node/bin:${PATH}"

# --- scip-typescript / scip-python, same pins as scip-spike.yml (finding
# 41). Installed with npm's default global prefix, which resolves relative
# to the npm binary's own location (/opt/node/bin) -- so both land under
# /opt/node, no separate prefix flag needed.
ARG SCIP_TYPESCRIPT_VERSION=0.4.0
ARG SCIP_PYTHON_VERSION=0.6.6
RUN npm install -g \
        "@sourcegraph/scip-typescript@${SCIP_TYPESCRIPT_VERSION}" \
        "@sourcegraph/scip-python@${SCIP_PYTHON_VERSION}"

# --- scip-go, same pin as scip-spike.yml (finding 41). `go install` needs
# network access to fetch scip-go's own module graph -- that happens only
# in this throwaway build stage, never in the shipped image or at request
# time (the same no-installs-at-request-time boundary #110 risk 1 draws
# for indexing a *target* repository).
ARG SCIP_GO_VERSION=v0.2.7
ENV GOPATH=/opt/gopath
RUN go install "github.com/scip-code/scip-go/cmd/scip-go@${SCIP_GO_VERSION}"

# --- Python runtime. scip-python wraps Pyright, which needs a real Python
# interpreter to discover the environment (search paths, stdlib) even with
# no project dependencies installed -- eval/scip_index.sh's default,
# no-install mode still ran scip-python successfully in finding 41, but
# only because the CI runner it ran on had a Python 3.12 from
# actions/setup-python. python3 in bookworm is 3.11, close enough for
# environment discovery; nothing here type-checks *against* it, Pyright
# carries its own typeshed stubs. Installed in `runtime` directly (below),
# not copied from here -- an apt package with its own shared-library
# dependencies is safer to install once, in place, than to copy across
# stages the way the two self-contained tarballs above are.

# ---------------------------------------------------------------------------
# Stage 5: runtime -- the image that actually ships.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# git        -- src/service/clone.rs shells out to it to clone repositories.
# ca-certificates -- both git's HTTPS clones and the leiden download step
#                    upstream needed it; the running service needs it too
#                    for the same reason (cloning github.com over HTTPS).
# coreutils  -- src/service/clone.rs's du -sb clone-size check. Present by
#               default on Debian (it is an essential package) but named
#               explicitly rather than assumed, since "essential on Debian"
#               is not a guarantee `apt-get install` itself makes portable.
# libgomp1, libstdc++6 -- transitive runtime needs of libigraph.so.4 and
#               liblibleidenalg.so.1, confirmed empirically (this change
#               could not run `ldd` inside a container -- no Docker here --
#               so this was checked against a locally built `tolmap`
#               binary linked against real igraph/libleidenalg .so files
#               instead: `ldd target/release/tolmap` and `ldd
#               libigraph.so.4` both show libgomp.so.1, i.e. igraph was
#               built with OpenMP; without libgomp1 installed the binary
#               fails to start with an unresolved-symbol/library error at
#               the runtime stage, not a build-time one). libc6, libgcc-s1
#               and libm are always present on a Debian base and are not
#               listed here.
# python3, python3-pip -- issue #110 P1b: scip-python's underlying Pyright
#               needs a real interpreter to discover environment search
#               paths even when indexing without installed dependencies
#               (docs/FINDINGS.md finding 41's default mode); pip is not
#               invoked by the default no-install mode but keeping it
#               present matches what finding 41's CI runner had
#               (actions/setup-python) and costs a few MB.
RUN apt-get update && apt-get install -y --no-install-recommends \
        git \
        ca-certificates \
        coreutils \
        libgomp1 \
        libstdc++6 \
        python3 \
        python3-pip \
    && rm -rf /var/lib/apt/lists/*

# Worker hardening (src/service/jobs.rs::process_worker_exe, docs/SCIP_SANDBOX.md
# #4.1 point 4): a dedicated, unprivileged system user the `tolmap worker`
# child is dropped to, distinct from the service process itself. No `USER`
# directive follows this -- the service (`tolmap serve`, this image's `CMD`)
# deliberately keeps running as root, which is what lets it drop the worker
# child's privilege (`CommandExt::uid()/gid()`) and `chown` each job's
# directory to this uid before handing it to the child in the first place;
# a root-owned service that spawns a non-root worker is the whole point,
# not an oversight of one.
#
# Fixed, pinned uid/gid (10001:10001) rather than a dynamically-allocated
# one from `useradd`'s default range, so it is a stable value the service
# can bake in as `ENV` and read back (`service::config::env_var_or`, the
# same pattern as `TOLMAP_STATIC_DIR` below) instead of doing a
# passwd/getpwnam lookup at runtime. 10001 does not collide with anything
# else this `debian:bookworm-slim` base or the packages above create.
# `--no-create-home` (no home directory needed -- the worker's own
# directory is a fresh, chowned-to-this-uid per-job dir, not $HOME) and
# `--shell /usr/sbin/nologin` (this account is exec'd into directly by
# `Command::uid()/gid()`, never logged into).
RUN groupadd --system --gid 10001 tolmap-worker \
    && useradd --system --no-create-home --shell /usr/sbin/nologin \
        --uid 10001 --gid 10001 tolmap-worker

# Baked-in image-layout constants (same pattern as TOLMAP_STATIC_DIR
# below): the uid/gid above is fixed at image build time, not a
# deployment-tunable fly.toml value, so the running service reads it back
# from here rather than fly.toml's [env]. See
# `service::config::ServeConfig::worker_uid`/`worker_gid`.
ENV TOLMAP_WORKER_UID=10001
ENV TOLMAP_WORKER_GID=10001

# The exact path the rust stage's build.rs baked into the binary's rpath
# (`-Wl,-rpath,/opt/leiden/lib`, since LEIDEN_PREFIX=/opt/leiden there) --
# copied from `native` directly rather than `rust-builder` only because
# `native` is where it was built and both would be byte-identical; only
# lib/ is needed at runtime, not include/ (headers are compile-time only).
COPY --from=native /opt/leiden/lib /opt/leiden/lib

# SCIP indexers (issue #110 P1b). Go and Node are copied whole, at the same
# /opt paths they were extracted to in `scip-tools`, so every shebang and
# rpath-equivalent baked in at install time still resolves -- only
# scip-go's own binary is copied singly, since GOPATH's module cache
# (everything else under /opt/gopath) is a build-time-only artifact.
COPY --from=scip-tools /opt/go /opt/go
COPY --from=scip-tools /opt/node /opt/node
COPY --from=scip-tools /opt/gopath/bin/scip-go /usr/local/bin/scip-go
ENV PATH="/opt/go/bin:/opt/node/bin:${PATH}" \
    GOTOOLCHAIN=local

# Env overrides for P1a's ingest module (branch `feat/scip-ingest`, issue
# #110 P1a) to find these binaries -- chosen here because P1a had not yet
# named them when this change was written; documented in docs/DEPLOY.md so
# P1a's `TOLMAP_SCIP_*` reads match. Defaulted to the absolute paths above
# rather than bare names on PATH, so a future PATH change in this file
# cannot silently change which binary an unset-override falls back to.
ENV TOLMAP_SCIP_TYPESCRIPT=/opt/node/bin/scip-typescript \
    TOLMAP_SCIP_PYTHON=/opt/node/bin/scip-python \
    TOLMAP_SCIP_GO=/usr/local/bin/scip-go

WORKDIR /app
COPY --from=rust-builder /app/target/release/tolmap /app/tolmap
COPY --from=web-builder /app/web/dist /app/web/dist

# Opt-in static serving (src/service/http.rs, docs/API.md): this is an
# image-layout constant -- the built bundle always lands at this exact
# path inside this image -- not a deployment-tunable value, so it is baked
# in here rather than left to fly.toml's [env]. TOLMAP_BIND_ADDR,
# TOLMAP_DB_PATH, TOLMAP_CACHE_DIR and the TOLMAP_MAX_*/TOLMAP_RATE_LIMIT_*
# limits *are* deployment-tunable (they depend on the volume and the VM
# shape) and are set in fly.toml instead, not here.
ENV TOLMAP_STATIC_DIR=/app/web/dist

EXPOSE 8787

CMD ["/app/tolmap", "serve"]
