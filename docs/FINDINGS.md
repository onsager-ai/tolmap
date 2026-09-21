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

> **Superseded by finding 12 (2026-09-19).** The relative-import fix changed the graph enough that both implementations converge on sqlalchemy too: 100.0% placement and 0.0000 modularity delta on all nine fixtures. The paragraph below records what was measured before that fix and is kept for the reasoning, not as a live gap. Whether the underlying divergence is closed or merely unexercised by the corrected graph was not established either way.

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

### Ruling, 2026-09-20: geometry is not a cross-implementation contract

Decided by the project owner after reading the table above. **Cross-implementation parity means membership, modularity, edges, landmarks, symbols and references. It does not, and will not, include coordinates.** `src/parity.rs` already enforces exactly that set, so this ruling changes no code -- it records that the omission is deliberate and closes the question rather than leaving it open as intended-but-unscheduled work.

`CLAUDE.md`'s "same repo at same commit must produce a byte-identical map" is therefore a statement about *one* implementation against itself, which the port satisfies (three runs, one hash, no `std::HashMap` in the source). It is not a claim that the Rust and Python maps are visually superimposable, and at 6+ districts they are not.

What this costs, stated plainly so nobody rediscovers it as a surprise: the reference stops being an oracle for geometry. A future change to the blend, the clustering or the layout can only be checked against the port's *own* prior output, and anyone who opens a reference map beside a port map of the same repository will see a differently-shaped picture. That is expected, and this paragraph is the thing to point at when it is noticed.

Reopening this means porting NumPy's MT19937 seeding and uniform-sampling routines, its array-fill order, and `spring_layout`'s force and cooling formulas in the same floating-point operation order. That work buys visual agreement with a frozen reference implementation that nothing will run after launch, which is why it was not scheduled.

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

## 13. Polyglot union extraction: co-change is not the only bridge, and the map can redraw the file extension

Step 1 (union extraction) merges any number of `(pkg, language)` sources before `finish_graph` runs co-change, semantic and proximity over the combined file set, instead of unioning two finished graphs (which would carry zero cross-language edges of any signal by construction). Step 2 measures what that merge actually produces, with `tolmap polyglot-report`, on two corpora: a small synthetic fixture built for this purpose (`eval/gen_synthetic_polyglot_fixture.py`, committed as `data/ci/synthetic_polyglot.graph.json`) and a real one that turned out to be free.

**`docs/ARCHITECTURE.md`'s claim is wrong as stated, on both corpora.** "Co-change becomes the only signal that bridges languages" is false: only `static` is architecturally zero cross-language (resolution only ever looks a target up in its own source's known-file set — see `extract::union_sources`). `proximity`, `semantic` and `cochange` all measurably cross, on both a hand-built fixture and a real 522-file repository:

| signal | synthetic fixture (50 files) | prometheus @ 296080c (522 files) |
|---|---|---|
| static | 0.0% (architectural) | 0.0% (architectural) |
| cochange | **100.0%** of its own raw mass | 3.5% |
| proximity | 0.0% | **0.06%** |
| semantic | **14.4%** | 0.6% |

**Superseded in part by finding 14**: the claim below that a repository above the 600-file semantic threshold loses cross-language semantic bridging entirely is false when two sources share a root, as codex and dify do. The condition is the sources' roots, not the file count. The rest of this finding stands.

Proximity's near-zero-but-not-zero figure on prometheus is itself informative, not noise: `docs/ARCHITECTURE.md` said proximity "does go to 0 across a `server/` + `web/` split by construction" — true when the two languages root at disjoint directories, as the synthetic fixture deliberately does (Go at `.`, TypeScript at `src/`). Prometheus's Go and TypeScript both root at `.` (the UI lives at `web/ui/...` alongside Go's own `web/api/...`), so a Go file and a TypeScript file occasionally do share a path prefix, and proximity crosses too, just barely. The mechanism is real; whether it fires depends on where the two languages' roots sit relative to each other, which is a property of the repository, not of the pipeline.

**Prometheus at its pinned commit (`data/fixtures.toml`) does carry a real TypeScript UI** — verified at the pin, not assumed: 100 `.ts` files under `web/ui/{react-app,mantine-ui,module}`, two of those three with their own `package.json`+`tsconfig.json`. `tolmap detect` on the checkout finds it as a second candidate (`ts at . (78 files, low confidence)` — no *root* `package.json`, so detection falls back to mapping the repository root directly rather than resolving the workspace; the 78 is `extract::source_files` under `pkg="."`, which is lower than 100 because the walk excludes `.d.ts`/`.test.ts`/`.spec.ts`). Both `ALL_SOURCES_MIN_FILES` (78 ≥ 25) and `ALL_SOURCES_MIN_SHARE` (78 of 522 detected files = 15% ≥ 5%) are cleared, so `--all-sources` selects it. This makes the existing go-only `data/prometheus.json` fixture a free monoglot-projection baseline: nothing about the merge was built to make this comparison come out any particular way.

**Below-prune-floor hazard, sized on both corpora — real, but repository-dependent, not universal.** Finding 10's mechanism (`blend()` divides every edge by the single largest blended edge in the *whole* graph; a language whose dominant edge is smaller than the other's can have its entire distribution pushed toward the floor) does reproduce across languages, but its size varies enormously with how balanced the two languages' own signal masses are:

| | synthetic fixture: go | synthetic fixture: ts | prometheus: go | prometheus: ts |
|---|---|---|---|---|
| below floor, single-source | 0.0% | 0.0% | 1.2% | 0.0% |
| below floor, merged | **25.0%** | 0.0% | 1.1% | 0.0% |

On the synthetic fixture (deliberately built with 25 near-edgeless Go filler files diluting a small real Go static-edge cluster) a quarter of Go's intra-language edges get pushed under the floor by merging in TypeScript. On prometheus, where Go's own static graph is large and dense (6,456 single-source candidate edges against TypeScript's 122), the merge barely moves Go's floor share at all (1.2% → 1.1%) and TypeScript's stays at 0% either way. **The hazard is real and worth reporting per merge, not a fixed cost of merging as such** — exactly finding 10's own conclusion, one level up: report it, do not assume it, and do not retune the floor from what one repository shows.

**NMI/adjusted Rand against the language label: near 1.0 means the map redrew the file extension, and it is repository-dependent whether that happens.**

| | synthetic fixture | prometheus |
|---|---|---|
| NMI | 0.878 | **0.279** |
| adjusted Rand | 0.920 | **0.112** |
| districts | 2 | 12 |
| files in a ≥90%-one-language district | 100.0% | 82.2% |

The synthetic fixture's high NMI is an artefact of its own construction, not a property of merging in general: 46 of its 50 files are deliberately edge-less filler (to clear `--all-sources`' floor without diluting the fixture's three deliberately-placed signals — see the generator's docstring), so `merge_tiny` has nothing to do but glom each language's filler back onto its own real cluster by directory. Prometheus, a real interconnected repository with no filler, lands far lower: 12 real districts, most of them still meaningfully mixed (82.2% in a ≥90%-one-language district means 17.8% of files sit in a district that is genuinely both languages). Modularity alone does not distinguish these cases — the synthetic fixture and prometheus both score respectably on q (0.16 vs 0.578) — which is exactly why `docs/ARCHITECTURE.md`'s revised polyglot paragraph does not use q as the polyglot acceptance signal.

