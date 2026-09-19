# Findings

Everything here was measured on real repositories during the prototype. Where a claim was falsified, the falsification is kept — it is the more useful half.

## The corpus

| repo | lang | files | districts | modularity |
|---|---|---|---|---|
| django | py | 851 | 12 | 0.497 |
| prometheus | go | 444 | 11 | 0.540 |
| sqlalchemy | py | 258 | 9 | 0.798 |
| vue (core) | ts | 239 | 9 | 0.573 |
| scrapy | py | 188 | 8 | 0.348 |
| celery | py | 161 | 6 | 0.332 |
| rich | py | 100 | 3 | 0.445 |
| flask | py | 24 | 3 | 0.053 |
| httpx | py | 23 | 2 | 0.060 |

The pipeline is language-agnostic: Go and TypeScript went through it with no language-specific tuning beyond their parsers, and scored *above* most of the Python repositories. Vue's districts reproduced its package architecture (reactivity, runtime-core, compiler-core, compiler-sfc, compiler-ssr, server-renderer) without the algorithm knowing anything about Vue.

## 1. Coefficients are not influence shares

Writing the blend as a per-edge sum lets signal *density* rewrite the intent. Proximity is dense — every pair of files in a directory carries a value — while imports are sparse. On scrapy the nominal weights produced:

| signal | coefficient | actual share of total weight |
|---|---|---|
| static (imports) | 0.45 | 22% |
| cochange | 0.35 | 33% |
| proximity | 0.08 | **36%** |
| semantic | 0.12 | 8% |

The "weak prior" became the largest term. On Flask, imports landed at 4% of mass against a nominal 0.45. **Normalise each signal so its summed mass across all candidate edges matches the intended share.**

## 2. The signal balance inverts with maturity

On scrapy, co-change pairs outnumbered import pairs roughly 4:1. On django the ratio inverted to 1:3 — a large mature codebase has focused commits that couple little. Any fixed per-edge coefficients would be tuned for one repository and wrong for the next. This is the strongest argument for mass normalisation.

## 3. Clustering churn is the primary engineering problem, not layout

The draft claimed anchoring means "adding three files perturbs three positions." It does not. Over 300 commits of scrapy (20 files added, 4 removed, 168 carried over), anchored recomputation halved movement against a cold run but still left 49% of surviving files moving more than a tenth of the map.

Decomposing by what happened to each file's *district* locates it exactly:

| | median displacement | share of files |
|---|---|---|
| stayed in district | 0.049 | 60% |
| district dissolved | 0.189 | 29% |
| reassigned elsewhere | 0.248 | 12% |

**40% of carried-over files changed district over 300 commits.** The layout anchoring works; community assignment is the unsolved part.

## 4. Warm-starting the partition generalises, and is not a trade

Seeding Leiden with the previously committed membership, measured across four repositories over 200–300 commits:

| repo | files | cold retention | warm retention | modularity |
|---|---|---|---|---|
| httpx | 23 | 100% | 100% | 0.060 → 0.060 |
| celery | 161 | 77% | **96%** | 0.332 → 0.328 |
| scrapy | 188 | 60% | **88%** | 0.348 → 0.335 |
| django | 851 | 46% | **88%** | 0.497 → **0.507** |

Two things matter more than the headline. **Cold retention degrades monotonically with size** — community assignment is least stable exactly where a map is most needed. And the modularity effect ranges from −3.8% to *+2.1%*: on django the warm start produced a *better* partition as well as a stabler one, because the previous membership is a good initialisation. The honest statement is that the modularity effect is within noise while the stability effect is large.

## 5. Small repositories do not cluster

httpx (23 files) scored 0.060 and Flask (24) scored 0.053 — indistinguishable from no community structure. Both collapsed to two or three districts, and their region contours shattered into one island per file. There is a floor somewhere above ~50 files below which the map is the wrong representation and the landmark list is the entire product.

Above that floor, this finding originally claimed modularity measures **architecture quality, not size**, citing sqlalchemy at 258 files (0.798) against django at 851 (0.497). That comparison does not survive varying a constant this document never varied before drawing the conclusion: `prune()`'s absolute edge-weight floor. 0.798 is not a wrong number — it is the modularity of the graph this pipeline builds for sqlalchemy at the shipped floor of 0.02 — but the cross-repo comparison, and the "architecture, not size" interpretation placed on it, do not hold. With the floor made inert (0.000) sqlalchemy scores 0.465 against django's 0.497 — django is the more modular of the two, the opposite ranking. See finding 10.

## 6. Blast radius and modularity agree, independently

