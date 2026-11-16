# Target architecture

## What the shipped code is

The Python pipeline (`src/tolmap/`, 2081 lines) and the vanilla-JS viewer (`viewer/template.html`, 1048 lines) are a **reference implementation**, not the product codebase. They exist for two reasons and should not accumulate features:

1. **They are the test oracle.** The port is correct when it reproduces their output on the same repository at the same commit with the same seed. `data/` holds nine such outputs to check against — membership, modularity, edges, landmarks, symbols and references. Coordinates were not reproducible before finding 9 and the fixtures have been re-recorded since; the acceptance gate tests the half that was always stable. **That is the contract, not a temporary scope:** across the two implementations the oracle covers membership, modularity, edges, landmarks, symbols and references, and *not* node or district coordinates. Ruled 2026-09-20 -- see finding 11's closing section for what was measured and why.
2. **They encode findings that are expensive to rediscover.** Every non-obvious line is commented with why, and `docs/FINDINGS.md` records what was falsified. Read both before writing the Rust.

Freeze them at v0.1. New work goes in the Rust/TS tree.

## Stack

| layer | choice | notes |
|---|---|---|
| indexer | Rust | tree-sitter is a Rust/C library; this is native ground, not a compromise |
| store | SQLite embedded | see source-of-truth below |
| API | axum | JSON with durable progress polling |
| frontend | React + TypeScript + Vite | |
| UI | Tailwind + shadcn | chrome, panels, cards |
| data/routing | TanStack Query + TanStack Router | Router also closes the shareable-URL gap the prototype has |
| map surface | imperative renderer behind a ref | **not** React elements — see below |

## What lives where

**The repository holds intent. The store holds derivation.** That single line settles every case, and it is the resolution to a mistake an earlier draft of this document made.

That draft proposed committing the computed layout into the mapped repository. It was wrong: a layout is a *snapshot*, and a snapshot checked into git goes stale the first time someone merges without regenerating — silently, and carrying the authority of being committed. Coordinates, district membership, the name cache and blast radius are all derived from a specific commit and belong to the store, keyed by `(repo, commit_sha)`. Nothing derived goes in the repository.

What does belong in the repository is configuration — the choices a maintainer makes that the tool cannot infer, which are stable under recomputation because they are not computed:

```toml
# .tolmap/config.toml — optional; the tool works with no file at all

[[source]]                      # override auto-detection when it guesses wrong
path = "lib/sqlalchemy"
lang = "py"

exclude = ["examples/**", "**/generated/**"]

[cluster]
resolution = 1.1                # more districts when raised

[districts]                     # pin a name the team already uses
# keyed by an ANCHOR FILE, not a district id: ids are not stable across
# reclustering, but "whichever district contains this file" is
"src/payments/stripe.ts" = "payments"

[landmarks]
pin = ["src/gateway/router.ts"]
suppress = ["src/util/log.ts"]
```

The anchor-file keying matters more than it looks. District identifiers are an artefact of one clustering run; a name pinned to an id would break the next time the partition shifts, which is exactly the drift finding 4 is about. A name pinned to a file survives, because the question it asks — *what is the district containing this file called* — stays meaningful however the boundaries move.

The warm start reads the previous membership from the store, not from a file. Finding 4 requires that the previous partition be *available*; it does not require it to be committed.

Later, and only for teams that want layout drift visible in a pull request, a `tolmap check` run can write a small derived summary into the repo — district count, modularity, which files changed district — under the explicit lockfile contract of recording the commit it came from and refusing to be treated as current when that does not match `HEAD`. That is a reviewable *diff artifact*, not a source of truth, and it is out of MVP scope.

## Personal repository library

Issue #314 replaces the hosted-first direction with a personal CLI and a web
viewer over shared state. The CLI uses one private data root, defaulting to
`$XDG_DATA_HOME/tolmap` or `$HOME/.local/share/tolmap`, and application settings
in `$XDG_CONFIG_HOME/tolmap/config.toml` or `$HOME/.config/tolmap/config.toml`.
`.tolmap/config.toml` continues to express repository intent.