**Projection drift is real and asymmetric, not zero.** Retention of a language's own single-source district assignment, measured against the merged map, by best-Jaccard match (the same matching `src/parity.rs` uses for placement):

| | synthetic fixture | prometheus |
|---|---|---|
| go retention | 76.0% | 85.1% |
| ts retention | 100.0% | **51.3%** |

Neither figure is zero, confirming the prediction in the spec this finding was written against: mass shares and the semantic IDF document frequency both change when a language is added (`semantic_vectors`' document-frequency denominator is the size of the *whole* parsed set), so the blend is not a superposition of two independent maps. On prometheus the smaller side (TypeScript, 78 files against Go's 444) drifts far more than the larger one — the same "smaller corpus is the less stable one" shape finding 9's jitter table and finding 3's churn numbers already established, now showing up as a consequence of *which* language is smaller in a merge, not of repository size alone.

**Per-language static_max (step 1 item 4) is exercised, not just asserted in isolation.** `single_language_static_max_matches_the_old_global_max` and `cross_language_static_max_does_not_let_one_language_drag_the_other` (`src/extract.rs`) cover the unit-level claim; on prometheus, Go's single-source candidate-edge count (6,456) against TypeScript's (122) is exactly the kind of imbalance a shared global max would have punished — TypeScript's own static edges are dense (many resolve to 1.0, single-file relative imports) while Go's `resolve_multi` spreads each import's weight across its target directory (`share = 1.0 / targets.len()`), so a shared max computed from Go's larger, more spread-out edge population would have systematically underweighted TypeScript's tighter one. Per-language buckets avoid that; this finding does not attempt to quantify what the old (hypothetical, since this repository was never mapped polyglot before) global-max behaviour would have produced, since nothing in the shipped pipeline ever computed it that way for a real merge.

**Determinism.** Three `tolmap build --all-sources` runs on the synthetic fixture, sha256sum compared: byte-identical (`ac19f64...`, all three). This is the check finding 10's closing note records for the single-language path, now run for the merge; it is also a CI job (`gate`, `.github/workflows/ci.yml`), not just a one-time measurement, along with the ceilings this finding's synthetic-fixture numbers set (`eval/check_polyglot_ceilings.py`: NMI ≤ 0.95, below-floor share ≤ 0.5 per language).

