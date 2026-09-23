> **Historical record — removed 2026-09-23 by owner decision.** After comparing phone screenshots of dify and vscode with terrain off and on at 2× and 4×, the owner chose to remove terrain entirely. The four reasons were: **districts look emptier** (large districts become sparse clumps with blank space); **the square parcel grid looks artificial**; **harder to read** (sub-district clusters and outlines add clutter without telling you anything); and **not useful enough** (it does not earn its complexity). See [finding 24](FINDINGS.md#24-terrain-was-built-measured-and-removed). The measurements below remain as historical analysis.

# Terrain-aware subdivision of oversized districts

This began as the measured Stage 3 proposal for issue #41. Terrain shipped behind `--terrain` in #47; finding 22 records the later policy that makes it automatic above 2,000 mapped source files, while keeping smaller default maps byte-identical.

Every number is from `eval/terrain_spike.py` against maps built at `586d5be` (post-#44), on the four reference repositories at their pinned commits (crawlab `ee11cd7`, codex `5c5308f`, dify `e2bdeec`, n8n `0b2ff22`) and the acceptance fixtures in `data/`. "How to reproduce" at the end.

## What stage 2 changed about the problem

Fixing module resolution (#44, finding 15) took n8n from 369 districts to 85 and its mainland share from 62% to 86%. It also concentrated the repository: **n8n's largest district now holds 3,683 files, 30.7% of the repository** — `packages/nodes-base/nodes/` (298 vendor directories, 2,593 files) plus `@n8n/nodes-langchain` (474), one community because every integration imports `packages/workflow/src/index.ts`. The task brief measured this district at 3,039 files / 25.4% on its own tree; the shape is the same and the number is larger here.

So the question is no longer "what do we do with 365 districts". It is **what to draw for one district that holds a third of a repository and whose members legitimately belong together.**

## Summary of the proposal

1. **Size band, from the corpus.** A district is oversized when it exceeds `2T`, where `T(N) = √N / 0.517` and 0.517 is the median `districts/√files` of the acceptance fixtures at or above finding 5's 50-file floor. The brief proposed codex as the reference scale; **the corpus falsifies that choice** (§1).
2. **Decompose, don't label.** An oversized district is decomposed into three kinds of terrain, each measured, none guessed: **arterials** (files whose removal strands a sub-district's worth of files), **parcels** (connected pieces too small to be sub-districts once arterials leave) and **organic components** (everything else, split by recursive Leiden when still above the band).
3. **Render each kind as what it is.** Arterials are roads. Organic sub-districts are sub-contours inside the district, named after the parent with a stable suffix. Parcels are a grid ordered by address — the directory path — which asserts nothing about how they relate, because the graph says nothing.
4. **Sub-districts are a second level.** The top-level partition, district count and district names do not change. Subdivision is what you see inside a district, not a replacement for it.
5. **The only constants are the census tract ratios** (split above 2×, merge below 0.3×), borrowed and not tuned, plus finding 5's existing 50-file floor. The classification is insensitive to the merge ratio across 0.2–0.5 (§4).

Decisions for the reviewer are collected at the end.

## 1. How many districts should a map show

Töpfer and Pillewizer's Radical Law, `n_f = n_a·√(M_a/M_f)`, reduces to `districts = c·√files` for a fixed reference. The brief set `c` from codex (887 files, 32 districts, "map reads well").

**codex is not a usable reference.** Its constant is c = 1.041, about twice every acceptance fixture above the 50-file floor (0.400–0.583, median 0.517). codex is itself fragmented: 12 of its 31 districts are below 1% of files and 14 have no internal import edge. Taken as the reference, it declares the fixtures badly under-districted — the test the reference exists to pass:

| reference | django districts > 2T | files in them | prometheus | sqlalchemy | vue | scrapy | celery | rich |
|---|---|---|---|---|---|---|---|---|
| codex, c = 1.041 | 7 of 12 | 736 (86%) | 4 of 11 (78%) | 4 of 8 (69%) | 4 of 8 (70%) | 3 of 8 (49%) | 3 of 7 (63%) | 3 of 4 (88%) |
| fixture median, c = 0.517 | 2 of 12 | 351 (41%) | 2 of 11 (53%) | 0 | 0 | 0 | 0 | 1 of 4 (41%) |

Under the fixture median the band leaves four of the seven fixtures untouched and flags the two largest districts in django and prometheus — each of which turns out, below, to be a district worth subdividing (django's is mostly glued-on locale files; prometheus's is service discovery). The over-fragmentation index (actual / predicted districts) becomes:

| map | files | districts | predicted (c = 0.517) | index | brief's index (codex reference) |
|---|---|---|---|---|---|
| crawlab | 575 | 11 | 12 | 0.89 | 3.84 |
| codex | 887 | 31 | 15 | **2.01** | 1.00 |
| dify | 6,335 | 136 | 41 | **3.30** | 2.81 |
| n8n | 11,982 | 85 | 57 | **1.50** | 0.87 |

The brief's reading that post-fix n8n "lands on the law" does not survive the change of reference: n8n is 1.5× over, dify 3.3× (issue #40's Python zero-edge tail, as the brief expected), and codex 2× — codex's own fragmentation is what made it look like a baseline.

As the brief noted, the law says how many, not which. It sets the scale `T`; the selection of what becomes a sub-district is the terrain decomposition in §2.

## 2. The mechanism, implementation-ready

All of it runs on **the weighted, pruned graph the partitioner receives** (`tolmap dump-blend`), restricted to one district. That is not the map's `E` (the directed import list that `eval/mapstats.py` and the brief call "kept edges"); subdivision re-clusters the partition graph, so terrain is measured there.

For a repository of `N` files: `T = √N / 0.517`, `hi = 2T`, `lo = 0.3T`.

**Eligibility.** A district is subdivided when its size exceeds `hi` **and** it holds at least 50 files. The second condition is finding 5's floor, below which a repository does not cluster; it keeps rich's 41-file district (41% of a 100-file repository, 2 files over its `hi` of 39) from being cut into 7–15-file pieces.

**Step 1 — arterials.** An arterial is a file whose removal strands at least `lo` files — a whole sub-district's worth — out of the district's largest connected component. Stranding is measured against the district's own state before the removal, so a district that is already disconnected does not make every file look load-bearing. Remove the file that strands the most, recompute, repeat until no file strands `lo`.

- Only articulation points can strand anything, and every articulation point's stranded count is exact from one block-cut tree, `O(V+E)`. (`eval/terrain_spike.py` caps the search at the 64 highest-degree articulation points for speed; the implementation must not.)
- Ties break on file path. Arterials leave the **clustering** graph, not the map: they are drawn as roads (`roads` already exists in the map JSON), and their edges still count for landmarks and blast radius.
- Why not a degree break, as the brief proposed. The ordered degrees do break sharply where arterials are real — n8n d0's 3,358 → 347 (9.7×), codex d0's 224 → 21 (10.7×) — but in the middle they are ambiguous (dify's heads run 1.9×, 2.2×, 2.3×, 3.3×, 3.7× with no arterial behind any of them), and a ratio threshold is a tuned constant by another name. Degree share fails too: crawlab's densest districts have their top file adjacent to 65% and 54% of members with no hub at all — a dense core, not a highway. Stranding measures the property the metaphor names: *this file is what holds these files together.* It reuses `lo`, so it adds no constant.

**Step 2 — parcels and organic components.** With arterials removed, take the district's connected components. Components smaller than `lo` are **parcels**; the rest are **organic**.

**Step 3 — organic sub-districts.** An organic component within `hi` is one sub-district. Above `hi`, run Leiden on it with the map's own settings (resolution 1.1, seed 7, the same pinned `Optimiser` fields); fold any resulting community smaller than `lo` into the neighbouring community it shares most weight with — one always exists, because the component is connected — **refusing any fold that would push the target above `hi` while another neighbour can take it**; recurse on anything still above `hi`.

The cap is not decoration. Without it the folding snowballs: on n8n's `cli` district Leiden splits a 614-file component into 15 communities (121, 63, 61, 58, …), 13 of them below the 63-file floor, and folding each into its strongest neighbour rebuilt the original 614 — undoing the split the band had asked for. With the cap the same component yields 398 and 239 and every sub-district in the corpus lands in band. Leiden runs to convergence (`n_iterations=-1`), as `src/tolmap/pipeline.py` and `native/leiden_bridge.cpp` both do; leidenalg's default of two passes is a different search, and on this corpus it moved the sub-districts of 9 of the 22 districts.

**Step 4 — the size band, with hysteresis.** Split above `2T`, merge below `0.3T`: the US Census tract rule (target 4,000, split above 8,000, merge below 1,200), which exists so that boundaries do not oscillate between counts. `T` comes from the corpus (§1); the two ratios are borrowed, not tuned, and are the proposal's only new constants. §4 measures how much they matter.

**Terrain is the decomposition, not a label.** A district is *organic* when it has no parcels, *plat* when it has no organic component, *mixed* otherwise — each part is rendered as what it is, so no threshold decides the class.

**Determinism.** Every iteration is over sorted file indices or paths; Leiden is seeded; suffixes (§6) are assigned by a total order. No set is iterated upstream of a seeded stage (finding 9). BTreeMap/BTreeSet in the Rust, as everywhere else.

## 3. Applied to every oversized district in the corpus

22 districts clear both eligibility conditions across the four reference repositories and three fixtures. Arterials are shown with the number of files each strands.

| map | district | files | share | terrain | arterials (files stranded) | parcels (files) | organic sub-districts | sizes | in band | contiguous | median dir coherence |
|---|---|---|---|---|---|---|---|---|---|---|---|
| django | d0 conf & locale | 202 | 23.7% | mixed | none | 171 (171) | 1 | 31 | 1/1 | 1/1 | 0.19 |
| django | d1 template & utils | 149 | 17.5% | mixed | none | 10 (10) | 5 | 45, 32, 25, 20, 17 | 5/5 | 5/5 | 0.30 |
| prometheus | d0 service discovery | 131 | 29.5% | organic | none | 0 | 4 | 49, 36, 24, 22 | 4/4 | 4/4 | 0.28 |
| prometheus | d1 tsdb storage | 103 | 23.2% | organic | none | 0 | 4 | 44, 25, 20, 14 | 4/4 | 4/4 | 0.43 |
| crawlab | d0 entity & controllers | 106 | 18.4% | organic | none | 0 | 3 | 57, 35, 14 | 3/3 | 3/3 | 0.23 |
| crawlab | d1 frontend & crawlab-ui | 104 | 18.1% | mixed | none | 2 (2) | 4 | 31, 27, 23, 21 | 4/4 | 4/4 | 0.61 |
| codex | d0 windows-sandbox-rs | 227 | 25.6% | mixed | `schema/typescript/v2/index.ts` (190) | 81 (136) | 3 | 34, 31, 25 | 3/3 | 3/3 | 1.00 |
| dify | d0 workflow & nodes | 840 | 13.3% | mixed | none | 2 (2) | 9 | 202, 116, 105, 88, 80, 69, 64 … | 9/9 | 9/9 | 0.12 |
| dify | d1 plugins & components | 551 | 8.7% | mixed | none | 3 (4) | 5 | 230, 94, 93, 73, 57 | 5/5 | 5/5 | 0.12 |
| dify | d2 features & (commonLayout) | 502 | 7.9% | mixed | none | 3 (3) | 5 | 229, 82, 81, 57, 50 | 5/5 | 5/5 | 0.14 |
| dify | d3 datasets | 479 | 7.6% | mixed | none | 3 (3) | 5 | 212, 101, 65, 50, 48 | 5/5 | 5/5 | 0.10 |
| dify | d4 base & chat | 432 | 6.8% | organic | none | 0 | 4 | 166, 100, 88, 78 | 4/4 | 4/4 | 0.10 |
| dify | d5 api | 379 | 6.0% | mixed | none | 172 (177) | 1 | 202 | 1/1 | 1/1 | 0.15 |
| dify | d6 api & controllers | 316 | 5.0% | mixed | none | 71 (76) | 1 | 240 | 1/1 | 1/1 | 0.12 |
| dify | d7 api & services | 308 | 4.9% | mixed | none | 149 (167) | 1 | 141 | 1/1 | 1/1 | 0.22 |
| n8n | d0 nodes-base & nodes | 3,683 | 30.7% | mixed | `workflow/src/index.ts` (1,674) and 7 more (§6) | 834 (2,430) | 7 | 369, 345, 178, 110, 92, 77, 74 | 7/7 | 7/7 | 0.09 |
| n8n | d1 cli | 1,261 | 10.5% | mixed | none | 6 (8) | 7 | 398, 239, 164, 140, 112, 105, 95 | 7/7 | 7/7 | 0.11 |
| n8n | d2 frontend & editor-ui | 1,074 | 9.0% | mixed | none | 41 (42) | 10 | 125, 123, 122, 118, 116, 98, 96 … | 10/10 | 10/10 | 0.13 |
| n8n | d3 typeorm | 636 | 5.3% | mixed | `typeorm/src/index.ts` (196) | 19 (39) | 3 | 308, 162, 126 | 3/3 | 3/3 | 0.22 |
| n8n | d4 instance-ai | 604 | 5.0% | mixed | none | 8 (8) | 6 | 172, 105, 98, 86, 70, 65 | 6/6 | 6/6 | 0.19 |
| n8n | d5 agents & cli | 529 | 4.4% | mixed | none | 2 (3) | 4 | 175, 157, 125, 69 | 4/4 | 4/4 | 0.16 |
| n8n | d6 api-types & dto | 450 | 3.8% | mixed | `@n8n/cli/src/index.ts` (78) | 2 (2) | 2 | 370, 77 | 2/2 | 2/2 | 0.13 |

Across all 22: **94 organic sub-districts, 94 in band, 94 contiguous.** 11 arterials in 4 districts, **none in any fixture district and none in dify**. "Median dir coherence" is the share of a sub-district's files in its single most common directory; it is low almost everywhere because modern repositories nest deeply, and it is the least informative of the four redistricting criteria here — it is reported, not gated.

Three kinds of district come out of this, and they are the three the brief anticipated:

- **Organic** (prometheus, crawlab, dify's frontend, most of n8n): no arterial, few or no parcels; Leiden finds 2–10 sub-districts per district, all in band. prometheus d0 "service discovery" is instructive: `discovery/install/install.go` is adjacent to 58% of the district — a registry that imports every discovery plugin — yet removing it strands nothing, because the plugins share `discovery/` machinery with each other. A degree test would have called it an arterial; the stranding test correctly does not.
- **Arterial-bound plat** (n8n d0, codex d0): one or more hubs hold together files that otherwise have nothing in common.
- **Glued plat** (django d0, dify d5–d7): parcels with no arterial, because nothing held them together in the first place — see §5.

## 4. How much the two borrowed constants matter

The merge ratio sets both the arterial floor and the parcel threshold. Varying it:

| district | size | merge = 0.2 | 0.3 (proposed) | 0.5 |
|---|---|---|---|---|
| django d0 | 202 | 0 arterials / 85% parcels | 0 / 85% | 0 / 85% |
| django d1 | 149 | 0 / 7% | 0 / 7% | 0 / 7% |
| prometheus d0 | 131 | 0 / 0% | 0 / 0% | 0 / 0% |
| prometheus d1 | 103 | 0 / 0% | 0 / 0% | 0 / 0% |
| crawlab d0 | 106 | 0 / 0% | 0 / 0% | 0 / 0% |
| crawlab d1 | 104 | 0 / 2% | 0 / 2% | 0 / 2% |
| codex d0 | 227 | 1 / 60% | 1 / 60% | 1 / 71% |
| n8n d0 | 3,683 | 8 / 62% | 8 / 66% | 6 / 69% |
| n8n d3 | 636 | 1 / 6% | 1 / 6% | 1 / 6% |
| n8n d6 | 450 | 1 / 0% | 1 / 0% | 0 / 0% |
| dify d0 | 840 | 0 / 0% | 0 / 0% | 0 / 0% |
| dify d3 | 479 | 0 / 1% | 0 / 1% | 0 / 1% |

The classification is stable across a 2.5× range of the ratio; only n8n's marginal arterials move (d0 loses its two weakest at 0.5, d6 its only one). **No fixture district acquires an arterial at any setting.** The split ratio moves how many districts are eligible, as it should — at 1.5 / 2.0 / 3.0: django 3 / 2 / 1, prometheus 2 / 2 / 1, crawlab 3 / 2 / 0, codex 2 / 1 / 1, dify 10 / 8 / 4, n8n 10 / 7 / 4.

## 5. The plat is already in the corpus, and `merge_tiny` hides it

django's largest district, **"conf & locale", is 31 files from one real Leiden community plus 171 singletons that `merge_tiny` attached to it** — 170 of them `django/conf/locale/<lang>/{__init__,formats}.py`, two files for each of ~85 languages, and one `django/core/mail/backends/__init__.py`, each with no edge to anything else in the district. Raw Leiden on django's partition graph returns 215 singleton communities (modularity 0.5061, the map's own figure). `merge_tiny` (`src/pipeline.rs`, minimum 4) folds each small community into the large community it shares most pruned weight with — and when it has **no** edge to any large community, into **the district that dominates its directory**, then its parent directory's (`sibling_target`, `parent_target`; the reference's docstring says the same). The locale files have no edge into any large community, so they were placed by path. That is why this and several other districts are disconnected in the very graph the partitioner saw.

That is the brief's 308-integration case in miniature, in an acceptance fixture, under a name that hides it. The terrain decomposition finds it with no arterial at all: 171 parcels, 170 of them inside one locale directory (corrected from "each one a locale directory" by the implementation's measurement, PR #47). It will render as a grid of 85 locales in alphabetical order — which is how a person looks for `pt_BR`.

dify d5–d7 decompose the same way for a different reason. Their parcels are Python files under `api/` with no resolved edge, which is issue #40's coverage gap rather than independence by design. The grid is still the right rendering: it asserts no relationship, which is exactly what the graph supports. When #40 recovers those edges they will stop being parcels without any change here.

`merge_tiny` itself is out of scope for this proposal — it is a top-level step and this proposal does not touch the top level. But its directory fallback is the one place the shipped pipeline already does what issue #41 forbids ("merging two unrelated one-file districts … makes the map wrong in a way the reader cannot see") and what §9 rules out: **it uses path to decide top-level membership.** Recorded here so it is not rediscovered.

## 6. The 308-plugin case

n8n d0, 3,683 files. After the eight arterials leave — `workflow/src/index.ts` (strands 1,674), `nodes-base/utils/utilities.ts` (490), `@n8n/utils/src/sleep.ts` (453), `nodes-base/utils/descriptions.ts` (173), `nodes-base/utils/query-escaping.ts` (153), `@n8n/utils/src/format-pem-block.ts` (150), `@n8n/ai-utilities/src/index.ts` (117), `nodes/Google/GenericFunctions.ts` (83):

- **834 parcels holding 2,430 files (66%).** 579 are single files; the largest holds 51. By area: **390 are single credential files in one flat directory, `packages/nodes-base/credentials/`**; **277 lie inside a single `nodes-base/nodes/<Vendor>/` directory and hold 1,594 of the 2,430 parcel files**; 84 are pieces of `@n8n/nodes-langchain`; the remaining 83 are scattered, 7 of them spanning areas. A grid ordered by address still orders both big groups by vendor name — the credentials sort as `<Vendor>Api.credentials.ts` — but it is two grids' worth of plat, not one.

  **Corrected.** An earlier version of this section said 827 of 834 parcels (99.2%) lie inside a single vendor directory and that the grid is therefore a grid of integrations. That figure came from a bucketing that assigned every file outside `nodes-base/nodes/` to a coarse package bucket and counted the bucket as one vendor; the implementation (PR #47) measured the literal directory and got 277 of 834 (33.2%). The 390 credential parcels were the difference. Kept here because the error is the more useful half.
- **7 organic sub-districts, 1,245 files:** `@n8n/nodes-langchain` (369, all langchain), AWS (92, 90 of them `nodes/Aws`), Pipedrive (77), TheHiveProject (74), a Google + DataTable cluster (110), and two mixed-vendor clusters of 345 (Microsoft 118, Discord 38, Google 33, Postgres 25, …) and 178 (Onfleet, Webflow, Currents, Stripe, …).

Two things the brief predicted do not hold on this tree:

- **There is no ~1,120-file organic core.** Removing `workflow/src/index.ts` alone leaves a largest component of 1,989 files (54%), and that component is itself held together by shared utilities — each of the next seven arterials strands 83–490 files. After all eight the largest component is 369 files and it is `nodes-langchain`. The "organic core" was a second-order plat.
- **The two mixed-vendor sub-districts are a weak result.** They are connected (every edge exists) and in band, but they group integrations that share a helper below the arterial floor rather than a purpose. They are recorded as the known weakness of the stranding floor, not argued away.

For comparison with the brief's two rejected approaches on its tree (recursive Leiden: 145–530 sub-districts, largest 276–872; delete the hub and cluster: 780 sub-districts, 538 singletons): this gives **7 named sub-districts and one grid**.

## 7. Scoring a subdivision

The redistricting criteria, per sub-district:

| criterion | measure | result on the 94 organic sub-districts |
|---|---|---|
| population equality | size within `[0.3T, 2T]` | 94/94 |
| contiguity | one connected component in the partition graph | 94/94 |
| communities of interest | share of files in the modal directory | median 0.09–1.00 per district; reported, not gated |
| compactness | Polsby-Popper `4πA/P²` on the contour the map draws | **not predictable from the current layout — see below** |

**Polsby-Popper cannot be measured before the layout exists.** The script mirrors `blobs.rs`'s contour construction exactly (420-cell raster, size-scaled Gaussian, descending threshold until 95% of members are inside) and scores each sub-district on the map's own raster with its siblings substituted for the parent. On current coordinates, **23 of 94 sub-districts score 0** — their members are interleaved with their siblings' in a layout that was never asked to separate them, so they win no raster cells — and the median is 0.30 against 0.85 for the districts they came from. That measures agreement with a layout built for different groups, not the shape a subdivision-aware layout would draw. PP therefore moves into the implementation's falsification tests (§11), measured on its own layout. A plat cell is a square by construction (PP = π/4 ≈ 0.785).

## 8. Names

Sub-districts inherit the parent's name with a suffix — census tract 101 becomes 101.01 and 101.02 — which is how `CLAUDE.md`'s "never rename a district without the previous name in hand" survives subdivision:

- First appearance: suffixes `·1, ·2, …` in descending size, ties by the smallest member path.
- Later commits: match each sub-district to the previous commit's by best Jaccard overlap above 0.35 — the matching `src/parity.rs` and finding 4 already use — and keep its suffix. An unmatched sub-district takes the next unused number. **A retired suffix is never reused**, as census tract numbers are not.
- A sub-district's name is derived (`parent · suffix`), not cached separately, so renaming a parent — which already requires the previous name in hand — carries its sub-districts with it.
- Parcels are named by address: the directory they occupy (`nodes-base/nodes/Stripe`). They are numbered, not named, which is the chōme and 1811-grid principle the brief cited.
- Arterials keep their file names, as roads do.

## 9. Where path may be used, and where it may not

Issue #41's "no speculative structure" rule is a **top-level** rule. Merging two unrelated one-file districts at the top level asserts a relationship the graph does not contain, and the reader cannot tell. **Inside an oversized district, membership has already been established by the graph**; ordering parcels by their directory address to decide where within the district each sits asserts nothing new. The municipal boundary is political; the street address is geometric.

So: path is **usable** to order parcels inside an established district, and to name them. Path is **not usable** to decide district membership, to merge anything at the top level, or to join parcels into a sub-district. This proposal uses path only in the first sense. The shipped `merge_tiny` already uses it in the second (§5), which is a reason to revisit `merge_tiny`, not a precedent. Recording the line here so it is not relitigated.

## 10. Rendering and schema

- **Schema** (defined once in Rust, TypeScript generated — `CLAUDE.md`): per file, an optional sub-district index within its district, or a parcel marker; per district, its arterials and the parcel order. All fields are optional and absent whenever terrain resolves off, so maps at or below the automatic threshold retain their bytes.
- **Map surface**: arterials as roads inside the district; organic sub-districts as sub-contours within the district contour; parcels as a grid packed into the plat part of the district's area, ordered by address, cell area proportional to file count so finding 8's area encoding holds.
- **Cards**: tapping a sub-district, a parcel, or an arterial each produces a card, on a phone as well as a desktop. The three touch-only bugs documented in the reference renderer apply to every new hit target.
- **Mode**: `tolmap build` defaults to automatic terrain above 2,000 mapped source files. `--terrain` forces it on and `--no-terrain` forces it off. The 2026-09-22 decision and threshold are recorded in finding 22.

## 11. Scale, and the falsification tests

| repository size | T | split above | merge below | behaviour |
|---|---|---|---|---|
| 24 files (flask, httpx) | 9 | 19 | 3 | no-op: below the 50-file floor, and no district is above 19 (largest 7) |
| 50 files | 14 | 27 | 4 | no-op by the floor: no district can be both > 27 and ≥ 50 files |
| 851 files (django) | 56 | 113 | 17 | 2 districts subdivided; one is the locale plat |
| 11,982 files (n8n) | 212 | 423 | 63 | 7 districts subdivided; one is the 308-plugin case |
| 50,000 files | 433 | 865 | 130 | **extrapolated, not measured** — nothing that size is pinned |

At 50,000 files a connected component of up to 129 files is drawn as a parcel. At that scale 129 files is 0.26% of the repository, which is consistent with the law, but it is the prediction most likely to be wrong and the first to test when a repository that size is pinned. Cost is linear in the district: one block-cut tree per arterial iteration (n8n d0 needs eight) plus Leiden on subgraphs no larger than the district.

The implementation PR must report each of these, and each is a way to be wrong:

1. **Default unchanged.** With the flag off, all nine fixtures and all four reference maps are byte-identical to `main`. With the flag on, top-level membership, district count and district names are unchanged.
2. **No arterial in any fixture.** django, prometheus and crawlab have none at merge ratios 0.2–0.5. A hub detector that fires on prometheus's `install.go` is miscalibrated.
3. **The locale plat.** django d0 decomposes into one 31-file sub-district and 171 parcels, 170 of them inside one `conf/locale/<lang>/` directory (the 171st is `core/mail/backends/__init__.py`). If the locale files are not parcels, the terrain test is wrong.
4. **The 308-plugin case.** `workflow/src/index.ts` is n8n d0's first arterial. (This test originally also required ≥ 95% of parcels inside one vendor directory, from the 99.2% figure corrected in §6; measured literally it is 277 of 834, 33.2%, and the test as written fails. The parcels by file count are mostly vendor directories — 1,594 of 2,430 files — and the largest parcel group by count is credentials.)
5. **Generated code.** codex d0's barrel `schema/typescript/v2/index.ts` is an arterial, and at least half the district (measured 60%) becomes parcels.
6. **Band and contiguity.** Every organic sub-district is within `[0.3T, 2T]` and connected. Measured 94/94 and 94/94.
7. **Compactness, on the new layout.** Median Polsby-Popper of sub-districts is at least the *lowest* district PP of the same map. A subdivision drawing worse shapes than the worst district the map already draws is not an improvement.
8. **Determinism.** Two builds with the flag on are byte-identical. A warm-started build of the next commit keeps every matched sub-district's suffix.
9. **Phone.** Sub-district, parcel and arterial cards on touch, per `CLAUDE.md`'s viewer check.

## 12. Not in this proposal

- **Top-level `merge_tiny`.** §5 records that its directory fallback places edgeless files by path; changing it changes the top-level map and the fixtures, and belongs in its own issue and its own re-derivation.
- **dify's Python parcels.** Issue #40. The grid is the honest rendering until those edges exist.
- **The brief's baseline table.** Its district counts and Q do not reproduce on `main` before or after #44, though its edge counts do exactly (finding 15). The discrepancy is upstream of this proposal and unexplained.
- **Mixed-vendor organic sub-districts in n8n d0** (§6). A second stranding pass inside organic components, or a lower floor there, would split them; either adds a rule this proposal does not yet have evidence for.

## Decisions

Ruled 2026-09-21 by the project owner, in the session that wrote this proposal, after reading the measurements above:

1. **Reference scale: the fixture median, c = 0.517**, not codex (c = 1.041) as the brief proposed, and not codex's mainland-only 0.638. §1 is the evidence.
2. **Arterials by stranding**, not by a degree break and not by requiring both. §2 step 1 is the evidence.
3. **Sequencing: this proposal is implemented first**; the top-level `merge_tiny` directory fallback (§5) goes to its own issue and its own fixture re-derivation afterwards. Its cost is accepted: the glued-plat results (django d0, dify d5–d7) are re-measured once when `merge_tiny` changes.
4. **Implementation: one Codex PR**, reviewed and re-measured against §11 before merge.

Taken as defaults, not asked: the census ratios (2×, 0.3×) as the only new constants (§4); sub-districts as a second level with the top-level map unchanged; the 50-file eligibility floor; the flag spelled `--terrain`; a parcel's card shows its address and lists its files, like a district card at parcel scale.

## How to reproduce

```
export LEIDEN_PREFIX=$HOME/.local/leiden
cargo build --release
# per reference repository, clone pinned per the table above
./target/release/tolmap build <repo> --all-sources --name <n> --out DIR --no-parcels
./target/release/tolmap dump-blend <repo> --all-sources --out DIR/<n>.blend.json
# per fixture: cp data/<n>.json DIR/, and dump-blend with its --pkg/--lang from data/fixtures.toml

python eval/terrain_spike.py scale --maps DIR crawlab codex dify n8n
python eval/terrain_spike.py terrain --pp --maps DIR django prometheus rich crawlab codex dify n8n
python eval/terrain_spike.py sensitivity --maps DIR django:0 django:1 prometheus:0 prometheus:1 crawlab:0 crawlab:1 codex:0 n8n:0 n8n:3 n8n:6 dify:0 dify:3
```

The script needs `igraph`, `leidenalg`, `numpy`, `scipy` and `scikit-image`, all in the repository's `.venv`. Leiden runs through the Python `leidenalg` with the map's resolution, seed and `n_iterations=-1`. On the whole-repository partition graph that reproduces the map's own modularity exactly on django (0.5061), prometheus (0.5397), n8n (0.7476) and codex (0.4858), so the script is running the same search as the Rust bridge; the sub-district counts above are that search's, and the Rust implementation must reproduce them or explain why not.
