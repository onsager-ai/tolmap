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

## 32. The service sends pre-split district symbols with gzip

The owner chose to enable gzip in session `16030105`, transcript line 1874, 2026-09-24T00:24:36Z. The service now reads `<commit>.symbols/<district>.json` directly for current commits. An older commit without that directory still projects from its map and full symbols document. The same gzip layer covers `/api/*` and the hosted static files, while tower-http's default predicate excludes SSE.

The [standard-runner remote build and HTTP measurement](https://github.com/onsager-ai/tolmap/actions/runs/35938706461) built dify at pinned commit `9a0961a4a9a305cfc6971cb3a8969b69b4ef66f2`. `eval/measure_service_symbols.py` chose the largest district by the size of its pre-split file, then requested district 0 through the running service. A second store row pointed to the same map and full symbols sibling without a pre-split directory to exercise the legacy path. The script checked that the direct bytes matched the file, that the legacy JSON matched the direct JSON, and that gzip decoded to the raw response. Requests used one persistent localhost HTTP connection; 20 warm samples per path alternated order. Thus the timings include HTTP transfer and are not isolated CPU time inside the handler.

| dify district 0 | measured result |
|---|---:|
| Raw HTTP response | 2,845,209 bytes |
| Gzip HTTP response | 530,283 bytes (18.6% of raw) |
| Legacy projection median response time | 122.36 ms |
| Pre-split file median response time | 1.32 ms |

The measurement is one standard-runner build with warm filesystem cache, not a latency guarantee across machines or concurrency levels. The response is byte-identical to the stored district file before transport compression; no map geometry, `S`, or `U` changed.

## 33. A file-backed symbol record keeps the saved parse without raising peak RSS

The owner prioritized performance follow-ups in session `16030105-19f0-4a84-933b-c5953f23c6b3`, transcript line 1874, 2026-09-24T00:24:36Z (tracking issue #82). The symbols pass previously read and parsed every mapped file after extraction. Extraction now collects compact spans, raw imports, code-line counts and reference candidates while each file's tree is alive, then drops the tree at the end of that iteration. Each per-file record is serialized into a temporary indexed stream and released before the next file. After graph construction fixes file order, the symbols pass reads one record at a time, resolves references, and attaches cards to the same map geometry. The stream is removed on completion or error. Graph-only extraction skips this work. The map's `C` and symbol areas share one code-line mask; prefix counts and an active-span sweep avoid repeated scans within a file.

The first [paired run](https://github.com/onsager-ai/tolmap/actions/runs/35940060179) kept all records in memory through graph construction: dify saved 4.02 s but its peak RSS rose from 201,624 to 301,956 KB. A [streaming JSON-spool trial](https://github.com/onsager-ai/tolmap/actions/runs/35941824692) held dify RSS at 203,012 versus 200,608 KB, but wall time regressed to 60.11 versus 53.19 s; its per-record stream decoder alone put symbol resolution at 11.23 s. An [early in-memory resolution trial](https://github.com/onsager-ai/tolmap/actions/runs/35942517196) saved 3.70 s on dify but still peaked at 290,940 versus 200,656 KB. Both trials kept map and symbols hashes equal to their paired `main` builds. The final indexed stream reads each record into a short-lived byte slice before JSON decoding; neither failed trial was retained.

The [final paired standard-runner Actions build](https://github.com/onsager-ai/tolmap/actions/runs/35943068048) used this branch at `2ac0079` against current `main` (after #94), with each repository pinned by `eval/corpus.toml`. All four build jobs and symbol geometry audits passed. SHA-256 of the full map JSON **and** full symbols JSON matched `main` on every row, covering all map fields (`F`/`N`/`E`/`L`/`S`/`U` included), hierarchy, references and card geometry. The audits found zero eligible cards without a ring, zero collapsed contours, and matching static district projections.

| repository | files | wall s branch / main | saving s | peak RSS KB branch / main | RSS increase |
|---|---:|---:|---:|---:|---:|
| django/django | 851 | 6.66 / 7.28 | 0.62 | 45,332 / 45,096 | 0.5% |
| langgenius/dify | 6,347 | 48.37 / 52.43 | 4.06 | 202,764 / 201,544 | 0.6% |
| prometheus/prometheus | 631 | 10.05 / 11.10 | 1.05 | 45,648 / 43,540 | 4.8% |
| vuejs/core | 239 | 2.52 / 2.77 | 0.25 | 31,968 / 31,316 | 2.1% |

These are paired whole-build observations from one run, not isolated parse costs or a cross-run speed guarantee. The maximum observed RSS increase is 4.8%, below the review's approximately 10% ceiling on all four. Dify's saving remains roughly four seconds, well below the hoped-for 17–26 seconds; there is no evidence here for claiming the larger target.

The final branch log breaks dify's 48.37 s build into disjoint measured phases (seconds, rounded to three decimals). `extract` excludes the `symbol_collection` and `graph` rows; their sum is `extract_total`. `symbol_collection` includes serializing per-file records; `symbol_resolution` includes reading them. Minor unlisted setup and timer rounding account for the gap to `/usr/bin/time` wall time.

| phase | dify s |
|---|---:|
| extract (other parsing and metadata) | 8.394 |
| symbol collection | 20.101 |
| graph (resolution, history, blend) | 1.411 |
| extract total | **29.906** |
| partition | 0.570 |
| neighbourhoods | 0.163 |
| naming | 0.024 |
| geometry | 1.837 |
| parcels | 6.790 |
| map JSON write | 0.027 |
| symbol resolution | 0.384 |
| cards | 8.262 |
| symbols JSON and district-directory write | 0.234 |

Symbol collection remains the largest individual phase, and parcels and cards are visible further costs. This PR does not change those other phases. The [CI gate](https://github.com/onsager-ai/tolmap/actions/runs/35943050822) passed Rust formatting, clippy, release build and tests, generated TypeScript bindings, offline parity and three-build determinism, plus web build/lint and deploy-environment parity. The full nine-fixture parity job is on-demand/nightly and was skipped on this PR.

## 34. Inward corner smoothing removes card staircases while retaining raster ownership

The owner prioritised smoothing the symbol cards in session `16030105`, transcript line 1874, 2026-09-24T00:24:36Z, after the #92 viewer made the #90 card outlines visible. The initial outlines exposed both raster staircases and tiny `+` shapes. A five-cell cross comes from the connected raster allocation itself, not from contour hole handling: the old contour faithfully traced those five cells. Small leaf regions now draw a compact octagon inside an owned cell. Container cards keep their full owned extent so descendants can remain inside them. Larger exteriors move convex corners inward by a quarter raster cell; holes keep their exact contour. Each candidate is checked against the raster ownership at cell centres and quarter-cell probes, and a parent restores its unsmoothed outline if smoothing excludes a child. This keeps the connected, code-line-weighted raster assignment and its reserved-child fallback; a vector power-diagram rewrite would need to reproduce both guarantees. The white gutters between cards remain intentional.

The [paired standard-runner Actions run](https://github.com/onsager-ai/tolmap/actions/runs/35942382674) built this branch at `e27c489` and `main` on each pinned `eval/corpus.toml` commit. The audit reads the packed full symbols document and every static district slice. “r” is the median per-file Pearson correlation of top-level symbol card area with code lines, for files with at least three eligible symbols and variation in both values (the same definition as finding 31). The staircase measure is the share of card contour edges with an exactly horizontal or vertical endpoint pair after decoding integer deltas. Sizes are decimal MB; gzip uses `gzip.compress` with `mtime=0` on compact JSON. Wall time covers the whole build, not just cards.

| repository | eligible cards with ring | files in r / median r | symbols raw MB, main → branch | symbols gzip MB, main → branch | median vertices/card, main → branch | mean vertices/card, main → branch | axis-aligned edges, main → branch | wall s, main → branch |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| django/django | 10,542 / 10,542 | 259 / .918 | 3.415 → 4.480 | .578 → .745 | 10 → 8 | 16.81 → 17.38 | 99.79% → 38.75% | 7.32 → 7.35 |
| langgenius/dify | 48,161 / 48,161 | 2,367 / .966 | 18.643 → 24.429 | 3.322 → 4.434 | 14 → 14 | 20.29 → 21.01 | 99.74% → 37.98% | 52.49 → 52.95 |
| prometheus/prometheus | 9,099 / 9,099 | 340 / .627 | 2.856 → 3.734 | .486 → .632 | 10 → 8 | 16.48 → 17.13 | 99.63% → 39.92% | 9.18 → 9.21 |
| vuejs/core | 2,847 / 2,847 | 142 / .954 | 1.029 → 1.368 | .180 → .235 | 10 → 8 | 17.25 → 18.06 | 99.77% → 39.01% | 2.84 → 2.89 |

All **70,649 / 70,649** eligible symbols have rings. The decoded audit found zero collapsed rings, duplicate consecutive points, collinear points, child centroids outside their parent, or child areas larger than the parent across all four repositories. A separate 5×5 probe of intersecting sibling bounding boxes found zero sampled overlap pairs in dify, prometheus and vue, and one in django, with an estimated overlap of `5.51×10⁻⁷` world units². That is a sampling diagnostic, not an exact polygon-intersection proof; the contour constructor separately rejects ownership across neighbouring raster cells at quarter-cell probes. All static district slices match the API projection, and every paired map hash, including `F`/`E`/`L`/`S`/`U`, is byte-identical to `main`. The 11-decimal integer-delta encoding and per-district split are unchanged.

The packed document grows about a third in raw bytes because diagonal deltas encode nonzero values on both axes, even though mean vertices rise by less than 0.8 per card. The measured whole-build wall differences are 0.03–0.46 s and should not be read as isolated smoothing cost. The [CI gate on this geometry revision](https://github.com/onsager-ai/tolmap/actions/runs/35942380709) checks formatting, clippy, tests, generated bindings, offline parity and determinism; the full nine-fixture parity job was not dispatched for this geometry-only change. A [separate viewer check](https://github.com/onsager-ai/tolmap/actions/runs/35941887362) passed `check:view`, screenshots and the viewer performance bench.

## 35. A method taken as a value is a symbol reference

At the django corpus pin, `load_middleware` chooses `self._get_response_async` or `self._get_response` in a conditional expression. Neither expression calls its method, so finding 30's call-only candidate collection omitted both edges. The later invocation of `self._middleware_chain(request)` is dynamic and still has no statically resolved edge to either method. The symbol pass now collects an uncalled attribute expression as a candidate when the existing resolver identifies a function or method exactly. This includes `self.m`, `cls.m`, `ClassName.m`, imported `module.func`, TypeScript `this.m`, and Go type-qualified method expressions. It skips store targets and callees already collected as calls. The resolver's existing rules still exclude self-references and ancestors. It does not infer Python base-class members or Go instance receiver types; `x.Method` remains unresolved when `x` has no existing type binding.

The [paired standard-runner Actions build](https://github.com/onsager-ai/tolmap/actions/runs/35946597440) built this branch and `main` from the same four `eval/corpus.toml` pins. The measurement step asserted byte equality of each **entire map JSON**, including `S` and `U`, then counted symbol-document edges. “Occurrences” sums each edge row's count; “resolved / total” is the existing *call* coverage metric and stays unchanged because value sites are not calls. These counts are lower bounds: no target is credited from an unresolved attribute or guessed receiver type.

| repository | symbol edges, main → branch | occurrences, main → branch | resolved / total calls, both | map SHA-256 prefix, both |
|---|---:|---:|---:|---|
| django/django | 8,851 → 9,452 | 10,393 → 11,159 | 8,622 / 32,639 | `24849f34f13f` |
| langgenius/dify | 33,239 → 33,455 | 39,106 → 39,336 | 23,584 / 176,252 | `2b7c2dcbda9f` |
| prometheus/prometheus | 4,609 → 4,638 | 6,460 → 6,495 | 6,035 / 42,569 | `eb0e4c425f75` |
| vuejs/core | 4,398 → 4,400 | 5,919 → 5,921 | 3,188 / 7,877 | `c6716cd7883e` |

In django's `django/core/handlers/base.py`, `load_middleware → _get_response` and `load_middleware → _get_response_async` each change from zero to **one** occurrence. The unit fixture covers that exact conditional-expression pattern, a callback argument, `functools.partial(self.m)`, a qualified method, an imported function, and a method-named store target that must produce no reference. The [PR CI gate](https://github.com/onsager-ai/tolmap/actions/runs/35946590199) passed Rust formatting, clippy, release build and tests, generated TypeScript bindings, offline parity and three-build map-plus-symbol determinism, along with web build/lint and deploy-environment parity. Full nine-fixture parity was skipped by its on-demand/nightly trigger.


## 36. Worker progress is observable without changing map or symbol bytes

Issue [#97](https://github.com/onsager-ai/tolmap/issues/97), PR 1, moves clone, detection and indexing into a `tolmap worker` child and reports versioned stages. Progress counters are observational: they do not feed partitioning, geometry, symbols or serialization. The service keeps admission, queueing, SQLite and artifact registration. Its existing file, history, clone-size and timeout limits still apply. A worker crash reports `worker_crashed` with exit status or signal and the last stage; the next job remains admissible.

The [paired standard-runner run](https://github.com/onsager-ai/tolmap/actions/runs/35947936989) used pinned `eval/corpus.toml` revisions with `main` after #99 as the comparator. Both map JSON and full symbols JSON matched byte for byte on dify and n8n; the [django validation run](https://github.com/onsager-ai/tolmap/actions/runs/35948343291) also matched both files and passed the method-value reference audit after its baseline assertion was updated to handle #99 on both sides. The branch's [CI gate](https://github.com/onsager-ai/tolmap/actions/runs/35948332422) passed fmt, clippy, release build and tests, generated TypeScript bindings, offline parity, synthetic polyglot determinism, footprint targets, symbol-card and static district audits, web build/lint and deploy-environment parity. The full nine-fixture parity job was skipped by its on-demand/nightly policy.

| Repository | Files | Paired map / symbols identity | Warm main median s | Warm progress median s | Measured change |
|---|---:|---|---:|---:|---:|
| django/django | 851 | yes / yes | — | — | paired 6.11 → 6.14 s |
| langgenius/dify | 6,347 | yes / yes | 41.741 | 42.065 | +0.776% |
| n8n-io/n8n | 11,991 | yes / yes | 58.404 | 59.245 | +1.440% |
| n8n-io/n8n, independent repeat | 11,991 | yes / yes | 101.0445 | 101.163 | +0.117% |

The warm timing script built each repo on one runner in `main → progress → progress → main` order, with the same clone, options and output hashing on all four builds. The two dify samples were 41.762 and 42.368 s for progress versus 41.646 and 41.836 s for main. The first two n8n samples were 59.341 and 59.149 s for progress versus 58.221 and 58.587 s for main. Dify met the under-1% target in that run; n8n did not. An [independent post-#99 n8n repeat](https://github.com/onsager-ai/tolmap/actions/runs/35948788520) measured progress at 101.314 and 101.012 s versus main at 101.007 and 101.082 s, giving +0.117%. The runners have different absolute speeds, so only within-run ratios are comparable. Both n8n runs preserved map and symbols bytes; the 0.117–1.440% range does not establish a stable under-1% bound.

The dify worker timeline below comes from the same standard-runner run. It used the pinned clone, `all_sources: true` and a measurement-only `max_files: 20000` so both detected sources are represented; the service still sends `all_sources: false` and its configured limit. The worker emitted 172 progress events, finished successfully in 41.925 s, and reported the following `stage_finished` events in arrival order. Durations are measured in the child and include only each named stage; the gap between events plus setup explains why their sum is less than wall time. This timeline is the first input for PR 2's cost model.

| Stage | Duration s | Finished at s |
|---|---:|---:|
| `clone` | 0.032 | 0.035 |
| `detect` | 0.103 | 0.139 |
| `parse` | 9.713 | 9.908 |
| `resolve` | 0.103 | 10.011 |
| `parse` | 17.200 | 27.239 |
| `resolve` | 0.120 | 27.360 |
| `history` | 0.382 | 27.803 |
| `blend_prune` | 0.044 | 28.352 |
| `partition` | 0.219 | 28.579 |
| `neighbourhoods` | 0.131 | 28.893 |
| `naming` | 0.010 | 28.903 |
| `regions` | 1.613 | 30.516 |
| `footprints` | 5.109 | 35.690 |
| `write_map` | 0.021 | 35.712 |
| `symbols` | 0.390 | 36.135 |
| `symbol_cards` | 5.549 | 41.684 |
| `write` | 0.182 | 41.866 |

The two parse passes sum to 26.913 s; symbol cards take 5.549 s and footprints 5.109 s. All named stages sum to 40.921 s of the 41.925 s job.

## 37. Typed references resolve inherited calls without changing the map

Owner decision 2026-09-24 (AskUserQuestion, session 16030105, [issue #103](https://github.com/onsager-ai/tolmap/issues/103)): **“Typed edges + viewer.”** This Rust PR supplies the data; a later PR draws it. The separate symbol document now carries a four-column `[source, target, occurrences, kind]` edge, with a document and district `kinds` legend. A pair can have several kind rows. Existing seven-column symbol and three-column edge documents load with `abstract = false` and kind `unknown`. The in-repository Python MRO uses C3 order and stops at an unknown base; TypeScript follows its resolved `extends` chain. `self`, `cls`, `this` and `super` calls only gain a target when no unknown base precedes the definition. Override edges point to the nearest resolved base method.

[Paired standard-runner Actions builds](https://github.com/onsager-ai/tolmap/actions/runs/35951389954) built this branch and `main` from the same four pinned `eval/corpus.toml` commits. The measurement asserts byte equality of each **entire map document**, including `F`/`N`/`E`/`L`/`S`/`U` and every other field. It matches old symbol identities by file, name, kind and line span, and asserts that every old edge pair retains at least its old occurrence count after excluding new `overrides` and `possible_implementation` rows. All four builds and those assertions passed. Counts below are edge **rows** by kind, not occurrences. `P` is `possible_implementation`; all new documents have zero `unknown` rows. Raw and gzip are bytes for the complete symbols JSON, including card geometry.

| repository | call | extends | implements | overrides | annotation | decorator | value | P | abstract symbols | inherited calls resolved | symbols raw / gzip bytes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| django/django | 8,529 | 1,507 | 0 | 2,037 | 14 | 385 | 764 | 0 | 2 | 1,691 | 4,630,844 / 764,617 |
| langgenius/dify | 19,483 | 1,155 | 13 | 150 | 12,896 | 598 | 227 | 0 | 136 | 85 | 24,806,260 / 4,459,001 |
| prometheus/prometheus | 4,164 | 12 | 4 | 0 | 431 | 0 | 30 | 902 | 191 | 0 | 3,812,783 / 638,820 |
| vuejs/core | 2,320 | 4 | 4 | 3 | 2,075 | 0 | 2 | 0 | 376 | 0 | 1,393,529 / 237,247 |

Dify's prior `parent_class_method` count was 273 and is **239** after the change; **85** calls resolved through an inherited lookup, while total resolved calls rose from **23,584 to 23,685** out of the same 176,252 sites. Django's inherited count is 1,691; its `parent_class_method` unresolved count fell from 1,180 to 323 and total resolved calls rose by 1,691. Calls through an unresolved external base remain uncredited. Go's 902 `possible_implementation` edges in prometheus match declared method names and parameter/result **counts** for in-repo structs and interfaces. This is a candidate relationship, not exact `implements`: the resolver does not type-check parameter/result types, pointer method sets, or generic constraints, and it skips interfaces with embedded methods it cannot enumerate. The count therefore must never be presented as proven interface satisfaction.

The [CI gate](https://github.com/onsager-ai/tolmap/actions/runs/35951387619) ran Rust formatting, clippy, release build and tests, generated binding comparison, offline parity and three-build map-plus-symbol determinism; web build/lint and deploy-environment parity ran as well. The full nine-fixture parity job was not dispatched by this push/PR event. The viewer remains for the follow-up PR.

## 38. ETA replay shows useful late estimates and an early n8n underestimate

Issue [#97](https://github.com/onsager-ai/tolmap/issues/97), after [PR #98](https://github.com/onsager-ai/tolmap/pull/98), removes the hosted file-count, clone-size, history-depth and wall-time refusals. The worker spec carries only a clone-cache eviction budget; eviction never rejects or evicts the active clone. The service still limits request frequency and queue length. A large job can therefore run for a long time, fail cloning if the volume fills, or fail as `worker_crashed` if the machine kills its worker. This is the owner's 2026-09-24 MVP trade-off; issue #97 records the production worker-class and routing design. This finding uses number 38 because [open PR #104](https://github.com/onsager-ai/tolmap/pull/104) already reserves 37.

The cost model starts from finding 36's 18 dify stage durations and the committed corpus build totals (django 6.14 s, dify 41.925 s, n8n 101.163 s). After clone it sees clone bytes and history count; after detection it sees file and byte totals per selected language. Each completed job stores stage durations and features in SQLite. A stage's linear file/byte prediction receives a clipped observation-to-seed ratio from at most 256 recent jobs, with three seed observations' weight, so one job can change a coefficient only within a bounded range. Current-stage progress uses done/total and an EWMA rate; parse and resolve retain the unvisited source passes in multi-source builds. The ETA interval widens beyond observed file and byte sizes. Queued start estimates sum the running job's remaining midpoint and the queued jobs ahead. Cancellation ends with `failed`/`cancelled` to preserve the status enum, and kills the worker process group before the queue advances.

The [standard-runner replay](https://github.com/onsager-ai/tolmap/actions/runs/35953085187) used `eval/worker_timeline.py` on pinned `eval/corpus.toml` clones, `all_sources: true`, with one worker timeline per repository. `tolmap eta-replay` read each event in arrival order and used only features and progress available by each checkpoint. The table is **absolute error of the remaining-ETA midpoint**, in seconds, at 10%, 50% and 90% of each job's actual wall time. Displayed errors are truncated down to 0.001 s. The replay is cold-start: no learned SQLite timing rows were supplied. The service currently selects one source, so these all-sources replay numbers measure the model's multi-source path rather than a service job's exact workload.

| Repository | Wall s | 10% error s | 50% error s | 90% error s |
|---|---:|---:|---:|---:|
| django/django | 7.076 | 2.340 | 2.400 | 0.081 |
| langgenius/dify | 54.022 | 14.057 | 0.616 | 1.000 |
| n8n-io/n8n | 106.564 | 42.035 | 24.734 | 4.082 |
| prometheus/prometheus | 11.796 | 2.045 | 3.036 | 0.427 |
| vuejs/core | 2.675 | 5.505 | 3.498 | 0.918 |
| **Median absolute error** | — | **5.505** | **3.036** | **0.918** |

N8n is the material miss: at 10% wall time the model predicted 53.872 s remaining versus 95.908 s actual, and at 50% it predicted 28.547 s versus 53.282 s. A file count and byte total do not capture its per-file extraction and symbol-card cost from a cold seed. This run does **not** establish accurate early ETAs for repositories of n8n's scale. The model will learn completed stage durations on the service machine, but that post-refit accuracy was not measured here. The [CI gate](https://github.com/onsager-ai/tolmap/actions/runs/35953081409) passed Rust formatting, clippy, build and tests, checked-in TypeScript binding generation, offline parity and determinism, web build/lint, and deployment-environment parity; full nine-fixture parity remained skipped by the workflow's on-demand/nightly policy.

## 39. Vector power cells replace raster card ownership

The owner's overnight go for #82 (session `16030105`, transcript line 181, 2026-09-23T14:16:42Z) lists "card outlines still have raster-stepped peninsulas" as an open item. Finding 34 only rounded the raster staircases: every card edge still followed grid ownership, and a connected raster region could wrap round a sibling into a notched shape (dify `api/models/model.py` around `PluginDataMigration` in the deep-zoom screenshot). Finding 34 also warned that a vector rewrite must keep the raster solver's two guarantees, connected weighted quotas and a card for every crowded child.

`src/symbol_cards.rs` now computes each level as a power diagram on the polygons themselves. A level is the file footprint `P` (module-level code plus top-level symbols) or a class card's body (its members). Each child's cell is the parent region clipped by its power bisectors. A convex parent gives convex cells, and a non-convex region is split into pieces by an exact half-plane split rather than Sutherland–Hodgman, whose zero-width bridges would stroke a line through the neighbouring file. Each iteration takes a diagonally preconditioned optimal-transport step on the power weights towards the code-line targets, then moves every site to its piece's centroid (a capacity-constrained power diagram). It stops at 2% maximum area error or after 80/48/24 iterations for levels of ≤64/≤256/larger. Sites start from `SEED`-seeded rejection sampling, ordered top-down so children in source order start below the header. Weights are capped against each neighbour at 0.9 × squared site distance (the Nocaj–Brandes cap), which keeps every site inside its own cell and every cell non-empty. A level that still ends with an empty cell falls back to area-proportional vertical strips, which cannot be empty. How often that fallback fires was not instrumented. A card is its largest piece after three steps. First, small convex corners are cut off (at most 8% of a non-convex piece's area), which removes the footprint's one-cell raster steps. Second, a piece below 90% of its hull, or whose vertex mean falls outside it, is cut along the edge line at a reflex vertex that loses least, until it is near-convex. Third, the piece is inset by the raster pass's gutter distances: by half-planes when convex, otherwise by a mitred offset checked for edge flips and self-intersection, then by star-shaped scaling, halving the distance when neither fits. A class card's header is the top share of its area above one horizontal cut, found by bisection; members share the body below. The 1e-11 packed ring encoding, the per-district split files and the map are unchanged.

The [paired standard-runner run 35961974960](https://github.com/onsager-ai/tolmap/actions/runs/35961974960) built this branch at `4c4248b` and `main` at `12c87a5` from the four pinned `eval/corpus.toml` commits. For every repository, the map JSON is byte-identical to `main`, and so are the symbol rows and the edge rows (`typed_edges_measure.json`). Only card, header and module rings changed. "r" is finding 31's median within-file Pearson correlation of top-level card area with code lines. Convexity is exterior area ÷ convex-hull area per card. "Axis" is finding 34's staircase measure, the share of card edges that are exactly horizontal or vertical. Sizes are the full symbols document, decimal MB, with gzip at level 9 and `mtime=0`. Card seconds is each build's own `Drawing symbol cards: done in` log line, and one pair is not a speed guarantee. All values are main → branch.

| repository | cards / eligible (missing) | files in r, median r | vertices/card median, mean, p90, max | axis-aligned edges | convexity median, p10, min | cards ≥ 0.99 convex | cards with >1 contour | symbols raw MB | symbols gzip MB | card pass s |
|---|---:|---:|---|---:|---|---:|---:|---:|---:|---:|
| django/django | 10,542 / 10,542 (0 → 0) | 259: .918 → 1.000 | 8, 17.4, 38, 332 → 5, 5.4, 7, 16 | 38.7% → 10.9% | .966, .741, .316 → 1.000, .996, .872 | 49.1% → 91.7% | 1,549 → 0 | 4.631 → 2.149 | .765 → .866 | 1.23 → 1.68 |
| langgenius/dify | 48,168 / 48,168 (0 → 0) | 2,367: .966 → 1.000 | 14, 21.0, 48, 672 → 5, 5.5, 8, 18 | 38.0% → 14.8% | .936, .755, .239 → 1.000, .974, .830 | 41.8% → 84.7% | 9,586 → 0 | 24.806 → 10.119 | 4.459 → 4.050 | 6.83 → 7.81 |
| prometheus/prometheus | 9,099 / 9,099 (0 → 0) | 340: .627 → .775 | 8, 17.1, 38, 338 → 5, 5.3, 7, 15 | 39.9% → 11.9% | 1.000, .747, .302 → 1.000, .991, .867 | 53.3% → 90.3% | 1,230 → 0 | 3.813 → 1.749 | .639 → .720 | 1.12 → 1.70 |
| vuejs/core | 2,847 / 2,847 (0 → 0) | 142: .954 → 1.000 | 8, 18.1, 38, 410 → 5, 5.4, 7, 13 | 39.0% → 10.1% | .968, .762, .417 → 1.000, .980, .886 | 49.3% → 87.6% | 484 → 0 | 1.394 → .618 | .237 → .254 | .37 → .63 |
| microsoft/vscode (run 35962631998) | 134,320 / 134,320 (0 → 0) | 3,304: .964 → 1.000 | 8, 16.5, 34, 1,240 → 5, 5.3, 7, 21 | 42.1% → 12.1% | 1.000, .741, .000 → 1.000, 1.000, .854 | 52.2% → 93.2% | 16,994 → 0 | 54.843 → 28.655 | 9.198 → 10.811 | 20.24 → 22.75 |

The r values of 1.000 are rounded from .9999. The 2% area tolerance fits top-level cards to their code-line mass almost exactly. Prometheus stays at .775 for a reason the geometry cannot fix: a Go type's methods in the same file are its children, so its mass includes them, but its own code-line count covers only the type declaration. Raw size falls by more than half because a five-vertex card is shorter than a traced contour. Gzip rises 7–13% on django, prometheus and vue and 18% on vscode, because raster deltas were small repeated integers and polygon deltas are not. Dify's gzip still falls 9%, and its largest district file is 1.655 MB raw, 0.659 MB gzip. The card pass costs 0.26–0.98 s more per build, and 2.51 s more on vscode.

The audit gates (`eval/symbol_stats.py`) found zero collapsed rings, duplicate or collinear points, child centroids outside their parent, and children larger than their parent in all four repositories. Every static district file matched its API projection. Exact Sutherland–Hodgman intersection of every overlapping-bounding-box sibling pair where both cards are convex (10,556 / 37,174 / 8,959 / 2,657 pairs) found **zero** overlapping pairs. The 5×5 sampled check found none either. A stricter per-vertex diagnostic is new here: across the four repositories, **31 of 187,776** child-card vertices fall outside their parent card (main: 62,947 of 525,797), and **168 of 242,460** top-level card vertices fall outside the file footprint (main: 64,903 of 916,633). These residual cases are mostly vertices on or within rounding of the outline, where a piece took no gutter. That is inference from the examples the audit records, not a proof. microsoft/vscode was added to the audit after [remote-build run 35961771005](https://github.com/onsager-ai/tolmap/actions/runs/35961771005) found three raster cards, about 1e-7 wide, with their centre outside their parent (symbol 3651 in parent 3650, among others). The [vscode paired run 35962631998](https://github.com/onsager-ai/tolmap/actions/runs/35962631998) built this branch at `4c4248b` against `main`. Its map is byte-identical to `main`, and the vector geometry passes every gate with zero failures: centroids outside their parent, children larger than their parent, collapsed rings, duplicate points and collinear points. Its 136,944 convex sibling pairs have zero overlaps, and 38 of 492,531 child vertices and 141 of 248,391 top-level vertices fall on the wrong side of their outline (main: 133,751 of 1,215,253 and 54,615 of 953,622). The same job's `eval/measure_typed_edges.py` step stops at its own `symbol identity is ambiguous` assertion: at least two vscode symbols share a file, name, kind and span, so that script cannot key vscode's edges. Symbol-row and edge-row identity is therefore shown for the four repositories above, not for vscode. The dify district-0 symbols file came out byte-identical in this run and the previous run on the same geometry code (35961072875), a determinism check that CI's three-build hash check repeats on the synthetic fixture.

The two remaining axis-aligned sources are deliberate. The header band is one horizontal cut, so members in the top row have a horizontal top edge. Top-level cards also keep straight edges wherever the file footprint `P` itself is axis-aligned, because the map's footprints are still raster contours and are not changed here.

The viewer fixtures `web/check-fixtures/*.symbols.tar.gz` were regenerated from run 35961974960 in the same layout. Both paired map SHA-256 values equal `check-view-stability.mjs`'s pinned hashes. On the viewer check, the desktop deep-zoom frames in both themes show `PluginDataMigration` and its neighbours as convex, gutter-separated cards with no stepped peninsulas. The phone "deep-zoom" frame does not reach card zoom on this branch or on `main`, so it cannot show card geometry; that is a screenshot-script gap, not a geometry result.

## 40. Symbol collection was quadratic in tree depth through `Node::parent()`

The owner prioritized performance follow-ups in session `16030105-19f0-4a84-933b-c5953f23c6b3`, transcript line 1874, 2026-09-24T00:24:36Z (tracking issue #82). Finding 33 measured symbol collection at 20.1 s on dify, the largest single build phase. This finding uses number 40 because [open PR #112](https://github.com/onsager-ai/tolmap/pull/112) already cites 39.

The build log now splits `symbol_collection` into disjoint steps: `spans`, `receivers` (Go), `lines` (code-line prefix sums), `imports`, `candidates` (reference candidates), `encode` (JSON record) and `write` (spool file). A [timing-only baseline run](https://github.com/onsager-ai/tolmap/actions/runs/35958212033) at `685028d` produced map and symbols hashes identical to `main` (`453daf2`) on all four repositories. It showed that the suspected costs were small. Spool serialization plus writing took 0.195 s on dify, import collection 0.102 s, and line counting 0.005 s. **Candidate collection took 22.628 of 23.746 s.** Its share was the same on the other repositories: django 2.582 of 2.749 s, prometheus 6.337 of 6.804 s, and vue 1.154 of 1.212 s.

The cause was `Node::parent()`. Tree-sitter 0.25 stores no parent pointer. `ts_node_parent` starts at the tree root and descends through `ts_node_child_with_descendant`, which scans siblings at every level. The candidate walk called `covered_by_reference_wrapper` on every syntax node that was not a call. That function climbed `parent()` to the nearest definition. `value_attribute` climbed `parent()` from every attribute, member or selector expression to the root. As a result, each ancestor chain cost O(depth² × fan-out) per node. Deep TypeScript/JSX expressions and long class bodies make that expensive. The walks also allocated a tree cursor and a `Vec` for every node's children.

All four collection walks (spans, Go receivers, Go imports and candidates) now use one pre-order cursor walk. The walk carries the stack of named ancestors. Whether a node lies below a reference wrapper becomes a flag inherited from its parent: `wrapper(parent) || (!definition(parent) && flag(parent))`. This is the same rule the upward search applied. The value-reference check walks the ancestor stack instead of calling `parent()`. The walk enters named nodes only, as `named_children` recursion did. For any node it visits, the stack therefore holds exactly the nodes `parent()` returned. The previous recursive implementations remain in a test module. Tests assert identical spans, receivers, Go imports, candidates and shadowed names on inline Python, TypeScript, TSX and Go samples, and on every Python file under `src/tolmap/` and every TypeScript file under `web/src/`. The TypeScript sample contains JSX, so parsing it with the non-TSX grammar also covers a tree with syntax errors.

The [final paired standard-runner run](https://github.com/onsager-ai/tolmap/actions/runs/35959267885) built this branch at `94f11fb` and `main` at `dc9f14b` from the same four `eval/corpus.toml` pins. SHA-256 of the full map JSON **and** full symbols JSON matched `main` on every row. That covers `F`/`N`/`E`/`L`/`S`/`U`, hierarchy, typed edges from #104 and card geometry. The typed-edge audit and the card-geometry audit passed. Wall time and peak RSS come from `/usr/bin/time`. Collection steps for `main` are from the timing-only baseline run on a different runner, because `main` does not log them.

| repository | files | map / symbols SHA-256 prefix (both) | symbol collection s, main → branch | candidates s, baseline → branch | spans s, baseline → branch | build wall s, main → branch | peak RSS KB, main → branch |
|---|---:|---|---:|---:|---:|---:|---:|
| django/django | 851 | `24849f34f13f` / `3add7c6c474e` | 2.507 → 0.365 | 2.582 → 0.241 | 0.127 → 0.085 | 6.93 → 4.83 | 46,984 → 47,344 (+0.8%) |
| langgenius/dify | 6,347 | `2b7c2dcbda9f` / `45848ce8a527` | 23.693 → 2.526 | 22.628 → 1.614 | 0.809 → 0.605 | 50.80 → 29.79 | 204,988 → 204,480 (−0.2%) |
| prometheus/prometheus | 631 | `eb0e4c425f75` / `9d44825f614d` | 6.692 → 0.687 | 6.337 → 0.350 | 0.172 → 0.121 | 10.94 → 4.92 | 47,304 → 46,828 (−1.0%) |
| vuejs/core | 239 | `c6716cd7883e` / `ad9a9c41dee2` | 1.225 → 0.135 | 1.154 → 0.088 | 0.046 → 0.035 | 2.68 → 1.63 | 32,568 → 32,156 (−1.3%) |

On the same runner, the dify job also ran balanced warm replays in the order `main → branch → branch → main`, with the same clone and output hashing throughout. Main took 50.816 and 50.719 s; the branch took 29.487 and 29.426 s. The medians are **50.768 → 29.457 s (−42.0%)**, and all four outputs were byte-identical. An [earlier paired run](https://github.com/onsager-ai/tolmap/actions/runs/35958481265) of the same code at `eb74c9c` had a faster runner and measured dify at 37.38 → 23.12 s, with collection at 16.266 → 1.707 s. The saving scales with runner speed; it is not a fixed number of seconds.

The dify worker stages from the final run are below, in seconds. Only the two parse passes changed, because symbol collection runs inside them. Every other stage is within 0.1 s of `main`.

| stage | main | branch |
|---|---:|---:|
| parse (Python pass) | 11.69 | 5.11 |
| parse (TypeScript pass) | 20.63 | 6.07 |
| resolve (both passes) | 0.29 | 0.29 |
| history | 0.45 | 0.47 |
| partition + neighbourhoods + naming | 0.44 | 0.44 |
| regions | 1.89 | 1.89 |
| footprints | 6.52 | 6.61 |
| symbols (resolution) | 0.51 | 0.52 |
| symbol cards | 6.81 | 6.78 |
| write map + symbols | 0.21 | 0.22 |

After this change, dify's symbol collection (2.53 s) is smaller than extraction's parsing and metrics (8.75 s), symbol cards (6.78 s) and footprints (6.61 s). Those three are now the large phases, and this change does not touch them. The remaining collection time is mostly candidates (1.61 s) and spans (0.61 s). The span walk still scans earlier spans linearly to find each symbol's parent. That scan is kept for exactness because the step is no longer material. The ETA model in `src/service/eta.rs` is still seeded from finding 36's timeline, which had a 26.9 s dify parse. A cold-start ETA will therefore overestimate the parse stage until learned rows move it, and each learned ratio is clipped. This was not re-measured here. The [CI gate on `94f11fb`](https://github.com/onsager-ai/tolmap/actions/runs/35959226248) passed Rust formatting, clippy, release build and tests (including both equivalence tests), generated TypeScript bindings, offline parity and three-build map-plus-symbol determinism. The full nine-fixture parity job was skipped by its on-demand/nightly policy.

## 41. SCIP gives exact references where a project's configuration loads without installs, and fails exactly where the hand-written resolver fails when it does not

Owner decision 2026-09-24 (AskUserQuestion, session `16030105`, transcript line 3770, 05:07:45Z): **"Go to SCIP now."** This is issue [#110](https://github.com/onsager-ai/tolmap/issues/110)'s P0: CI only, no product change. This finding uses number 41 because main has 40 and [open PR #112](https://github.com/onsager-ai/tolmap/pull/112) cites 39.

**What ran.** `remote-build.yml` with `command=scip-spike` calls `scip-spike.yml`, a reusable workflow. Standalone `workflow_dispatch` only works once a file is on the default branch. The indexers were scip-typescript 0.4.0, scip-python 0.6.6 and scip-go v0.2.7, on standard runners, for the five `eval/corpus.toml` pins. **Default mode installs nothing.** TypeScript gets no `pnpm install`, Python gets no pip/uv, and Go runs with `GOPROXY=off` and an empty module cache. That is the only mode the hosted worker could run without a sandbox (#110, risk 1). Dify's TypeScript and Python and prometheus's Go also ran with the lockfile install first, on the throwaway runner. TypeScript is indexed per tracked `tsconfig.json`, deepest first. Python runs at the repository root, and for dify also at `api/`, the nested root from finding 25. `eval/scip_ingest.py` walks the index document by document and derives four things, restricted to the map's `F`:
- file→file pairs: an occurrence in A of a non-local symbol whose definition occurrence is in B;
- symbol→symbol pairs, credited to the innermost span in tolmap's own symbols document, on both ends;
- relationship pairs;
- references that resolve into repository files outside the map.

`eval/scip_compare.py` compares these with the map's `E` (the resolved imports) and the symbols document, both built by `tolmap` from main at `840e531` on the same checkout. It then re-partitions: SCIP pairs replace `static_signal` in the `dump-graph` graph, computed as `finish_graph` does (undirected sum, per-language maximum floored at 1.0, 0.02 raw-weight floor). `tolmap build --graph` then runs the unchanged mass-normalised blend (finding 1), prune and Leiden. `tolmap parity` scores the result against a control map built from the unmodified graph. A SCIP pair that was not a candidate before gets exact proximity and zero cochange and semantic. finish_graph had dropped such a pair only below 0.02, so both values are small, though not provably zero. The same construction run on the hand-written imports (`hand-rebuilt`) reproduces the control at 100% placement on four repositories. On prometheus it gives 97.8%, because `dump-graph` rounds Go's 1/|D| import shares to three decimals. That is this construction's noise floor.

Runs: [index + analysis 35960792519](https://github.com/onsager-ai/tolmap/actions/runs/35960792519) and its [final analysis 35962203018](https://github.com/onsager-ai/tolmap/actions/runs/35962203018), which reused that run's indexes. [Run 35960017915](https://github.com/onsager-ai/tolmap/actions/runs/35960017915) ([reanalysed](https://github.com/onsager-ai/tolmap/actions/runs/35960796138)) used `--pnpm-workspaces --infer-tsconfig` for TypeScript, recorded below as a failed method.

### Cost and file edges

Hand-written pairs are the map's `E`, restricted to that language. Recall is the share of hand-written pairs SCIP also finds. At directory granularity, the target is its directory, which is the fair unit for Go: `resolve_multi` spreads a Go import over every file of the package. For comparison, today's whole `tolmap build` takes 7.0 s / 46 MB on django, 42.0 s / 202 MB on dify, 11.0 s / 46 MB on prometheus, 2.7 s / 32 MB on vue and 75.8 s / 297 MB on n8n.

| repo | index | wall s | peak RSS GB | index MB | files indexed / mapped | hand | SCIP | both | recall | SCIP-only share | dir recall |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| django | py | 79.2 | 5.39 | 103.7 | 851 / 851 | 3,173 | 4,018 | 3,141 | 0.990 | 21.8% | 0.998 |
| dify | py at root | 246.4 | 7.89 | 171.3 | 1,987 / 1,989 | 7,611 | 10,961 | 6,806 | 0.894 | 37.9% | 0.961 |
| dify | py at `api/` | 236.1 | 7.17 | 155.6 | 1,815 / 1,989 | 7,611 | 11,453 | 7,383 | 0.970 | 35.5% | 0.990 |
| dify | py at `api/`, installed (+24.5 s) | 285.5 | 10.53 | 165.9 | 1,815 / 1,989 | 7,611 | 11,486 | 7,381 | 0.970 | 35.7% | 0.989 |
| dify | ts | **failed** (exit 1, 0.4 s) | — | — | 0 / 4,358 | 16,546 | — | — | — | — | — |
| dify | ts, installed (+23.0 s) | 251.7 | 5.67 | 230.5 | 4,350 / 4,358 | 16,546 | 22,285 | 16,539 | **1.000** | 25.8% | 1.000 |
| n8n | ts | 314.5 | 8.84 | 582.7 | 11,991 / 11,991 | 39,402 | 23,840 | 20,263 | **0.514** | 15.0% | 0.420 |
| prometheus | go | 3.0 | 0.80 | 53.5 | 409 / 444 | 5,574 | 5,436 | 4,842 | 0.869 | 10.9% | 0.997 |
| prometheus | go, installed (+25.9 s) | 299.2 | 2.98 | 61.5 | 409 / 444 | 5,574 | 5,438 | 4,842 | 0.869 | 11.0% | 0.997 |
| prometheus | ts at `web/ui` | 6.7 | 0.54 | 4.9 | 186 / 187 | 518 | 530 | 514 | 0.992 | 3.0% | 0.989 |
| vue | ts | 20.0 | 0.77 | 16.9 | 233 / 239 | 1,186 | 1,911 | 1,183 | **0.998** | 38.1% | 0.995 |

**Where a project's configuration loads, SCIP is a near-superset of today's graph.** Django keeps 99.0% of hand-written pairs. Vue keeps 99.8%, and all 259 of its cross-package pairs through the root tsconfig's `@vue/*` paths. Prometheus's UI keeps 99.2%, and dify installed keeps 16,539 of 16,546. Each adds 3–38% more pairs, for example `django/contrib/admin/sites.py → django/apps/registry.py` and `packages/compiler-dom/src/transforms/vText.ts → packages/compiler-core/src/ast.ts`. The hand-only residue is small and explicable. In django it is `gis/geos/*.py → prototypes/__init__.py`, where Pyright credits the re-exported name to its defining submodule. In vue it is benchmark files outside every tsconfig.

**Without installs, the TypeScript monorepos fail, in two different ways.** Every dify tsconfig `extends` `@dify/tsconfig/*`, a workspace package that exists only after install. scip-typescript reports TS6053 and exits with *"no files got indexed"*. n8n's configs load, but its `@n8n/*` workspace imports resolve through `node_modules`. The hand-written resolver finds 11,690 cross-package pairs in n8n; SCIP finds 85. Overall recall is 0.514. Installed, dify finds 3,622 cross-package pairs against the hand-written 45. That is #101's class of import (`web/… → packages/dify-ui/src/…`). #101's own example, `cli/ → packages/contracts`, resolves 284 references under SCIP, but into `packages/contracts/generated/`, which tolmap's source collection excludes. So an exact index alone would not draw that edge. The map would also have to admit generated files. The first run's `--pnpm-workspaces --infer-tsconfig` method was worse. It inferred a tsconfig for each package without one, which displaced vue's root config: 6 of 259 cross-package pairs. Installed dify stayed at recall 0.420, because the root inferred config claimed every file first.

**Python and Go do not need installs for in-repo edges.** Installing dify's API dependencies changes its in-repo pairs by 33 (11,453 → 11,486), and prometheus's Go by 2. What installs change is external references: dify Python 15,867 → 42,183, and prometheus Go 8,579 → 13,730. They also cost time and memory: Go goes from 3.0 s to 299 s and 0.8 to 3.0 GB, and dify Python peaks at 10.5 GB. Rooting Pyright at `api/` raises dify's recall from 0.894 to 0.970. That is finding 25's nested root again, and an indexer needs it as much as the resolver does. Go's file-level recall of 0.869 is the resolver's package fan-out: directory-level recall is 0.997. The 35 unindexed Go files sit in `tsdb/fileutil` (17), `util/runtime` (7), `model/labels` (4) and nested-module tool directories. By their directories these look like platform- and build-tag variants plus modules outside the root `go.mod`, but that is inferred from the directories and was not checked file by file.

### Symbols, calls and relationships

tolmap's call edges are nearly all confirmed. SCIP has the same symbol pair for 97.4% of django's call pairs (8,311 / 8,529), 99.4% of dify Python's at the root, 96.5% of prometheus Go's, 89.4% of vue's, 86.8% of n8n's and 76.4% of installed dify TypeScript's. SCIP credits many more in-repository references. Callable references inside symbols that reach a mapped definition number 11,408 for django, against 10,178 tolmap call-edge occurrences. The other pairs are 3,722 against 3,036 for vue, 12,493 against 5,245 for prometheus Go, 28,310 against 11,565 for dify Python, and 60,601 against 43,752 for n8n. Finding 30's share for the two single-language repositories is resolved calls over call sites: django 10,313 / 32,639 (31.6%) and vue 3,188 / 7,877 (40.5%). The SCIP numerator over the same denominator would be 35.0% and 47.3%. **That is an estimate, not a like-for-like share.** SCIP has no notion of a call site. Its callable references include methods taken as values and exclude class instantiation (a type reference) and calls on untyped receivers, which produce no occurrence at all. Symbol pairs overall are 18,453 SCIP against 12,382 tolmap on django, and 19,952 against 4,616 on prometheus Go.

Relationships are exact where tolmap's are heuristic. scip-python's `is_implementation` pairs match 3,016 of django's 3,544 extends/implements/overrides pairs. For prometheus, scip-go emits 1,630 in-repo implementation pairs; they confirm **334 of the 902** `possible_implementation` edges finding 37 warned were candidates, not proven satisfaction. TypeScript relationships are sparse (vue 16, dify installed 117). Definition occurrences carry an enclosing range for 38% of django's definitions and 14% of installed dify TypeScript's. P1 cannot rely on enclosing ranges alone to find the innermost symbol, so it will need spans, as this ingest does with tolmap's.

### Re-partition

Primary weighting: each directed pair weighs its distinct referenced symbols. `binary` weighs every pair at 1. `uses` drops pairs that only a module or package symbol supports: import-only module references, and Go package clauses. Warm-started builds seed Leiden from the control map, as finding 4's production path would. The `hand-rebuilt` warm-start floor is 98.9–100%.

| repo | static signal | districts | q | placement vs control | warm: districts / q / placement |
|---|---|---:|---:|---:|---:|
| django | today | 12 | 0.5092 | — | — |
| django | SCIP py | 13 | 0.4968 | 71.1% | 12 / 0.4980 / **91.3%** |
| django | SCIP py, binary | 12 | 0.5095 | 73.4% | 11 / 0.5106 / 93.7% |
| dify | today | 78 | 0.7374 | — | — |
| dify | SCIP py (root) + hand TS (fallback) | 43 | 0.7279 | 91.4% | 41 / 0.7259 / 96.6% |
| dify | SCIP py + TS, installed | 47 | 0.7035 | 68.1% | 47 / 0.7143 / **87.9%** |
| n8n | today | 80 | 0.7793 | — | — |
| n8n | SCIP ts (no installs) | **250** | 0.8891 | 46.7% | 245 / 0.8968 / **56.0%** |
| prometheus | today | 11 | 0.6281 | — | — |
| prometheus | SCIP go + ts | 15 | 0.6717 | 83.0% | 11 / 0.6741 / **95.2%** |
| prometheus | SCIP go + ts, binary | 13 | 0.6372 | 92.6% | 11 / 0.6421 / 96.5% |
| vue | today | 8 | 0.5389 | — | — |
| vue | SCIP ts | 7 | 0.4844 | 77.8% | 7 / 0.4854 / **85.8%** |

The partition moves more than the edge overlap suggests. Django's graph keeps 99% of its hand-written pairs and gains 22%, yet a cold re-partition places only 71.1% of files in the matched district. Warm-started, it places 91.3% with the same twelve districts. Dify's installed graph is exact on both languages and consolidates 78 districts to 47, and Python alone to 43. The `uses` variant is not consistently better. It is worse on django (59.9% cold, 87.3% warm) and prometheus (79.6% cold), and better only for dify's `api/`-rooted Python (89.6% against 84.4% cold). n8n without installs is the regression. Losing the cross-package pairs fragments the map from 80 districts to 250, which raises q (0.78 → 0.89) exactly as finding 15 predicts for a graph that lost its bridges. Q is computed on each variant's own graph, so a q difference is not a quality ranking.

### Determinism

scip-python (django) and scip-typescript (vue) wrote byte-identical indexes on the same runner. django's hash also matched across the first and final runs. **scip-go did not.** Both of prometheus's runs differed in bytes in each dispatch, but every derived file, symbol and relationship pair was identical: the nondeterminism is document or occurrence order. A P1 ingest must sort everything it reads, as this one does, and must not cache by index hash.

### What this settles and what it does not

It settles four things:
- SCIP is exact and a near-superset of today's graph when the project's own configuration loads.
- Python and Go get their in-repo edges without installs.
- scip-go's relationships replace the Go `possible_implementation` heuristic.
- The default no-install mode loses exactly the edges #101 is about: a TypeScript monorepo whose tsconfigs or workspace imports live in uninstalled packages indexes nothing (dify) or loses its cross-package graph (n8n).

It does not settle whether the hosted worker may install. That needs a sandbox, which is an owner decision (#110 risk 1). Cost is minutes and GB: peak RSS reaches 5–10.5 GB for Pyright on django/dify and 8.8 GB for scip-typescript on n8n, against today's 46–297 MB builds. That bears on the worker class and hosting spend, both reserved to the owner. The runners' timings vary; the first run measured django at 133 s against 79 s here. Kotlin, Java, C#, Rust and C/C++ were not attempted. The parity oracle's fixtures would move under any SCIP default: vue's cold placement is 77.8% against today's map. #110 risk 4's re-derivation stands.

## 42. TypeScript workspace-package imports: `exports` must stay a fallback chain, not a hard replacement

Issue [#101](https://github.com/onsager-ai/tolmap/issues/101): dify's `cli` imports `@dify/contracts`, and the built map had zero edges from `cli/` to `packages/contracts`. `resolve_multi`'s TypeScript branch already mapped a workspace package's `package.json` `name` to its directory (finding 15), but only through three literal probes (`{dir}.ts`, `{dir}/index.ts`, `{dir}/src/index.ts`); it never consulted `exports`, `main`, `types`/`typings`, or `module`, and it treated *any* nested `package.json` anywhere in the tree as a global bare-specifier target, with no regard for whether the repository's own package manager would actually resolve that name there.

**What shipped.** Workspace-member discovery now reads only the repository root: `pnpm-workspace.yaml`'s `packages:` block (a hand-written line scanner tolerant of blank lines and `#`-comments between items -- n8n's file has one -- not a YAML parser; no dependency added), root `package.json` `workspaces` (array or `{packages:[]}`), and root `lerna.json` `packages`, matched against a restricted glob (`*` one segment, `**` zero or more, `!` negation). A nested `package.json` outside every discovered glob no longer contributes a resolution target -- closing a real name-collision risk (a vendored example or test fixture with a `package.json` sharing an external npm package's name could previously shadow it). `exports` resolution handles the `"."` and subpath keys, including the wildcard subpath pattern (`./api/*`), and the condition keys `types`, `import`, `default`, `require`. A `main` pointing at built output (`dist/index.js`) additionally tries the same stem under `src/`, exact file only.

**The `exports`-replaces-everything design was wrong, and vue proved it.** The first version of this fix treated `exports` as fully replacing `main`/`types`/`module`/`src-index` resolution whenever a manifest declared `exports` at all (Node's own encapsulation rule), and `exports` condition lookup returned only the *first present* condition (`types` first) rather than trying each one. Re-deriving the fixtures and running the offline parity gate caught this immediately: vuejs/core's committed fixture dropped from 1186 to 934 edges, district placement fell to 88.3% (gate ≥95%) and modularity delta rose to 0.0347 (gate ≤0.02). Every vue-core workspace package (`packages/reactivity`, `packages/vue`, `packages/runtime-core`, ...) declares an `exports` `"."` entry whose *every* condition points at unbuilt `dist/...` output (excluded from source collection, `MULTI_SKIP_DIR`) or a non-TypeScript `index.js` require stub; `types` is present and "won" first, so resolution stopped there and never reached `main`'s dist-stem fallback -- the only thing that ever found the real `src/index.ts`. The fix makes the whole candidate chain flat and cumulative: `exports` (now every present condition, in order, not just the first) is tried first, then legacy `types`/`module`/`main` with its `src/` stem fallback, then a blind `src/index.ts(x)`/`index.ts(x)` probe, all as one list, continuing past a failing tier instead of stopping at it. This is not Node's own resolution algorithm -- every candidate still only counts if it names a parsed file, so it stays "keep looking, invent nothing," the rule every other resolver in this file already follows. A regression test (`exports_pointing_at_unbuilt_output_falls_back_to_main_and_then_src_index`) replicates the exact shape.

**Fixture re-derivation and CI gates.** [Full nine-fixture parity](https://github.com/onsager-ai/tolmap/actions/runs/35961750117), rebuilt live from each pinned commit, passed on all nine after the fix -- vue is byte-identical to its committed fixture again (100.0% placement, `E` 1186 edges, modularity delta 0.0000). No fixture needed re-recording. The [offline CI gate](https://github.com/onsager-ai/tolmap/actions/runs/35961346951) (flask/httpx parity, synthetic module-resolution/polyglot/islands fixtures, determinism, clippy, tests) passed on the final commit.

**Measured on the real corpus**, [remote `dump-blend` run 35961782984](https://github.com/onsager-ai/tolmap/actions/runs/35961782984) (candidate/post-prune edges in the blended-and-pruned graph the partitioner receives, plus the new `workspace_import_coverage` diagnostic's buckets) and [remote `build` run 35962900049](https://github.com/onsager-ai/tolmap/actions/runs/35962900049) (the map's own committed `E` list, via a new `eval/measure_module_resolution.py` step):

| repo | candidate edges, main → fix | post-prune edges, main → fix | `E` (map import edges), main → fix | zero-edge files, main → fix | districts, main → fix | q, main → fix |
|---|---:|---:|---:|---:|---:|---:|
| langgenius/dify | 33,998 → **37,280** | 28,668 → **30,853** | 24,157 → **27,502** | 454 → 454 | 78 → 72 | 0.7374 → 0.6951 |
| n8n-io/n8n | 140,813 → **141,915** | 58,350 → **59,062** | 39,402 → **40,504** | 199 → **197** | 80 → 74 | 0.7793 → 0.7773 |
| microsoft/vscode | 81,237 → 81,237 | 47,617 → 47,617 | 74,719 → 74,719 | 5 → 5 | 21 → 21 | 0.5325 → 0.5325 |
| twentyhq/twenty | 131,693 → 131,693 | 100,536 → 100,536 | 82,364 → 82,364 | 554 → 554 | 102 → 102 | 0.791 → 0.791 |
| django/django (control) | -- | -- | byte-identical (`map_sha256` match) | -- | 12 → 12 | 0.5092 → 0.5092 |

Dify's district count and modularity moved the way finding 12 already documents for a denser static graph (more real edges, fewer isolated components, lower modularity) -- not itself a regression signal per finding 5/10's cross-repo modularity caution. Zero-edge-file counts held flat for dify: the coordinator's own evidence for issue #101 was files that already had a co-change edge and were missing only the direct import, not files with no edge at all, and the measurement confirms it -- these are additional, more precise edges between already-connected files, not rescued orphans.

**Workspace-import resolution buckets** (`workspace_import_coverage`, every non-relative TypeScript import specifier classified as `resolved` / `resolved_but_excluded` (a real file on disk, but under an excluded directory such as `generated/`) / `unresolved` / `external`):

| repo | resolved | resolved but excluded | unresolved | external |
|---|---:|---:|---:|---:|
| langgenius/dify | 3,453 | **441** | 0 | 19,105 |
| n8n-io/n8n | 13,561 | 1 | 1,524 | 13,405 |
| microsoft/vscode | 0 | 0 | 0 | 835 |
| twentyhq/twenty | 13 | 0 | **14,734** | 97,004 |

Dify's 441 `resolved_but_excluded` are exactly the case the owner narrowed the issue to mid-task: `@dify/contracts/api/openapi/types.gen`-shaped imports, where `packages/contracts`'s `exports` maps `./api/*` to `./generated/api/*.ts`, and `generated` is in `MULTI_SKIP_DIR`. Correct `exports` resolution still adds zero edges for these -- the lower-bound rule cares whether the file was parsed, not whether the manifest names it; the diagnostic's printed example list is exactly `packages/contracts/generated/api/...`. The exclusion list was deliberately left untouched (a separate product decision, not this fix's to make).

Microsoft/vscode declares no `workspaces` field, `pnpm-workspace.yaml`, or `lerna.json` at all -- it is not an npm/yarn/pnpm/lerna workspace in the sense this fix targets, so every non-relative import is classified `external` and the map is untouched; this was predicted before the run, not discovered by it.

Twentyhq/twenty is a real workspace (18 packages under `packages/*`, declared as literal paths in `workspaces.packages`) but its own `E`, districts and modularity are byte-identical before and after. 14,734 of its own bare-package imports match a declared workspace name yet fail to resolve, against only 13 that succeed. The proximate cause: `twenty-shared`'s own `package.json` has no `exports` field, so its subpath imports (`twenty-shared/types`, `twenty-shared/utils`, ...) fall to the plain `<dir>/<subpath>` probe -- but `twenty-shared`'s real source lives under `src/types`, `src/utils`, the same `src/`-root convention this fix already applies to the *bare* (no-subpath) case via `main`'s dist-stem fallback and the blind `src/index.ts` probe, just not yet to a subpath. This is a real, measured, lower-bound-respecting gap (nothing is invented, nothing is lost -- twenty's `E` is unchanged, not reduced), filed separately as issue [#115](https://github.com/onsager-ai/tolmap/issues/115) rather than folded into this fix.

**Two CI failures on the corpus runs above are pre-existing and unrelated, confirmed by byte-identical hashes, not fixed here.** (1) Microsoft/vscode's "Measure symbol card geometry" step failed its existing assertion (three symbol cards with a child center outside its parent, on rings roughly `1e-7` wide) -- but vscode's `map_sha256` and `symbols_sha256` are identical to main's in the same run, so this exact failure already exists on main; [PR #112](https://github.com/onsager-ai/tolmap/pull/112) replaces card geometry entirely. (2) Twentyhq/twenty's and django/django's "Measure typed edges and map parity against main" step failed a *different* assertion, `existing edge occurrences lost`, despite both repos' `map_sha256` and `symbols_sha256` being identical to main's. Cause: `eval/measure_typed_edges.py`'s `pairs()` helper filters out inherited-call/possible-implementation edges (kind 4 and 8) from the *current* side only, not from *previous* -- so any document containing inherited-call edges at all reports "losses" against itself, independent of whether the two builds differ. That script was written to gate finding 37's own typed-edges PR, whose `compare_ref` predated the feature; run against `compare_ref=main` (which already has it) for an unrelated PR, it always misfires on a repository with any inherited calls. Neither failure blocks this PR's own byte-identical map/symbols evidence above, and neither is this fix's to repair.

No dependency changes. `eval/measure_module_resolution.py` and a new `--dump-blend`-only diagnostic (`workspace_import_coverage`, wired into `blenddump.rs`'s output as `workspace_imports`) are the only additions outside `src/extract.rs` itself.

## 43. Generated-code imports redirect to the importing package's entry file, not the excluded file

Owner decision (issue [#101](https://github.com/onsager-ai/tolmap/issues/101), AskUserQuestion session `16030105`, transcript line 4511, 2026-09-24T06:39:30Z): "Count as edge targets only." Finding 42 measured dify's `packages/contracts`, whose `exports` maps `./api/*` to a `generated/` file (`MULTI_SKIP_DIR`) that correct `exports` resolution still leaves unparsed -- the lower-bound rule cares whether the file was parsed, not whether the manifest names it -- so the import added zero edges. This PR redirects such an import to the importing package's nearest non-generated file instead of leaving it unresolved: first the package's own entry point, resolved by the same candidate chain as `import "<pkg>"` (`exports` `"."`, then `types`/`module`/`main` with its `src/` stem fallback, then `src/index.ts(x)`/`index.ts(x)`), used only if that entry itself was parsed; otherwise the lexicographically first parsed `.ts`/`.tsx` file at the shallowest depth in the package directory (deterministic -- `by_file` is a sorted `BTreeMap`, and every path in it already excludes `MULTI_SKIP_DIR` since source collection never walks into those directories); otherwise the import stays unresolved, unchanged from before. Generated files still gain no node, footprint or district of their own -- only the edge's target moves. `workspace_import_coverage` gains a `redirected_from_excluded` bucket alongside the existing `resolved_but_excluded`, so the numbers say how many of those specifiers actually turned into an edge.

[Full nine-fixture parity](https://github.com/onsager-ai/tolmap/actions/runs/35966951605) and the same run's offline gate (flask/httpx parity, synthetic polyglot/module-resolution/islands fixtures, determinism, clippy, `cargo test`) passed on the final commit; the redirect rule affects none of the nine committed fixtures.

**Measured on the real corpus**, [remote `build` run 35966535603](https://github.com/onsager-ai/tolmap/actions/runs/35966535603) (each map's own committed `E`, districts and modularity, via `eval/measure_module_resolution.py`) and [remote `dump-blend` run 35977859629](https://github.com/onsager-ai/tolmap/actions/runs/35977859629) (the `workspace_import_coverage` diagnostic's buckets, scoped to the three workspace repos):

| repo | import edges, main → branch | districts, main → branch | q, main → branch | `resolved_but_excluded` | `redirected_from_excluded` |
|---|---:|---:|---:|---:|---:|
| langgenius/dify | 27,502 → **27,868** | 72 → 71 | 0.6951 → 0.6868 | 441 | **441** |
| n8n-io/n8n | 40,504 → **40,505** | 74 → 78 | 0.7773 → 0.7775 | 1 | **1** |
| twentyhq/twenty | 82,364 → 82,364 | 102 → 102 | 0.791 → 0.791 | 0 | 0 |
| django/django (control) | byte-identical (`map_sha256`, `edges_identical`, `symbols_identical` all `true`) | 12 → 12 | -- | -- | -- |

Dify's 441 `resolved_but_excluded` specifiers (unchanged from finding 42's count -- neither the exclusion list nor the parse set moved) all redirect. `cli/` now carries real edges into `packages/contracts`: 18 new `cli/*.ts -> packages/contracts/console.ts` edges (the package's resolved entry) alongside the 4 `cli/*.ts -> packages/contracts/openapi-ts.api.config.ts` edges finding 42's `exports` fix had already added, both confirmed present in the branch's blended graph and absent (for the 18) from main's. The 441 specifier-level redirects collapse to 366 net new directed edges in the blended graph (27,868 − 27,502): multiple imports from one file into the same redirected entry, and multiple originally-distinct excluded targets that redirect to the same entry, dedupe into one edge each. N8n's single `resolved_but_excluded` specifier also redirects, adding exactly one edge (40,504 → 40,505). Twenty has none: its own gap is issue [#115](https://github.com/onsager-ai/tolmap/issues/115)'s separate `src/`-subpath convention, untouched by this rule. Django is a Python control repo with no TypeScript workspace at all and its map, edges and symbols are confirmed byte-identical.

District count and modularity move for dify and n8n the way finding 12 already documents for a denser static graph (more real edges can merge or split communities); this is not itself a regression signal per finding 5/10's cross-repo modularity caution, and none of the nine committed fixtures are affected by it.

The [full-corpus `build` run](https://github.com/onsager-ai/tolmap/actions/runs/35966535603) (dispatched without a `repos` filter, so it built the whole default corpus rather than only the four repos this fix concerns) shows dify's and n8n's own jobs as failed: both hit the pre-existing `eval/measure_typed_edges.py` `assert primary_map == compare_map` at the top of that script, which finding 42's PR (`#113`) already documented as expected to misfire on these exact four repos once their maps legitimately differ from main -- `remote-build.yml`'s own inline comment on the following step says so directly. The four repos' own `Measure workspace-package import resolution`/`measure_module_resolution.py` steps, which do not assume a byte-identical map, ran and passed regardless (`if: always()`). Several unrelated repos in that same full-corpus run also failed for reasons this PR did not investigate, since they are outside its four-repo measurement scope.

No dependency changes. Only `src/extract.rs` changed: `redirect_excluded_workspace_import` and `shallowest_parsed_file_in_package` added to the TypeScript resolution chain, plus one new `redirected_from_excluded` counter in `workspace_import_coverage`.

## 44. The product's SCIP ingest reproduces P0's oracle exactly; where an index is admitted, districts move, and where it falls back, nothing does

Owner decisions, session `16030105`, AskUserQuestion: line 3770 (2026-09-24T05:07:45Z) **"Go to SCIP now"**; line 4359 (06:15:43Z) **"Bigger production machine"**, **"Installs in a sandbox"**; line 4376 (06:26:42Z) **"Go: P1a+P1b now, P1c design first"**. This is issue [#110](https://github.com/onsager-ai/tolmap/issues/110)'s P1a, [PR #119](https://github.com/onsager-ai/tolmap/pull/119). It uses number 44 because main has 43 (#118).

**What ships.** `tolmap build --refs scip` (default `hand`; also `WorkerSpec.refs` and `TOLMAP_REFS`) runs scip-python and scip-go at the repository root and scip-typescript once over every tracked `tsconfig.json`, deepest first (finding 41's method), from `src/indexers.rs`. Nothing is installed: Go runs with `GOPROXY=off`, `GOTOOLCHAIN=local` and an empty module cache. `src/scip_ingest.rs` streams each index one `Document` at a time and derives what P0's `eval/scip_ingest.py` derives. In-repo file pairs weighted by distinct referenced symbols replace the `static` signal ahead of the unchanged mass-normalised blend (finding 1). Reference occurrences are credited to the innermost span of the finished symbols document on both ends. `is_implementation` rows become `implements` (type → interface), `extends` (other type → type) or `overrides` (callable → callable), and Go's `possible_implementation` rows are dropped. A language takes the SCIP path only if its indexer exits 0 and the index keeps at least `MIN_RECALL` = 0.80 of the hand-written graph's intra-language pairs, compared by file for Python and TypeScript and by target directory for Go, because `resolve_multi` spreads a Go import over the whole package. The map's `coverage.references` records the path, a reason code, the indexer version, recall and pair counts for each language.

**Why 0.80.** Finding 41's no-install table puts every configuration that loaded at 0.894 or more: dify's Python at the root is the lowest, then django 0.990, vue 0.998, and prometheus's Go 0.997 by directory. n8n's TypeScript lost its `@n8n/*` graph and kept 0.514. On today's main, which has #113 and #118, n8n keeps 0.5003 and dify's Python 0.8942, so the margins are 0.30 below and 0.09 above.

[Remote-build run 35980662134](https://github.com/onsager-ai/tolmap/actions/runs/35980662134) (`command=scip-ingest`) built the branch at `257b74e`, rebased on main `0c56429`, on standard runners with P0's pinned indexers (scip-typescript 0.4.0, scip-python 0.6.6, scip-go v0.2.7). It built the five `eval/corpus.toml` pins with `--refs hand` once, `--refs scip` three times cold (the first keeping its indexes in `TOLMAP_SCIP_INDEX_DIR`), and `--refs scip` once warm-started from the hand map. `eval/scip_ingest_measure.py` then ran P0's Python ingest on the very indexes the Rust build read.

### The Rust ingest against the oracle

| repo | language | path (reason) | recall, Rust = oracle | hand pairs | SCIP pairs, Rust = oracle | map `E` = oracle pairs | symbol reference pairs / occurrences, Rust = oracle | implementations typed / oracle |
|---|---|---|---:|---:|---:|---|---|---:|
| django | py | scip (indexed) | 0.9899 | 3,173 | 4,018 | yes | 18,453 / 28,689 | 3,219 / 3,219 |
| dify | py | scip (indexed) | 0.8942 | 7,611 | 10,961 | yes | 62,900 / 139,239 | 3,439 / 3,439 |
| dify | ts | hand (indexer_failed, exit 1) | — | 20,257 | — | — | — | — |
| n8n | ts | hand (below_min_recall) | 0.5003 | 40,505 | 23,840 | — | — | — |
| prometheus | go | scip (indexed) | 0.9969 (directory) | 956 | 5,436 | yes | 19,952 / 47,072 | 1,630 / 1,630 |
| prometheus | ts | scip (indexed) | 1.0000 | 514 | 530 | yes | 1,598 / 3,988 | 21 / 21 |
| vue | ts | scip (indexed) | 0.9975 | 1,186 | 1,911 | yes | 9,367 / 17,257 | 16 / 16 |

**Every number agrees with the oracle exactly.** For every admitted language, the map's `E` is the oracle's file-pair set with no pair on either side alone. Every credited symbol pair has the oracle's occurrence count, with zero pairs differing. Every oracle implementation pair is in the map as an inheritance row. For the two fallbacks, the gate's pair count and recall equal the oracle's. The SCIP pair counts also equal finding 41's P0 table (4,018; 10,961; 5,436; 530; 1,911; 23,840). Rust and Python decode the same `scip.proto` (v0.10.0) independently, so this is two implementations agreeing, not one checking itself.

### Against `--refs hand`

Placement is `tolmap parity` of the SCIP map against the hand map. "Warm" seeds Leiden from the hand map, as the product would when a repository switches. Peak RSS is the largest single process, which is the indexer.

| repo | hand districts / q | SCIP districts / q | cold placement | warm districts / q / placement | zero-edge files hand → SCIP | `E` hand → SCIP | build s hand → SCIP | peak RSS MB hand → SCIP |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| django | 12 / 0.5092 | 14 / 0.5004 | 70.5% | 12 / 0.5024 / 89.8% | 215 → 216 | 3,173 → 4,018 | 5.2 → 130.1 | 45 → 4,904 |
| dify | 71 / 0.6868 | 36 / 0.6875 | 73.1% | 34 / 0.6826 / 94.6% | 292 → 231 | 27,868 → 31,218 | 30.8 → 297.6 | 191 → 6,914 |
| n8n | 78 / 0.7775 | 78 / 0.7775 | 100.0% | 78 / 0.7776 / 99.9% | 134 → 134 | 40,505 → 40,505 | 65.8 → 371.4 | 273 → 9,123 |
| prometheus | 11 / 0.6266 | 16 / 0.6693 | 80.7% | 14 / 0.6804 / 89.5% | 8 → 35 | 6,088 → 5,966 | 5.7 → 17.3 | 44 → 738 |
| vue | 8 / 0.5389 | 7 / 0.4739 | 74.5% | 8 / 0.4806 / 83.7% | 0 → 0 | 1,186 → 1,911 | 1.9 → 24.2 | 32 → 739 |

**A fallback costs time, not bytes.** n8n's TypeScript falls back. Its SCIP map places 100% of files as the hand map does, with the same districts, `q` and `E`; the only differences are the recorded `coverage.references` and 5.1 minutes of scip-typescript at 9.1 GB. The 99.9% warm figure is Leiden seeded from the hand map on an identical graph, not a change in edges. dify's TypeScript fails in 0.5 s (TS6053, finding 41) and keeps its hand-written graph, while dify's Python is admitted. That consolidates dify from 71 districts to 36 and cuts its zero-edge files from 292 to 231.

**Where an index is admitted, the partition moves more than the pair overlap suggests**, as finding 41 found with its approximate construction. Cold placement is 70.5–80.7%, and warm-starting brings it to 83.7–94.6%. The product's numbers differ slightly from P0's re-partition (django 71.1% → 70.5%, prometheus 83.0% → 80.7%) because the product computes co-change, proximity and semantic exactly for pairs that are new to the candidate set, where P0 zeroed co-change and semantic. Also, #113 and #118 changed the hand-written TypeScript graph in between.

**Determinism.** Three cold `--refs scip` builds wrote byte-identical map and symbols documents on all five repositories. That includes prometheus, whose scip-go index bytes differ between runs (finding 41): the ingest sorts everything it derives. CI's new `scip` job repeats the three-build check on every push, on the synthetic polyglot fixture (scip-go, scip-typescript) and the synthetic islands fixture (scip-python). It also asserts that each language actually took the SCIP path and that a missing indexer falls back rather than failing.

### Symbols

Rows by kind for SCIP-path languages, hand → SCIP. `reference` is a SCIP pair the tree-sitter pass did not resolve, so no syntactic kind is known. Pairs it did resolve keep their hand-written kind with SCIP's occurrence count, and hand pairs SCIP does not confirm are dropped.

| repo | language | call | annotation | decorator | value | reference | extends | implements | overrides | possible_implementation |
|---|---|---|---|---|---|---|---|---|---|---|
| django | py | 8,529 → 8,311 | 14 → 14 | 385 → 4 | 764 → 717 | 0 → 9,407 | 1,507 → 1,574 | 0 → 0 | 2,037 → 2,173 | 0 → 0 |
| dify | py | 9,790 → 9,734 | 3,808 → 3,360 | 598 → 54 | 224 → 220 | 0 → 49,532 | 1,138 → 2,519 | 0 → 0 | 136 → 1,116 | 0 → 0 |
| prometheus | go | 3,685 → 3,555 | — | — | 30 → 29 | 0 → 16,368 | 0 → 0 | 0 → 517 | 0 → 1,113 | 902 → 0 |
| prometheus | ts | 479 → 438 | 431 → 431 | — | — | 0 → 729 | 12 → 12 | 4 → 4 | 0 → 17 | — |
| vue | ts | 2,320 → 2,073 | 2,075 → 2,063 | — | 2 → 2 | 0 → 5,229 | 4 → 4 | 4 → 7 | 3 → 11 | — |

Go's 902 `possible_implementation` candidates (finding 37) are replaced by 517 exact `implements` rows (struct → interface) and 1,113 `overrides` rows (method → interface method). Inheritance rows are the union of the hand-written rows and SCIP's, because hand rows are resolved declarations rather than guesses. dify's `overrides` go from 136 to 1,116 and its `extends` from 1,138 to 2,519.

### What this costs, and three effects that are real

- **Cost is minutes and gigabytes.** Indexing took 125 s for django's Python, 266 s for dify's Python, 305 s for n8n's TypeScript, 22 s for vue, and 5 + 7 s for prometheus. Peak RSS was 4.9–9.1 GB against 32–273 MB for the hand-written build. This is why the owner chose a bigger production machine; hosted jobs stay `hand` until P1b's image and that machine are live.
- **Unindexed files lose their edges.** The fallback is per language, not per file. prometheus has 35 of 444 Go files that scip-go does not document (platform and build-tag variants plus nested modules, finding 41), and they lose every static edge: zero-edge files go from 8 to 35. A per-file fallback, which would keep hand-written edges for files the index does not document, is a design question for P2, not a tuning change.
- **Decorators no longer credit their function.** A decorator sits above the line where its function's span starts, so crediting on the span, as P0 does, gives the decorator reference to the enclosing class or module. Decorator rows fall from 385 to 4 on django and from 598 to 54 on dify. Crediting from the tree-sitter pass's `credit_start` would restore them, but it breaks exact agreement with the oracle, so it is left for P2 together with re-deriving the fixtures.
- **Go package clauses are edges.** P0's primary weighting counts each file's `package x` clause as a reference to the package symbol, whose definition sits in one file of the package. Every file in a package therefore links to that file. On the synthetic polyglot fixture this turns 3 hand-written Go pairs (by directory) into 36 SCIP pairs. P0 measured the `uses` variant, which drops these pairs, and found it not consistently better (finding 41), so the weighting stays P0's.

### `--refs hand` is unchanged

- **Nine fixtures:** [remote-build run 35980673775](https://github.com/onsager-ai/tolmap/actions/runs/35980673775) built `257b74e` and main on the nine fixture pins. Map and symbols documents are byte-identical on all nine. The run's red status is its `eval/symbol_stats.py` step. That step fails with `KeyError: 'symbol_rings'` in `sample_sibling_overlaps` on maps without card geometry, reading outputs that are byte-identical to main's, so it would fail the same way on main.
- **Corpus:** [remote-build run 35980684177](https://github.com/onsager-ai/tolmap/actions/runs/35980684177) built `257b74e` and main on all 132 `eval/corpus.toml` pins. 128 are byte-identical in both the map and the symbols document. The other 4 failed identically on both binaries: sveltejs/svelte and withastro/astro clear no `--all-sources` floor, axios/axios has no supported source, and microsoftgraph/msgraph-sdk-go stops during region drawing with no build output.
- **CI:** the offline flask/httpx parity and the three-build `--all-sources` determinism check pass unchanged.

### Not settled here

Whether SCIP becomes the default is P2. That means re-deriving the fixtures, since the oracle's membership moves (vue cold 74.5%), deciding the per-file fallback and decorator crediting above, and showing the viewer which references are exact. Dependency installs, which would admit dify's and n8n's TypeScript (finding 41: dify installed keeps 16,539 of 16,546 pairs), wait for P1c's sandbox. Rooting Python per project, which finding 41 measured at 0.970 for dify's `api/`, is not done.

## 45. Under `--refs scip`, eight of the nine fixtures move below the parity threshold against their hand maps; hand stays the default, and SCIP gets fixtures of its own as the oracle

Owner decision, session `16030105`, AskUserQuestion, transcript line 6390 (2026-09-25T13:42:06Z): **"P1c sandbox + P2a together (Recommended)"**. This is issue [#110](https://github.com/onsager-ai/tolmap/issues/110)'s P2a, [PR #127](https://github.com/onsager-ai/tolmap/pull/127). It uses number 45 because main has 44.

**What changed.** This PR was written to make `scip` the default and measured what that would do; the flip was then dropped by the owner (see the end of this finding). What remains: `tolmap build --refs` and the service's `TOLMAP_REFS` both take `RefsMode::default()`, which stays `hand`, so the two cannot drift apart. CI's `scip` job checks that a build naming no `--refs` is byte-identical to an explicit `--refs hand` build on a runner with every indexer installed, and that the missing-indexer fallback works under `--refs scip`. The measurements below were taken while the branch defaulted to `scip`; every build in them named `--refs` explicitly, so they do not depend on the default.

**The oracle keeps testing what it tested.** `data/*.json` is the frozen Python reference's output (two fixtures are the Rust hand path's, see `data/fixtures.toml`), and that reference only knows the hand resolver. Every gate that compares with it passes `--refs hand` explicitly instead of relying on the default, so no future change of default can move what the oracle tests:
- ci.yml `gate`: the offline flask/httpx parity and synthetic polyglot `--graph` builds (which ignore `--refs`, but are pinned so no gate depends on the default), the module-resolution fixture, the three-build polyglot determinism check and the islands fixture;
- ci.yml `full-fixtures`;
- remote-build.yml's build steps, unless `extra_args` names `--refs`, and only for a binary that has the option. These runners install no indexer, so a `--refs scip` build there would be an all-fallback SCIP build whose map differs from a hand build only by `coverage.references`;
- the hand baselines in scip-spike.yml and scip-ingest.yml, and `web/scripts/generate-maps.sh`, whose naming caches are seeded from `data/*.json`.

[Remote-build run 36144144728](https://github.com/onsager-ai/tolmap/actions/runs/36144144728) built the nine fixture pins with this branch and with main (`compare_ref=main`). Map and symbols documents are byte-identical on all nine. [CI run 36144110628](https://github.com/onsager-ai/tolmap/actions/runs/36144110628)'s `full-fixtures` passed on all nine with `--refs hand`.

### SCIP against hand on the nine fixtures

The same run's new `scip-fixtures` job built each fixture pin twice on one standard runner, with `--refs hand` and `--refs scip`. It used the full-fixtures clone and the pinned indexers: scip-python 0.6.6, scip-go v0.2.7 and scip-typescript 0.4.0. `eval/scip_fixtures.py measure` then scored the SCIP map against the hand map with the parity gate's placement metric. It checks its Python mirror of `src/parity.rs` against `tolmap parity` on every pair, and they agreed on all nine. The hand maps placed 100.0% of files, with Δq 0.0000, against the committed `data/*.json` on all nine, so "against hand" here is also "against the oracle".

| fixture | files | districts hand → SCIP | q hand → SCIP | placement SCIP vs hand | Δq | ≥ 95% and ≤ 0.02 | `E` hand → SCIP (both) | zero-edge files hand → SCIP | reference path (reason, recall, files indexed) | build s hand → SCIP (indexing) | peak RSS MB hand → SCIP |
|---|---:|---:|---:|---:|---:|---|---:|---:|---|---:|---:|
| celery | 161 | 7 → 8 | 0.3606 → 0.3349 | 62.7% | 0.0257 | **no** | 669 → 710 (648) | 6 → 7 | py: scip (indexed, 0.9686, 161/161) | 0.7 → 29.8 (29.1) | 30 → 1,401 |
| django | 851 | 12 → 13 | 0.5061 → 0.5034 | 85.8% | 0.0027 | **no** | 3,173 → 4,018 (3,141) | 215 → 216 | py: scip (indexed, 0.9899, 851/851) | 2.0 → 96.6 (94.5) | 52 → 3,900 |
| flask | 24 | 4 → 3 | 0.0326 → 0.0425 | 70.8% | 0.0099 | **no** | 102 → 113 (93) | 0 → 0 | py: scip (indexed, 0.9118, 24/24) | 0.3 → 7.0 (6.7) | 22 → 397 |
| httpx | 23 | 4 → 2 | 0.0297 → −0.0043 | 47.8% | 0.0340 | **no** | 87 → 74 (71) | 0 → 1 | py: scip (indexed, 0.8161, 23/23) | 0.1 → 10.6 (10.4) | 23 → 443 |
| prometheus | 444 | 9 → 13 | 0.5430 → 0.5725 | 72.3% | 0.0295 | **no** | 5,574 → 5,436 (4,842) | 4 → 31 | go: scip (indexed, 0.9969 by directory, 409/444) | 2.4 → 6.5 (4.0) | 39 → 783 |
| rich | 100 | 4 → 5 | 0.3437 → 0.3263 | 48.0% | 0.0174 | **no** | 426 → 424 (420) | 0 → 0 | py: scip (indexed, 0.9859, 100/100) | 0.5 → 21.9 (21.5) | 25 → 724 |
| scrapy | 188 | 8 → 7 | 0.3454 → 0.3268 | 77.7% | 0.0186 | **no** | 902 → 1,222 (902) | 5 → 5 | py: scip (indexed, 1.0000, 188/188) | 0.6 → 22.7 (22.1) | 31 → 1,155 |
| sqlalchemy | 258 | 6 → 6 | 0.4318 → 0.4318 | 100.0% | 0.0000 | yes | 2,762 → 2,762 (2,762) | 0 → 0 | py: **hand** (below_min_recall, 0.6608, 258/258) | 4.5 → 134.2 (129.7) | 52 → 3,901 |
| vue | 239 | 8 → 7 | 0.5389 → 0.4739 | 74.5% | 0.0650 | **no** | 1,186 → 1,911 (1,183) | 0 → 0 | ts: scip (indexed, 0.9975, 233/239) | 0.8 → 15.0 (14.2) | 32 → 684 |

Build seconds and peak RSS come from one runner and one build each, so they are indicative, not a benchmark.

**Eight of nine fixtures fall below the parity threshold against their own hand maps.** Placement runs from 47.8% (httpx) to 85.8% (django). Every fixture that took the SCIP path fails placement, and five of the eight also fail modularity. The one pass, sqlalchemy, is a fallback. scip-python keeps only 0.6608 of its hand-written pairs, so the map is the hand map, having spent 130 s indexing. Why sqlalchemy's recall is that low was not investigated. vue's corpus pin is its fixture pin, and its numbers are exactly finding 44's: 7 districts, q 0.4739, 74.5%. Django's differ (85.8% against finding 44's 70.5%) because `eval/corpus.toml` pins a different django commit (`dd6f6b1`, hand q 0.5092, against the fixture's `a3f0642`, 0.5061).

**The small fixtures move most, on the fewest edges.** Their partitions are weakly modular (flask q 0.03, httpx 0.03), so a handful of changed pairs moves whole districts. httpx is the extreme case. SCIP finds fewer pairs than the hand resolver (74 against 87, recall 0.8161, just over the gate), the map drops from four districts to two, and q goes negative. That is a partition no better than chance by modularity's own measure. The gate admits httpx's index at 0.8161, and this is what the admitted index draws. Whether 0.80 is the right floor for a 23-file package is a question this finding raises, not one it settles. Placement against a fixture measures agreement, not correctness. This table does not say which map is better, only that the default moves the maps the oracle recorded.

**Cost.** Indexing adds 4 s (prometheus's Go) to 130 s (sqlalchemy) per build, and peak RSS rises from 22–52 MB to 0.4–3.9 GB. These are small repositories. Finding 44 has the corpus-scale numbers.

### The SCIP path's own fixtures

The Python reference cannot produce a SCIP map, so the SCIP path is regression-gated against fixtures re-derived under it: `data/scip/<name>.json`, recorded by `eval/scip_fixtures.py record` from the `--refs scip` maps of the run above. Each fixture is a summary, not a map: `F`, each file's district `D`, `q`, the district and edge counts, and the build's `coverage.references` block, which names the indexer versions. The nine together are 124 KB. A whole map would re-commit layout geometry that no gate reads. ci.yml's `scip-fixtures` job, run on dispatch and nightly like `full-fixtures`, rebuilds the nine with `--refs scip` and requires, per fixture:
- ≥ 95% placement against the SCIP fixture;
- modularity within 0.02;
- an identical `F`;
- the same reference path for every language.

The last requirement makes a language that flips between SCIP and fallback, such as sqlalchemy clearing 0.80 under a new indexer, a visible failure rather than a silent membership change. Every run also uploads a fresh `record/` set, so the SCIP fixtures are re-derived by committing that set with a finding, never by the job itself.

### ETA

A queued job's ETA had no repository features, and the per-language indexing seeds cost nothing without languages. Under `TOLMAP_REFS=scip`, a queued job would therefore have been quoted a hand build's prior (6.14–101.163 s). The service now records its reference mode in the job's features at enqueue. With nothing else known, a SCIP job's prior spans finding 44's whole-build range, 17.3 s (prometheus) to 371.4 s (n8n). On the hand default, `refs` stays absent and the prior is exactly what it was. Once the worker reports its languages, the per-language indexing seeds apply as before.

### Not settled here

- Per-file fallback and decorator crediting (finding 44).
- Why sqlalchemy's recall is 0.66.
- Whether the 0.80 floor suits very small packages.

### The default did not flip

Owner decision, session `16030105`, AskUserQuestion, transcript line 6937 (2026-09-25T16:56:34Z): **"Tune hand, SCIP as oracle (Recommended)"**, with the owner's own note at line 6929 (16:51:25Z): *"Hand written is in more control with less deps and more efficient"*. `hand` stays the default for the CLI and the service. SCIP becomes the measuring stick the hand resolver is tuned against, and this finding's `data/scip` fixtures and `scip-fixtures` job become that oracle's own regression gate. Two reasons. The first is cost: the table above shows indexing adds seconds to minutes and 0.4–3.9 GB per build on these small fixtures, where the hand build takes seconds and tens of MB, and it needs indexers and toolchains on the worker. The second is [PR #130](https://github.com/onsager-ai/tolmap/pull/130)'s churn diagnosis: most of the district movement above comes from SCIP's distinct-symbol weighting, not from its pairs. The pairs are more exact, and the hand resolver's fixable errors in them are package `__init__` over-attribution, re-exports and Go's per-directory spread. Hand is right about star imports, which SCIP cannot see. So the movement measured here is not evidence that the SCIP maps are better, and it is cheaper to fix the hand resolver's known errors against SCIP than to ship the indexers.

## 46. Sandboxed installs work and are contained, but registry-only egress admits neither monorepo that needs them: dify needs nodejs.org, n8n needs codeload.github.com

Owner decisions on [#117](https://github.com/onsager-ai/tolmap/pull/117) (session `16030105`, line 4551, 2026-09-24T06:51:34Z): nsjail on the single machine, egress to `registry.npmjs.org` only, a 20 min / 20 GB install bound that falls back and never fails a job, npm-ecosystem installs only, and fail safe. Go for this change: session `16030105`, line 6390, 2026-09-25T13:42:06Z, "P1c sandbox + P2a together (Recommended)". This is issue [#110](https://github.com/onsager-ai/tolmap/issues/110)'s P1c, [PR #129](https://github.com/onsager-ai/tolmap/pull/129). It uses number 46 because open PR #127 adds 45.

**What ships.** Before scip-typescript, a TypeScript workspace (`pnpm-workspace.yaml`, or `package.json` `workspaces`) with a pnpm or npm lockfile at its root is installed with `pnpm install --frozen-lockfile --ignore-scripts --ignore-pnpmfile` or `npm ci --ignore-scripts`, using the image's own pnpm 12.6.0 and npm, with pnpm's `packageManager` handling off (`src/indexers.rs`). It runs in nsjail 3.6, which root starts with `--disable_clone_newuser`. The install therefore runs as the worker's uid with no capabilities and no user namespace. Its root is a tmpfs holding read-only `/usr`, `/etc` and the Node prefix. The checkout is mounted read-write at its own path with its `.git` read-only over it. The only network is loopback, and its only exit is an in-process CONNECT proxy that allows `registry.npmjs.org:443` and dials only globally routable addresses. A self-test runs in the jail before every install. The job service runs the install when its unprivileged worker asks over the worker protocol (`install_request`); `tolmap build --install sandbox` runs it in process as root. `coverage.references.ts.install` records `installed`, `skipped` or `fell_back`, with a reason.

### What CI proves on every push

The `scip-install` job ([run 36151261955](https://github.com/onsager-ai/tolmap/actions/runs/36151261955)) runs on a standard runner, with `tolmap` as root through sudo:
- **Egress.** In the jail, `tests/fixtures/sandbox/egress_check.js` passes 17 of 17 checks. `registry.npmjs.org` answers a real request over TLS through the proxy. The proxy refuses (403) `github.com`, `codeload.github.com`, `registry.yarnpkg.com`, `registry.npmjs.org.evil.example`, `registry.npmjs.org:80`, `169.254.169.254` on 80 and 443, `[fdaa::3]` and `127.0.0.1:8787`, and refuses plain HTTP to the metadata address (405). Direct connections to `169.254.169.254`, a GitHub address and `1.1.1.1` fail with ENETUNREACH, and DNS fails.
- **Install and determinism.** A synthetic pnpm workspace (`tests/fixtures/scip/ts-workspace`) installs one real npm package, `tiny-invariant`. It installs in 0.3 s, and three cold installs and builds are byte-identical. The install is what admits TypeScript: with it, recall is 1.0000 (3 of 3 hand pairs) and TypeScript takes the SCIP path. Without it the cross-package import does not resolve, recall is 0.6667 and TypeScript falls back to `hand`. `node_modules` is owned by the checkout's owner, not root.
- **Fallbacks.** Each fallback is recorded in the map, the build succeeds, and no `node_modules` is left behind. Covered: nsjail missing (`sandbox_unavailable`); not root (`sandbox_unavailable`); a 0.1 s time bound (`install_timeout`); a one-byte disk budget (`install_disk_budget`, 3 `node_modules` directories removed).

The image workflow ([run 36151267765](https://github.com/onsager-ai/tolmap/actions/runs/36151267765)) repeats the egress check and the install inside the built image (`--privileged`, standing in for a root VM). Run as a default container, which lacks `CAP_SYS_ADMIN`, the self-test fails, the install falls back as `sandbox_unavailable` and TypeScript goes to `hand`. That is the fail-safe path for a host that forbids namespaces. It also runs a SCIP job through `tolmap serve`: the service is root, the worker is uid 10001, and the worker asks for the install over the protocol. The job finishes `done`, with the `install` stage done in 0.34 s and `installed` recorded in the map. The image grows from 752 MB to 816 MB.

### On the three TypeScript monorepos

[Remote-build run 36154802822](https://github.com/onsager-ai/tolmap/actions/runs/36154802822) (`command=scip-install`, branch at `c8dda3a`, standard runners) built each `eval/corpus.toml` pin three times: `--refs hand`, `--refs scip` without installs, and `--refs scip --install sandbox`. [Run 36152353599](https://github.com/onsager-ai/tolmap/actions/runs/36152353599), one commit earlier, gave the same paths and pair counts, except dify's failure reason (below).

| repo | TS hand pairs | no install | with install | install outcome | install stage s | MB written | registry MB in | refused by the proxy |
|---|---:|---|---|---|---:|---:|---:|---|
| vue | 1,186 | scip, recall 0.9975, 1,911 pairs | scip, recall 0.9975, 1,911 pairs | installed | 10.3 | 1,312 | 171.6 | — |
| n8n | 40,505 | hand (below_min_recall 0.5003), 23,840 pairs | the same | fell_back (install_failed) | 120.7 | 6,751 | 811.2 | `codeload.github.com` x3 |
| dify | 20,257 | hand (indexer_failed, TS6053) | the same | fell_back (install_failed) | 114.0 | 3,455 | 477.5 | `nodejs.org` x3 |

| repo | wall s: hand / scip / scip+install | peak RSS MB: scip / scip+install | districts, q (both SCIP builds) | placement, install vs no-install |
|---|---|---|---|---:|
| vue | 1.9 / 20.0 / 35.5 | 855 / 1,114 | 7, 0.4739 | 100.0% |
| n8n | 64.6 / 301.8 / 428.2 | 8,082 / 8,428 | 78, 0.7775 | 100.0% |
| dify | 31.4 / 227.2 / 342.7 | 6,814 / 8,654 | 36, 0.6875 | 100.0% |

**Under registry-only egress, the installs that would matter do not happen.** n8n's lockfile has one git dependency, `wa-sqlite` as a `codeload.github.com` tarball. The proxy refuses it, pnpm fails after its retries, and the install falls back. dify pins its Node.js runtime as a dependency (`node@runtime:24.21.0`). pnpm fetches it from `nodejs.org`, which the proxy also refuses. Both failures are the policy working as the owner set it: the design (docs/SCIP_SANDBOX.md §3.4) predicted that a lockfile needing git hosts "fails its install and falls back". The measurement shows that this is exactly the case for the two monorepos finding 41 found losing edges. vue installs cleanly and gains nothing, because its configuration already loads without installs. The two admission gains finding 41 measured with unrestricted installs therefore remain out of reach: dify's TypeScript going from nothing to recall 1.000, and n8n's cross-package graph. Reaching them needs `nodejs.org` (dify) or `codeload.github.com` (n8n) on the allowlist, which is an owner decision. The design flags git hosts as an exfiltration channel.

**A fallback costs time, not bytes.** Every map is byte-for-byte what installs off would give, with 100% placement against the no-install SCIP map and the same districts and modularity. Only `coverage.references.ts.install` is added. The time is the failed install: n8n and dify spend about two minutes each fetching from the registry before pnpm gives up on the refused host, plus scip-typescript's own run. Peak RSS rose by up to 1.8 GB (dify); whether that is pnpm itself or run-to-run variance in the indexers was not separated (the previous run showed the same direction, 6.4 → 8.1 GB). The 20 GB budget was never approached. The jail's writes (6.8 GB for n8n, including pnpm's store) are removed, except any `node_modules` the checkout had before.

**Two bugs found on the way, both fixed here.** nsjail 3.6 multiplies `--rlimit_fsize`/`--rlimit_as` by 1 MiB after parsing, so `inf` overflows to a negative file-size limit. Every write in the jail then failed with EFBIG, and pnpm died with SIGSEGV/SIGXFSZ and no output; the jail now passes 16 TiB. And `pnpm_config_runtime_on_fail=ignore` made pnpm drop dify's `node@runtime:` specifier before comparing with the lockfile, so its frozen install failed as outdated ([run 36152353599](https://github.com/onsager-ai/tolmap/actions/runs/36152353599)). The setting added no containment, because the proxy already refuses `nodejs.org`, so it was removed.

### What was not exercised

- **The production host.** Everything above ran on GitHub's Ubuntu 24.04 runners (cgroup v2, AppArmor) and in Docker on them. The production host's kernel is untested, and so is its cgroup v1 memory controller. The memory cgroup (`TOLMAP_INSTALL_MEMORY_MAX`) stays off by default and was not exercised. Also untested there: whether a root VM there lets nsjail create mount, pid and net namespaces without user namespaces. Docker `--privileged` is a stand-in for a root VM, not the VM. The staging host was not tested either. Where either forbids namespaces, the restricted-container run shows the fallback, not an install.
- **npm lockfiles on a real repository.** None of the three uses npm; only the unit tests cover npm's policy path.
- **Resource containment of a hostile install.** There is no memory or pids limit by default, so an install that exhausts memory or pids can still hurt the machine within its 20 minutes. The indexers still run unsandboxed after the install, as the worker's uid with network, and read `node_modules` the jail wrote. docs/SCIP_SANDBOX.md §2 found no code-execution path in scip-typescript's reading of it, but marked that **unverified**, and it still is.

## 47. Most of SCIP's district churn is the partitioner's own sensitivity plus SCIP's symbol-count weights, not a wrong pair set; sqlalchemy's 0.66 recall comes from re-exports

The owner chose "Investigate, then decide (Recommended)" in an AskUserQuestion answer, session `16030105`, 2026-09-25. This is issue [#110](https://github.com/onsager-ai/tolmap/issues/110)'s P2a follow-up to finding 45, [PR #130](https://github.com/onsager-ai/tolmap/pull/130). It uses number 47 because main has 46 (#129). It is an evidence PR: its only product-code change is `dump-graph --refs` below. It is the diagnosis finding 45's "The default did not flip" cites; the owner's "Tune hand, SCIP as oracle" decision there rests on it.

Finding 45 measured SCIP maps placing 47.8–85.8% of files where the hand maps do. Placement measures agreement, not correctness. This finding asks what that number is made of, and whether it would be about as low under any change to the graph.

**Method.** `tolmap dump-graph` gains an eval-only `--refs` (default `hand`, so every existing dump, including `data/ci/*.graph.json`, is unchanged; it never installs dependencies, as the measured path did not). It dumps the exact graph a `build --refs scip` partitions. `eval/scip_churn.py` assembles variants of the hand graph the way `finish_graph` does, and ci.yml's `scip-churn` job (dispatch only, behind the `scip_churn` input) rebuilds each variant through the product's own `tolmap build --graph`. Each variant is scored with the parity gate's placement against the hand graph's map.

The variants:
- **node order**: the hand graph with its nodes shuffled, 8 draws. The graph is identical; only vertex ids change, so Leiden walks a different trajectory under the same `SEED = 7`.
  - `SEED` is a compile-time constant with no flag, so a true second seed would need a product change and was not run.
  - Node order is the seed proxy.
- **random pairs**: SCIP's own counts of removed and added static pairs, at random positions (`random.Random(SEED + k)`), 8 draws in each of two ways. Removals are uniform over hand's static pairs. Additions are uniform over all file pairs (`uniform`), or taken from pairs the graph already links by co-change, proximity or semantics (`candidate`, which is local the way real references are).
  - Added pairs carry SCIP's added weights, shuffled.
  - A random pair that no signal links gets semantic 0, which makes it lighter than a real one.
  - This is the noise baseline: a change of SCIP's size and mass that carries no information.
- **pairs only**: SCIP's pair set, with hand's weights on the pairs both graphs have.
- **weights only**: hand's pair set, with SCIP's weights on the shared pairs.
  - SCIP weights are brought to hand units by one factor, the ratio of the two graphs' static mass on the shared pairs. The blend normalises each signal on its mass (finding 1), so the factor matters only where a variant mixes the two.

**The construction is checked, not trusted.** On all nine fixtures, four rebuilds each place 100.0% of files with Δq 0.0000:
- the repository hand map against the hand graph's map;
- the hand graph reassembled by the variant code;
- the repository SCIP map against the SCIP graph's map;
- the reassembled SCIP graph.

Three runs produced these numbers: [CI run 36150835159](https://github.com/onsager-ai/tolmap/actions/runs/36150835159), [36153172950](https://github.com/onsager-ai/tolmap/actions/runs/36153172950) and [36156781762](https://github.com/onsager-ai/tolmap/actions/runs/36156781762), job `scip-churn`, standard runners, pinned indexers. The score tables of all three runs are byte-identical. The same dispatches' `scip-fixtures` gate passed on all nine, so these SCIP maps are the ones finding 45 recorded. The runs used #127's branch while it still defaulted to `scip`; every build named `--refs` explicitly and every dump used `--refs`, so no number depends on the default.

### Is it noise?

Placement against the hand map, as a percentage.
- "random" pools 16 draws (8 uniform, 8 candidate).
- "below SCIP" counts the random draws that placed fewer files than SCIP did.

| fixture | SCIP | node order, 8 draws | random pairs, 16 draws | below SCIP | pairs only | weights only |
|---|---:|---|---|---:|---:|---:|
| celery | 62.7 | 71.4–100.0 | 53.4–88.2 | 3/16 | 80.1 | 62.1 |
| django | 85.8 | 79.2–93.3 | 37.5–77.4 | 16/16 | 64.9 | 82.6 |
| flask | 70.8 | 100.0 (all 8) | 45.8–95.8 | 7/16 | 83.3 | 41.7 |
| httpx | 47.8 | 82.6–100.0 | 43.5–95.7 | 2/16 | 95.7 | 47.8 |
| prometheus | 72.3 | 69.1–94.6 | 34.7–66.7 | 16/16 | 66.7 | 74.8 |
| rich | 48.0 | 24.0–80.0 | 24.0–98.0 | 7/16 | 72.0 | 48.0 |
| scrapy | 77.7 | 72.9–97.3 | 60.1–94.1 | 6/16 | 79.3 | 83.5 |
| sqlalchemy (fallback) | 100.0 | 95.0–100.0 | — (no change) | — | 100.0 | 100.0 |
| vue | 74.5 | 95.8–100.0 | 87.4–95.8 | 0/16 | 77.4 | 95.0 |

**The partitioner disagrees with itself about as much as SCIP disagrees with it, on half the fixtures.** On the identical graph, shuffling node order alone moves rich to as low as 24.0%. It also reaches 69.1% on prometheus, 71.4% on celery and 79.2% on django. On four of the eight admitted fixtures, SCIP's placement sits inside that band: django, prometheus, rich and scrapy.

On rich, every one of the eight shuffled partitions has a higher q (0.3537–0.3691) than the hand map's own 0.3437. The fixture map is one draw among many near-equal optima. A ≥ 95% placement gate cannot tell a real change from a reshuffle on graphs like that. The hand gate passes because the port follows the reference's exact Leiden trajectory (finding 11). That is agreement on one draw, not stability.

**Measured against a random change of the same size, SCIP is no worse than noise on seven of eight.**
- On django and prometheus, SCIP moves fewer files than every one of the 16 random draws. Its 861 and 453 added pairs agree with the structure the other signals already carry.
- On celery, flask, httpx, rich and scrapy, it is an ordinary draw: 2–7 of 16 draws land lower.
- On vue alone, SCIP moves more than every random draw: 74.5% against 87.4–95.8%.

vue's change is structural. SCIP adds 673 static pairs carrying 30.7% of the static mass, and pairs-only reproduces most of the move (77.4%).

**On the small Python fixtures, the weights drive the move, not the pairs.** Weights-only (hand's pairs, SCIP's weights) reproduces or exceeds SCIP's drop:
- httpx 47.8%, identical to SCIP, though with 3 districts where SCIP has 2;
- rich 48.0%, identical;
- celery 62.1% against SCIP's 62.7%;
- flask 41.7%, below SCIP's 70.8%.

Pairs-only keeps 72.0–95.7% on the same four.

SCIP weighs a pair by the distinct symbols it references. Hand weighs it by the import statements that resolve to it. SCIP's weighting is much more heavy-tailed:

| fixture | top-decile share of static mass, hand → SCIP | largest ÷ median pair weight, hand → SCIP |
|---|---|---|
| celery | 0.17 → 0.33 | 6.0 → 12.5 |
| django | 0.16 → 0.36 | 7.0 → 30.0 |
| flask | 0.30 → 0.38 | 10.0 → 18.0 |
| httpx | 0.16 → 0.29 | 3.0 → 9.5 |
| prometheus | 0.44 → 0.50 | 11.0 → 105.3 |
| rich | 0.21 → 0.39 | 5.0 → 14.0 |
| scrapy | 0.13 → 0.29 | 4.0 → 17.5 |
| vue | 0.19 → 0.49 | 7.0 → 70.9 |

On weakly modular graphs, a few heavy hub pairs decide which districts merge. That is P0's primary weighting (finding 41), a modelling choice, and this finding does not show it is better or worse. On django and prometheus the pair set moves more than the weights do: pairs-only gives 64.9% and 66.7%, weights-only 82.6% and 74.8%. Pairs-only mixes the two scales on those graphs, so read it as indicative.

### What SCIP changes, pair by pair

Undirected file pairs of the emitted import list (`E`):

| fixture | hand | SCIP | shared | hand only | SCIP only |
|---|---:|---:|---:|---:|---:|
| celery | 659 | 700 | 639 | 20 | 61 |
| django | 3,091 | 3,933 | 3,072 | 19 | 861 |
| flask | 83 | 91 | 78 | 5 | 13 |
| httpx | 82 | 69 | 66 | 16 | 3 |
| prometheus | 5,574 | 5,313 | 4,842 | 732 | 471 |
| rich | 393 | 391 | 388 | 5 | 3 |
| scrapy | 883 | 1,190 | 883 | 0 | 307 |
| vue | 1,086 | 1,756 | 1,083 | 3 | 673 |

**SCIP-only pairs are references with no import between the two files.** On every Python fixture, the file on one end never imports the other (`ast` over the source): 13 of 13 on flask, 61 of 61 on celery, all 863 directed pairs on django, 308 on scrapy. They come from inferred types, inherited members, and names reached through a re-export.

One flask pair was checked by hand. `wrappers.py` reads `current_app.config` and `current_app.debug`, and both are defined on `App` in `sansio/app.py`, which `wrappers.py` never imports. The hand graph cannot see these pairs, and they are exact references, not guesses. Hand's pair count is the lower bound the product rule asks for, and SCIP's extras do not break that rule.

**Hand-only pairs, classified.** `eval/scip_churn.py pairs` classifies all 65 directed hand-only pairs on the five Python fixtures that have any: flask 5, httpx 16, celery 20, django 19, rich 5. The categories are heuristic, from each source file's import statements:
- **25 are star imports**, all 16 of httpx's and 9 of django's. `from ._api import *` in an `__init__.py` makes no symbol occurrence, so SCIP cannot see the dependency. These are real imports that SCIP misses.
- **21 are a submodule imported through its package**: flask 4, celery 11, rich 5, django 1. An example is `from .. import typing as ft` in `flask/views.py`. The hand resolver also links the package's `__init__.py`, whose own content is never used. These hand pairs are over-attributions.
- **12 are a name the package re-exports**: flask 1, celery 4, django 7. An example is `from . import Flask` in `flask/cli.py`. Six of django's seven are GEOS modules that do `from django.contrib.gis.geos import prototypes as capi` and call `capi.create_point`, a name `prototypes/__init__.py` re-exports from `prototypes/coordseq.py` and its siblings. The classifier tags those six only as imports of a package `__init__.py`; they were checked in the source. Hand credits the re-exporting `__init__.py`, and SCIP credits the file that defines the name, which then shows up as a SCIP-only pair. Both see the dependency, and SCIP places it more exactly.
- **7 are other cases**: function-level imports, and one django pair with no import of its target, `staticfiles/testing.py → django/__init__.py`, which is a hand resolver error. These were not checked one by one.

So hand's extras are not mostly wrong. About a third over-attribute to a package, and about a third are imports SCIP cannot see. Go is different by construction. prometheus's 732 hand-only pairs are `resolve_multi` spreading each import over its whole package directory (finding 44). At file granularity they are an upper envelope, not a lower bound. SCIP's Go pairs are file-exact.

### httpx: why four districts become two

Two things happen at once, and the decomposition says the second decides:
- **`_transports` loses its internal glue.** Its five internal pairs are the star imports in `_transports/__init__.py`, which SCIP drops. The district's share of blended, pruned weight inside itself falls from 10.2% to 5.8%.
- **Symbol counts pull the core modules together.**
  - `_client–_models` goes from 1.05% to 4.67% of all weight.
  - `_client–_transports/default`, `_config–_transports/default` and `_exceptions–_transports/default` roughly double.
  - The weight joining `_transports` to hand's "api & client" district rises from 6.0% to 9.1% of all weight, and the weight joining "auth & init__" to "api & client" from 12.8% to 13.7%.

Leiden then merges "auth & init__", "urls & types", "api & client" and one transport file into one 16-file district.

The weight inside hand's four districts is only 38.8% of the hand graph (34.0% under SCIP). Most of httpx's weight crosses district lines, which is why q is 0.03 to begin with. Pairs-only (the star imports dropped, weights kept) places 95.7%. Weights-only places 47.8%.

On the SCIP graph, SCIP's own two-district partition scores a lower Newman modularity (0.0515, at resolution 1, on the blended and pruned graph) than the hand partition scored on the same SCIP graph (0.0793). Leiden, at resolution 1.1 and followed by `merge_tiny`, does not find the better split. httpx is the only fixture where this is inverted. The two-district map is a weak optimum on a nearly structureless graph, not a finding about httpx's architecture.

### sqlalchemy: 0.66 is re-exports, not configuration

- **The gate compares by file.** scip-python keeps 1,825 of the 2,762 directed hand pairs (0.6608, which matches the product's gate exactly).
- **75% of the misses point into packages.** 701 of the 937 misses (74.8%) target a package's `__init__.py`:
  - 427 are a submodule imported through its package (`from .. import util`, then `util.x`);
  - 218 are a name the package re-exports.
  - Of the rest, the 60-pair sample the job keeps shows two patterns. Some go to facade modules such as `types.py`, `schema.py` and `sql/expression.py`. Others are `from . import <dialect>` registration imports in an `__init__.py`. The remaining 236 were not classified one by one.
  - In each case, SCIP credits the file that defines the symbol, not the file that re-exports it.
- **Counting re-exports lifts recall above the floor.** When a hand pair into a package's `__init__.py` counts as kept because SCIP links the same source to a file inside that package, recall is 0.9008, or 0.9182 with `lib/` on the search path (run 36156781762, `missed-*.json`). That clears the 0.80 floor.
- **Configuration does not fix it.**
  - Putting `lib/` on the search path, by `PYTHONPATH` or by a `pyrightconfig.json` with `extraPaths`, resolves 70 more absolute imports. Recall goes to 0.6872, and both routes give the same pair counts and the same missed pairs.
  - sqlalchemy's own `[tool.pyright]` sets no paths.
- **Cython is not the cause.** Only two `.pxd` files exist, outside the mapped set. The `_cy.py` modules are ordinary Python, and all 258 mapped files were indexed.
- **`TYPE_CHECKING` imports are not the cause.** 704 are kept and 108 are missed.

The gate measures agreement with hand's attribution, and sqlalchemy imports almost everything through re-exporting packages.

### What this says for the default

Most of the churn is not evidence against SCIP's references. Four of eight admitted fixtures sit inside the partitioner's own run-to-run band. Seven of eight move no more than a random change of the same size. SCIP's pair set is the more exact one: its extras are real references, and its misses are mostly star imports and re-export attribution.

The real change is the weighting. Distinct-symbol weights concentrate the static signal on hub pairs and decide the small fixtures' districts. vue is the one fixture where SCIP's pairs change the structure beyond noise.

Confidence:
- **High** that the decomposition is right. It is deterministic, construction-checked on all nine fixtures and reproduced in three runs.
- **Moderate** on the noise bands. Eight draws per control give a rough range, not a distribution. The random-pairs baseline depends on how "random" is drawn, which is why both ways are shown.

### Not settled here

- Whether a flatter SCIP weighting (log or capped symbol counts) keeps SCIP's pairs without the weight-driven moves. Pairs-only approximates it, 64.9–95.7%. It was not measured as a product option.
- Whether the recall gate should count package re-exports as kept, which would admit sqlalchemy at 0.9008. That is a gate change, left to the owner.
- A true second Leiden seed. `SEED` has no flag, and node order stands in for it.
- The TypeScript and Go hand-only pairs were counted but not classified: vue 3, prometheus 732.

## 48. Scored against SCIP, the hand resolver's precision is 0.66–1.00 and its over-attribution to packages is 335 submodule pairs and 413 re-export pairs, 701 of them sqlalchemy's

Owner decision, session `16030105`, AskUserQuestion, transcript line 6937 (2026-09-25T16:56:34Z): **"Tune hand, SCIP as oracle (Recommended)"**, with the owner's note *"Hand written is in more control with less deps and more efficient"*. `hand` stays the default, and SCIP measures it. This is issue [#110](https://github.com/onsager-ai/tolmap/issues/110)'s scoring job, [PR #131](https://github.com/onsager-ai/tolmap/pull/131). It is evidence only: no product code changes.

**What ships.** ci.yml's `hand-score` job runs nightly and on dispatch, on a standard runner with the pinned indexers (scip-python 0.6.6, scip-go v0.2.7, scip-typescript 0.4.0). For each fixture pin it dumps the hand graph (`dump-graph --refs hand`) and runs the indexer through `dump-graph --refs scip`, keeping the index. `eval/hand_score.py` then compares two sets of directed file pairs over the same mapped files:
- **hand**: the dumped graph's `imports`;
- **SCIP**: P0's oracle ingest (`eval/scip_ingest.py`) of the kept index.

The oracle is used rather than the SCIP graph because sqlalchemy's index falls below the 0.80 admission floor, so its SCIP graph is hand's. Where a language is admitted, the job checks that the SCIP graph's pairs equal the oracle's. They did, on all eight admitted fixtures.

Two ratios, named for this job:
- **recall**: the share of SCIP pairs hand also has;
- **precision**: the share of hand pairs SCIP also has.

Precision is what the product gate calls recall. On every file-granularity row it equals the gate's number in the map's `coverage.references`, and prometheus's by-directory precision equals its gate's 0.9969, so the two implementations agree.

Pairs that only one side has are classified by heuristics that read the source (`eval/hand_score.py`'s docstring has the rules), and the JSON keeps a fixed-seed sample of each class for checking by hand. The counts here are directed pairs, compared direction by direction. Finding 47 counted a hand pair as missing only when SCIP had neither direction, so these counts are larger: flask has 9 hand-only pairs here against finding 47's 5.

[CI run 36176225788](https://github.com/onsager-ai/tolmap/actions/runs/36176225788), job `hand-score`, at `f4667d8`:

| fixture | lang | hand pairs | SCIP pairs | shared | recall | precision | precision, re-exports counted |
|---|---|---:|---:|---:|---:|---:|---:|
| celery | py | 669 | 710 | 648 | 0.9127 | 0.9686 | 0.9895 |
| django | py | 3,173 | 4,018 | 3,141 | 0.7817 | 0.9899 | 0.9953 |
| flask | py | 102 | 113 | 93 | 0.8230 | 0.9118 | 0.9902 |
| httpx | py | 87 | 74 | 71 | 0.9595 | 0.8161 | 0.8161 |
| prometheus | go | 5,574 | 5,436 | 4,842 | 0.8907 | 0.8687 | 0.8687 |
| prometheus, by directory | go | 956 | 1,206 | 953 | 0.7902 | 0.9969 | |
| rich | py | 426 | 424 | 420 | 0.9906 | 0.9859 | 1.0000 |
| scrapy | py | 902 | 1,222 | 902 | 0.7381 | 1.0000 | 1.0000 |
| sqlalchemy | py | 2,762 | 2,802 | 1,825 | 0.6513 | 0.6608 | 0.9008 |
| vue | ts | 1,186 | 1,911 | 1,183 | 0.6190 | 0.9975 | 0.9975 |

The last column counts a hand pair into a package's `__init__.py` as confirmed when SCIP links the same source to any file under that package. That is finding 47's rule, and it reproduces its 0.9008 for sqlalchemy.

**Hand-only pairs, by class:**

| fixture | hand-only | star import | submodule via package | re-export | package spread | other |
|---|---:|---:|---:|---:|---:|---:|
| celery | 21 | 0 | 12 | 4 | 0 | 5 |
| django | 32 | 9 | 2 | 14 | 0 | 7 |
| flask | 9 | 0 | 7 | 2 | 0 | 0 |
| httpx | 16 | 16 | 0 | 0 | 0 | 0 |
| prometheus | 732 | 0 | 0 | 0 | 710 | 22 |
| rich | 6 | 0 | 6 | 0 | 0 | 0 |
| scrapy | 0 | 0 | 0 | 0 | 0 | 0 |
| sqlalchemy | 937 | 13 | 308 | 393 | 0 | 223 |
| vue | 3 | 0 | 0 | 0 | 0 | 3 |
| all | 1,756 | 38 | 335 | 413 | 710 | 260 |

**SCIP-only pairs, by class:**

| fixture | SCIP-only | re-export | inherited member | inferred type | other |
|---|---:|---:|---:|---:|---:|
| celery | 62 | 33 | 1 | 11 | 17 |
| django | 877 | 745 | 55 | 31 | 46 |
| flask | 20 | 2 | 16 | 1 | 1 |
| httpx | 3 | 0 | 0 | 3 | 0 |
| prometheus | 594 | 0 | 0 | 22 | 572 |
| rich | 4 | 0 | 0 | 4 | 0 |
| scrapy | 320 | 243 | 2 | 70 | 5 |
| sqlalchemy | 977 | 700 | 55 | 105 | 117 |
| vue | 728 | 447 | 0 | 80 | 201 |
| all | 3,585 | 2,170 | 129 | 327 | 959 |

**What the numbers say:**
- **Hand's precision is high except where packages re-export.** Six fixtures are at 0.91 or above. httpx's 0.8161 is entirely star imports (16 of 16 hand-only pairs), where hand is right and SCIP cannot see the dependency. sqlalchemy's 0.6608 is mostly packages: 308 submodule pairs and 393 re-export pairs out of 937 hand-only pairs.
- **The two over-attribution classes are the fixable part.** "Submodule via package" is a pair to a package `__init__.py` that the import names only as the head of `from pkg import sub`: 335 pairs on five fixtures. "Re-export" is a pair to a package `__init__.py` for a name it re-exports from another file: 413 pairs on four fixtures. Each claims a dependency on a file whose own content is not what is used, against CLAUDE.md's rule that numbers must be a lower bound.
- **SCIP's extras are mostly the other side of the same coin.** 2,170 of the 3,585 SCIP-only pairs are re-exports: hand links the package that re-exports a name, and SCIP links the file that defines it. Next come inferred types (327) and inherited members (129), which no import-level resolver sees.
- **Go is a different problem.** 710 of prometheus's 732 hand-only pairs are `resolve_multi` spreading an import over every file of the imported package (finding 44). By directory, hand's precision is 0.9969. That is a separate follow-up and is not gated here.
- **Some over-attribution is classed "other".** sqlalchemy's 223 "other" hand-only pairs include facade modules that are not packages (`schema.py`, `types.py`, finding 47's sample). The classifier only calls a pair to an `__init__.py` a re-export, so these counts are a lower bound on over-attribution.

### Pairs SCIP confirms only through a namespace

SCIP also has a pair for an import statement that names a module. `from celery.utils import functional` is itself an occurrence of the module symbol `celery.utils`, and that symbol is defined in `celery/utils/__init__.py`. The ingest flags a pair that only namespace or module symbols support (its `uses` column is 0). Such a pair is the import naming the package, not a use of anything the package's `__init__` defines.

This matters for scoring package over-attribution: a hand pair to an `__init__` that SCIP has only this way counts as "confirmed". The job therefore also scores against SCIP's use pairs.

The same run's numbers ([CI run 36180594364](https://github.com/onsager-ai/tolmap/actions/runs/36180594364), at `2038510`; every count and fingerprint above is unchanged):

| fixture | lang | SCIP use pairs | shared by a use | recall (uses) | precision (uses) | hand pairs SCIP has only through a namespace (to a package) |
|---|---|---:|---:|---:|---:|---:|
| celery | py | 608 | 546 | 0.8980 | 0.8161 | 102 (89) |
| django | py | 3,261 | 2,388 | 0.7323 | 0.7526 | 753 (717) |
| flask | py | 113 | 93 | 0.8230 | 0.9118 | 0 (0) |
| httpx | py | 73 | 70 | 0.9589 | 0.8046 | 1 (1) |
| prometheus | go | 1,873 | 1,279 | 0.6829 | 0.2295 | 3,563 (4) |
| rich | py | 423 | 419 | 0.9905 | 0.9836 | 1 (1) |
| scrapy | py | 1,066 | 746 | 0.6998 | 0.8271 | 156 (155) |
| sqlalchemy | py | 2,802 | 1,825 | 0.6513 | 0.6608 | 0 (0) |
| vue | ts | 1,586 | 928 | 0.5851 | 0.7825 | 255 (233) |

- **The hand-only classes undercount package over-attribution.** 1,196 of the 1,268 namespace-only Python and TypeScript pairs target a package file.
- **prometheus's are Go package clauses (finding 44).** Every file's `package x` references the package symbol.
- **sqlalchemy's index has no namespace-only pairs at all.** Why was not investigated. Its relative `from .. import util` form may make no module occurrence that resolves in-repo.

**Construction checks.** On every admitted fixture, the SCIP graph's pairs equal the oracle's. Precision equals the product gate's recall. sqlalchemy's re-export-counted precision reproduces finding 47's. And every hand map places 100.0% of files, with Δq 0.0000, against its committed fixture, so the job scores the maps the oracle recorded. The classifier has a synthetic self-test (`eval/hand_score.py self-test`, in the push-time `gate` job) with one pair of each Python class and the Go spread.

### The baseline and its gate

`data/scip/hand_score.json` is this run's `baseline` output. It holds the counts and ratios above and a SHA-256 of each fixture's sorted SCIP pair set. The job's last step fails on a regression against it, and only on one:
- **The oracle moved.** A different SCIP fingerprint means an indexer, a pin or the mapped file set changed. Every other number is then not comparable, so re-baseline with a finding.
- **Over-attribution rose.** The "submodule via package" and "re-export" hand-only counts may only go down.
- **Pairs confirmed by a use fell.** `shared_uses` may not decrease: hand losing a pair SCIP confirms by a use shrinks the lower bound.

The first version gated all of `shared`. The package fix's first run ([CI run 36179004373](https://github.com/onsager-ai/tolmap/actions/runs/36179004373)) tripped that gate on celery (648 → 586), django (3,141 → 3,059) and rich (420 → 419). On celery, all 85 SCIP-confirmed pairs the fix removed were namespace-only, each supported by one module symbol. The gate as first written would have blocked the very removal its over-attribution rule asks for, so it now holds confirmation by a use. `shared` is still reported. This change was made after seeing that run, and it is recorded here for that reason.

It does not gate star imports (hand is right), "other" (heuristic and mixed), the SCIP-only classes (they move when hand gains a correct pair) or the ratios, which follow from the gated counts. An improvement never fails. The gate prints it, so the baseline is lowered in the same change, with a finding.
