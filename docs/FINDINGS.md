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

**The decision, recorded rather than hidden.** Three routes were available: a relative floor (a percentile of the normalised distribution) instead of an absolute one, applying the floor before the max-rescale, or leaving `prune()` and `blend()` exactly as shipped and reporting the sensitivity instead. This document took the third: **the pipeline was unchanged at the time.** Finding 23 revisits that ruling with the larger issue #57 corpus and the owner's decision to make node-relative pruning the default. The mitigation here still applies to historical and explicit-`absolute` comparisons: kept-edge count and below-floor share are reported alongside any modularity figure being used for a cross-repo comparison, not modularity alone.
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

## 18. An evaluation corpus of 132 repositories, and three assumptions tuned on nine do not survive it

Issue #50 asked for ~100 pinned repositories, ~25 per band, so layout and viewer tuning (#48, #42, #34) could be checked against a distribution instead of the nine committed fixtures plus crawlab/dify/n8n. What actually got built is 132 repositories — 128 built, 4 failed — pinned in `eval/corpus.toml` and built with `eval/build_corpus.py`. Twelve repositories still *pending* in `builds.json` after a machine crash partway through the corpus run were driven one at a time by an external wrapper calling `build_corpus.py --candidates ... --jobs 1` (`~/.cache/tolmap-corpus/logs/ultra-serial.log`): `aws/aws-sdk-go-v2`, `DataDog/datadog-agent`, `cloudflare/cloudflare-go`, `elastic/beats`, `googleapis/google-api-go-client`, `hashicorp/terraform-provider-azurerm`, `hashicorp/terraform-provider-google`, `microsoftgraph/msgraph-sdk-go`, `microsoftgraph/msgraph-sdk-python`, `pulumi/pulumi-aws`, `pulumi/pulumi-azure-native`, `pulumi/pulumi-gcp` — all twelve were still carrying their pre-build *screened* `ultra` band at that point, which is why the wrapper treated them as the serialized, largest-memory-user set. Only 4 of the 12 (`aws/aws-sdk-go-v2`, `microsoftgraph/msgraph-sdk-python`, `pulumi/pulumi-aws`, `pulumi/pulumi-azure-native`) actually measured `ultra` once built; 5 measured `large` and 2 measured `medium` (see "Bands were wrong twice" below) — this list is who the wrapper drove, not evidence for any particular corrected band count. Everything else ran through the normal `--jobs 4` path. Maps and per-repo `/usr/bin/time -v` logs live under `$TOLMAP_CORPUS_DIR` (`~/.cache/tolmap-corpus`), never in this repository.

Reviewing the WIP before building on it found three things wrong, in `eval/build_corpus.py`, `eval/corpus_stats.py`, and the missing manifest itself. Each is described where it was found, below.

### Bands were wrong twice, and `build_corpus.py` itself is fine

`eval/build_corpus.py`'s `--candidates` mode assigns a band to a repository *before* building it, from an external size estimate — its own docstring calls this "candidate screening", not a measurement. Issue #50 defines a band by what `--all-sources` actually indexes: small < 300, medium 300–2k, large 2k–8k, ultra ≥ 8k *mapped source files*, explicitly not total repo files. `builds.json` never reconciled the two. Recomputing band from each built repo's measured `files` count disagreed with its stored screening band on **81 of 128 built repositories — 63%**. `Azure/azure-sdk-for-go` was screened `ultra` and measured 349 files (`small`); `DataDog/datadog-agent` was screened `ultra` and measured 7,968 (`large`, 32 files under the 8,000-file `ultra` line). The corpus's screened distribution was small 24 / medium 29 / large 32 / ultra 47; its measured distribution is **small 49 / medium 41 / large 30 / ultra 12**. That the measured `ultra` band also has 12 repositories is a coincidence, not corroboration: the external wrapper's serialized set (above) was chosen by *screened* band and includes 5 repositories that measured `large` and 2 that measured `medium`, while 7 of the 11 built repositories that measured `ultra` were never touched by the wrapper at all — also screened `ultra`, but already built through the normal path before the crash (`Azure/azure-sdk-for-python`, `elastic/kibana`, `googleapis/google-cloud-python`, `home-assistant/core`, `kubernetes/kubernetes`, `n8n-io/n8n`, `twentyhq/twenty`). `eval/manifest_from_builds.py` (new, committed alongside the manifest it writes) recomputes band from measured files for every built repo; a failed repo has no measured count, so it keeps its screening band as the only estimate available. `eval/build_corpus.py` itself has no bug — it does exactly what its docstring says (resumable, one failure doesn't stop the run, `ultra` repos serialized after `regular` ones) — the band field just meant "guess" all along and nothing downstream had said so.

The 13 repositories built with explicit `--pkg`/`--lang` instead of `--all-sources` (`encode/httpx`, `immerjs/immer`, `odoo/odoo`, `pallets/click`, `pallets/flask`, `pallets/itsdangerous`, `pmndrs/zustand`, `psf/requests`, `pytest-dev/pytest`, `python-attrs/attrs`, `spf13/cobra`, `spf13/viper`, `tiangolo/sqlmodel`) carry no reason in `builds.json` either — it isn't a field `build_corpus.py` records. Their `eval/corpus.toml` `reason` entries were written by inspecting each cached clone's top-level layout (`ls`, not a rebuild): eleven are `src/`-layout or root-level single-package repos where auto-detection could otherwise land on the wrong directory; `python-attrs/attrs` genuinely has two sibling packages under `src/` (`attr`, the original, and `attrs`, its re-export) and needs two `--pkg` flags to cover both; `odoo/odoo` is a monorepo of hundreds of sibling addon packages under `addons/` plus the core `odoo/` package, where `--all-sources`' single-source auto-detect is ambiguous, so the whole repo is forced to one Python source instead. This is read from the filesystem, not recovered from a record that no longer exists, and is noted as such in `manifest_from_builds.py`.

### `corpus_stats.py`'s px²/file did not match #48's no-change proof

Issue #50 deliverable 3 asks for on-screen px² per file at fit zoom, "same method as #48's no-change proof" (`web/scripts/no-change-proof.ts`, PR #49). The WIP's `mainland_fit_scale` bounded the fit-zoom viewport to only the *mainland* districts' blob polygons, and `px_per_file` only measured mainland districts. Neither matches the actual proof: `geometry.ts`'s `fitScale` (unchanged by #49, and the function `no-change-proof.ts` calls) bounds the viewport over **every** file's node position and **every** district's blob, mainland or not; `densityByDistrict` in `no-change-proof.ts` measures every district *except* `unconnected` ones — mainland and island alike, since an island still gets a drawn region and a dot budget. The WIP's version silently shrank the box (dropping island/unconnected blobs and any node outside a mainland blob) and silently shrank the district set, which inflates px²/file in both directions at once. Fixed in `eval/corpus_stats.py`: `world_fit_scale` now unions all node positions and all district blobs exactly as `worldBounds(doc, "r")` does, and `px_per_file` includes `mainland_ids | island_ids`. Sanity check against PR #49's own reported n8n figures (mainland-only, 11,982 files, cb04469+#42: min 7 / median 12 px²/file at 390×700) — this corpus's n8n pin (11,991 files, a few commits later) now measures min 0.0 / median 7.5 including islands, which is the expected direction: adding smaller island districts to the set can only pull the minimum down.

### `collect-maps.mjs` reads the manifest, and the baseline catalogue is untouched

With neither `$TOLMAP_CORPUS_DIR` nor `$TOLMAP_MAPS_DIR` set, `web/scripts/collect-maps.mjs`'s output (`public/maps/index.json`) is byte-for-byte identical to the pre-WIP script's output — checked by running the committed `HEAD` version and the WIP version back to back and diffing. With `TOLMAP_CORPUS_DIR` set, the catalogue grows from 9 hardcoded `STEM_OWNERS` entries to all 128 built corpus repositories; all 9 baseline slugs (`scrapy/scrapy`, `django/django`, `pallets/flask`, `sqlalchemy/sqlalchemy`, `celery/celery`, `Textualize/rich`, `encode/httpx`, `prometheus/prometheus`, `vuejs/core`) turn out to already be corpus members at their own pins, so the corpus version wins each and the catalogue has exactly 128 entries, not 137. `public/maps/` stays gitignored and nothing under it is committed.

### The per-band table

| metric | small | medium | large | ultra |
|---|---:|---:|---:|---:|
| repositories (built/failed) | 49/0 | 40/1 | 28/2 | 11/1 |
| files | 51 [19–224] | 704 [353–1,517] | 4,050 [2,145–6,799] | 11,991 [8,757–39,964] |
| districts | 5 [3–8] | 16 [11–50] | 52 [21–364] | 330 [87–3,411] |
| mainland | 5 [3–8] | 13 [9–22] | 18 [9–25] | 16 [3–27] |
| islands | 0 [0–0] | 1 [0–5] | 16 [2–230] | 309 [45–1,867] |
| unconnected | 0 [0–0] | 1 [0–11] | 7 [0–63] | 13 [0–335] |
| landmarks | 10 [7–13] | 21 [15–55] | 56 [27–371] | 338 [90–2,548] |
| modularity q | 0.254 [-0.003–0.454] | 0.563 [0.439–0.706] | 0.716 [0.511–0.865] | 0.823 [0.692–0.987] |
| kept edges | 133 [21–1,234] | 4,986 [512–27,090] | 21,976 [5,428–168,774] | 41,758 [28,709–160,031] |
| zero-edge files | 1 [0–18] | 36 [2–462] | 142 [15–1,264] | 338 [6–25,620] |
| 390×700 px²/file min | 127.4 [37.4–250.8] | 33.3 [19.9–58.1] | 6.7 [0.0–21.7] | 0.0 [0.0–0.0] |
| 390×700 px²/file district median | 189.5 [123.8–286.7] | 60.2 [28.9–99.0] | 12.2 [5.9–28.4] | 2.0 [0.0–5.5] |
| 1440×900 px²/file min | 904.2 [251.6–1435.6] | 223.8 [141.4–395.9] | 51.1 [0.0–101.3] | 0.0 [0.0–0.0] |
| 1440×900 px²/file district median | 1338.7 [858.5–1820.7] | 356.0 [207.0–748.8] | 91.4 [40.3–162.2] | 16.8 [0.0–38.9] |
| 390×700 share of files under floor | 0.0% [0.0–0.0%] | 0.0% [0.0–64.3%] | 99.6% [30.9–99.9%] | 99.8% [60.9–100.0%] |
| 1440×900 share of files under floor | 0.0% [0.0–0.0%] | 0.0% [0.0–0.0%] | 0.0% [0.0–4.9%] | 42.0% [2.6–97.0%] |
| build seconds | 0.3 [0.2–0.9] | 2.9 [1.5–8.2] | 19.5 [8.4–52.9] | 81.6 [42.1–380.9] |
| peak RSS MB | 20.8 [17.5–34.1] | 114.2 [42.9–254.5] | 519.8 [213.3–1266.9] | 1786.7 [838.9–5305.8] |

Each cell is the per-repository median [p10–p90]; failed repositories are counted in the first row and excluded from every metric row below it (no map, nothing to measure) — under CLAUDE.md's lower-bound rule, that means every distribution above is a floor on how bad things get, not the whole truth. The two `px²/file min` rows are dominated by a geometric artifact rather than a zoom measurement past `small` band; see "#48's 30 px²/file floor" below for why, and for the `share of files under floor` rows that replace them as the load-bearing statistic. Reproduce with `python eval/manifest_from_builds.py && python eval/corpus_stats.py`.

### The 1% mainland share (#42) does not stay flat — it was never tested past 21x

PR #42 measured mainland-district *count* staying roughly flat (16/27/28 before finding 15's resolver fix) across a 21x file-count range and flagged in its own text that this was weaker evidence than it looked. This corpus measures the thing that actually matters for readability — mainland *file share*, not district count — across a ~5,016x range (8 to 40,129 files), by band:

| band | mainland file share, median [p10–p90] | mainland district count, median |
|---|---|---|
| small | 100.0% [100.0–100.0%] | 5 |
| medium | 98.9% [90.9–100.0%] | 13 |
| large | 92.3% [40.9–97.9%] | 18.5 |
| ultra | **54.7% [7.7–88.9%]** | 16 |

Small and medium hold the 1%-threshold's implicit promise — mainland is nearly everything. Large already shows a p10 of 40.9%: one in ten large repos has fewer than half its files inside a mainland district. Ultra breaks it outright: the median ultra repo has barely more than half its files classified mainland, and the worst-case (p10) ultra repo has 92.3% of its files outside the 1% line — in a district too small by that rule to be a "place" at all. The threshold is a constant fraction of file count; district size distributions get heavier-tailed as repos grow (that is exactly what finding 15 measured happening to n8n), so a fixed 1% line inevitably classifies a shrinking share of a growing repo as mainland. **Falsified as a scale-invariant rule** — 1% works because the corpus PR #42 measured it on topped out at 11,982 files, a fifth of this corpus's largest ultra repo.

### #48's 30 px²/file floor: the minimum is a geometry artifact, not a zoom measurement

The first version of this finding counted how many repositories per band had *any* district under the floor at fit zoom, using the per-repo px²/file **minimum**. That statistic turned out not to be measuring what it looked like it was measuring: `date-fns/date-fns`, `DataDog/datadog-agent`, `angular/angular`, `calcom/cal.com` and `cockroachdb/cockroach` all show a minimum of exactly `0.0`, at both viewports, and the cause has nothing to do with viewport size.

`contours()` in `src/blobs.rs` traces each district's region from a Gaussian-blurred density field on a fixed grid, then keeps a marching-squares contour only if it has at least 12 points *and* is at least 12% of that district's largest contour's point count (`src/blobs.rs:880`). A district with too few member files never produces enough grid coverage to clear that filter and gets an **empty `blob`** — no polygon at any zoom, not a small one; `world_fit_scale` and `district_densities` then correctly compute `0.0` px²/file for it, because its on-screen area really is zero. Across the full corpus, **0% of the 1,463 mainland districts are zero-blob** (mainland is always ≥1% of a repo's files, which is enough to clear the contour filter every time measured), but **67.9% of the 18,043 island districts are** (12,248 of them) — covering 7.7% of all mapped files considered (24,258 of 315,927). Zero-blob districts are small by construction (median 1 file, p90 4 files, max observed 33), so a single tiny island anywhere in a large map pins that repo's px²/file minimum at exactly `0.0` regardless of everything else on the map — which is why the minimum column stops being informative well before `ultra` band, and is why it reads `0.0` for a `large`-band repo like `cockroachdb/cockroach` just as readily as for an `ultra` one. This is not a corpus_stats.py bug: a zero-blob district's file-dot budget genuinely is zero in the real renderer too (`area * k² / floor` with `area = 0`), so the number is correct — it just isn't answering "is the floor engaging because of viewport scale," which is the question #48 asked.