| repo | modularity | largest blast radius |
|---|---|---|
| sqlalchemy | 0.798 | 5 files / 2 districts |
| vue | 0.573 | 39 files / 3 districts |
| scrapy | 0.348 | **76 files / 8 districts (all of them)** |

These two quantities are computed from different data — one from Leiden on the blended graph, one from symbol names in import statements — and they corroborate each other. Scrapy's `Crawler`, `Request`, `Response` and `Spider` each reach 7–8 districts; that is *why* its modularity is low, expressed as four files you can open.

## 7. Three extraction bugs worth knowing about

Each of these silently produced plausible-looking but wrong results.

- **Re-exports.** `from sqlalchemy.orm import Session` lands on `orm/__init__.py`, which defines nothing and merely forwards. Reference resolution must follow the chain (4 hops is enough) or nearly every reference resolves to an empty file.
- **Module-style access.** `from . import interfaces` binds a *module*; the code then writes `interfaces.Dialect`. This is the dominant intra-package style in several libraries — missing it halves the reference graph.
- **Nested package roots.** `lib/sqlalchemy` and `src/flask` are two-segment roots; anchoring module names on a single segment made every absolute import fail to resolve. Fixing it added 26% more import edges to sqlalchemy. Its 0.798 modularity survived the fix (0.820 → 0.798), so the high score was not an artefact of the bug.

## 8. Geometry

- **Projection must be consistent.** Drawing symbols as elevation towers on a plan-view map mixes projections and reads as bar charts stuck in the ground.
- **A featureless disc means a bad district.** Districts with no internal structure render as near-perfect circles with evenly spread points. The shape itself diagnoses partition quality — a free property nobody designed in.
- **Weighted Voronoi keeps the area encoding.** A plain Voronoi tiles the plane but its cell areas encode point density, which is a layout artefact. A power diagram — `cell(i) = argmin |x−pᵢ|² − wᵢ` — tiles it too, and the weights can be solved until area tracks line count. Measured correlation: 0.62 (django) to 0.95 (rich).
- **0.62 is a geometric ceiling, not a tuning problem.** Raising the raster from 168 to 320 moved it 0.618 → 0.618. Files on a district boundary have nowhere to expand into.

## 9. The map was not reproducible, and the cause was the hash seed

`SEED = 7` is threaded through every stage that draws a random number, and the partition really is byte-stable: leidenalg returns identical membership across runs, across `PYTHONHASHSEED` values, and — measured on all nine fixtures at their current heads — identical to the membership committed in `data/`. The geometry was not. Two runs of the reference on the same tree at the same commit moved files by this much:

| repo | files | median | max |
|---|---|---|---|
| flask | 24 | 0.886 | 2.024 |
| rich | 100 | 0.748 | 2.466 |
| vue | 239 | 0.549 | 2.136 |
| celery | 161 | 0.530 | 2.203 |
| prometheus | 444 | 0.403 | 1.862 |
| scrapy | 188 | 0.388 | 1.727 |
| sqlalchemy | 258 | 0.355 | 1.470 |
| django | 851 | 0.267 | 1.288 |
| httpx | 23 | 0.002 | 1.187 |

The right comparison is finding 3's own churn figure, not its round-number threshold. Weighting that table by share of files gives a median displacement of `0.049·0.60 + 0.189·0.29 + 0.248·0.12` = **0.114** over 300 commits of scrapy. Against that, the median run-to-run jitter of doing nothing at all ran **2.3× to 6.6×** on every repo above the finding-5 floor — django lowest at 2.3×, rich highest at 6.6×, scrapy itself 3.4×. Below the floor flask reaches 7.8×.

**The small repositories are the worse case, not the milder one.** flask's median is 0.886 against django's 0.267: a large repo's districts are big enough that a file reshuffled within its sub-cluster stays roughly where it was, while a small one has few enough sub-clusters that reordering them moves everything. httpx is the exception that confirms it — with two districts and 23 files there is almost nothing left to permute, so its median is 0.002 while its maximum is still 1.187, meaning one or two files were thrown across the map on every run.

Fixing `PYTHONHASHSEED` made the output byte-identical; leaving it unset made every run differ. The leak is `nx.Graph.subgraph()`, which returns a *view* whose node iteration follows a set of the node names. Nothing reads that order deliberately, but `subdivide` builds igraph vertex indices from it and `spring_layout` seeds its position array from it, so the interpreter's per-process hash randomisation reached the coordinates through two layers that both look seeded.