The service SQLite store owns registrations (stable UUID, unique display name,
source identity), durable attempts, and personal snapshot rows keyed by UUID.
Publication sequence defines latest; commits are not unique snapshot keys.
SQLite migration history remains intact. No existing data is discovered, read or imported.

Personal indexing reuses local independent Git capture and atomic artifact
publication. Each successful refresh retains its map, names, symbols, extraction
metadata and independently pinned source objects. A short SQLite transaction
publishes the row only after artifacts are complete. Kernel `flock` locks guard
repository writes and shared capacity slots across processes. Readers keep the
last published row during indexing; abandoned attempts become failed when the
next writer acquires the repository lock. Lock files live outside removable
repository directories so removal cannot create two lock inodes for one ID.

`tolmap web` loads the same library and configuration. Its personal service
adapter reuses the service router's static/SPA handler and React's existing
host-profile composition, catalogue, history, progress and source reader. Public
catalogue/admission routes are not mounted. Management calls the CLI library;
web-owned jobs use the existing durable jobs table for reloadable progress and
batches, with a bounded FIFO before shared process locks. Browser snapshot URLs
use `/snapshot/{id}` so repeated commits retain distinct map/source identities.
Home, history and job reads poll shared state; the CLI never depends on a web
process. Settings need restart, committed data changes do not. There is no auth;
loopback is default and off-loopback startup prints the explicit exposure warning.

## Rendering: do not put nodes in the React tree

A mid-sized map is 850 files, 12 districts, a few thousand import edges, and up to 8000 symbols. Rendering nodes as React components means reconciling thousands of elements on every pan frame.

Structure it as: React owns the chrome, the panels and all application state; the map is one component holding a ref, and inside that ref an imperative renderer draws. State flows down as props into an explicit `render(state)` call; interaction flows up through callbacks.

Start by porting the existing SVG renderer — it works at this scale and its pointer handling is hard-won (three touch-only bugs are documented in its comments). Keep the interface narrow enough that swapping in Canvas 2D or WebGL later is a change to one file. The threshold where SVG stops being enough is somewhere past 5000 visible nodes.

## Port map

| module | lines | Rust equivalent | risk |
|---|---|---|---|
| `extract.py` | 364 | `tree-sitter-python` + `git2` | low — Python's `ast` becomes tree-sitter like every other language |
| `multi.py` | 354 | `tree-sitter-go`, `tree-sitter-typescript` | low — same grammars, first-class bindings |
| `pipeline.py` | 297 | **Leiden + layout, own implementation** | **high, see below** |
| `blobs.py` | 320 | `ndarray` + own gaussian blur + own marching squares | medium — both are short and well-specified |
| `parcels.py` | 143 | `ndarray`, the power diagram is already a raster loop | low — it was written as an array loop, it ports directly |
| `naming.py` | 160 | plain Rust + an LLM client | low |
| `cli.py` | 122 | `clap` | low |
| `viewer/` | 1048 | React + TS, imperative map surface | medium — the touch handling is the subtle part |

## The one real risk: Leiden

`leidenalg` is Python bindings over a C++ library over igraph. There is no mature Rust equivalent, and community detection is the algorithmic core of this product — not a dependency to shop for.

Three options, in the order I would consider them:

1. **Implement Leiden in Rust.** The algorithm is well-specified (Traag, Waltman & van Eck 2019): local moving, refinement, aggregation, repeat. Roughly 500–800 lines.
2. **FFI to libleidenalg.** Faster to stand up, but pulls igraph's C build into the toolchain.
3. **Louvain plus a refinement pass.** Only if the schedule demands it. Leiden exists because Louvain produces badly connected communities; on this workload that shows up directly as unstable districts.

**The chosen path is 1 by way of 2**, and the reason is not the one an earlier draft of this document gave. That draft argued for going straight to an own implementation because "two requirements make a binding awkward anyway: strict determinism, and warm-starting from a previous membership". Neither survives measurement. `leidenalg` takes `initial_membership` as a first-class argument and `eval/batch_stability.py` already uses it, so warm-starting is not a reason. And determinism points the other way: leidenalg is the *most* deterministic component in the pipeline — byte-identical membership across runs, across `PYTHONHASHSEED` values, and against the committed fixtures — while the nondeterminism that actually existed sat in the pure-Python geometry that nothing flagged as a risk (finding 9).

