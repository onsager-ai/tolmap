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
**A faithful Rust port reproduces this bug exactly.** Rust's default `HashMap` iterates in a randomised order for the same reason Python's `set` does. Ported from the rendering logic alone it will present as "the map jiggles between runs", months after anyone remembers that a subgraph view was involved. Use `BTreeMap`/`IndexMap`, or sort at every set boundary, from the first commit of the geometry module. (Verified on the Rust port: it uses `BTreeMap`/`BTreeSet` throughout, no `std::collections::HashMap`/`HashSet` anywhere in `src/`, and three independent `tolmap build` runs on scrapy produced byte-identical output, `sha256sum` checked.)

## 11. The Leiden gap was never the algorithm — it was seven unset fields

A previous attempt at this port (branch `feat/rust-indexer-parity`, now superseded) measured district placement of 60–82% against the reference on four of nine fixtures and drew a specific conclusion from it: that Leiden's local-moving optimisation is "highly sensitive to the exact micro-sequence of RNG draws on a weakly modular graph," that this sensitivity is inherent to the algorithm rather than to any particular implementation, and that only a from-scratch native-Rust Leiden — not FFI to the same `libleidenalg` the Python reference itself calls into — could close the gap. That branch's CI workflow shipped with a header comment recording this as an accepted, permanent limitation and cited `docs/ARCHITECTURE.md`'s "one real risk" section as having predicted it.

The conclusion was wrong, and the graph was never the variable. The Rust FFI bridge (`native/leiden_bridge.cpp`) constructs a libleidenalg C++ `Optimiser` and set only its RNG seed, leaving every other field at the C++ constructor's default. Those defaults are not what the Python reference runs: `la.find_partition()` — what `leidenalg`'s Python `Optimiser` actually calls — sets `refine_consider_comms = RAND_NEIGH_COMM` (4), where the bare C++ constructor defaults it to `ALL_NEIGH_COMMS` (2). Refinement is the phase that distinguishes Leiden from Louvain (it is what guarantees well-connected communities), so the port was running a different search over the same graph and landing on different, equally valid local optima — not chaos, a **configuration bug**. Six more fields (`consider_comms`, `optimise_routine`, `refine_routine`, `refine_partition`, `consider_empty_community`, `max_comm_size`) carried the same unexamined-default risk and are now pinned explicitly alongside it.

Measured on this tree's re-recorded fixtures (`data/*.json` after PRs #1, #8, #9 — not the stale ones the previous branch's numbers came from), before and after pinning those seven fields, same graph, same seed, same everything else:

| repo | placement before | placement after | modularity delta before | modularity delta after |
|---|---|---|---|---|
| scrapy | 72.3% | 100.0% | 0.0034 | 0.0000 |
| rich | 67.0% | 100.0% | 0.0129 | 0.0000 |
| celery | 77.6% | 100.0% | — | 0.0000 |
| prometheus | 81.5% | 100.0% | — | 0.0000 |
| django | 96.1% | 100.0% | — | 0.0000 |
| vue | 100.0% | 100.0% | — | 0.0000 |
| sqlalchemy | 98.4% | 98.4% | — | 0.0000 |
| flask | 100.0% | 100.0% | — | 0.0000 |
| httpx | 100.0% | 100.0% | — | 0.0000 |

(scrapy and rich's "before" figures and modularity deltas were reproduced independently in this session by reverting `native/leiden_bridge.cpp` to the pre-fix state and rebuilding, not just read off the fix commit's message. celery/prometheus/django's "before" numbers are the fix commit's own measurement, not independently re-verified here.)

Modularity now matches the reference to four decimal places on all nine fixtures rather than drifting up to 0.0129. sqlalchemy's placement holds at 98.4% before and after — the one fixture where a real, still-open community-detection difference remains (see below), rather than a settings artefact.

**The general lesson: "same algorithm, same seed" is not "same search."** A C++ library's constructor defaults are not its own Python wrapper's defaults, and nothing about the API surface says so — `Optimiser()` compiles and runs identically whichever defaults it carries, silently returning a locally-optimal but differently-shaped partition. The fix was seven assignments; finding the seven assignments required reading `leidenalg`'s Python source for what it actually passes to the C++ layer, not just its C++ headers' declared defaults. The previous branch treated the symptom (low placement on weakly-modular graphs) as confirmation of a plausible-sounding hypothesis (Leiden is chaotic there) without checking that hypothesis against the one thing that would have falsified it cheaply: whether the two bindings were actually configured the same way. They were not.

### What remains open after this fix

**sqlalchemy's landmark/placement gap (98.4%, not 100%) is real community-detection divergence, not a settings bug or a landmark-selection bug.** The reference groups `dialects/` as one 74-file community; this port's partition splits 4 of those files into their own community (`connectors/`), landing 254/258 files in the district the reference assigns. This is downstream of Leiden's local-moving search finding a different (still valid, similarly-modular) optimum on this specific graph — the kind of graph-topology sensitivity the previous branch wrongly generalised to all nine fixtures actually does exist, just on one of them, and at a much smaller scale (4 files, not 30–70% of a repo). Not investigated further here — closing it would mean tracing the exact node-visit order Leiden's local-moving phase uses on this graph and comparing it hop-for-hop against `leidenalg`, which is a different (and much narrower) question than the one the previous branch was actually answering wrong.

