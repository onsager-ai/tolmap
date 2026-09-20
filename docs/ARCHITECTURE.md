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
| store | SQLite embedded, Postgres for hosted | see source-of-truth below |
| API | axum | JSON, plus SSE for indexing progress |
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

## MVP: a site that maps any public repository

The first product is a website in the shape of DeepWiki — paste a repository, get a map, no install. The map of a repository lives at `/<owner>/<repo>`, with no forge prefix: `<host>/scrapy/scrapy`.

Two consequences of dropping the prefix, both worth handling on day one rather than discovering later. The router must **reserve its own top-level names** (`about`, `docs`, `api`, `new`, `settings`, `assets`, and anything else the app will ever want) before any of them collides with a real GitHub owner. And the scheme leaves no room to disambiguate a second forge, so supporting GitLab later means a query parameter, a separate host, or breaking the URLs. DeepWiki accepted the same trade; it is a reasonable one, but it is a decision, not a default.

DeepWiki answers *what is this code*. tolmap answers *where is it and what does it reach*. The two are complementary, and the positioning line is that one produces prose and the other produces a place.

### What a public endpoint forces that a CLI does not

**Auto-detection. This is the largest piece of unbuilt work in the MVP, and it is easy to underestimate.** The reference CLI requires `--pkg lib/sqlalchemy --lang py`. Nobody pasting a URL will supply that, and getting it wrong does not fail loudly — it produces a sparse graph and a plausible-looking wrong map (finding 7). Detection needs to be its own module with its own tests:

- Language: count source files by extension, excluding vendored and generated trees.
- Source root, per ecosystem: `go.mod` puts it at the repo root; `pyproject.toml` or `setup.py` points at `src/<pkg>` or a top-level package directory; `package.json` plus `tsconfig.json` points at `src` or a `packages/*` workspace.
- Exclusions: tests, fixtures, vendor, generated code, examples. The prototype's list is a starting point, not a complete one.
- Confidence: when detection is uncertain, the site should say what it chose and let the user override, rather than silently mapping the wrong tree.

**Polyglot repositories.** Most real repositories are not one language. The prototype maps one language per run; the Rust port merges (`extract::build_multi_source`, `tolmap build --all-sources`/`--pkg`×N `--lang`×N). Merging is the better answer, because districts are about concerns and a concern crosses languages. Cross-language *import* edges genuinely do not resolve — `static` is architecturally zero across a language boundary, since resolution only ever looks a target up in its own source's known-file set — but the once-stated corollary, "co-change becomes the only signal that bridges languages," was measured (`docs/FINDINGS.md` finding 13, `tolmap polyglot-report`) and is false as stated: `proximity` (a path-prefix ratio) and `semantic` (IDF over an identifier vocabulary pooled across every source) both cross too, on both a synthetic fixture and a real repository (prometheus at its pinned commit, which turned out to carry a 78-file TypeScript UI). Their cross-language share is usually small (0.06%–14.4% of each signal's own mass, across the two corpora measured) and `cochange` is typically the largest bridge in absolute terms, but "the only signal" is wrong, not just imprecise — keep β meaningful, but do not architect around static/semantic/proximity contributing literally nothing across languages, because they don't.

**Polyglot is the first port feature with no oracle behind it.** The frozen Python reference maps one language per run; all nine fixtures in `data/` are single-language, and `CLAUDE.md`'s ≥95% placement / 0.02 modularity acceptance gate is defined against that reference's output — there is no reference merged map to gate a polyglot port against, and there will not be one without extending the frozen Python, which `CLAUDE.md` rules out. Two numbers replace byte parity as the polyglot contract: the projection-drift retention (how much a language's own district assignment moves when another language is merged in — measurably nonzero, 51%–100% retention across the two corpora measured) and the synthetic fixture `eval/gen_synthetic_polyglot_fixture.py` generates and CI checks on every push (three `--all-sources` builds byte-identical to each other, plus `tolmap polyglot-report`'s NMI and below-prune-floor share under the ceilings `docs/FINDINGS.md` finding 13 records). Modularity alone cannot be the polyglot acceptance gate: it looks equally good whether a merge found genuine cross-language structure or simply redrew each language's own file extension as a district boundary (NMI against the language label near 1.0 is the tell, not q) — see finding 13's two-corpus comparison.

**Indexing is a job, not a request.** Django took minutes in Python. Submit, queue, work, stream progress over SSE, cache by `(repo, commit_sha)`. A second visitor to the same commit gets the cached map.

**Clone strategy.** Co-change needs commit history, so a depth-1 clone is not enough. `--filter=blob:none` gives the full commit graph without file contents, then the working tree is materialised once — that is what the prototype used and it is the right default. Budget disk and evict.

**Limits, because it is a public endpoint that clones and burns CPU on demand.** Caps on file count, clone size, history depth and wall time, with a clear "this repository is too large for the hosted index" rather than a timeout. Rate limit per IP and per repo.

### MVP scope, explicitly

In: public GitHub repos, auto-detection, the map view with districts, landmarks, search, blast radius, and a shareable URL.

Out: private repos and auth, the committed-layout mode, `tolmap check`, incremental indexing, symbol-precise references via SCIP. All of these are real and all of them are after the site exists.

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

The map JSON is the contract between indexer and viewer. Define it once as Rust types and generate the TypeScript from them (`ts-rs` or `typeshare`) rather than maintaining two hand-written definitions. The current shape is in `data/*.json`; the compact index-addressed layout (`F`, `N`, `E`, `L`, `S`, `U`) exists because nine maps had to fit in one HTML page, and can be relaxed now that data is fetched.

## Milestones

The MVP is the site, so the order below front-loads what the site needs and defers what only teams-with-repos need.

1. **Indexer parity.** Rust `tolmap build` reproduces the nine reference maps within the acceptance thresholds. No UI, no server. The Leiden risk lives here, so it goes first and alone.
2. **Auto-detection.** Given only a clone, choose language and source root, and report confidence. Test it against the nine reference repos, whose correct answers are already known, plus a handful of deliberately awkward ones (a monorepo, a repo with `src/` and no package metadata, a polyglot service).
3. **Job service.** axum, SQLite, a queue, clone management, SSE progress, cache by `(repo, commit_sha)`, and the limits above.
4. **Viewer port.** React/TS shell, imperative map surface, URL state via TanStack Router — which is also the shareable link the prototype lacks.
5. **Ship the site.** `<host>/<owner>/<repo>`, with reserved top-level names in place from the first route.

Then, in whatever order demand dictates: incremental indexing, `tolmap check` for CI, the committed-layout mode, SCIP-precise references, private repos.
