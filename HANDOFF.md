# Handoff

You are picking up a measured prototype, not a product.

**The Python and vanilla-JS code in this package is a reference implementation and a test oracle. Do not build on it.** The product is Rust + React/TypeScript, and the current direction is a **personal repository library with a primary CLI and shared web viewer** (issue #314). The historical public MVP notes below describe the earlier direction. `docs/ARCHITECTURE.md` has the target stack, the MVP scope, the module-by-module port map and the two places real risk lives. Freeze `src/tolmap/` and `viewer/` at v0.1.

Read `docs/FINDINGS.md` before writing any of the port. Several obvious-looking "improvements" have already been tried and falsified there, and the reference implementation is the thing the port is checked against.

---

## Current product

The Rust CLI owns the shared personal repository library: `tolmap add`, `repos`,
`status --repo NAME`, `refresh`, `rm` and database-aware queries. `tolmap web`
provides the React browser over the same configuration and SQLite state.
[LOCAL.md](docs/LOCAL.md), [API.md](docs/API.md) and [QUERY.md](docs/QUERY.md)
are the current usage contracts. Explicit file/graph inputs remain supported.

Snapshot source is copied into independent retained Git objects. Writes use
per-repository and shared-capacity locks; reads keep the last complete snapshot.
The executor still isolates workers, kills cancelled process groups and supports
SCIP installs in nsjail. Public catalogue admission and distributed worker modes
are removed. Organization composition remains CLI-only. `access` remains an
opt-in library policy for embedders, not router authentication.

Preserve the measured pipeline and renderer findings. Overview is district outlines
and roads; file symbols require 40 px and class members 110 px. Builds, tests,
typechecks and browser qualification run in CI; local formatting is allowed.