**Tier-1 layout coordinates (district placement in the 2D plane, and therefore every node position derived from it) do not match the reference and are not close to matching.** `docs/ARCHITECTURE.md` already flags this as the risk finding 9 predicted ("dependent on someone else's RNG handling"); this is the measurement. The reference's tier-1 layout is `nx.spring_layout(G, weight="weight", seed=SEED, iterations=400, k=1.1)`, which draws its initial node positions from **numpy's** RNG: networkx's `@np_random_state` decorator turns an integer seed into a `numpy.random.RandomState`, and the layout then calls `seed.rand(len(G), dim)`. That is legacy MT19937 — the same Mersenne Twister *core* that `python_sample` in `src/pipeline.rs` already reproduces bit-for-bit for landmark betweenness sampling, but seeded by NumPy's `init_by_array` and consumed by NumPy's own double-generation routine rather than CPython's, so the existing port does not simply drop in. The Rust port's `force_layout` is a from-scratch Fruchterman-Reingold implementation: a deterministic golden-angle spiral for initial placement (not random at all), and its own force/cooling-schedule formulas — the same algorithm *family* as `spring_layout`, not a port of it.

Measured on this tree, matching candidate districts to reference districts by best-Jaccard overlap (the same matching `src/parity.rs` uses for placement) and comparing district-centroid positions:

| repo | districts matched | raw median | raw max | Procrustes-aligned median | Procrustes-aligned max |
|---|---|---|---|---|---|
| httpx | 2 | 1.624 | 1.624 | 0.000 | 0.000 |
| flask | 3 | 1.917 | 2.198 | 0.037 | 0.043 |
| rich | 3 | 0.034 | 0.072 | 0.020 | 0.039 |
| celery | 6 | 1.774 | 2.990 | 1.118 | 1.365 |
| scrapy | 8 | 0.966 | 2.295 | 0.819 | 1.934 |
| prometheus | 11 | 0.828 | 2.012 | 0.560 | 1.135 |
| vue | 9 | 1.001 | 2.107 | 1.041 | 1.757 |
| sqlalchemy | 9 | 0.880 | 2.572 | 0.775 | 1.205 |
| django | 12 | 0.919 | 1.645 | 0.677 | 1.375 |

