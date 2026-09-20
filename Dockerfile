# Fly.io deployment (2026-09-20): one image, one always-on machine, serving
# both the API and the built web bundle from a single origin -- see
# docs/API.md's TOLMAP_STATIC_DIR section and fly.toml.
#
# Four stages. The first two exist because of the one real native
# dependency this crate has (docs/ARCHITECTURE.md's "The one real risk:
# Leiden"): build.rs compiles native/leiden_bridge.cpp against
# libleidenalg + igraph and bakes `-Wl,-rpath,<LEIDEN_PREFIX>/lib` into the
# binary, so the runtime stage must carry those two shared libraries at the
# *exact* path the rust stage built against -- not just "somewhere on
# LD_LIBRARY_PATH". Get that path wrong and the binary does not start, and
# fails at dynamic-link time with no useful message (this file's own
# comments, not a guess: see build.rs).
#
# Image tags are pinned to a specific version everywhere, not `latest` --
# `docker.io/library/rust` and `docker.io/library/node`'s registry tag
# lists were checked (2026-09-20) to confirm the exact tags below exist.
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
# Stage 4: runtime -- the image that actually ships.
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
RUN apt-get update && apt-get install -y --no-install-recommends \
        git \
        ca-certificates \
        coreutils \
        libgomp1 \
        libstdc++6 \
    && rm -rf /var/lib/apt/lists/*

# The exact path the rust stage's build.rs baked into the binary's rpath
# (`-Wl,-rpath,/opt/leiden/lib`, since LEIDEN_PREFIX=/opt/leiden there) --
# copied from `native` directly rather than `rust-builder` only because
# `native` is where it was built and both would be byte-identical; only
# lib/ is needed at runtime, not include/ (headers are compile-time only).
COPY --from=native /opt/leiden/lib /opt/leiden/lib

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
