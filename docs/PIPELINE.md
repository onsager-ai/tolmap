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

`parcels.py` solves a power diagram per district so each file's plot area tracks its line count. Optional; the map is useful without it.

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