| quantity | before | after |
|---|---|---|
| membership, modularity, edges, landmarks, symbols, references | byte-stable | byte-stable |
| coordinates, district centroids, blob polygons | median 0.002–0.886 move per run, maxima to 2.47 | byte-stable |

`ordered_subgraph()` in `blobs.py` builds a real graph with nodes in the caller's order and edges sorted. Every fixture is now byte-identical across four builds — two with `PYTHONHASHSEED` unset, one at 0, one at 31337 — where before the fix the same four builds produced four distinct maps:

| | django | prometheus | sqlalchemy | vue | scrapy | celery | rich | flask | httpx |
|---|---|---|---|---|---|---|---|---|---|
| distinct maps, before | 4 | 4 | 4 | 4 | 4 | 4 | 4 | 4 | 4 |
| distinct maps, after | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 1 |

Three things follow, and the third is the reason this is written down.

The coordinates in `data/*.json` were generated under one arbitrary hash seed and are **not** reproducible even by the corrected reference — membership, modularity, edges, landmarks, symbols and references all still reproduce exactly, and the fixtures remain a valid oracle for everything except position.

The acceptance gate for the port (≥95% of files in the district the reference assigns, modularity within 0.02) tests only the half that was always stable. That was a better gate than it looked.

**A faithful Rust port reproduces this bug exactly.** Rust's default `HashMap` iterates in a randomised order for the same reason Python's `set` does. Ported from the rendering logic alone it will present as "the map jiggles between runs", months after anyone remembers that a subgraph view was involved. Use `BTreeMap`/`IndexMap`, or sort at every set boundary, from the first commit of the geometry module.

## 10. `prune()`'s absolute floor makes cross-repo modularity a different pipeline at each end of the corpus

Finding 5's sqlalchemy-beats-django comparison turned out to depend on a constant nobody had varied: `prune()`'s edge-weight floor, shipped at an absolute 0.02. Measured by holding `keep_per_node=14` fixed and sweeping the floor alone, same pipeline, resolution 1.1 (issue #11):

| repo | floor 0.02 (shipped) | 0.010 | 0.005 | 0.002 | 0.000 |
|---|---|---|---|---|---|
| sqlalchemy | 0.7976 | 0.7212 | 0.5457 | 0.4654 | 0.4654 |
| rich | 0.4447 | 0.3524 | 0.3524 | 0.3524 | 0.3524 |
| django | 0.4968 | 0.4968 | 0.4968 | 0.4968 | 0.4968 |
| scrapy | 0.3479 | 0.3479 | 0.3479 | 0.3479 | 0.3479 |

Only these four repositories were swept across the floor; the other five in the corpus table (prometheus, vue, celery, flask, httpx) were not, and this document does not extend the sweep to them.

**Mechanism.** `blend()` normalises each signal to its share of total mass (finding 1), then divides every edge by the single largest one to rescale into a friendly range. A repo whose static (import) graph is sparse relative to its co-change graph concentrates that normalisation into one dominant edge, which drags the rest of the distribution down and collapses most of it under a fixed *absolute* floor. sqlalchemy's maximum edge sits 299.5× above its median, putting the median at 0.0033 — six times under the shipped floor; django's max/median ratio is 11.5×, and nothing falls under the floor at all. Share of candidate edges falling below the shipped 0.02 floor after normalisation, per repo measured (issue #11): **sqlalchemy 96.1%, rich 54.4%, flask 35.4%, celery 1.2%, scrapy 0.1%, django 0.0%.** That range is the finding: the same code is a different pipeline at its two ends, keeping a near-tree of 256 of sqlalchemy's 6622 candidate edges (mean degree 2.6, 73 components) at one end and effectively the whole candidate graph at the other.

**Consequence for the port's acceptance gate.** The gate in `CLAUDE.md` reads "modularity within 0.02" of the reference. On sqlalchemy, modularity itself swings 0.33 across plausible floors, and the shipped floor sits between the 90th and 99th percentile of its normalised weights — so a port whose raw signal masses differ even slightly from the reference's can fall on the other side of that cut into a different modularity regime, while every edge it keeps still looks correct in isolation. See issue #3.

**The decision, recorded rather than hidden.** Three routes were available: a relative floor (a percentile of the normalised distribution) instead of an absolute one, applying the floor before the max-rescale, or leaving `prune()` and `blend()` exactly as shipped and reporting the sensitivity instead. This document takes the third: **the pipeline is unchanged, deliberately.** The mitigation is textual, not algorithmic — from this finding on, kept-edge count and below-floor share are reported alongside any modularity figure being used for a cross-repo comparison, not modularity alone. A finding that records the option not taken is more useful than one that pretends there was no choice.