The robust measure is the **share of all mapped files that sit in a district under the floor at fit zoom**, over the whole repository (including zero-blob districts, whose budget really is zero, and including unconnected files in the denominator even though they can never be thinned, so this is a whole-map figure, not one scoped only to files the budget could apply to):

| band | share under floor @ 390×700 (phone), median [p10–p90] | share under floor @ 1440×900 (desktop), median [p10–p90] |
|---|---|---|
| small | 0.0% [0.0–0.0%] | 0.0% [0.0–0.0%] |
| medium | 0.0% [0.0–64.3%] | 0.0% [0.0–0.0%] |
| large | **99.6% [30.9–99.9%]** | 0.0% [0.0–4.9%] |
| ultra | **99.8% [60.9–100.0%]** | 42.0% [2.6–97.0%] |

On a phone, the median `large`-band repo already has 99.6% of its files sitting in a thinned district at fit zoom — not an edge case but the typical case once a repo passes roughly 2,000 files, and `huggingface/transformers` (`large`, 3,020 files) reaches 100%. `hashicorp/consul` (`medium`, 1,513 files) is at 97.4%, well inside a band the original nine-repo derivation set (24 to 11,982 files, dominated by small/medium, with no repository in the `large` band at all) called safe. On desktop the picture is milder for `large` (median 0%, but a p90 of 4.9% means one in ten `large` repos already has real files thinned there too) and only becomes substantial at `ultra` (median 42.0%; even the best-case `ultra` repo in this corpus, `Azure/azure-sdk-for-python` at 40,129 files, is at 42.1%).