So the fork is decomposed rather than chosen. FFI to libleidenalg first, as a scaffold, to get the parity harness green and prove the extract/blend/prune port in isolation — that is the larger surface anyway, roughly 2000 Python lines against ~20 for the partition call itself. Then an own Rust Leiden behind the same trait, with the FFI build kept in CI as a live differential oracle. One high-risk item becomes two independently verifiable ones, and the rewrite is checked against a running oracle rather than nine frozen JSON files. The C toolchain is accepted for the scaffold and not for the shipped artefact.

**Acceptance test for whichever path:** on scrapy, django and vue, the Rust partition must place ≥95% of files in the district their Python counterpart assigned, and modularity must land within 0.02. That check belongs in CI from the first commit of the clustering module.

The commits are pinned in `data/fixtures.toml`, not in the fixtures themselves — the compact viewer schema still carries no `params` block and no commit field (`pipeline.run()` emits one, `blobs.build()` drops it during compaction), so `data/*.json` alone still cannot tell you what was indexed to produce it. The manifest is the fix: one entry per fixture with its clone URL, pinned SHA, `pkg`, `lang`, whether it was built with parcels, and the headline numbers (files, districts, modularity) to make a drifted rebuild visible without opening the JSON. `eval/verify_fixtures.py` checks a fixture out at its pin, seeds the naming cache, rebuilds, and diffs the result byte-for-byte against the committed map; run it after touching anything upstream of `blobs.build()`.

The manifest also records a real inconsistency in the corpus rather than hiding it: only `flask` and `sqlalchemy` were recorded with the parcels (weighted-Voronoi plot) layer: the other seven fixtures were built with `--no-parcels` and carry no `P` key. A faithful rebuild of those seven therefore differs from a plain `tolmap build` in exactly the `P` key and nowhere else; `verify_fixtures.py` passes `--no-parcels` per fixture according to the manifest so this doesn't read as drift.

## Shared schema

## Organization maps

An organization map is composition, not a larger clustering run. A checked-in
manifest names each public member repository and pins it to a full commit.
`tolmap org check` validates that manifest; `tolmap org build` reads the
committed member trees, resolves declared package dependencies, places the
member maps as whole regions, and writes an `OrganizationDocument`. Organization composition stays available through the CLI; its HTTP endpoint is retired.

The organization layer never mutates, re-clusters, or re-lays out a member map,
and it computes no cross-repository modularity. Dependency, external and
unresolved counts are lower bounds because only declared Cargo, npm/pnpm, Go
and Python dependencies are visible. File-level cross-repository references,
cross-repository lenses, comparison of an organization across commits, and Hub
storage or access policy are deliberately out of scope; they need their own
specifications after the composition contract is stable.

The map JSON is the contract between indexer and viewer. Define it once as Rust types and generate the TypeScript from them (`ts-rs` or `typeshare`) rather than maintaining two hand-written definitions. The current shape is in `data/*.json`; the compact index-addressed layout (`F`, `N`, `E`, `L`, `S`, `U`) exists because nine maps had to fit in one HTML page, and can be relaxed now that data is fetched. Optional `D` sidecars retain debt-only evidence without changing `E` or any blending, clustering or layout input: `D.generated` is file-indexed header detection, with an optional ts-rs generator-file index only for a unique `#[derive(... TS ...)]` Rust struct or enum match. `D.package_roots` records sorted Cargo, npm, Go and Python package directories, so debt cycles can be projected onto build units without changing the map.

## Current application boundaries

`personal` owns registry resolution, acquisition and operations. `personal_index`
retains independent committed source and runs the shared executor. `personal_web`
adapts that library to HTTP and React. `service::http` owns static bundle routing;
`service::jobs` owns shared progress response types. The executor retains process
isolation, cancellation and SCIP sandboxing. Distributed scheduling, network worker
listeners, public catalogue admission and request rate limits are removed.

The `access` module remains an opt-in embedding contract. The standalone personal
router does not authenticate or call it. Comparison/provenance library helpers stay
for CLI and protected embedders; comparison HTTP endpoints are retired. SQLite
migration history is unchanged. See [ACCESS.md](ACCESS.md).