**What this does and does not settle.** This is a measurement, not a retuning — `ALPHA`/`BETA`/`GAMMA`/`DELTA`, the 0.02 prune floor, `keep_per_node=14`, and `ALL_SOURCES_MIN_FILES`/`ALL_SOURCES_MIN_SHARE` (25 files, 5% share) are exactly as shipped in this change, per `CLAUDE.md`. Two corpora is not a corpus-wide sweep the way findings 5/10/12 ran across all nine fixtures — prometheus is the only real polyglot repository measured here, because it is the only one of the nine already pinned that turned out to qualify (`data/fixtures.toml`'s other eight are single-language at their pins, unverified further here). Whether the below-floor hazard or the projection-drift asymmetry generalise beyond these two repositories, and what floor/threshold changes (if any) they would justify, is the next measurement, not this one.

## 14. Three more real polyglot repositories: the corpus triples, and two of finding 13's claims do not survive it

Finding 13 measured union extraction on one real polyglot repository (prometheus) and one synthetic fixture, and said plainly that whether its results generalised was the next measurement. This is that measurement: three more real repositories, chosen because they are polyglot in production rather than because they were convenient — crawlab (Go backend + TypeScript frontend), openai/codex (a Rust codebase whose tooling is TypeScript + Python) and dify (Python backend + TypeScript/React frontend).

Pins, and what `--all-sources` selected at them:

| repo | commit | sources selected | files mapped |
|---|---|---|---|
| crawlab | `ee11cd7` (branch `develop`) | `go at core` (219), `ts at .` (356) | 575 |
| openai/codex | `5c5308f` | `py at .` (146), `ts at .` (741) | 887 |
| langgenius/dify | `2590d90` | `py at .` (1,978), `ts at .` (4,355) | 6,333 |

All three were measured after `.tsx` collection landed (issue #35), so the TypeScript side includes JSX components; before that change dify mapped 1,554 TypeScript files instead of 4,355 and its numbers were measured against a frontend with its components missing.

**What these repositories are *not*.** Each of the three has a large body of code tolmap cannot see, and reading the numbers below without that in hand would overstate what was measured. codex is 4,631 `.rs` files — the actual product — of which tolmap maps none, so "codex" here means its tooling, not codex. crawlab has 321 `.vue` files and n8n 1,305, none collected (issue #35 covers `.tsx` only). The maps are honest about what they contain, but they are maps of a subset, and `CLAUDE.md`'s rule that numbers must be a lower bound applies to this finding as much as to the reference graph.

### Districts do not redraw the file extension, and the effect strengthens with size

| | synthetic (f13) | prometheus (f13) | crawlab | codex | dify |
|---|---|---|---|---|---|
| files | 50 | 522 | 575 | 887 | 6,333 |
| NMI vs language | 0.878 | 0.279 | 0.386 | 0.258 | **0.245** |
| adjusted Rand | 0.920 | 0.112 | 0.140 | 0.082 | **0.046** |
| files in a >=90%-one-language district | 100% | 82.2% | 100% | 98.5% | 91.5% |

Finding 13's headline result holds on all three: NMI against the language label sits far below 1, so the partition is finding structure rather than reproducing the file extension. The synthetic fixture's 0.878 remains an artefact of its own filler-heavy construction, now with three more real corpora saying so.

The purity column is the one that complicates the story. crawlab's districts are **100% language-pure** — every district is >=90% one language — while its NMI is a middling 0.386. Those are not in tension: crawlab's map has several districts per language and never one that mixes them, which is exactly what a Go backend and a TypeScript frontend communicating over HTTP should look like to a pipeline whose static resolver cannot cross languages.

> **Falsified in part by finding 15 (2026-09-21).** This paragraph originally concluded: "There are no shared imports to find, and the map correctly does not invent any." The cross-language half survives — Go imports do not name TypeScript files — but the clean-looking Go districts were not evidence for it. crawlab's five `go.mod` files are all nested, while the resolver read only a root `go.mod`; the Go half therefore had **zero import edges, including Go-to-Go**, and its apparent structure came entirely from the other signals. Finding 15 replaces that evidence with the corrected measurement: 4,654 kept edges in the 219-file Go-only map, while the static resolver remains incapable of crossing languages.

### `docs/ARCHITECTURE.md`'s size condition is wrong: it is about roots, not about 600 files

Finding 13 recorded that above 600 files the semantic candidate sweep is restricted to same-directory pairs, and concluded that "in any polyglot repo large enough to cross this threshold, the semantic candidate sweep stops proposing cross-language pairs at all", leaving co-change as the only bridge. That is **falsified** by codex (887 files) and dify (6,333 files), both of which are above the threshold and both of which still show semantic crossing languages:

| signal, share of its own mass crossing languages | crawlab (575) | codex (887) | dify (6,333) |
|---|---|---|---|
| static | 0.0% | 0.0% | 0.0% |
| cochange | 3.0% | **9.8%** | 3.9% |
| proximity | 0.0% | 0.0% | 0.0% |
| semantic | 0.6% | **0.3%** | **0.2%** |

The mechanism finding 13 described is right; the condition it attached is wrong. Restricting the sweep to same-directory pairs only kills cross-language bridging when a directory cannot span two languages — which is true when each source roots at its own `pkg` (prometheus's Go at `.` versus a UI under `web/ui`, or the synthetic fixture's deliberate `.`/`src` split), and false when two sources share a root. codex and dify both select `py at .` and `ts at .`, so their directories are full of Python and TypeScript files side by side, and the same-directory sweep proposes cross-language pairs freely. `static` remains the only architecturally-zero signal, exactly as finding 13 established.

The practical consequence is that "does semantic bridge languages here" is answered by the repository's layout, not by its size — and the layout is visible from `tolmap detect` output before anything is indexed.

### The below-floor hazard reverses direction on codex

Finding 10's hazard — `blend()` dividing every edge by the largest blended edge in the whole graph, so a language with a smaller dominant edge gets pushed toward the prune floor — was measured by finding 13 as real but repository-dependent. These three add a case it did not predict.

| | below floor, single-source | below floor, merged |
|---|---|---|
| crawlab go | 0.0% | 2.7% |
| crawlab ts | 0.0% | 0.7% |
| **codex py** | **88.9%** | **76.4%** |
| **codex ts** | **79.9%** | **27.9%** |
| dify py | 71.9% | **0.0%** |
| dify ts | 0.0% | 0.0% |

On codex and dify, **merging improves both languages' below-floor share**, in dify's case from 71.9% to zero. The hazard is not "merging pushes a language under the floor"; it is "the floor is relative to the largest edge in whatever graph is being blended", and merging can move that maximum in either direction. A language whose own single-source graph is dominated by one very heavy edge (codex's Python tooling, dify's Python backend) has most of its own distribution under the floor *before* any merge; adding a second language with a heavier edge population does not make that worse, it can make it better by changing which edge sets the scale.

This does not overturn finding 10, which is about cross-repository comparison. It does mean the merged-versus-single comparison cannot be summarised as a hazard in one direction, and reporting it per merge — which `polyglot-report` does — is the right response rather than tuning the floor.

### Projection drift does not track which language is smaller

Finding 13 observed on prometheus that the smaller language drifted more (TypeScript 78 files, 51.3% retention; Go 444 files, 85.1%) and connected it to the "smaller corpus is less stable" shape of findings 3 and 9. That does not hold:

| | smaller side | retention | larger side | retention |
|---|---|---|---|---|
| prometheus (f13) | ts (78) | 51.3% | go (444) | 85.1% |
| crawlab | go (219) | 62.6% | ts (356) | 81.5% |
| dify | py (1,978) | 40.5% | ts (4,355) | 85.5% |
| **codex** | **py (146)** | **76.0%** | **ts (741)** | **48.0%** |

Three of four match the pattern; codex inverts it, with the larger language drifting nearly twice as much as the smaller. So size is not the variable. What codex has that the others do not is a Python side that is almost entirely isolated (9 cross-language candidate edges in total, all 9 kept) attached to a TypeScript side whose own graph is weakly connected — 79.9% of its single-source edges below the floor. A language whose single-source partition was already marginal has little to retain, regardless of how many files it has.

Nothing here is a reason to change a parameter. It is a reason not to predict drift from file counts, which is what finding 13's phrasing invited.

### `.tsx` collection changed dify's numbers materially

dify is the one repository in this corpus measured both before and after issue #35, which is worth recording because it shows how much a collection gap distorts a polyglot measurement rather than merely shrinking it:

| dify | before `.tsx` | after `.tsx` |
|---|---|---|
| files | 3,532 | 6,333 |
| candidate edges | 9,077 | 17,744 |
| py+ts candidate cross-language edges | 284 (198 kept) | 294 (221 kept) |
| NMI vs language | 0.254 | 0.245 |
| files in a >=90%-one-language district | 84.1% | **91.5%** |

Adding 2,801 React components made districts *more* language-pure, not less. The components attach to the TypeScript files that import them, thickening the TypeScript side's own structure faster than they add cross-language links — the frontend had been represented by the sliver of it that happened to be plain `.ts`, and that sliver sat closer to the Python side than the real frontend does.

### What this settles and what it does not

Settled: finding 13's central claim survives three more real repositories, and two of its secondary claims do not — the 600-file condition on semantic bridging is really a condition on source roots, and the below-floor hazard has no fixed direction.

Not settled: still nothing about a repository with three or more languages (every corpus here is a pair), nothing about Rust, Java or C++ (tolmap parses none of them), and nothing about `.vue`. Parameters are untouched — `ALPHA`/`BETA`/`GAMMA`/`DELTA`, the 0.02 prune floor, `keep_per_node=14`, `ALL_SOURCES_MIN_FILES`/`ALL_SOURCES_MIN_SHARE` and resolution 1.1 are exactly as shipped, per `CLAUDE.md`. This finding measures; it does not retune.

## 15. Internal module names were invisible, and high modularity was the symptom

The multi-language resolver made two versions of the same assumption. TypeScript returned immediately for every specifier not beginning with `.`, dropping tsconfig `paths` aliases and workspace package names. Go read one `go.mod` at the repository root, so a repository made entirely of nested modules had no module path at all. crawlab is exactly that repository: five `go.mod` files, all below the root, and all 347 of its internal Go imports were invisible. Independent parsing of the source put the missed share at 23.7% for the vue fixture, 48.6% for n8n, 63.5% for crawlab's TypeScript, 64.9% for dify, and 100% for crawlab's Go. codex, at 0.2%, is the control.

**The fix is a repository metadata index, not a heuristic.** One sorted walk, with the same skip directories as source collection, records `(module path, directory)` from every `go.mod`, and originally recorded one repository-global `(specifier prefix, directory)` table from every tsconfig `paths` entry and `package.json` name. Resolution tried longest prefixes first with a lexicographic tie-break, required equality or a `/` boundary, and accepted a candidate only when that exact file was already in the parsed set. `"@/*"` therefore registered `"@/"`, not `"@"`; an unresolved alias still contributed zero edges. TypeScript probed the relative resolver's existing candidates plus `index.tsx` and `src/index.ts`. Go kept the existing one-import-across-the-package weighting (`1/|D|`).

`extends` is deliberately not followed. It can point into `node_modules`, which would make the map depend on whether dependencies happen to be installed rather than on the commit. JSONC is handled by a character scanner that preserves string literals and removes trailing commas: a regex sees the `/*` inside `"@/*"`, then the `*/` inside `"**/*.ts"`, and silently deletes the paths object between them. dify's `web/tsconfig.json` is the real file that falsified the regex version of this work.

**The repository-global TypeScript table was then falsified.** A two-package synthetic repository gave both packages the same `@/*` alias. Rust sent `pkgA/src/main.ts` to `pkgB/src/target.ts`, while Python sent it to `pkgA/src/target.ts`: Rust's `stack.pop()` directory walk and Python's `os.walk` run in opposite orders, and the table's `(-prefix length, prefix)` sort did not distinguish duplicate prefixes. The oracle therefore agreed only when repositories happened not to contain a collision.

n8n made the error measurable rather than merely synthetic. The first table had 177 prefixes, 55 of them duplicates; 203 of 19,558 alias-resolved imports (1.0%) chose a different package than the governing tsconfig. In `packages/cli/src/active-workflow-manager.ts`, `@/constants` incorrectly reached `packages/@n8n/ai-workflow-builder.ee/src/constants.ts`, and `@/node-types` reached `packages/@n8n/task-runner/src/node-types.ts`; both belong under `packages/cli/src/`.

The replacement retains each alias's declaring tsconfig directory as a scope, distinct from its target. Workspace `package.json` names have the empty global scope. For each importing file, governing ancestors rank before non-ancestors, deeper ancestors rank first, then longer prefix, prefix text and target directory break ties. The ancestor test is path-segment-wise, so `packages/cli` does not govern `packages/cli-extra`. Non-ancestor aliases remain as the final fallback, preserving repositories whose declaration lies outside the importer's subtree. Both implementations spell out that complete ordering rather than inheriting their different walk orders. An independent audit now reports `differ=0` on all four repositories:

| repo | alias-resolved imports | differ |
|---|---:|---:|
| crawlab | 346 | 0 |
| codex | 4 | 0 |
| dify | 2,782 | 0 |
| n8n | 19,558 | 0 |

Both implementations carry the same correction. Rust has hand-counted unit cases for a paths alias (one edge), a workspace package name (one), a nested Go module whose import spreads across two files (two), JSONC comments plus trailing commas (one), and a nonexistent alias target (zero). The frozen Python reference has no test suite; its coverage is the nine-fixture re-derivation below, following finding 12's correctness-oracle precedent rather than inventing a second test harness.

Three more Rust cases cover the scoped correction: two packages with the same alias produce two within-package edges and no crossing edge; a nearer tsconfig alias beats a global workspace package name; and a non-ancestor alias remains usable when nothing nearer resolves. The generated collision fixture is also checked through both implementations, whose complete static edge sets are identical at 51 edges.

### Four production repositories

Same pinned clones, same binary otherwise, `--all-sources --no-parcels`; `eval/mapstats.py` derives the shape columns from the compact map. A mainland holds at least 1% of the files, issue #41 measurement 1's reporting threshold, not a pipeline parameter.

| repo | districts | mainland | mainland file share | small | zero-edge districts | zero-edge files | small with no mainland edge | kept edges | q | largest district |
|---|---|---|---|---|---|---|---|---|---|---|
| crawlab | 19 → 11 | 16 → 10 | 98.3% → 99.8% | 3 → 1 | 9 → 3 | 43.7% → 6.4% | 3/3 → 1/1 | 259 → 5,363 | .7420 → .5964 | 83 → 106 |
| codex | 34 → 31 | 20 → 19 | 94.9% → 96.1% | 14 → 12 | 14 → 14 | 7.8% → 7.8% | 12/14 → 11/12 | 1,748 → 1,751 | .4819 → .4858 | 227 → 227 |
| dify | 226 → 136 | 26 → 19 | 76.9% → 87.3% | 200 → 117 | 121 → 92 | 7.9% → 7.0% | 188/200 → 105/117 | 6,762 → 17,147 | .8714 → .7463 | 600 → 840 |
| n8n | 369 → 85 | 26 → 15 | 62.4% → 85.8% | 343 → 70 | 68 → 18 | 2.1% → 0.9% | 313/343 → 32/70 | 21,047 → 38,543 | .9176 → .7476 | 560 → 3,683 |

Scoping changed only n8n relative to the first corrected table: districts 84 → 85, mainland file share 86.6% → 85.8%, small districts 69 → 70, stranded small districts 32/69 → 32/70, Q .7460 → .7476, and the largest district 3,726 → 3,683. Mainland count, both zero-edge columns and the 38,543 kept edges stayed fixed. Every column for crawlab, codex and dify stayed fixed.

codex is the useful negative control: only three more kept edges, the same 227-file largest district, and a 0.0039 Q movement. A broad package-name matcher would have moved it much more; the boundary and parsed-file checks are doing real work.

The clearest single-language case is crawlab's 219-file `core` source:

| crawlab Go only | before | after |
|---|---:|---:|
| districts | 10 | 7 |
| kept edges | 0 | 4,654 |
| mainland file share | 100.0% | 100.0% |
| small districts | 0 | 0 |
| zero-edge districts | 10 | 1 |
| zero-edge files | 100.0% | 13.2% |
| q | .5339 | .3334 |

This corrects finding 14's evidence, not its cross-language mechanism. A Go import still cannot point at a TypeScript file. What looked like clean Go-side architecture was a graph with no Go import edges at all, including Go-to-Go; co-change, proximity and semantics had been carrying the partition alone.

**Q falls because the graph stops being shattered.** A collection of weakly connected islands has trivially high modularity: almost every retained edge stays inside the island Leiden already made a community. n8n's .9176 here — and especially the .976 carried by the earlier brief below — was a symptom of missing edges, not a quality signal. The resolver adds no tuning parameter and leaves all coefficients, the 0.02 floor, `keep_per_node=14`, resolution 1.1 and the all-source thresholds unchanged.

### Determinism, performance and the synthetic failure case

Two consecutive dify builds were byte-identical (`e501fe5c…`), and CI's three-run synthetic-polyglot check remained byte-identical (`ac19f644…`) with its NMI and below-floor ceilings green. The generated TypeScript fixture now has 53 source files and 51 alias imports. Its original 51-file package is unchanged: all 50 internal imports still use `@/…`, with no relative import or alternate signal to rescue the graph, so the preserved `origin/main` binary still fails with `Error: cannot blend an empty graph`. A second two-file package declares the same alias and exercises the collision; both resolvers produce the same 51 static edges, including its within-package edge. CI generates the repository locally and builds it from source, because a pre-extracted graph would bypass the resolver under test.

Wall-clock on this run did not reproduce the supplied performance prediction:

| repo | before | after |
|---|---:|---:|
| crawlab | 0.83s | 0.75s |
| codex | 1.22s | 1.20s |
| dify | 10.92s | 10.09s |
| n8n | **19.21s** | **25.09s** |

The supplied run measured dify 10.4s → 10.4s and n8n 27.1s → 25.8s; those n8n timings do not reproduce here. Alternating repeats confirmed 19.05s/19.63s before against 25.26s/25.74s after, while graph extraction alone was flat (11.42s → 11.44s), locating this run's difference downstream in partition/geometry on the corrected graph. Nothing in clustering or layout was changed to chase it; doing so would violate this fix's scope and make the graph measurement no longer isolated.

### The oracle changed once, and only where predicted

After seeding names from the committed fixtures, seven Python fixtures and prometheus reproduced byte-for-byte. prometheus has a single root `go.mod`, so nested-module lookup is a no-op. vue alone changed, exactly as the Rust-side falsification predicted: 9 → 8 districts, q .5729 → .5389, 932 → 1,186 kept edges, and largest district 48 → 53. `@vue/shared`, imported across the workspace, no longer sits behind an unresolved package name; `server-renderer` joins `runtime-dom`.

Changed memberships miss the old fingerprint cache by design. The three exact memberships (`sfc compiler`, `ssr compiler`, `test runtime`) kept their names. Every actual rename, matched by best member overlap, was:

| old | new |
|---|---|
| runtime core | runtime-core |
| compiler transforms | compiler-core |
| shared & dom runtime | runtime-dom & server-renderer |
| v2 compat | compat & runtime-core |
| server renderer | runtime-dom & server-renderer |

`reactivity` also changed membership (20 → 21 files) but the deterministic fallback chose the same text, so it is not a rename. No name was hand-edited. `eval/verify_fixtures.py` then reproduced all nine files byte-for-byte, and Rust parity against them was 100.0% placement, 0.0000 Q delta, with `F`/`E`/`L`/`S`/`U` identical on every fixture.

The scope-aware follow-up replayed that procedure and all nine committed fixtures were byte-identical. In particular, vue did not move a second time: it has no duplicate prefix, so the fixture already recorded by this finding remains the correct oracle.

### The earlier brief's partition table remains unexplained

The task brief carried a different baseline — crawlab 34 districts / q .853, codex 34 / .611, dify 454 / .943, n8n 489 / .976 — and after-counts 99 / 32 / 240 / 102. Neither side reproduces on this tree. The graph half does: kept-edge counts match that brief exactly on all four repositories, before and after (259→5,363; 1,748→1,751; 6,762→17,147; 21,047→38,543), and source selection/file counts are identical (575 / 887 / 6,335 / 11,982). The divergence is downstream in partitioning.

Already ruled out: `merge_tiny`'s minimum (setting it to 1 produces 54/138/1,405/1,996), Leiden resolution (raising it adds districts while lowering Q, opposite the brief), warm start (the CLI is cold-start only and repeated builds are byte-identical), source selection, and the pinned library versions. This tree reproduces the committed corpus exactly on all nine fixtures, so the table above is the result consistent with `data/`. Nothing was tuned to close the unexplained gap.

## 16. Oversized districts now expose measured terrain, and two path claims in the proposal were too strong

Terrain subdivision from `docs/TERRAIN.md` is implemented in Rust behind `tolmap build --terrain`. The top-level partition is not rerun or edited: an eligible district (more than `2√N/0.517` files and at least 50 files) is decomposed on the weighted, pruned partition graph into uncapped Tarjan articulation points, small connected parcels and recursive Leiden organic components. The 0.517 constant is stored once, not derived from `data/` at runtime. Arterials leave only this second clustering graph; their files, map edges, landmarks and blast radius remain.

The schema carries one optional `terrain` map. Each eligible district stores ordered arterial file indices and their in-district links, organic member indices/suffix/centre/contours, parcel address/member indices/rectangle, and the suffix high-water mark. With the field absent, the old JSON serialization is unchanged. The cold CLI assigns suffixes by descending size then smallest path. The service already had the previous `MapDocument` in hand for its top-level warm start, so, when terrain is enabled, it matches both the parent district and its sub-districts through `src/parity.rs`'s existing greedy best-Jaccard matcher at overlap **at least 0.35**; unmatched suffixes advance the persisted high-water mark and retired suffixes are not reused. The acceptance gate remains byte-for-byte on its original Python-mirroring boundary (`j < 0.35` is skipped, so exactly 0.35 matches); terrain reuses that existing semantics, with a boundary unit test. `TOLMAP_TERRAIN` is an explicit service opt-in and defaults to false. It is intentionally absent from `fly.toml`, `railway.json` and `deploy/railway.staging.env`, so merging this work changes no hosted map until an owner enables it; the warm-path plumbing remains dormant while it is off.

The layout deliberately starts by completing the old layout and inter-district relaxation. Terrain then replaces points only inside eligible districts, fitted back into that district's old centre and radius; top-level contours and every ineligible node remain unchanged. Disconnected organic/plat regions use the existing deterministic packer, organic files retain the force layout within their group, parcels use alphabetical row-major shelves whose rectangle area is exactly proportional to file count, and arterial files iterate to the weighted centroid of their in-district neighbours. The viewer renders these as inner contours, a higher-zoom parcel grid and intra-district roads. All three new selection shapes use `MapRenderer`'s one delegated `data-k` pointer path, including coarse-pointer hit widening; there is no second touch handler.

### Default and top-level invariants

The feature binary with the flag off and a separate binary built from `origin/main` produced the same SHA-256 on all thirteen pinned inputs:

| map | flag-off SHA-256 (feature = `origin/main`) |
|---|---|
| celery | `ca2a8caddaddd82105a7fae6cf8a7d9b9d0f736f5cdf3566bf423ba77d68bffc` |
| django | `3707caf2824bffd31939e4b25f91a3dcc82a6d324c8bdb2a2f69dfeec3c3ab86` |
| flask | `364492da14a7c2e9b2a58d6fce01058298eff765f640fe4b824695e2c421e391` |
| httpx | `4d7d33da6555b5fd07c870ad81510f6f15870c0f7feafad24a1387dfdd166529` |
| prometheus | `32a73d552a62d068d69f1907a182add1fa3cc076a4f6f243361850788e6a35c6` |
| rich | `34fd4793ba8dee3e96505020a0be5bd03298396122ea167eeceaa28030f6d82d` |
| scrapy | `9e7b18b8bec1f8506e0ad01895eb0b96102336e788748941b8df6cbbc826e0b4` |
| sqlalchemy | `26fcd5d015fe6119c18a038c6a5d7fdf16054c8431875c6d62622f50e250289d` |
| vue | `0c644e6cefdcfaa2fa5fe79326d6f576ebd6bcb7355c3a19473d19917e43dce2` |
| crawlab | `3aabf8c10b045b7fbac48e2651ac4d61226997b4ad6442e877850c19c4be9d8d` |
| codex | `2ba1f647e9d986a4732dd9b7c7d7aab7a0bd4ae756e13acce0c3b5686d41e5e1` |
| dify | `e501fe5c14d0cbb688174fda41c6fe1c3ef8de6ad06140a6548171b3ab119aa9` |
| n8n | `af37bd49d715bc6258830eec5e7eb9122c1949da9b2941cffbb843da9b216555` |

Offline parity on each acceptance fixture was **100.0% placement / 0.0000 Q delta**, with `F`, `E`, `L`, `S` and `U` identical on all nine. With the flag on, all thirteen retained identical membership, district count/names, Q, `F/E/L/S/U`, inter-district roads and top-level district objects. Every node row outside an eligible district was also byte-identical. Eligible-district counts were: django 2, prometheus 2, crawlab 2, codex 1, dify 8, n8n 7, and zero for celery/flask/httpx/rich/scrapy/sqlalchemy/vue.

### The spike, reproduced without its 64-articulation cap

The uncapped Rust articulation search chose no different arterial. All 22 districts match `/tmp/tolmap-refs/s3/spike-expected.json` on arterial paths and order, parcel count/file count, and organic sizes:

| map/district | spike arterials | Rust | spike parcels/files | Rust | spike organic sizes | Rust |
|---|---:|---:|---:|---:|---|---|
| django d0 | 0 | 0 | 171 / 171 | 171 / 171 | 31 | 31 |
| django d1 | 0 | 0 | 10 / 10 | 10 / 10 | 45,32,25,20,17 | 45,32,25,20,17 |
| prometheus d0 | 0 | 0 | 0 / 0 | 0 / 0 | 49,36,24,22 | 49,36,24,22 |
| prometheus d1 | 0 | 0 | 0 / 0 | 0 / 0 | 44,25,20,14 | 44,25,20,14 |
| crawlab d0 | 0 | 0 | 0 / 0 | 0 / 0 | 57,35,14 | 57,35,14 |
| crawlab d1 | 0 | 0 | 2 / 2 | 2 / 2 | 31,27,23,21 | 31,27,23,21 |
| codex d0 | 1 | 1 | 81 / 136 | 81 / 136 | 34,31,25 | 34,31,25 |
| dify d0 | 0 | 0 | 2 / 2 | 2 / 2 | 202,116,105,88,80,69,64,63,51 | 202,116,105,88,80,69,64,63,51 |
| dify d1 | 0 | 0 | 3 / 4 | 3 / 4 | 230,94,93,73,57 | 230,94,93,73,57 |
| dify d2 | 0 | 0 | 3 / 3 | 3 / 3 | 229,82,81,57,50 | 229,82,81,57,50 |
| dify d3 | 0 | 0 | 3 / 3 | 3 / 3 | 212,101,65,50,48 | 212,101,65,50,48 |
| dify d4 | 0 | 0 | 0 / 0 | 0 / 0 | 166,100,88,78 | 166,100,88,78 |
| dify d5 | 0 | 0 | 172 / 177 | 172 / 177 | 202 | 202 |
| dify d6 | 0 | 0 | 71 / 76 | 71 / 76 | 240 | 240 |
| dify d7 | 0 | 0 | 149 / 167 | 149 / 167 | 141 | 141 |
| n8n d0 | 8 | 8 | 834 / 2,430 | 834 / 2,430 | 369,345,178,110,92,77,74 | 369,345,178,110,92,77,74 |
| n8n d1 | 0 | 0 | 6 / 8 | 6 / 8 | 398,239,164,140,112,105,95 | 398,239,164,140,112,105,95 |
| n8n d2 | 0 | 0 | 41 / 42 | 41 / 42 | 125,123,122,118,116,98,96,80,79,75 | 125,123,122,118,116,98,96,80,79,75 |
| n8n d3 | 1 | 1 | 19 / 39 | 19 / 39 | 308,162,126 | 308,162,126 |
| n8n d4 | 0 | 0 | 8 / 8 | 8 / 8 | 172,105,98,86,70,65 | 172,105,98,86,70,65 |
| n8n d5 | 0 | 0 | 2 / 3 | 2 / 3 | 175,157,125,69 | 175,157,125,69 |
| n8n d6 | 1 | 1 | 2 / 2 | 2 / 2 | 370,77 | 370,77 |

That is **11 arterials**, **94/94 organic sub-districts in band**, and **94/94 connected** in the weighted partition graph, exactly the spike's predictions. No acceptance-fixture district acquired an arterial (measured 0). Rich d0 is 41 files and remained ineligible (measured zero terrain districts), despite clearing the relative `hi` line.

The named arterial checks also reproduce. n8n d0 starts with `packages/workflow/src/index.ts`. Codex d0 contains `codex-rs/app-server-protocol/schema/typescript/v2/index.ts`; 136/227 files, **59.9%**, became parcels against the spike's rounded 60%.

Two stronger path descriptions in the proposal do **not** reproduce literally, even though the component counts above do. Django d0 has the predicted 31-file organic sub-district and 171 one-file parcels, but **170/171**, not 171/171, are under `django/conf/locale/<lang>/`; the remaining parcel is `django/core/mail/backends/__init__.py`. In n8n d0, the predicted **827/834 (99.2%)** is reproducible only as “every member shares one third-level package directory”; only **277/834 (33.2%)** parcels are literally contained in one `packages/nodes-base/nodes/<Vendor>/` directory. Many of the others are singleton credentials under `packages/nodes-base/credentials/`, so no renderer or address rule can truthfully move them under `nodes/<Vendor>` without changing membership by path. These are discrepancies in the proposal's prose, not an uncapped-search or Rust/Python decomposition difference, and neither was tuned away.

### Compactness, determinism and cost

The first terrain layout reused the top-level `0.62` within-group spread. It failed the compactness falsification on django (sub-district median 0.1331 vs lowest top-level 0.4047), prometheus (0.1291 vs 0.5235) and crawlab (0.1263 vs 0.4679): separately contoured siblings overlapped and became ribbons. Reducing only the within-subdistrict footprint to 0.12 of the existing normalized group spacing, while keeping the same force positions and file-count scaling, produced the final emitted-contour measurements:

| map | spike prediction | median sub-district PP | lowest top-level PP | result |
|---|---|---:|---:|---|
| django | median ≥ lowest top-level | 0.6793 | 0.4047 | pass |
| prometheus | same | 0.6833 | 0.5235 | pass |
| rich | no eligible district | n/a | 0.1719 | pass |
| crawlab | median ≥ lowest top-level | 0.6442 | 0.4679 | pass |
| codex | same | 0.6708 | 0.0000 | pass |
| dify | same | 0.7507 | 0.0000 | pass |
| n8n | same | 0.7606 | 0.0000 | pass |

Two cold terrain builds were byte-identical for dify (`85d63a9cfddf8c1bde8659dd5c4d71810f3da3ea7caa7e032f91d8f3583d5a2b`) and n8n (`9f5fd4663f1522f0ac910530bb16d81bd0be1bed3e4225b00a16f9b64f18f683`). Three `--all-sources --terrain` builds of the generated synthetic polyglot fixture were also identical (`5b16ccccdf34dba4d4172b91056820db6bb3bd692ab74abbb3e07f044f2bf6e3`), and the three-run check is now in CI.

Single alternating wall-clock builds on this host, including extraction:

| map | flag off | `--terrain` | delta |
|---|---:|---:|---:|
| crawlab | 0.81s | 0.92s | +0.11s / 13.6% |
| codex | 1.21s | 1.31s | +0.10s / 8.3% |
| dify | 11.30s | 12.14s | +0.84s / 7.4% |
| n8n | 32.82s | 34.47s | +1.65s / 5.0% |

`cargo fmt --check`, `cargo test --release` (115 tests across all targets), generated bindings `--check`, parity on all nine fixtures, and the web production build passed. Every parity run reported 100.0% placement, 0.0000 modularity delta and identical `F/E/L/S/U`. The service was not run end to end in this follow-up; the configuration unit test instead proves `TOLMAP_TERRAIN` is false when unset or invalid and true only on an explicit valid opt-in, while `jobs.rs` passes that resolved value directly as `BuildFeatures::terrain`. Deployment-env parity also passed without adding the setting to either hosted configuration. Clippy reported only the three warnings already present at HEAD. Web lint reported only its pre-existing `useJobProgress.ts` warning. A phone was not available: pinch bookkeeping, the six-pixel tap/drag threshold, delegated selection and coarse-pointer hit strokes were checked in code and through type/build/lint, but actual sub-district, parcel and arterial taps on a phone remain owed after this run.

## 17. A map with 365 districts is not a map: the tail splits in two, and only one half is places

Finding 14's corpus made the scale problem impossible to keep ignoring. n8n's map had **365 districts**, dify's 216, at the commit finding 14 measured. Nothing in the pipeline was wrong — modularity was fine, placement parity was 100%, the partition was finding real structure — and the artefact was still unusable, because a legend with 365 entries is a list, not a map. This finding records what was measured about that tail and what was done about it (issue #34); it does not claim to have solved it (issue #41 is the open design brief).

**Re-measured after rebasing onto `main` at `cb04469`** (finding 15's module-resolution fix, #43's edgeless-partition guard and #47's opt-in terrain subdivision). Finding 15 landed after this branch was first written and, on its own, took n8n from 369 districts to 85 and dify from 226 to 136 by recovering import edges the old resolver dropped — so every count below that depends on district totals is now different from what shipped originally, even though nothing in *this* finding's mechanism changed. Classification and placement still only read the partition; membership itself is verified byte-identical between a `main` binary (no islands) and this one on all four repositories below (same `F`, `E`, `q`), so the movement in district counts is entirely finding 15's, not this feature's.

### One percent of the files is the line

| repo | files | districts | mainland | island | unconnected |
|---|---|---|---|---|---|
| crawlab | 575 | 11 | 10 | 0 | 1 |
| prometheus | 444 | 11 | 8 | 2 | 1 |
| dify | 6,335 | 136 | 19 | 26 | 91 |
| n8n | 11,982 | 85 | 15 | 52 | 18 |

A district holding at least 1% of the repo's files is **mainland**. Before finding 15, the mainland column was 16, 27, 28 — roughly flat, and roughly increasing with repo size, across the 21x range in file count. It no longer is: 10, 19, 15 — n8n now has *fewer* mainland districts than dify despite having almost twice the files, because finding 15 concentrated n8n's structure into a few very large communities (its largest single district is 3,683 files, 30.7% of the repository — finding 15) rather than spreading it across many mid-sized ones. The share threshold is still doing real work (1% of crawlab is about 6 files, 1% of n8n about 120, and mainland's *file* share — 99.8% / 87.3% / 85.8%, matching finding 15's own column — stays high everywhere), but "the mainland count stays roughly constant across the corpus" is weaker evidence for 1% specifically than it was before finding 15 shrank the tail; see "What is not settled" below.

The share is an integer percentage compared as `size * 100 >= total * 1`, not an `f64` against `0.01`, so a district sitting exactly on the boundary classifies the same way on every platform rather than depending on how the literal rounds.

### Below the line there are two different things, and one of them is not a place

The districts below 1% are not one population. Splitting them by whether the district holds any file incident to a resolved import edge:

| | below 1% | with an import edge (**island**) | with none (**unconnected**) | files in the unconnected group |
|---|---|---|---|---|
| crawlab | 1 | 0 | 1 | 1 |
| dify | 117 | 26 | 91 | 433 |
| n8n | 70 | 52 | 18 | 105 |

An island is small but real: it is connected to the repository, it just is not big enough to be one of its landmarks. An unconnected district is a group of files with no drawn edge to anything — configuration, generated code, fixtures, scripts. Drawing a region around those is the map asserting a place where there is only a residue, and it accounts for 91 of dify's 136 districts on its own. Unconnected districts therefore get **no region**: `blobs::contours`'s polygon is dropped and the viewer lists them instead of drawing them. Their files keep a defined, deterministic point and a `NodeRow`, as the schema requires.

**The edge set that decides this is `imports`, not the blended graph.** Classifying against `layout.graph` — the blended, pruned, multi-signal graph the partitioner actually runs on — was tried first, on a literal reading of "kept edge", and did not reproduce either corpus at the time it was tried: a file with no import at all can still carry co-change, proximity and semantic mass. `imports` is also the edge set the viewer draws as `E`, so "unconnected" ends up meaning what a person looking at the map would take it to mean: nothing is drawn from it to anything. (The specific before/after numbers this comparison originally produced were measured against finding 14's corpus and are not re-run here; the mechanism -- `imports` over `layout.graph` -- did not change in this rebase.)

### Three rules that were measured and rejected before this one

- **Cap the district count by adapting the resolution.** Rejected: it changes membership, which breaks the acceptance gate by construction and makes the same repository partition differently depending on its size.
- **Anchor each small district to the mainland district it is adjacent to.** Impossible, not merely unhelpful: re-measured on the current corpus (same `imports`/`E` edge set `classify_districts` itself reads), **105 of dify's 117** below-threshold districts and **32 of n8n's 70** have no edge to any mainland district at all. There is nothing to anchor to. (crawlab: 1 of 1. prometheus: 1 of 3, unchanged.)
- **Group the tail by path prefix into archipelagos at adaptive depth.** Did not converge on finding 14's corpus — n8n's packages all share a `packages/` prefix, and a depth deep enough to separate them produced 96 groups of which 66 were singletons. Not re-run against the current, much smaller n8n tail (70 districts, not 337); this rejection is about the mechanism (path prefix carries no relationship information) rather than the corpus size, so it is left as originally measured rather than re-run for a number that would not change the conclusion.

### What this costs, stated plainly

**Membership never changes.** Classification and placement read the partition; nothing here calls the partitioner. All nine acceptance fixtures pass at 100% placement and 0.0000 modularity delta, `F`/`E`/`L`/`S`/`U` byte-identical, prometheus and sqlalchemy included (finding 15 closed the sqlalchemy gap; see finding 11). crawlab, dify, n8n and prometheus built with `main`'s binary (`cb04469`, no islands) and with this one have byte-identical membership, `F`, `E` and modularity (`q`) — re-verified on this rebase, not merely asserted.

**The map gets bigger, so the viewer must frame the mainland.** Islands go on a ring around mainland's size-weighted centre of mass and unconnected districts on a second ring beyond it, which enlarges the coordinate extent:

| | extent before | extent after | mainland's share of the area |
|---|---|---|---|
| prometheus | 3.20 x 4.69 | 6.48 x 4.72 | 48.2% |
| crawlab | 4.12 x 3.09 | 4.92 x 3.10 | 83.4% |
| dify | 4.59 x 3.34 | 6.23 x 6.17 | 33.7% |
| n8n | 3.21 x 4.53 | 7.67 x 7.49 | 19.5% |

Prometheus is unchanged (it has no module-resolution or terrain exposure -- a single root `go.mod`), which is a useful cross-check that this table's method reproduces the original measurement. The other three moved with finding 15's district counts: crawlab, now down to a single one-file unconnected district out of 11, keeps the great majority of its area as mainland (83.4%, up from 50.5%); dify and n8n, which still carry 91 and 18 unconnected districts respectively spread across fewer, larger mainland communities, now give up *more* of the frame to the offshore rings than before (n8n's mainland share fell from 27.2% to 19.5%). A viewer that opens on the full data extent would therefore make n8n *worse*, showing its 15 mainland districts at a fifth of the frame. The initial view still frames the mainland bounding box; the rest is reached by zooming out. (Note that the coordinate space is not `[0, 1]` before this change either — `blobs::relax()` already expands past `normalize_points`'s box.)

**Prometheus's mainland moves by 0.135 and this is not a sizing bug.** Re-measured at 0.1353, unchanged. It is the only one of the nine acceptance fixtures with any pre-existing sub-1% district (re-verified on this rebase: the other eight still have none); the other eight show zero coordinate movement, by construction -- `relocate_offshore` only ever touches island/unconnected entries, and a fixture with neither leaves its `place_ring` calls with nothing to move. `blobs::relax()` pushes apart any two districts whose circles overlap, and on prometheus mainland's current position is partly the result of being pushed against one of those small districts. Moving the small district away — the entire point of the feature — removes a push that was part of today's placement. No placement rule can both relocate a district and leave undisturbed a district that only reached its position by pressing against the old location. The movement is bounded, small, and a `District.blob`/`NodeRow` coordinate, which finding 11's ruling keeps outside the acceptance gate.

On the three real polyglot repositories, by contrast, mainland *does* move now, measurably: max mainland-node displacement is 0.05 (crawlab), 1.11 (dify) and 0.72 (n8n), against 574/5,531/10,280 mainland-classed nodes respectively. This was not measured or claimed at the corpus finding 14/finding 17 originally shipped against — crawlab then had only 3 sub-1% districts and the other two were not checked this way. It is not a regression in this feature (membership is unaffected, as above, and the movement is still unconditionally the `District.blob`/`NodeRow` coordinate finding 11 excludes from the acceptance gate) but it is new, larger movement than the "only prometheus moves" framing implied, and it is finding 15's doing: more of each repository's below-1% tail is now packed as islands/unconnected around fewer, larger mainland communities, so `relax()` has more offshore mass to push mainland away from than it did against the pre-finding-15 corpus.

`eval/batch_stability.py` is unchanged and its numbers cannot move: it imports the frozen Python reference (`extract`, `pipeline`, `stability`) and calls leidenalg directly, so it never observes a Rust-side geometry change. The `CLAUDE.md` rule that measurement changes ship with their numbers is satisfied by this finding and by the parity evidence above.

### What is not settled

n8n now shows 52 islands, not 269 -- finding 15's module-resolution fix did most of the work issue #41 was opened to ask for, incidentally, by recovering edges that used to strand files into the tail in the first place. They are still on one ring, and calling them islands rather than districts still makes the map honest without making it legible on its own. The threshold is one parameter with one justification — that the mainland count stays roughly comparable across the corpus — and that justification is weaker post-finding-15 than it was (see above: n8n's mainland count is now below dify's despite being the larger repository), which is a weaker claim than "1% is the right number" was already. Nothing here addresses whether a repository of this size should be one map at all. That question is open as issue #41, deliberately stated as a brief rather than answered here.