**Not falsified as a floor value** — 30 is still a defensible threshold, and small/medium band mostly clears it (medium's phone p90 of 64.3% is the one exception worth watching). **Falsified as evidence that the floor is a large/ultra-only or desktop-safe concern**: on a phone viewport specifically, it already governs the median `large`-band repo's file-dot rendering almost completely.

### Landmark counts per band, and what the outliers turned out to be

| band | landmarks, median [p10–p90] | max |
|---|---|---|
| small | 10 [7–13] | 22 |
| medium | 21 [15–55] | 969 (`date-fns/date-fns`) |
| large | 56 [27–371] | 919 (`microsoft/vscode`) |
| ultra | 338 [90–2,548] | **10,034** (`microsoftgraph/msgraph-sdk-python`) |

n8n and dify (both cited in the task as 88/140) measure **90 and 136 landmarks** on this corpus's pins — close but not identical, since these are n8n-io/n8n at `d9dc457` and langgenius/dify at `9a0961a`, a few commits past whatever pins produced 88/140.

`microsoftgraph/msgraph-sdk-python` having 10,034 landmarks on 16,636 files is not the landmark selector picking too many "important" files — it is not proportional or bimodal in the way an importance heuristic saturating would look. `landmarks()` (`src/pipeline.rs:627-756`) picks up to 2 each of `entry`/`bridge`/`hub`/`hazard` *globally*, plus exactly **one `capital` landmark per district**. Reading `doc["L"]`'s `why` field confirms it: msgraph-sdk-python's 10,034 landmarks break down as capital 10,029 / bridge 2 / hub 2 / hazard 1 — landmark count is tracking **district count**, and msgraph-sdk-python's partition produced **10,030 districts for 16,636 files**, 84.9% of them (8,512) singleton one-file districts, with raw modularity **q = 0.0000 exactly**. Across the whole corpus, singleton-district share has a median of 0% and a p90 of 16.7% — msgraph-sdk-python's 84.9% is far outside that distribution. `date-fns/date-fns` (969 landmarks, the medium-band max) is worse by this measure: **97.1% of its 963 districts are singletons**. Both repos show the same shape — one or two very large districts (msgraph-sdk-python's `d0` is 3,548 files named "models"; date-fns's `d0` is 498 files) plus a long tail of near-total singleton districts, each named after its one file.

This looks like finding 10's already-documented below-floor hazard (`blend()` divides every edge by the single largest blended edge in the whole graph, so one dominant edge scale anywhere in the graph can push most other real edges under the prune floor and get them dropped) showing up at a severity not previously measured on a real repo this large — plausible given msgraph-sdk-python's own landmark data includes a fan-in outlier of 26,342 on a single file. `microsoft/vscode` (919 landmarks, the large-band max) also has modularity q = 0.0000, but for what looks like a different and more alarming reason: **its map has exactly 1 kept edge for 5,919 files** — not a clustering artifact, since a codebase this size plainly has extensive internal imports; more likely an import-resolution failure specific to this repo (`--all-sources` selected `ts at src`, a large monorepo where path aliases or project references may defeat the static resolver).

**This looks like a pipeline problem, not a landmark-heuristic problem, and it is not fixed here** — this PR is eval-only and does not touch partition, blend or extraction. Filed as issue #57 with this evidence (`msgraph-sdk-python`/`date-fns`'s district fragmentation, `vscode`'s near-total edge loss) for someone to pick up.

### Build cost vs. size holds in aggregate, and both msgraph SDKs are the exception, not the rule

Across all 128 built repos (8 to 40,129 files), build seconds and peak RSS both track file count with a strong power-law relationship: Pearson r = 0.963 on log(files) vs. log(seconds), 0.937 on log(files) vs. log(peak RSS) — three and a half orders of magnitude of file count, one consistent trend. Median cost per band, normalized:

| band | median build seconds | median peak RSS MB | sec / 1,000 files | MB / file |
|---|---|---|---|---|
| small | 0.3 | 20.8 | 7.2 | 0.42 |
| medium | 2.9 | 114.2 | 3.8 | 0.13 |
| large | 19.5 | 519.8 | 4.2 | 0.13 |
| ultra | 81.6 | 1,786.7 | 5.5 | 0.12 |

Cost per file actually *drops* from small to medium and then holds roughly flat — there is no evidence of runaway superlinear cost in the typical case. `microsoftgraph/msgraph-sdk-go` and `microsoftgraph/msgraph-sdk-python` are both well outside that typical case, and both are the same product's two language SDKs:

- `microsoftgraph/msgraph-sdk-go` **failed**: `--all-sources` selected `go at . (17,160 files, high confidence)`; extraction hit 23,880 MB (23.3 GiB) peak RSS against the 24 GiB `prlimit` cap and died with `memory allocation of 74 bytes failed` (returncode 134) — a genuine OOM, not a timeout, and it never produced a file count to band it by.
- `microsoftgraph/msgraph-sdk-python` **built**, ultra band (16,636 files), but at 844.5s / 13,788.6 MB (13.8 GB) peak RSS — **2.6x the memory and 2.6x the wall time of `Azure/azure-sdk-for-python`**, which indexed 2.4x more files (40,129) in 326.4s / 5,305.8 MB. No other repo in the corpus, at any file count, used more than 5.3 GB.

Both point at #52, not at a general scaling problem: the typical repo's cost is well-behaved and predictable from file count alone; whatever is expensive about the msgraph SDK family (both languages) is a property of that codebase — plausibly the same extremely high-fanin shared type/schema layer implicated in `msgraph-sdk-python`'s district-fragmentation problem above (issue #57) — not of size, since repos more than twice its size cost a fraction as much.

### What is unmeasured

- The corpus is 132 repositories, not the ~100 issue #50 asked for, and its measured-band split (49/41/30/12) is far from the ~25-per-band target, particularly at `ultra` (12, not ~25) and `small` (49, nearly double). Nothing here rebalances it — rebuilding was out of scope for this change, per the task's own instruction not to rebuild anything.
- Four repositories never produced a map: `axios/axios`, `sveltejs/svelte`, `withastro/astro` (`--all-sources` found no qualifying source in any of the three — all are JavaScript-heavy repos with no `.py`/`.go`/`.ts` majority, which the indexer does not extract) and `microsoftgraph/msgraph-sdk-go` (OOM, above). Per issue #50's failure policy, all four stay in `eval/corpus.toml` with their status and reason; none of the tables above are informed by their structure, because there is none to measure.
- The px²/file numbers are fit-zoom only, both viewports fixed at the two sizes #48 used; nothing here measures the zoom levels between fit and full reveal, which is what #48's own `drawnCount`/`fullRevealZoom` helpers characterize and this corpus does not re-derive.
- No visual/touch check ran against the corpus — deliverable 5 is numeric only. The `microsoftgraph/msgraph-sdk-python` 10,034-pin case in particular has not been opened in the viewer to see what "every landmark drawn at fit zoom" actually looks like.
- The 13 explicit-`--pkg`/`--lang` reasons in `eval/corpus.toml` are inferred from each clone's on-disk layout at review time, not recovered from a decision record; if any of those repos' pin commits move, the layout could no longer match the reason given.
- Issue #57 (partition fragmentation on `microsoftgraph/msgraph-sdk-python`/`date-fns/date-fns`; near-total edge loss on `microsoft/vscode`) is filed, not fixed. Their district, landmark, mainland-share and px²/file numbers elsewhere in this finding are real measurements of what the pipeline currently produces for them, not typical `large`/`ultra`/`medium`-band behavior — a fix would change those three repos' numbers without necessarily changing the per-band medians much, since they are minority outliers within their bands.

## 19. Go package fan-out multiplied owned path strings; lexical file IDs cut msgraph-sdk-go below 8 GiB

Issue #52 was not a general large-repository failure. `aws/aws-sdk-go-v2` had already built locally at roughly 2.5 GiB despite having more files and substantially more source than `microsoftgraph/msgraph-sdk-go`; msgraph instead exhausted a 24 GiB address-space cap during extraction. The difference was the shape of the Go packages. A Go import resolves to every source file in the imported package directory, and msgraph contains generated packages with 3,499, 2,223 and 1,663 files that are imported by thousands of files. The resolver therefore expands one source-level package import into thousands of per-file edges.

### The measured driver was fan-out times string ownership

Temporary counters at the post-resolution boundary, run locally before the machine build hold, measured:

| repository | resolved static entries | directed entries | resolved uses |
|---|---:|---:|---:|
| `microsoftgraph/msgraph-sdk-go` | **24,447,093** | **24,447,093** | **30,291,465** |
| `aws/aws-sdk-go-v2` | 160,031 | 160,031 | 450,092 |

That is about **153x** as many static entries for msgraph. The old representation made the count much more expensive than the edge payload alone suggests: static and directed edges used `BTreeMap<(String, String), f64>`, so every entry owned both endpoint paths; uses owned the two paths plus the symbol name; Go resolution cloned the imported package's complete target vector for every import; and the single-source union rebuilt those maps once resolution finished. The local msgraph baseline reached 24,453,860 KiB RSS and then failed after resolution. This confirms the proposed driver directly: package fan-out created tens of millions of entries, and owned path strings multiplied their memory cost.

The fix assigns every file a lexical `u32` `FileId` and carries `(FileId, FileId)` keys through extraction, `GraphData` and geometry. Go package targets are borrowed slices rather than per-import clones. The common single-source path moves its compact maps instead of rebuilding them, and large candidate lookup maps are dropped after their last use. Lexical IDs preserve the former string-key `BTreeMap` order; multi-source remapping preserves source order and ownership filtering; float additions occur in the same order. `GraphData` has a custom serializer and deserializer so checked-in graph JSON still contains paths, in the established field and element order. Every resolved edge remains present with the same weight: memory was reduced by representation, never by sampling or thinning.

### The held acceptance run now completes

The comparison ran remotely on a GitHub-hosted 4-vCPU / 16-GB runner, with `prlimit --as` set to approximately 14 GiB: [remote-build run 35723119573](https://github.com/onsager-ai/tolmap/actions/runs/35723119573). The workflow's `primary` binary was `main`; `compare` was this change.

| repository | `main` | lexical-`FileId` branch | output comparison |
|---|---|---|---|
| `microsoftgraph/msgraph-sdk-go` | exit 134 in `[1/5] extract` (`memory allocation of 9 bytes failed`); 14,630,972 KiB peak; 133.29 s | **built 17,160 files**; **7,933,056 KiB peak**; 422.34 s | n/a: `main` produced no map |
| `aws/aws-sdk-go-v2` | built 26,520 files; 2,451,836 KiB; 421.87 s | built; **2,309,052 KiB**; 416.60 s | **byte-identical** |
| `prometheus/prometheus` | built 631 files; 87,184 KiB; 3.04 s | built; **85,776 KiB**; 3.01 s | **byte-identical** |

The msgraph branch completed at about 7.6 GiB RSS, with roughly 6.4 GiB of headroom beneath the same cap that killed `main`. Its 422.34-second completion time cannot be compared as a slowdown against `main`'s 133.29 seconds because `main` stopped partway through extraction and never ran the remaining pipeline. AWS and Prometheus provide the completed-run controls: both use less memory, take essentially the same time, and emit byte-identical maps.

### The graph did not move

Before the remote run, a separate `origin/main` binary and the branch binary were compared locally on every source small enough for the machine's build hold. The final map bytes matched for Flask, HTTPX, the checked-in synthetic polyglot graph, a freshly generated 50-file Go/TypeScript polyglot source fixture, and tolmap's local Python reference package. The extracted synthetic-polyglot `GraphData` JSON also matched byte for byte, exercising the new indexed internal representation and path-based wire serializer. The remote AWS and Prometheus comparisons extend that evidence to real Go repositories, including a 26,520-file SDK.

No reference fixture was re-derived. Placement and modularity therefore did not need a new acceptance derivation: the serialized graphs and maps are unchanged, and the existing fixtures remain the lower bound they were before this representation change.

### The Python SDK is a different unresolved mechanism

`microsoftgraph/msgraph-sdk-python` still reached 13.8 GB for 16,636 files in finding 18. Python imports resolve to one file, so it cannot have this Go directory fan-out mechanism. Issue #57's remote `dump-blend` measurement found **99.998% of its blended edges below the 0.02 prune floor** after global-maximum normalization. That result explains the SDK's near-singleton partition and landmark explosion: almost every extracted edge disappears before Leiden. It does not explain why constructing the Python graph consumes 13.8 GB. The Python memory driver remains unmeasured and is not changed here; a blend/prune fix for #57 would also change every affected map and requires its own stability measurements and finding.

## 20. NodeNext `.js` specifiers erased TypeScript graphs, and restoring them loses no existing edge

Finding 18's `microsoft/vscode` outlier — one kept edge across 5,919 files — was an extraction failure, not a codebase with no internal structure. VS Code writes the relative import paths that NodeNext/Node16 expects to reach at runtime, such as `./arrays.js`, while its repository contains the TypeScript source `arrays.ts`. Of the relative import specifiers under `src/**/*.ts` measured for issue #58, **100,896 end in `.js` and 19 do not**. The resolver previously treated the written `.js` as part of an extensionless base and probed impossible names such as `.js.ts`, so nearly every internal import contributed no static edge.

The fix follows TypeScript's [file extension substitution](https://www.typescriptlang.org/docs/handbook/modules/reference.html#file-extension-substitution) rule for both relative paths and tsconfig aliases. An exact parsed JavaScript path wins; otherwise `.js` probes `.ts` and then `.tsx`, while `.jsx` probes `.tsx`. Every candidate still has to exist in the parsed-file set, and the historical extensionless probe order is unchanged. This is the same lower-bound contract as the rest of extraction: the resolver recovers only a file that was actually parsed, never a path inferred to exist.

GitHub-hosted [remote-build run 35723131758](https://github.com/onsager-ai/tolmap/actions/runs/35723131758) built `main` as the primary and this fix as the comparison at the same four pinned repository commits. The edge counts below are the map's committed-order `E` entries; peak RSS is `/usr/bin/time -v`'s maximum resident set size. Blank RSS cells were not reported in the supplied comparison.

| repo | files | edges, main → fix | edges lost | districts, main → fix | q, main → fix | peak RSS, main → fix |
|---|---:|---:|---:|---:|---:|---:|
| microsoft/vscode | 5,919 | 1 → **74,719** | 0 | 914 → **21** | 0.0000 → **0.5325** | 1,381,044 → **822,204 KiB** |
| colinhacks/zod | 247 | 24 → **580** | 0 | 9 → 9 | 0.4498 → **0.4893** | — |
| apollographql/apollo-client | 500 | 60 → **760** | 0 | 47 → **19** | 0.7651 → **0.6680** | — |
| n8n-io/n8n | 11,991 | 38,586 → **39,402** | 0 | 87 → **75** | 0.7454 → **0.7516** | — |

The loss check compared `E` as sets after confirming the two maps had the same `F` order. **Every edge present on `main` remains present after the fix on all four repositories; the comparison only adds edges.** The district and modularity movements are downstream results of supplying the partitioner with the graph that was previously missing. In particular, the task's prediction that n8n would be byte-identical was wrong: n8n also contains `.js` specifiers, gains 816 edges, and changes from 87 to 75 districts. The no-loss result is the relevant invariant under the lower-bound rule, and it holds.

No committed fixture changes. The seven Python fixtures and the Go fixture cannot exercise this TypeScript path. The pinned Vue fixture is the only committed TypeScript acceptance source; its files admitted by source collection contain no affected import or export specifier. The graphs under `data/ci/` are already extracted and bypass resolution. Nothing under `data/` was re-derived.

Two gaps remain outside this fix. TypeScript also substitutes `.mjs` → `.mts` and `.cjs` → `.cts`, but tolmap does not collect `.mts` or `.cts`; widening collection is a separate change, so those source substitutions remain unavailable here. `langchain-ai/langchain` having zero edges in finding 18's corpus is also unrelated: it is Python, and issue #58's comment records it as a separate likely source-root or package-name resolution failure.

## 21. Terrain's viewer zoom gates were flat constants with no measurement behind them; per-element on-screen area reuses the floors the map already trusts

**Superseded 2026-09-23:** terrain was removed by owner decision; see [finding 24](#24-terrain-was-built-measured-and-removed). The measurements below remain historical.

Issue #54 asked two questions about the terrain subdivision finding 16 shipped: whether it should become the corpus default (an owner decision, not made here — see below), and whether its four viewer zoom gates (contour, sub-district label, parcel grid, parcel label) actually sequence the map the way `docs/TERRAIN.md` intends: districts, then sub-districts (contours and labels, a few dots), then files. They shipped in #47 as four flat constants (`zoom < 1.25`, `zoom > 1.7`, `zoom > 2.4`, `zoom > 4`) picked with no corpus behind them — finding 16 built the mechanism and measured its geometry, cost and determinism, but not this.

### What was built

`MapRenderer.drawTerrain` (`web/src/map/MapRenderer.ts`) now gates every element on its own measured on-screen area clearing a floor the map already trusts for "is this a legible place," rather than a flat zoom constant, reusing two existing constants instead of adding new ones:

- **Sub-district contour**: `PIN_ESTABLISH_AREA` (`web/src/map/pins.ts`, 3 file-dots' worth of screen area) — the same bar a district's own capital pin already has to clear. A sub-district is a smaller place than its parent district but the same kind of place.
- **Sub-district label**: the exact text-footprint formula `drawLabels()` already uses for every other label (`txt.length * size * 0.62` wide, `size * 1.25` tall), checked against the contour's own characteristic on-screen size (`sqrt(area)` — blobs are traced to be reasonably round, finding 16's compactness pass, so this is cheaper than a bounding-box walk and reuses the area already computed for the contour gate).
- **Parcel cell**: one `DOT_DENSITY_FLOOR` of screen area (`web/src/map/constants.ts`) — a parcel packs ≥1 files into one grid cell and reads as one thing, so the natural floor is "at least as legible as a single file dot."
- **Parcel label**: unchanged — it already checked real on-screen width/height (`>30×>12px`), not a flat constant; only the redundant `zoom > 4` pre-gate in front of it was removed.
- **Arterials**: gated on the *parent* district's own on-screen area clearing `PIN_ESTABLISH_AREA` (they are roads through the whole district, not a sub-region of their own).

`web/scripts/terrain-zoom-measure.ts` (new, `npx tsx`-runnable, no browser) computes `zf_establish = sqrt(floor / worldArea) / fitScale` directly for every sub-district, parcel and district in a built terrain map — the same closed form the code above is checking every frame, just solved for `zf` instead of evaluated at one.

### Why area-based gates, not a fitted flat constant

A flat zoom constant cannot be correct for every district: a 3,700-file district and a 330-file district at the same map's eligibility floor have wildly different sub-district sizes, so any single zoom value either pops the big one's sub-districts too late or the small one's too early. Measured directly — every terrain-eligible district across all eight built maps below, both viewports:

| viewport | full file reveal zf (median) | sub-district establish zf (median) | parcel establish zf (median) |
|---|---:|---:|---:|
| 390×700 (phone) | 2.578 | 1.874 | 3.419 |
| 1440×900 (desktop) | 0.875 | 0.631 | 1.175 |

(medians over the 41–43 terrain-eligible districts across `langgenius/dify`, `n8n-io/n8n`, `aws/aws-sdk-go-v2`, `microsoft/vscode`, `twentyhq/twenty` — the large/ultra-band repos terrain actually exists for; `django/django` and `crawlab-team/crawlab` are the small controls and are excluded here because their districts already clear #49's full-reveal dot budget at or before fit zoom, discussed below.)

The sequence holds in aggregate on both viewports: sub-districts establish before files fully reveal as dots, which establish before parcels become their own legible cells. **It does not hold for every district**: in 6 of 41 (14.6%), the sub-district *median* establish-zf is slightly above the district's full-reveal zf — `langgenius/dify`'s "features" (334 files) and "plugins" (321 files), `n8n-io/n8n`'s "typeorm & @n8n" (639) and "api-types & dto" (458), `microsoft/vscode`'s "terminalContrib & terminal" (300), and `twentyhq/twenty`'s "twenty-front & core-modules" (679, only 2 sub-districts). Every one of these is either close to the `2T` eligibility floor or has a small, imbalanced sub-district (e.g. n8n's "api-types & dto" splits 370/77 — the 77-file straggler alone pulls the two-item median up). This is a property of the geometry these specific districts have, not a constant that could be re-picked to fix it: a global flat constant would do *worse* here (it cannot adapt per sub-district at all), and every other sub-district in these same districts — the ones the median doesn't represent — still establishes before its district's full reveal. Recorded as measured and not chased further, per CLAUDE.md's lower-bound rule: the numbers above are what this design achieves, not a claim that every district sequences perfectly.

Parcels establishing *after* full file reveal (not before, as the sub-district numbers do) is also correct, not a bug: a parcel's member files already show as ordinary file dots under #49's per-district budget regardless of parcel membership (the budget has no notion of terrain sub-structure — see "interaction with #49" below), so the parcel's address-grid rectangle is a *later*, finer-grained annotation on top of dots a reader can already see, not a precondition for seeing them. `docs/TERRAIN.md`'s "districts → sub-districts → files" sequence is about contours and labels, not about parcels, which sit structurally alongside dots rather than before them.

`django/django` and `crawlab-team/crawlab` (the two small controls) show the same ordering trouble as the 6/41 above, worse: their one or two eligible districts sit barely over the `2T`/50-file floor and already clear #49's full-reveal fast path (every file drawn) at or below fit zoom (measured zf full-reveal 0.21–0.83 across both, always ≤1), so their sub-district contours (measured establish zf 0.38–1.34) show up *after* every file is already a visible dot. Terrain on a repository this size is not wrong, just closer to redundant than illustrative — there is little a contour adds once every file already reads as a dot on its own.

### Interaction with #49's dot budget, #61's pins/badges and #67's island fade

- **#49's dot budget** (`DOT_DENSITY_FLOOR`, `dotFactor`): entirely independent of terrain membership. Every file in `doc.N` gets a dot (or doesn't) purely from its top-level district's own area/size, whether or not it belongs to a sub-district, a parcel or is an arterial. Terrain draws additional overlay geometry on top, never a replacement for or a modifier of which files get dots — this is why parcels can (and do) establish after full reveal, above. A parcel's own dots and its wrapping rectangle both drawing simultaneously is a real, measured visual layering (opacity kept low on the terrain overlay — 0.07–0.34 fill — specifically so this doesn't compete with the dots underneath), not chased further here: suppressing a file's own dot while its parcel/sub-district stands in for it would be a materially bigger change (the budget/ranking math would need to know about terrain membership) than fitting zoom ranges, and risks the fast-path/no-change-proof guarantee finding 16 and #49 both depend on.
- **#61's pins/badges**: every terrain-eligible district is, by construction, always **mainland** — `2T` (the terrain eligibility floor) is asymptotically larger than the 1% mainland-share floor (finding 17) for any repository size the corpus reaches, confirmed on all eight built maps (zero eligible districts classified island/unconnected). A mainland district's capital pin is never area-gated (`pins.ts`'s `districtClass(district) !== "mainland"` check), so it draws from zf=0 through `PIN_CAPITAL_HIDE_ZF` (3.4) regardless of terrain — comfortably overlapping the whole sub-district-establish window above, which is the intended handoff (the pin marks the place until its own contours are legible, then fades as file-level detail does the same job). The "+N hidden files" badge (`hiddenFileCount`) still counts against the *top-level* district's full file set, uninformed by sub-district/parcel structure — the same independence as the dot budget above, and the same reason it's left alone here.
- **#67's island fade**: moot for terrain specifically (terrain districts are never islands, above), but also not wired up either way — `drawTerrain` draws an eligible district's terrain unconditionally, with no `islandFadeVisible` check of its own. Harmless today only because eligibility already implies mainland; worth a comment if that invariant is ever revisited.

### Cost: `--terrain` on vs off, same commit, GitHub-hosted runners

Terrain builds ([remote-build run 35732732308](https://github.com/onsager-ai/tolmap/actions/runs/35732732308)) at `main`/1ec9320 for the sample issue #54 asked for, against the flag-off numbers already recorded for the same commit in [run 35728161408](https://github.com/onsager-ai/tolmap/actions/runs/35728161408) (`~/.cache/tolmap-corpus/builds.json`). Both are single wall-clock runs on shared GitHub-hosted runners:

| repo | files | wall s, off → on | Δ% | peak RSS MB, off → on | Δ% |
|---|---:|---|---:|---|---:|
| django/django | 851 | 3.25 → 2.73 | −16.0% | 121.8 → 122.0 | +0.2% |
| crawlab-team/crawlab | 482 | 1.38 → 1.49 | +8.0% | 52.2 → 52.5 | +0.7% |
| langgenius/dify | 6,347 | 22.47 → 23.72 | +5.6% | 485.9 → 488.2 | +0.5% |
| microsoft/vscode | 5,919 | 41.50 → 38.02 | −8.4% | 684.8 → 814.8 | **+19.0%** |
| elastic/kibana | 10,216 | 106.10 → 107.76 | +1.6% | 2,194.0 → 2,194.2 | +0.0% |
| n8n-io/n8n | 11,991 | 60.16 → 57.18 | −5.0% | 1,823.9 → 1,824.0 | +0.0% |
| twentyhq/twenty | 22,033 | 60.94 → 56.21 | −7.8% | 1,325.2 → 1,325.8 | +0.0% |
| aws/aws-sdk-go-v2 | 26,520 | 334.78 → 472.08 | **+41.0%** | 2,254.9 → 2,254.7 | −0.0% |

The decision used a follow-up at the same `main`/1ec9320 commit: three flag-off runs ([35736240399](https://github.com/onsager-ai/tolmap/actions/runs/35736240399), [35736256364](https://github.com/onsager-ai/tolmap/actions/runs/35736256364), [35736270568](https://github.com/onsager-ai/tolmap/actions/runs/35736270568)) and three `--terrain` runs ([35736232192](https://github.com/onsager-ai/tolmap/actions/runs/35736232192), [35736249067](https://github.com/onsager-ai/tolmap/actions/runs/35736249067), [35736263167](https://github.com/onsager-ai/tolmap/actions/runs/35736263167)). Each run built all eight repositories. Medians from their `results.json` artifacts:

| repo | wall s, off → on | Δ% | peak RSS MB, off → on | Δ% |
|---|---:|---:|---:|---:|
| django/django | 3.30 → 3.55 | +7.6% | 121.9 → 121.9 | +0.1% |
| crawlab-team/crawlab | 1.39 → 1.39 | +0.0% | 52.5 → 52.5 | 0.0% |
| langgenius/dify | 26.29 → 23.76 | −9.6% | 485.9 → 485.3 | −0.1% |
| microsoft/vscode | 32.50 → 33.21 | +2.2% | 684.4 → 814.7 | **+19.0%** |
| elastic/kibana | 104.00 → 106.19 | +2.1% | 2,194.0 → 2,194.1 | +0.0% |
| n8n-io/n8n | 55.21 → 57.31 | +3.8% | 1,824.2 → 1,824.1 | 0.0% |
| twentyhq/twenty | 61.12 → 65.59 | +7.3% | 1,325.1 → 1,325.4 | +0.0% |
| aws/aws-sdk-go-v2 | 430.83 → 477.32 | +10.8% | 2,254.7 → 2,254.7 | 0.0% |

Every median wall-time movement is within the observed runner noise: the on/off direction still reverses for dify, and the original aws +41% single-run outlier contracts to +10.8%. Peak RSS is unchanged to rounding except for **`microsoft/vscode` at +19%**, which repeats the original signal. This matches finding 16's cost model: terrain adds a smaller clustering pass per eligible district without retaining another repository-sized graph.

### Viewer checks

`tsc -b` and `oxlint` clean (oxlint's one warning, `useJobProgress.ts`, is finding 16's pre-existing one, unchanged). `npx tsx web/scripts/check-pinch-math.ts`: 4/4. `pnpm check:view` against a local swap of its hardcoded `n8n-io/n8n` map for `langgenius/dify` (n8n exceeds this task's browser-render ceiling; the swap is local-only, never committed): **46/47 pass**. The one failure, "multi-polygon district hover," is unrelated to this change and pre-existing at this pinned commit: the check needs a district with a disconnected multi-polygon `blob` and names `django/django`'s "sessions" district as the known one, but neither the terrain build nor a plain (flag-off) build of `django/django` at this commit has any multi-polygon district left (`districts[d].blob.length` is 1 for all 12) — geometry drift since that comment was written, not a terrain regression. `perf-bench.mjs --check-single-paint` against a production preview (`npx vite build` + `vite preview --port 5185`, never `pnpm build`) for `django/django` and `langgenius/dify`, both viewports: **4/4 pass** (`drawsOnLoad=1` everywhere) — `drawTerrain`'s new per-element area math runs inside the existing single paint, not an extra one.

The CLAUDE.md viewer check (districts named, landmarks listed, tapping a district/file/symbol produces a card, plus sub-district/parcel/arterial owed since #47) ran against `langgenius/dify` on desktop (1440×900) and an emulated phone (390×844, Playwright touch events) via a real vite dev server on port 5184: districts and landmarks are listed; tapping a district, a sub-district, a parcel, a file and a symbol each produced a card with the right content on both viewports. **Tapping an arterial could not be checked in a live browser**: `langgenius/dify`, `microsoft/vscode`, `elastic/kibana` and `twentyhq/twenty` — every terrain map at or under this task's ~6.5k-file browser ceiling — measured **zero arterials** (only `n8n-io/n8n`, 12, and `aws/aws-sdk-go-v2`, 3, have any, both over the ceiling). An arterial's hit target uses the identical delegated `f:<file>` click path an ordinary file dot uses (verified by reading `MapRenderer.ts`, not run), and a plain file tap on that same path was confirmed working above; a Node-only structural check confirmed every arterial's `file` and every `links` entry is a valid index into `doc.N` on both `n8n-io/n8n` (12 arterials) and `aws/aws-sdk-go-v2` (3) with zero out-of-range indices. That is corroborating, not a substitute for an actual tap. **No real phone was used for any of this** — every touch interaction above is Playwright's emulated touchscreen, not a physical device.

### What is unmeasured

- **Arterial tap, live.** See above — structurally verified, not clicked.
- **A phone.** Every touch check in this and finding 16 is Playwright's touch emulation.
- **The two owner decisions issue #54 asks for**: whether `--terrain` becomes the default above some file count (or for all repos, or stays opt-in — the default map is byte-for-byte unaffected by anything in this finding, since only viewer zoom thresholds changed), and whether `TOLMAP_TERRAIN` gets enabled on staging or production. Both need the cost table above and are not decided here.
- **`aws/aws-sdk-go-v2`'s wall-time cost, mechanistically.** The cost table above flags it; nothing here traced which of its many eligible districts is driving it or whether it is the recursive-Leiden-per-district cost finding 16's model predicts or something else.

## 22. Terrain defaults on above 2,000 mapped files; staging goes first

**Superseded 2026-09-23:** the default and staging setting were removed by owner decision; see [finding 24](#24-terrain-was-built-measured-and-removed).

Ruled 2026-09-22 by the project owner after finding 21's terrain zoom and cost review:

1. **“Default on above 2,000 files.”** The CLI's default is now `auto`: terrain is enabled only when the graph contains **more than 2,000 mapped source files**, the same count serialized as `F`. Exactly 2,000 remains off. `--terrain` forces it on and `--no-terrain` forces it off. The threshold is named once in `src/geometry.rs`. Finding 21's follow-up medians of three GitHub runner builds with terrain on and off put build-time changes within runner noise; peak memory was unchanged except for vscode at +19%.
2. **“Staging now, prod after a look.”** `TOLMAP_TERRAIN` accepts `false`, `auto`, and `true`, but an unset or invalid value still resolves to `false`. Railway staging explicitly sets `auto`; Fly production leaves the setting absent until the owner approves production after inspecting staging.

At the pinned counts in `eval/corpus.toml`, **39 successfully built corpus repositories** now clear the automatic threshold:

- Large band: `DataDog/datadog-agent` (7,968), `angular/angular` (3,082), `ant-design/ant-design` (2,166), `apache/airflow` (4,637), `apache/superset` (3,668), `aws/aws-sdk-go` (2,386), `backstage/backstage` (2,844), `calcom/cal.com` (4,432), `cockroachdb/cockroach` (6,785), `elastic/beats` (3,196), `getsentry/sentry` (4,583), `go-gitea/gitea` (2,254), `googleapis/google-cloud-go` (7,288), `grafana/grafana` (5,498), `hashicorp/terraform-provider-aws` (4,931), `hashicorp/terraform-provider-azurerm` (3,490), `hashicorp/terraform-provider-google` (2,508), `hashicorp/vault` (2,093), `huggingface/transformers` (3,020), `langgenius/dify` (6,347), `mattermost/mattermost` (4,667), `microsoft/vscode` (5,919), `odoo/odoo` (6,178), `prefecthq/prefect` (2,096), `pulumi/pulumi-gcp` (6,831), `storybookjs/storybook` (3,302), `supabase/supabase` (5,292), and `vercel/next.js` (2,035).
- Ultra band: `Azure/azure-sdk-for-python` (40,129), `aws/aws-sdk-go-v2` (26,520), `elastic/kibana` (10,216), `googleapis/google-cloud-python` (39,964), `home-assistant/core` (10,209), `kubernetes/kubernetes` (8,570), `microsoftgraph/msgraph-sdk-python` (16,636), `n8n-io/n8n` (11,991), `pulumi/pulumi-aws` (8,757), `pulumi/pulumi-azure-native` (11,650), and `twentyhq/twenty` (22,033).

All nine acceptance fixtures contain at most 851 mapped files. None crosses the threshold, no fixture was re-derived or changed, and the CI offline parity gate continues to prove their default artifacts byte-identical.

## 23. Node-relative pruning fixes the global-outlier collapse, with one owner-accepted retention regression

Issue #57 confirmed finding 10's mechanism at corpus scale. `blend()` first
normalises each signal on total mass, then divides every blended edge by one
global maximum. `absolute` applies a fixed 0.02 floor after that rescale. One
dominant edge therefore sets the scale for the entire repository and can push
otherwise useful local links below the floor. On msgraph-sdk-python and
date-fns, respectively 99.9982% and 99.8449% of candidate edges fell below
it. `node-relative` keeps the union of each endpoint's top 14 edges whose
weight is at least 2% of that endpoint's strongest incident edge, so a global
outlier no longer crushes unrelated neighborhoods.

PR #70 measured the alternatives without changing the default. The headline
comparison below is the old `absolute` route against `node-relative`, from
[absolute build 35736394530](https://github.com/onsager-ai/tolmap/actions/runs/35736394530),
[absolute dump 35736420144](https://github.com/onsager-ai/tolmap/actions/runs/35736420144),
[node-relative build 35736407326](https://github.com/onsager-ai/tolmap/actions/runs/35736407326),
and [node-relative dump 35736432761](https://github.com/onsager-ai/tolmap/actions/runs/35736432761).
“Below floor” uses each route's own rule, so the node-relative percentage is
the share rejected by both endpoints' local thresholds.

| repository | q, absolute → node-relative | districts | kept edges | below floor |
|---|---:|---:|---:|---:|
| microsoftgraph/msgraph-sdk-python | 0.0000 → **0.6079** | 10,030 → **45** | 1 → **43,713** | 99.9982% → **0.0000%** |
| date-fns/date-fns | 0.4922 → **0.5155** | 963 → **11** | 8 → **4,842** | 99.8449% → **0.0194%** |
| Azure/azure-sdk-for-python | 0.9942 → 0.9924 | 3,411 → **1,185** | 225 → **215,925** | 99.9652% → **0.0431%** |
| elastic/kibana | 0.9775 → 0.7625 | 1,553 → **57** | 1,265 → **37,990** | 96.8873% → **0.2461%** |
| crawlab-team/crawlab | 0.5476 → 0.4056 | 13 → **7** | 386 → **4,840** | 98.6007% → **1.3306%** |
| sqlalchemy/sqlalchemy | 0.5202 → 0.4318 | 8 → 6 | 1,168 → **2,445** | 82.3343% → **0.6063%** |
| prometheus/prometheus | 0.5397 → 0.5430 | 11 → 9 | 3,479 → 3,499 | 1.1927% → **0.0000%** |
| langgenius/dify | 0.7470 → 0.7470 | 133 → 133 | 23,967 → 23,967 | 0.0000% → 0.0000% |
| microsoft/vscode | 0.5325 → 0.5325 | 21 → 21 | 47,617 → 47,617 | 0.0000% → 0.0000% |
| django/django | 0.5092 → 0.5092 | 12 → 12 | 3,369 → 3,369 | 0.0000% → 0.0000% |

The first warm-start pass covered the established stability set. Absolute
[run 35741348832](https://github.com/onsager-ai/tolmap/actions/runs/35741348832)
and node-relative
[run 35741357110](https://github.com/onsager-ai/tolmap/actions/runs/35741357110)
were identical on all four:

| repository | absolute | node-relative | Δ |
|---|---:|---:|---:|
| django | 0.9186 | 0.9186 | 0.0000 |
| celery | 0.9814 | 0.9814 | 0.0000 |
| httpx (200 commits back) | 0.9565 | 0.9565 | 0.0000 |
| scrapy | 0.8512 | 0.8512 | 0.0000 |

The follow-up covered repositories whose current maps actually moved, using
the rule fixed before dispatch that node-relative could be at most 0.005 below
absolute on any repository. Absolute
[run 35743254814](https://github.com/onsager-ai/tolmap/actions/runs/35743254814)
and node-relative
[run 35743262096](https://github.com/onsager-ai/tolmap/actions/runs/35743262096)
produced:

| repository | absolute | node-relative | Δ |
|---|---:|---:|---:|
| sqlalchemy | 0.9881 | 0.9643 | **−0.0238** |
| prometheus | 0.9825 | 0.9857 | +0.0032 |
| kubernetes | 0.9230 | 0.9699 | +0.0469 |
| n8n | 0.9594 | 0.9691 | +0.0097 |
| msgraph-sdk-python | 0.8227 (8,327 districts, 11.8 GB) | 0.9355 (44 districts, 2.1 GB) | +0.1128 |
| crawlab | 0.0 | 0.0 | uninformative |
| date-fns | 0.0 | 0.0 | uninformative |

The sqlalchemy result plainly breaks the pre-set 0.005 rule: retention falls
from 0.9881 to 0.9643, a regression of 0.0238. On 2026-09-23 the project owner
reviewed that result and decided **“Switch anyway.”** This finding records the
override rather than weakening the rule after seeing the data.

Under absolute pruning, 82.3343% of sqlalchemy's candidate links fall below
the floor. **Inference, not measurement:** because so few links remain, its
districts appear to be held stable largely by `merge_tiny`'s directory
fallback; directory placement is stable by construction, so replacing the
global floor can reduce measured retention even while restoring graph links.
No run here isolated or counted fallback-driven assignments directly.

Crawlab and date-fns do not supply retention evidence in the second table.
Both were restructured within the 300-commit window: crawlab moved from Python
to Go, while date-fns moved files from `src/` to `pkgs/core/`. Their common-file
retention denominator collapses, so 0.0 versus 0.0 says nothing about either
prune route.

These remote two-commit runs stand in for `eval/batch_stability.py` in this
change. That script imports the frozen Python reference, which has no
`PruneVariant` route and cannot measure a Rust default switch. The remote mode
uses the product pipeline at both commits, supplies the earlier map through
`--previous-map`, and reports the same common-file district retention concept.

The owner-approved result is that `node-relative` is now the default for CLI
builds, blend dumps, polyglot reports, and service jobs. `absolute`,
`percentile`, and `pre-rescale` remain selectable so every table above stays
reproducible.

### Fixture re-derivation

[Remote build 35756191891](https://github.com/onsager-ai/tolmap/actions/runs/35756191891)
rebuilt all nine exact fixture pins with the new Rust default, each fixture's
recorded parcel setting, and its naming cache seeded from the committed map.
Only sqlalchemy and prometheus move between the absolute and node-relative
routes, matching PR #70's prediction:

| fixture | districts | q | placement against old fixture | Δq |
|---|---:|---:|---:|---:|
| sqlalchemy | 8 → **6** | 0.5202 → **0.4318** | 74.03% | 0.0884 |
| prometheus | 11 → **9** | 0.5397 → **0.5430** | 91.89% | 0.0033 |

Both re-derived maps keep `F`, `E`, `S`, and `U` byte-equivalent to their old
fixtures; `L` moves with district membership. The other seven committed maps
and every `data/ci/*` graph remain untouched. The frozen Python pipeline has no
node-relative route, so these two fixtures explicitly record
`generator = "rust-node-relative"`; `eval/verify_fixtures.py` continues to
byte-verify the seven Python-reference fixtures and reports these two as
remote-generated rather than claiming it can reproduce them.

Changed memberships reopen the deterministic naming cache, as in finding 15.
The matched district renames are recorded rather than hidden:

| fixture | previous name | re-derived name |
|---|---|---|
| sqlalchemy | engine | mysql & engine |
| sqlalchemy | dialects | dialects & postgresql |
| sqlalchemy | util | util & engine |
| prometheus | tsdb storage | tsdb |
| prometheus | labels & parsing | model |
| prometheus | promql & rules | promql |
| prometheus | remote write | storage |
| prometheus | web api | web & notifier |
| prometheus | chunk encoding | chunkenc & tsdb |
| prometheus | kubernetes discovery | kubernetes |

## 24. Terrain was built, measured and removed

Owner decision 2026-09-23 (AskUserQuestion, session 266f58ce): **“Remove terrain entirely.”** The owner compared phone screenshots of dify and vscode with terrain off and on at 2× and 4×. All four reasons were selected:

- **districts look emptier** (big districts become a few sparse clumps with blank space instead of the dense dot spread);
- **the square parcel grid looks artificial**;
- **harder to read** (sub-district clusters and outlines add clutter without telling you anything);
- **not useful enough** (it doesn't earn its complexity).

Terrain (#47) subdivided oversized districts into arterial files, organic sub-district contours, and an address-ordered square parcel grid. It adjusted positions within eligible districts and overlaid their geometry in the viewer; the top-level partition was unchanged. Finding 16 and `docs/TERRAIN.md` record the decomposition and geometry measurements. Finding 21 measured zoom gates across eight maps: median sub-district establishment at zf 1.874 on phone and 0.631 on desktop, before full file reveal at 2.578 and 0.875; parcel establishment followed at 3.419 and 1.175. Six of 41 eligible districts were exceptions near the eligibility floor. Repeated remote cost runs put median wall-time changes within runner noise, while vscode's peak RSS rose 19%. Finding 22 then enabled CLI terrain automatically above 2,000 files and selected `auto` for staging. Those policy choices are superseded by this removal.

The motivating graph observations remain true. Django's locale files form a plat with weak internal structure; n8n's integration directories accumulate under one large district through shared imports; and dify's unconnected Python files remain a separate issue (#40). Removing the terrain presentation does not claim those observations were false. The viewer now ignores legacy map documents' extra `terrain` key, while new documents omit it. Ordinary per-file weighted-Voronoi plots (`P`, `--no-parcels`), island fade, ranked pins and badges, package layout overlays, and node-relative pruning remain.

## 25. Dify's disconnected Python files came from a nested import root, not pruning

Issue #40 began with 465 zero-edge files in dify at `2590d90` and 255 in n8n at `02dc131`. The corpus and pipeline have moved since those pins. At the current `eval/corpus.toml` commits, dify `9a0961a` has 6,347 parsed files, n8n `d9dc457` has 11,991, and django `dd6f6b1` has 851. The following diagnosis came **before** the resolver change from [remote `dump-blend` run 35825859989](https://github.com/onsager-ai/tolmap/actions/runs/35825859989), using `--all-sources` and the node-relative default. The dump includes each zero-kept-edge file and its import specifiers with resolution outcomes. The file-level cause table is mutually exclusive; “no parsed target” means none of that file's imports resolved to another parsed file. It does not assert that an external dependency is broken.

To isolate the pin difference, [remote run 35828600689](https://github.com/onsager-ai/tolmap/actions/runs/35828600689) rebuilt **the issue's exact dify and n8n commits** with the pre-fix diagnostic binary and this PR's resolver, using a temporary copy of `eval/corpus.toml` and no workflow change. Even at those commits, the historical 465/255 counts do **not** reproduce on today's pipeline. Dify has 775 zero-edge files before this fix (748 Python, 27 TypeScript) and 294 after (267 Python, 27 TypeScript); n8n has 136 before and after. The pre-fix dify breakdown is 543 Python files with unresolved imports, 205 Python files with no static imports, 19 TypeScript files with unresolved imports, and 8 TypeScript files with no static imports; zero had a candidate edge removed by prune. **At least 361 of those 543 Python files** have an exact unprefixed import-head match under `api/`. The original 465/255 was measured on an earlier pipeline; the old binary and its exact graph are not part of this run, so this finding does not assign a cause to each of those historical individual files. It identifies the dominant actionable cause in the now-reproducible graph at both the issue and current pins.

| repo / language | zero kept edge | no static imports | imports, no parsed target | candidate removed by prune |
|---|---:|---:|---:|---:|
| dify / Python | **751** | 205 | **546** | 0 |
| dify / TypeScript | 29 | 9 | 20 | 0 |
| n8n / TypeScript | 136 | 44 | 92 | 0 |
| django / Python | 215 | 215 | 0 | 0 |

Dify's dominant actionable cause is a **source-root mismatch**. The selected Python source is the repository root (`--all-sources`), so `api/core/x.py` was indexed as `api.core.x`. The API project's code imports `core.x`; its `api/Dockerfile` sets `WORKDIR /app/api`. Among the 546 dify Python files whose imports found no parsed target, **at least 365** import a head that exactly matches an already parsed `api/<head>.py` or `api/<head>/__init__.py` when the `api/` prefix is removed. That count deliberately omits child-module-only hits and inbound edges, so it is a lower bound on files affected by this mechanism. No dify zero-edge file had even a candidate edge that pruning later removed. The n8n population is different: 35 of its 92 unresolved TypeScript files import a `.vue` file, which source collection does not parse. Django's 215 are mainly locale modules with no static imports, not a Python root mismatch.

Python's [import system](https://docs.python.org/3/reference/import.html#the-path-based-finder) searches absolute module names from `sys.path` entries; a process running in `api/` can therefore load `core.x`. The Rust resolver now recognises a nested Python project only when a `pyproject.toml` or `setup.py` marks its directory, scopes unprefixed names to that project, and still accepts a target only if that exact file was parsed. Repository-root names remain available. Relative imports must stay within their containing package, following [the language reference](https://docs.python.org/3/reference/simple_stmts.html#the-import-statement). This does not invent edges to external dependencies or `.vue` files.

[After `dump-blend` run 35826580911](https://github.com/onsager-ai/tolmap/actions/runs/35826580911) and [after map run 35826580869](https://github.com/onsager-ai/tolmap/actions/runs/35826580869) used the same pinned sources and default pruning as the [before map run 35825973796](https://github.com/onsager-ai/tolmap/actions/runs/35825973796). Counts are files with **no edge of any signal in the kept weighted graph**, not files absent from the drawn import list.

| repo | zero-edge files, before → after | districts | q | island districts | kept weighted edges |
|---|---:|---:|---:|---:|---:|
| dify | **780 → 293** (Python 751 → 264; TS 29 → 29) | 133 → 78 | .7470 → .7374 | 22 → 18 | 23,967 → 28,668 |
| n8n | 136 → 136 | 80 → 80 | .7793 → .7793 | 57 → 57 | 58,350 → 58,350 |
| vscode | 3 → 3 | 21 → 21 | .5325 → .5325 | 6 → 6 | unchanged |
| django | 215 → 215 | 12 → 12 | .5092 → .5092 | 1 → 1 | 3,369 → 3,369 |

Dify's 7,038 new directed import pairs lose **zero** old pairs, with identical file order. Its 91 unconnected districts fall to 41. N8n, vscode and django maps are byte-identical to their before maps after removing the new additive `coverage` field. The residual 293 dify files remain reported, not patched over: 166 Python files have no static imports, 98 have imports without a parsed target, and the 29 TypeScript files are unchanged. The map schema's optional `coverage` field records total and per-language zero-edge counts, so old documents omit the viewer footer line while new ones show the exact kept-graph count.

[Remote fixture build 35826580990](https://github.com/onsager-ai/tolmap/actions/runs/35826580990) rebuilt all nine exact fixture pins. Every map is byte-identical to a fresh [main build 35826832939](https://github.com/onsager-ai/tolmap/actions/runs/35826832939) after removing only `coverage`; no committed fixture or `data/ci` graph changed. Eight fixtures have 100% placement and 0.0000 q delta against their committed reference. Django has 75.2% placement and 0.0031 q delta **on both main and this branch** at its fixture pin, so that existing reference mismatch cannot be attributed to this resolver change. The [CI offline gate](https://github.com/onsager-ai/tolmap/actions/runs/35826621652) passed flask, httpx and synthetic polyglot parity, module-resolution and determinism checks; web build and lint passed locally (lint retained its existing `useJobProgress.ts` warning).

## 26. Folder labels use screen area; unconnected files leave the map

Owner decisions 2026-09-23 (AskUserQuestion, session 266f58ce): **“Folder labels on zoom-in”** and **“Collapse the island ring.”** Labels reuse #75's district path rows. A row needs at least 8% of its district's files, and at most four rows per mainland district qualify. The label sits at the median of its members' existing dot coordinates; neither file positions nor district geometry changes. It appears above fit zoom only when the square root of the district's world area times the current scale reaches **25% of the viewport's shorter side**. On the 390×844 phone dify view, the `workflow & nodes` district's area is 1.5138 world units², its fitted scale is 72.21 px/unit, and its equivalent side is **88.8 px at fit** versus the **97.5 px gate**. At the district's 2.2× zoom it is **195.5 px**. Its `web/app/components/workflow/` breakdown row holds **755/887 files (85.1%)**, so the label is eligible. Labels lose collision ties to district text and pins; a nearby district name can shorten the displayed tail from two path segments to one.

The terrain-off dify document captured before #79 has **420 files in 91 unconnected districts**; finding 25 records how the resolver update reduces a new build at the current pin to 293 zero-edge files and 41 unconnected districts. These files no longer produce SVG dots, labels, pins, selection rings or zoom bounds. The footer chip reports the file count and opens a folder-grouped list; a file there, in search, or in a deep link opens a card without moving the map. Real islands retain their map geometry and fade. The web viewer check compares visible dot `cx`/`cy` with `origin/main` at the same zoom and confirms the ring is absent after zooming out. This viewer-only change requires no schema or upstream layout rebuild.

## 27. The prototype redesign: four owner decisions that overturn earlier rules

Owner decisions, 2026-09-23. The owner built a visual prototype outside this repository on seven Python repositories (dify, superset, saleor, zulip, langchain, airflow, sentry). It ran runtime wrappers over the frozen reference at `1ec9320` and did not modify this repository. The owner handed it over with four decisions in its `HANDOFF_PROMPT.md` §0 and said "Follow the handoff md attached" in session `16030105-19f0-4a84-933b-c5953f23c6b3` (transcript line 13, 2026-09-23T13:43:38Z). The go to implement is at line 181, 2026-09-23T14:16:42Z. Tracking issue #82. The prototype's documents are historical records and use its older vocabulary; `docs/GLOSSARY.md` has the owner's final terms.

**D1 — symbols on the map, gated by level of detail.** This overturns `CLAUDE.md`'s "the map stops at the file" and the README section that said symbols were tried twice and reverted twice. Both attempts drew symbols at every zoom, and the overview lost the district silhouette. The prototype draws district outlines and roads at the overview. A file draws its symbols once its footprint is ≥ 40 px on screen, and a class expands its members once its short side is ≥ 110 px. The silhouette therefore survives at the overview, which was the reason for the earlier reversals.

**D2 — file footprints (`P`) on by default.** This overturns the 2026-09-22 decision to hide the plots geometry. Every map carries footprints, and the acceptance fixtures are re-recorded with `P`. Footprint area follows **code lines**: lines that are not blank, comment-only or docstrings. In dify, code lines are 76.8% of 370,771 total lines and docstrings are 7.1%. Areas are comparable only within one district, because the district's total area comes from the layout.

**D3 — neighbourhoods as gutters and shading.** Finding 24 removed terrain partly because sub-district outlines "add clutter" and "the square parcel grid looks artificial". A neighbourhood is a second-level group of files inside a district. The prototype draws neighbourhoods with no grid, only white gutters and three alternating shades, and the owner judged that readable. This is a different presentation, not a return of terrain.

**D4 — partition and layout unchanged.** The prototype grouped files by import-only Leiden with 50-seed consensus and packed districts tangentially along a maximum spanning tree. Its preregistered multi-repo test (parameters frozen from dify) passed both district-level adjacency criteria on only **2 of 6** new repositories: strongest neighbour = nearest ≥ 50%, and distance/strength ρ ≤ −0.30. The prototype's own rule was that < 4/6 means the method did not generalise, so neither goes into the default pipeline. Results at prototype scope:

| repo | files | districts | strongest = nearest | ρ | neighbourhood connected | area ~ code lines r | files without footprint |
|---|---:|---:|---|---|---:|---:|---:|
| dify (control) | 1854 | 14 | 0.57 ✓ | −0.57 ✓ | 1.00 | 0.88 | 17 |
| superset | 1102 | 14 | 0.57 ✓ | −0.18 ✗ | 1.00 | 0.74 | 11 |
| saleor | 1138 | 16 | 0.50 ✓ | −0.37 ✓ | 1.00 | 0.89 | 5 |
| zulip | 904 | 13 | 0.45 ✗ | −0.23 ✗ | 1.00 | 0.76 | 19 |
| langchain | 1737 | 9 | 0.44 ✗ | −0.44 ✓ | 1.00 | 0.71 | 111 |
| airflow | 591 | 9 | 0.56 ✓ | −0.65 ✓ | 1.00 | 0.77 | 5 |
| sentry | 4587 | 20 | 0.29 ✗ | −0.20 ✗ | 1.00 | 0.76 | 35 |

These numbers are the prototype's own and were not independently re-run here.

What carries over, per D1–D3: nested footprints (district › neighbourhood › file), neighbourhood gutters, district roads, hub rings, symbol cards gated by zoom, and the interaction model. The table also shows two defects the port has to fix rather than inherit. Every repository has files that received no footprint, and the port must guarantee every file a minimum area. Early prototype lines were also anchored on power-diagram sites rather than displayed footprint centroids, and only 17% of sites fell inside their own cell. The prototype's 10 layout metrics will be reported for each geometry change, but they are not a gate, with two exceptions that are hard targets: neighbourhood connectivity ≥ 95%, and files without a footprint = 0.

Consequence of D4: the product will not match the prototype's screenshots one-to-one. District placement, outlines, islands and the unconnected list stay as findings 17, 24 and 26 left them. The ported parts are what is drawn inside and between the outlines.

Two further owner choices, made the same day through AskUserQuestion (transcript line 163): phones keep the full-screen map shell, where one finger pans (the prototype's scrolling-page layout is not adopted). Hub rings (files with fan-in ≥ 30) replace hub pins. Entry, bridge and hazard stay as smaller ranked pins, capital pins are dropped because district names are always shown, and all five landmark kinds stay in the sidebar.

## 28. Code-line weights change footprint area, and the current solver can lose more cells

Owner decision D2 (finding 27, 2026-09-23) makes file footprint area follow **code lines**, excluding blank lines, comment-only lines and Python module/class/function docstrings. This supersedes `docs/PIPELINE.md`'s old statement that parcels track line count. The prototype's dify measurement was 76.8% code in 370,771 lines; that was a different source selection from this product's `--all-sources` dify build, so the percentages are not directly comparable.

[Actions comparison run 35874556711](https://github.com/onsager-ai/tolmap/actions/runs/35874556711) built each pinned repository with this branch and `compare_ref=main` on standard runners. The table reports summed `N[*][3]` total lines, summed new `C` code lines, and **median per-district Pearson r** between shoelace polygon area and file code lines / total lines. Each r uses only files with a `P` polygon in that district; a district needs at least three such files and nonzero variance. For the main polygons, the code-line r uses this branch's `C` aligned by the identical `F` order. All eligible districts, including their individual r values and sample counts, are in [the per-district CSV](measurements/code-lines-district-correlations.csv). An empty r there means the district did not meet the Pearson conditions.

| repo | total lines | code lines (share) | eligible districts | branch r code / loc | main r code / loc | missing P branch / main | wall s branch / main |
|---|---:|---:|---:|---:|---:|---:|---:|
| django/django | 150,765 | 103,970 (69.0%) | 12 | 0.956 / 0.941 | 0.940 / 0.959 | 48 / 63 | 3.28 / 3.15 |
| encode/httpx | 8,850 | 5,773 (65.2%) | 4 | 0.742 / 0.742 | 0.802 / 0.740 | 1 / 1 | 0.28 / 0.27 |
| langgenius/dify | 907,285 | 763,138 (84.1%) | 32 | 0.894 / 0.880 | 0.900 / 0.884 | 505 / 470 | 24.54 / 23.77 |
| pallets/flask | 9,537 | 4,281 (44.9%) | 4 | 0.921 / 0.895 | 0.874 / 0.899 | 0 / 0 | 0.41 / 0.40 |
| prometheus/prometheus | 201,894 | 156,902 (77.7%) | 10 | 0.978 / 0.975 | 0.973 / 0.979 | 42 / 22 | 3.37 / 3.22 |
| vuejs/core | 57,564 | 46,465 (80.7%) | 8 | 0.981 / 0.973 | 0.976 / 0.984 | 5 / 2 | 1.06 / 1.00 |

The branch median r for code lines exceeds its loc r in five of six repositories (httpx is essentially tied), but it does not exceed main's code-line r in every repository. This is not an optimizer guarantee: district boundaries, point locations and a finite raster constrain parcel areas. Dify has 78 districts but only 32 meet the correlation conditions; 43 have no `P` polygon at all, primarily the unconnected districts. Its eligible per-district branch code-line r spans **−0.536 to 1.000**, so the median does not imply every district fits well.

The count of files without `P` **did change**, contrary to the initial expectation that only area would move. The existing power diagram can assign no cell to a file when targets change; dify gains 35 missing footprints, prometheus gains 20, vue gains 3, while django loses 15. These are measured counts, not fixed by this PR. The separate nested-footprints work for #82 has the explicit zero-missing-footprint target; the present PR keeps the parcels change to one weight helper to avoid competing with that rewrite.

After removing only `C` and `P` from both JSON documents and comparing their compact JSON bytes, **all six branch maps are identical to main** on every remaining key. That includes `F`, `N`, `E`, `L`, `S`, `U`, district membership and geometry, and coverage. The code-line count is additive; this PR does not move files or alter the graph.

## 29. Nested footprints cover every measured file without moving the district map

Owner decisions D2–D4 (finding 27, issue #82) require file footprints within neighbourhoods, no parcel grid, and unchanged district membership and layout. This implementation runs seeded Leiden on each district's induced kept weighted graph, recursively splits large groups toward about 30 files, then folds groups smaller than three into their most attached neighbour. An indivisible group gets deterministic path chunks. Single-file districts remain single-file neighbourhoods. Labels use the most common member parent directory and its shortest suffix unique within the district, with sorted tie breaks. None of these operations changes the district partition.

Nested raster power diagrams allocate each district blob first to neighbourhoods by summed code-line weight, then each neighbourhood to files by individual code-line weight. Both tiers use a capped power difference and centroid relaxation. The neighbourhood allocation grows connected regions from seeds; the file allocation reserves one pixel for every file before solving, because an unconstrained power diagram previously swallowed sites and left files with no cell (findings 27–28). Displayed polygon centroids are emitted separately as `footprint_centroids`: `N` coordinates remain layout inputs for warm starts and retain the existing map position. The optional, index-aligned `file_neighbourhoods` and `neighbourhoods` fields carry membership, labels and polygon rings; `P` keeps its existing shape. Polygon rasterization uses even-odd rings so district holes are respected. An empty district mask receives a local fallback region, so no district is skipped. Code lines come from `C`, with LOC as the old-document fallback.

[Paired Actions builds with footprints](https://github.com/onsager-ai/tolmap/actions/runs/35879951742) and [without footprints](https://github.com/onsager-ai/tolmap/actions/runs/35879969118) used the same implementation commit `58815a547f2452e6b7541c79bd1ec50698f67270`, each repository's pinned `eval/corpus.toml` commit, `--all-sources`, and standard runners. Wall time includes the entire build, so the difference is an upper bound on added geometry cost, not an isolated geometry timer. RSS is the build's peak KiB. Small negative wall-time differences are runner noise. All six jobs on each run succeeded.

| repository | files | districts | neighbourhoods | wall s on / off | peak RSS KiB on / off | size min / p10 / median / p90 / max | size bins 1–2 / 3–10 / 11–40 / 41+ |
|---|---:|---:|---:|---:|---:|---|---|
| django/django | 851 | 12 | 63 | 1.89 / 2.19 | 44,624 / 44,604 | 4 / 6 / 13 / 20.8 / 39 | 0 / 22 / 41 / 0 |
| langgenius/dify | 6,347 | 78 | 512 | 20.08 / 12.33 | 198,980 / 198,864 | 1 / 3 / 11 / 26 / 40 | 26 / 227 / 259 / 0 |
| vuejs/core | 239 | 8 | 15 | 0.64 / 0.84 | 31,568 / 31,088 | 5 / 8.4 / 12 / 29.8 / 37 | 0 / 6 / 9 / 0 |
| prometheus/prometheus | 631 | 11 | 48 | 3.06 / 2.36 | 43,372 / 43,312 | 1 / 3 / 13 / 25.3 / 36 | 1 / 21 / 26 / 0 |
| n8n-io/n8n | 11,991 | 80 | 1,041 | 35.58 / 30.15 | 256,220 / 254,808 | 1 / 3 / 9 / 25 / 42 | 7 / 604 / 429 / 1 |
| aws/aws-sdk-go-v2 | 26,520 | 280 | 1,625 | 200.11 / 150.34 | 1,043,428 / 1,043,324 | 1 / 3 / 13 / 33 / 42 | 1 / 613 / 997 / 14 |

The two largest builds remain below the proposed 2× wall-time concern: n8n is 1.18× and AWS is 1.33× its own `--no-parcels` build. Dify is 1.63×. The AWS result is a 49.77-second increment in a 200.11-second full build, with 104 KiB higher peak RSS. The result artifacts contain each map, log and measurement JSON; the combined results are attached to both runs.

The port of the prototype's ten metrics reads the product JSON and uses each **displayed footprint centroid** for every positional metric. `overlap` counts district pairs whose centroid-and-radius envelopes overlap; `grid multiple` compares the import edge share of within-district Delaunay neighbours to sampled pairs. The two hard targets alone are gated in CI. Other values diagnose the unchanged layout and geometry without changing the acceptance criteria. `r` is the prototype's global footprint-area/code-line Pearson correlation; the extra per-district median is reported because D2 says areas are comparable only within a district.

| repository | district strongest=nearest | district ρ | neighbourhood strongest=nearest | neighbourhood ρ | grid multiple | overlap | connected | global area/code r | missing P | compactness | within-district median r |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| django/django | .083 | −.190 | .155 | −.254 | 2.525 | 7 | 100% | .503 | 0 | .759 | .793 |
| langgenius/dify | .000 | −.282 | .115 | −.184 | 5.914 | 29 | 100% | .427 | 0 | .785 | .965 |
| vuejs/core | .250 | −.308 | .375 | .039 | 2.035 | 1 | 100% | .731 | 0 | .590 | .960 |
| prometheus/prometheus | .200 | −.435 | .243 | −.352 | 2.046 | 5 | 100% | .697 | 0 | .736 | .903 |
| n8n-io/n8n | .113 | −.241 | .167 | −.291 | 3.333 | 59 | 100% | .322 | 0 | .785 | .916 |
| aws/aws-sdk-go-v2 | .004 | −.059 | .194 | −.115 | 1.073 | 399 | 100% | .449 | 0 | .785 | .890 |

Thus the measured lower bound is **46,579 file footprints out of 46,579 files**, and **3,304 connected neighbourhoods out of 3,304**, across six distinct repositories. These are six measured maps, not a universal proof for every source tree. Neighbourhoods of one or two files in the size table occur where a district has too few members to fold into another group. On the paired django and prometheus maps, every common JSON key, including `F`, `N`, `E`, `L`, `S`, `U`, `districts`, and `q`, is identical between on and off. The only on-only keys are `P`, `file_neighbourhoods`, `neighbourhoods`, and `footprint_centroids`. The [CI gate](https://github.com/onsager-ai/tolmap/actions/runs/35880498909) builds the synthetic polyglot map three times with footprints and compares hashes for determinism; it also checks parity on offline flask/httpx maps and gates zero missing footprints plus at least 95% connected neighbourhoods on those and the synthetic maps. The frozen `data/*.json` reference maps were not overwritten with Rust geometry; that supersedes finding 27's initial re-recording expectation for this PR.

The viewer's demo maps are Rust Actions builds. The branch's [django map artifact](https://github.com/onsager-ai/tolmap/actions/runs/35879951742/artifacts/10759254666) and [dify map artifact](https://github.com/onsager-ai/tolmap/actions/runs/35879951742/artifacts/10758864894) provide the two maps for the follow-up viewer work; they are gitignored and not committed. `web/scripts/check-view-stability.mjs` still pins the earlier django and dify maps and must be regenerated from these branch builds when the viewer begins drawing neighbourhood gutters, three alternating shades and file footprints. Symbols and changes to the district partition or layout remain outside this PR.

## 30. Hierarchical symbols are a separate document, and resolved calls remain a lower bound

Finding 29 is reserved for the concurrent nested-footprints PR #85. This PR adds no symbol geometry or viewer drawing. The map's oracle-constrained `S` and `U` take their old path unchanged; complete symbols and their reference edges live in a sibling JSON document. Each district response includes its own symbols and both endpoints of every touching edge, retaining global indices. Symbol and module code-line counts reuse the exact line mask introduced by finding 28.

[Forward-order Actions run 35878554527](https://github.com/onsager-ai/tolmap/actions/runs/35878554527) built this branch, then `main`, on the same pinned clone. [Reverse-order run 35879341393](https://github.com/onsager-ai/tolmap/actions/runs/35879341393) built `main`, then this branch. Both used standard runners and the same four corpus pins. The two symbol documents for each repository have **identical SHA-256 hashes** across the runs, even with build order reversed. The [PR gate run 35878549474](https://github.com/onsager-ai/tolmap/actions/runs/35878549474) passed format, clippy, tests, generated types, offline parity, and three-build map-plus-symbol determinism.

Kind counts below are `class / function / method / nested function / interface / type / const`. Sizes are decimal MB of compact UTF-8 JSON; largest district is the serialized API response including remote endpoint rows. Resolution is resolved repository-symbol calls divided by all calls **inside symbols**. Module-level calls have no enclosing symbol and are outside that denominator.

| repo | kinds | edges | resolved / calls | symbols MB | largest district MB | branch / main s | main / branch s |
|---|---:|---:|---:|---:|---:|---:|---:|
| django/django | 1831 / 1196 / 7266 / 249 / 0 / 0 / 0 | 8,851 | 8,622 / 32,639 (26.4%) | 0.534 | d4 0.173 | 6.53 / 3.40 | 3.89 / 6.71 |
| langgenius/dify | 6714 / 11243 / 11387 / 2274 / 3 / 5314 / 11226 | 33,239 | 23,584 / 176,252 (13.4%) | 2.750 | d0 0.498 | 32.04 / 15.10 | 22.40 / 48.59 |
| prometheus/prometheus | 1047 / 2169 / 4944 / 54 / 338 / 285 / 262 | 4,609 | 6,035 / 42,569 (14.2%) | 0.414 | d1 0.117 | 10.80 / 3.34 | 3.21 / 10.19 |
| vuejs/core | 18 / 1056 / 471 / 170 / 254 / 324 / 554 | 4,398 | 3,188 / 7,877 (40.5%) | 0.171 | d1 0.070 | 2.45 / 1.04 | 1.03 / 2.36 |

The paired added wall time is **2.82–3.13 s django, 16.94–26.19 s dify, 6.98–7.46 s prometheus, and 1.33–1.41 s vue**. Each range contains the two build orders, not repeated samples sufficient for a confidence interval. The added pass reparses mapped files once to keep the parity-constrained extraction path intact. Dify's 2.750 MB document is close in scale to the prototype's ~2.8 MB, but the source selection differs and the new document has no card geometry, so their counts are not directly comparable.

Unresolved calls remain explicit: dify records 97,279 `local_or_builtin`, 34,208 `external`, 10,966 `dynamic`, 6,118 `unresolved_attribute`, 3,824 `instance_or_untyped`, and 273 `parent_class_method`. These sum with its 23,584 resolved calls to the 176,252 total. The method count is therefore a lower bound: instance types and parent-class method targets are deliberately not inferred. The full per-repository JSON artifacts contain the other three repositories' reason counts.

## 31. Every measured symbol gets a card, but tiny rings rule out the prototype's quantization

Owner decision D1 (finding 27, issue #82) puts nested class, method, function, and module-level-code cards inside a file footprint only after the viewer's zoom gate. The card pass runs after parcels and writes optional geometry to the separate symbols document. Each symbol has an index-aligned list of contours; the first is the exterior and later contours can describe holes with even-odd fill. A connected raster region may surround a sibling, so exporting only its outer contour produced visible sibling overlap. A bounded 12-point contour is used when safe; the pass retains more corners where a shortcut would claim a neighbouring raster pixel. Collinear boundary points are removed without changing the shape. Children use their parent's owned raster region, with a top header band for the parent's own lines. A reserved subpixel rectangle gives a symbol a ring when its parent runs out of raster cells. Go methods whose receiver type is declared in another file are top-level cards within their own file's geometry while keeping their original symbol parent index.

[Paired standard-runner Actions builds](https://github.com/onsager-ai/tolmap/actions/runs/35890831908) used this branch at `983fda22e7bbc11536be1b96840a3d8bf56e90cb` and `main` on each repository's pinned `eval/corpus.toml` commit. All eight builds succeeded. The `eval/symbol_stats.py` metrics were written by Actions into each result artifact. “r” is the median of per-file Pearson correlations between card area and symbol code lines for symbols whose hierarchy parent is −1; a file needs at least three such symbols and variation in both values. Cross-file Go receiver methods retain their hierarchy parent, so they are excluded from this correlation even though their cards are placed at file level. Sizes are raw compact JSON decimal MB, before and after geometry. The alternative size applies the prototype's ×1e4 delta coding to the *same* rings, as a size comparison only.

| repository | symbols with ring / eligible | missing | files in r | median r | symbols MB before → float geometry (×1e4 hypothetical) | build wall s branch / main |
|---|---:|---:|---:|---:|---:|---:|
| django/django | 10,542 / 10,542 | 0 | 259 | .911 | .534 → 6.349 (1.829) | 4.61 / 3.81 |
| langgenius/dify | 48,161 / 48,161 | 0 | 2,367 | .962 | 2.750 → 35.723 (9.738) | 54.32 / 48.56 |
| prometheus/prometheus | 9,099 / 9,099 | 0 | 339 | .629 | .414 → 5.362 (1.522) | 11.38 / 10.14 |
| vuejs/core | 2,847 / 2,847 | 0 | 142 | .948 | .171 → 1.835 (.565) | 2.82 / 2.46 |

This is a measured lower bound of **70,649 rings for 70,649 eligible symbols** across these four repositories. The dify document is 35.723 MB raw and 5.710 MB under gzip; its largest serialized district response is 5.548 MB raw. ×1e4 coding would reduce the raw document to 9.738 MB, but it collapses **4,715 dify exterior rings to zero area**. Plain floating-point coordinates preserve those slivers. Wall time includes extraction, parcel geometry, symbol parsing, and cards; the paired branch-minus-main differences (0.36–5.76 s) are build-level observations, not isolated card-pass timing. The map JSON hashes, including `S`/`U`, are byte-identical to `main` for all four pairs.

A read-only audit of the four Actions artifacts found **zero** child centroid-outside-parent cases, **zero** child areas larger than their parent, and **zero** symbols with more than one exterior contour. A 5×5 grid sampled each sibling-pair bounding-box intersection: django, prometheus and vue had no sampled overlap; dify had one pair with estimated overlap **0.96 of one raster cell**, below the one-cell tolerance. This sampling is a diagnostic bound at that resolution, not an exact polygon-intersection proof. The [CI gate](https://github.com/onsager-ai/tolmap/actions/runs/35890833530) passed formatting, clippy, release build and tests, generated TypeScript binding comparison, offline parity, zero-missing-footprint checks, and the three-build map-plus-symbol hash determinism check. The full nine-fixture parity workflow was not dispatched for this geometry-only change; its scheduled job is skipped on this PR.

**PR #90 follow-up, size and static delivery.** The original float representation is too large for district fetching. The [paired Actions build and audit](https://github.com/onsager-ai/tolmap/actions/runs/35899515435) compares the pre-precision PR commit `288023c2ef9ed760cf0bc2fd4022f75385a23dc7` with the packed-card revision on the same pinned repository commits. The audit's gzip figures use Python `gzip.compress` at level 9 with `mtime=0` on both documents; this differs from the earlier one-off 5.710 MB dify gzip measurement, which used a different compressor setting. All sizes below are decimal MB of compact JSON bytes, with the exact byte counts in each run artifact's `symbol_stats.json`.

| repository | full symbols raw MB before → after | full symbols gzip MB before → after | largest district raw MB before → after | largest district gzip MB after | static districts |
|---|---:|---:|---:|---:|---:|
| django/django | 6.349 → 3.415 | .960 → .578 | 1.739 → .923 | .159 | 12 / 12 |
| langgenius/dify | 35.723 → 18.643 | 5.675 → 3.322 | 5.548 → 2.845 | .523 | 78 / 78 |

The baseline artifacts show that rounding to six decimals collapses **40** dify and **12** django exteriors; ten decimals still collapses **3** and **2**; eleven collapses **zero**. The port rounds all card coordinates at one quantizer to 10⁻¹¹ world units, removes duplicate and collinear points, and writes each contour as an absolute first integer pair followed by integer deltas. This preserves the narrow fallback cards while avoiding the larger 11-decimal float document (39.807 MB raw for dify). On the decoded packed artifacts, the Actions audit found **zero** collapsed contours, duplicate consecutive points, collinear points, child centroids outside parents, or child areas larger than parents; eligible cards remain **48,161 / 48,161** for dify and **10,542 / 10,542** for django. Every static district JSON was compared with the API's district projection, including crossing-edge endpoints, and each paired map hash stayed byte-identical. Build wall times were **52.25 / 51.56 s** (packed / pre-precision) for dify and **5.98 / 5.90 s** for django; these are whole-build observations, not isolated encoding costs.

The service still reads the full sibling document and sends the district API response **uncompressed**. Static map collection copies the generated `<name>.symbols/<district>.json` files, letting the next viewer PR request one district instead of the whole sibling. Enabling gzip on the API needs tower-http's `compression-gzip` feature and was left for a separate dependency decision. The [CI gate on the packed revision](https://github.com/onsager-ai/tolmap/actions/runs/35899513756) verifies formatting, clippy, tests, generated bindings, offline parity, the static-copy check, and three-build determinism; the full nine-fixture parity job was not dispatched.
