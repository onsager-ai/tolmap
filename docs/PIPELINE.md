# Pipeline

Seven stages. Five are pure geometry and must be byte-reproducible from a seed. A model appears twice: choosing district names, and (not yet wired) writing landmark captions. Both are cached against a content hash.

## 1 · extract
`extract.py` (ast) for Python, `multi.py` (tree-sitter) for Go and TypeScript. Produces per file: symbols, directed imports, identifier vocabulary, LOC, a branch-point complexity proxy, git churn, and symbol-level references.

Go imports name a *package*, so one import spreads 1/|D| weight across that directory's files; TypeScript and Python resolve to a single file.

## 2 · weight
```
w(a,b) = α·static + β·cochange + γ·proximity + δ·semantic     α .45 β .35 γ .08 δ .12
```
Each signal is scaled so its **summed mass** across all candidate edges matches its intended share — the coefficients are not influence shares (finding 1). Then prune to each node's strongest ~14 edges; a blended graph is far denser than an import graph and a dense graph has no communities to find.

## 3 · partition
Seeded Leiden (`RBConfigurationVertexPartition`), resolution ≈ 1.1 for 6–12 top-level districts, recursing inside large ones. Two operational rules: seed from the previously committed membership, and fold communities below ~4 members into whichever neighbour they are most attached to.

## 4 · name
`naming.py`. Deterministic IDF fallback ships; the model hook is defined and unwired. Cache against a fingerprint of cluster membership.

Because the hook is unwired, **regenerating a fixture in `data/` renames every district** unless the cache is seeded first — the fallback replaces `crawl control` with `downloadermiddlewares & extensio`. The fixture carries everything needed to rebuild the cache, so seed from it before rebuilding:

```
python eval/seed_names.py out/ scrapy
python -m tolmap.cli build ~/src/scrapy --pkg scrapy --lang py --name scrapy --out out
```

The fingerprint is a sha1 of the sorted member paths, so the cache hits only while membership is unchanged. If membership genuinely moved, the namer runs — that is the signal, not a failure.

Two districts never share a name, and a new name never takes one a district already holds from the cache. When a new name collides, the district holding the name keeps it and the newcomer is named from the terms that set it apart from that district: the same IDF weighting, with document frequency taken over the two districts only (`runtime & util` beside a holder of that name becomes `tsdb & runtime`). A number (`runtime & util 2`) is the last resort, when no term tells the two apart. A cached numbered name whose base another district holds is given a distinguishing name once, if one exists (finding 56).

## 5 · place
Two tiers, and the tiering is what sidesteps non-planarity: **edges are only ever drawn within one tier**. Tier 1 is a force layout on ~10 district nodes. Tier 2 is a squarified treemap inside each district, order frozen from the previous layout and areas quantised to `round(loc/25)` so ordinary edits do not reflow the packing.

Anchoring objective:
```
E = Σ w(i,j)·‖pᵢ−pⱼ‖²  +  λ·Σ aᵢ·‖pᵢ−p̂ᵢ‖²
```
`p̂ᵢ` from the committed layout; `aᵢ` is 1 for known nodes, ∞ for pinned, 0 for new. New nodes initialise at the weighted centroid of resolved neighbours.

## 6 · landmarks
Five computable selectors, each answering a different orientation question: entry, bridge (betweenness), hub (fan-in), capital (district centre), hazard (churn × complexity). Package `__init__` files and global exception modules are excluded from the structural selectors — everything touches them, so they win by default and carry no information.

## 7 · geometry
`blobs.py` rasterises a density field per district, assigns winner-take-all, and marching-squares the contour; the threshold drops until ≥95% of members fall inside, because a region must contain its own files. A district whose members form disconnected clumps renders as an archipelago rather than dropping the smaller islands.

`parcels.py` solves a power diagram per district so each file's plot area tracks its line count. This describes the frozen Python reference; its parcels were optional.

The Rust product adds a seeded neighbourhood partition after the district partition, using each district's induced kept weighted graph. Its default geometry solves nested regions (district › neighbourhood › file) and emits `P` for every file, neighbourhood outlines, and displayed footprint centroids separate from the layout coordinates in `N`. File weights come from the index-aligned code-line array `C`, falling back to LOC for older maps. `--no-parcels` remains a CLI opt-out. The district partition, modularity, and layout do not read the neighbourhood results.

## `.tolmap/layout.json` — not yet implemented

```json
{
  "schema": 1,
  "index":  { "tool": "tolmap", "commit": "a3f91c2" },
  "params": { "seed": 7, "resolution": 1.1,
              "alpha": 0.45, "beta": 0.35, "gamma": 0.08, "delta": 0.12 },
  "districts": [
    { "id": "d_payments", "name": "payments",
      "name_key": "8f2a91c4", "centroid": [0.62, 0.31], "area": 0.14 }
  ],
  "nodes": {
    "src/payments/stripe.ts": { "d": "d_payments", "xy": [0.64, 0.29], "pin": true }
  },
  "landmarks": [ { "node": "src/gateway/router.ts", "why": "entry", "rank": 1 } ]
}
```