("Procrustes-aligned" finds the best rotation/reflection/uniform-scale/translation of the candidate's district centroids onto the reference's, then measures what's left — isolating genuine shape difference from the rigid-transform freedom every force-directed layout has.)

At 2–3 districts, Procrustes alignment collapses the gap to near zero: the two layouts are the same triangle (or the trivial two-point case), just rotated/reflected/rescaled differently, which is exactly what different RNG streams and different initialisation should produce on a system this unconstrained. At 6+ districts the aligned error stays large (median 0.56–1.12, close to a plot's own unit scale of [0,1]): the two layouts are not the same shape up to a rigid transform, meaning `force_layout` and `spring_layout` are converging to genuinely different local optima of a non-convex multi-body system, not just different orientations of the same one. This is the expected behaviour of force-directed layout in general — it is well known to have many stable local minima for graphs with more than a handful of nodes — made worse here by initialising from a completely different distribution (a fixed spiral vs. a random draw) and running different force/cooling formulas.

**Closing this is not the same job as the betweenness-sampling fix in this same session.** That fix pinned settings on an already bit-exact-reproducible RNG stream (CPython's Mersenne Twister, already ported) feeding a deterministic, order-independent algorithm (Brandes' betweenness). Reproducing `spring_layout` bit-for-bit would require matching NumPy's MT19937 seeding and uniform-sampling routines exactly, its array-fill order, and `spring_layout`'s exact Fruchterman-Reingold formula (including its scipy-based sparse-graph code path) in the same floating-point operation order — a materially larger undertaking than seven struct fields, and one `docs/ARCHITECTURE.md` already flagged as possibly unreachable ("bit-exact reproduction may not be reachable from Rust at all"). Not attempted in this session; `src/parity.rs` does not gate on it, by design, for exactly this reason.

**What this means for "the same repo at the same commit produces a byte-identical map":** that guarantee holds for the Rust port against *itself* — verified above, three runs, one hash, no `std::HashMap` anywhere in the source — and holds for membership, modularity, edges, landmarks and symbols against the *Python reference*. It does **not** hold for node/district coordinates against the Python reference, and closing that gap means porting numpy's RNG and `spring_layout`'s exact algorithm, not just fixing a settings mismatch. A team relying on the port to place a district in the same visual spot the reference would, run over run across the two implementations, would be relying on something not yet true.

## 12. Relative imports resolved one package level too deep, and it was not evenly distributed

`extract.resolve()` had an off-by-one that dropped nearly every `from . import x` — not a resolution gap of the kind finding 7 already documents (calls through a variable, SCIP would close it), but a whole syntactic form silently discarded. The `+ 1` in its level arithmetic exists because `mod_name()` already strips `__init__` off a package's own module name, so a package `__init__.py` needs zero segments removed to resolve against itself — that term is correct for exactly that one case and wrong for every other module, which is the majority of files. For `pkg.sub.mod` (file `pkg/sub/mod.py`) at level 1, it kept the whole name, so `from . import x` became `pkg.sub.mod.x` — not a known module, and its parent equals `cur_mod` and is discarded two lines later. The import yielded nothing, and every deeper level was shifted by the same one.

**The fix threads `is_pkg` through `resolve()`.** A relative import resolves against the *containing package* — `cur_mod` itself for a package `__init__`, `cur_mod` minus its last segment for anything else — then strips `node.level - 1` further segments. `resolve()` cannot tell the two cases apart on its own; the caller passes whether the importing file is an `__init__.py`. A naive fix (dropping the `+ 1` unconditionally) would make `__init__.py` files resolve to their *parent* package, inventing edges that are not there — "numbers must be a lower bound" forbids that, so the `is_pkg` distinction is the whole fix, not an embellishment of it.

Static import pairs recovered, before and after, on the fixture clones:

| repo | before | after | change |
|---|---|---|---|
| flask | 17 | 83 | ×4.9 |
| httpx | 19 | 82 | ×4.3 |
| celery | 550 | 659 | +20% |
| scrapy | 877 | 883 | +0.7% |
| rich | 112 | 393 | ×3.5 |
| django | 2947 | 3091 | +4.9% |
| sqlalchemy | 521 | 2530 | ×4.9 |
| tolmap (self) | 0 | 9 | from nothing |

**Not evenly distributed, and not proportional to repository size.** The spread is the house style of each package: scrapy and django import absolutely almost everywhere and barely moved; flask, httpx, rich and sqlalchemy lean on relative imports and roughly quadrupled. tolmap's own source uses relative imports exclusively, so mapping this repository previously produced a map with no import edges at all — one district, no roads — regardless of how much of the codebase actually referenced the rest of itself.

**`multi.py`'s Go and TypeScript resolvers were checked for the same class of bug and are not affected.** `resolve_ts()` resolves a relative import against the *importing file's own directory* via `os.path.normpath`, with no package-boundary level arithmetic to get off by one — TypeScript relative imports don't have the concept this bug lives in. `resolve_go()` maps an absolute import path straight to a directory via `bydir`, which also has no level term. Measured, not assumed: `data/prometheus.json` and `data/vue.json` rebuild byte-identical to their pre-fix committed maps.

**What the re-record changed in the corpus.** Membership shifted on every Python fixture (prometheus and vue untouched, confirming the above). District *counts* moved on five of seven: celery 6 → 7, flask 3 → 4, httpx 2 → 4, rich 3 → 4, sqlalchemy 9 → 8; django and scrapy held their counts (12 and 8) with membership still moving underneath. Modularity moved with it — up on celery (0.332 → 0.361), django (0.497 → 0.506) and scrapy (0.348 → 0.345, essentially flat); down on flask (0.053 → 0.033), httpx (0.060 → 0.030), rich (0.445 → 0.344) and sqlalchemy (0.798 → 0.520). The sqlalchemy drop is the largest in the corpus and is consistent with finding 5/10's caution about cross-repo modularity comparisons: a much denser static graph changes which edges clear the prune floor, not just which community they land in.

Because membership changed, most district-fingerprint cache entries missed and the IDF fallback renamed those districts — the correct signal per `eval/seed_names.py`'s docstring, not a failure, and no name was hand-edited to avoid it. Districts whose membership happened to land exactly on their old fingerprint kept their name verbatim (all of prometheus's and vue's districts; django's "gis · gdal", "files & cache" and "messages"; httpx's "transports"; scrapy's "command line" and "spider middleware"; rich's "unicode tables"). Every other Python-fixture district renamed; the full old-name → new-name table is in the "data: re-record" commit that carries this finding.

**The Rust port needed the same fix.** `python_head()` in `src/extract.rs` carried the identical unconditional `+ 1`. Ported the same `is_pkg` distinction through `python_head()`, `resolve_python()` and `python_uses()`. The parity gate against the re-recorded `data/*.json`, all nine repos: 100.0% district placement and 0.0000 modularity delta on every fixture, with F/E/L/S/U all byte-identical. This is not a regression from the pre-fix baseline (98.4–100% placement, sqlalchemy the one holdout on landmarks alone) — sqlalchemy now matches exactly too. Its previous 98.4% was a Leiden local-optimum difference from `dialects/` splitting 4 files into their own community on the old, sparser graph (finding 11's "what remains open"); the corrected extraction changed the graph enough that both implementations converge to the same partition on it. Whether that means the underlying divergence is closed or just not currently exercised by this graph is not established either way — nothing here traced Leiden's node-visit order to check, and the previous holdout was itself graph-specific, not universal.
