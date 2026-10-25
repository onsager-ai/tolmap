# Pipeline

Seven stages. Five are pure geometry and must be byte-reproducible from a seed. A model appears twice: choosing district names, and (not yet wired) writing landmark captions. Both are cached against a content hash.

## 1 · extract
`extract.py` (ast) for Python, `multi.py` (tree-sitter) for Go and TypeScript. Produces per file: symbols, directed imports, identifier vocabulary, LOC, a branch-point complexity proxy, git churn, and symbol-level references.

Go imports name a *package*, so one import spreads 1/|D| weight across that directory's files; TypeScript and Python resolve to a single file.

The Rust product also reads Rust (`src/extract/rust.rs`, finding 57), which the reference never did. A crate's module tree is built from `Cargo.toml` and `mod` declarations, and each `use` or written-out path links the file that defines the name it uses. Each `use` declaration shares a mass of 1 among the files it reaches.

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

Two districts never share a name, and a new name never takes one a district already holds from the cache. When a new name collides, the district holding the name keeps it and the newcomer is named from the terms that set it apart from that district: the same IDF weighting, with document frequency taken over the two districts only, then the same over filename words (`runtime & util` beside a holder of that name becomes `tsdb & runtime`; prometheus's two `util/runtime/` districts are told apart by `statfs`). A number (`runtime & util 2`) is the last resort, when no term tells the two apart. A cached numbered name whose base another district holds is given a distinguishing name once, if one exists (finding 56).

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

## Incremental extraction and complete snapshots (#225)

The opt-in CLI `--extraction-cache <outside-source-directory>` stores derivation,
never configuration in the mapped repository. Standalone process jobs copy the
selected prior snapshot's extraction into their private output, then retain only
validated regular cache files with the next complete snapshot. The filesystem
store is scoped by its data root, owner and logical repository. Embedding hosts
must construct the repository/access-domain identity from their trusted adapter,
check current Policy before prior selection and every cache lookup/publication,
and use `Cache::with_authority`; standalone filesystem authority is not a hosted
provider grant. `cache_authority_revoked` from a cache read or write aborts the
build before map publication; ordinary cache I/O failures remain logged misses
or skipped writes. No cache hit authorizes access.

Each content-addressed record includes path, content SHA-256, language, symbol
collection mode, repository/domain, source selection/config digest, extractor
source digest (the per-file parser, Rust resolver, symbol collector, Go build
constraint logic, Cargo.toml and Cargo.lock), schema source digest, actual
raw-resolver version and indexer version. Cargo.lock pins the tree-sitter
grammars; both Cargo files are present in Rust CI and npm packaging, which
compile the crate from the repository root. Refs/install modes are intentionally
outside per-file provenance: they are consumed by SCIP replacement/install
after raw parsing and source resolution, so they cannot change these records.
Payload SHA-256 rejects truncation or accidental corruption. Extraction keeps
one source tree alive at a time and spools cached symbols exactly as freshly
collected records. Python syntax-error files remain conservatively excluded
and are parsed again rather than cached.

| Input/change | Extraction | Dependent work and observable reason |
|---|---|---|
| Unchanged bytes/path | Reuse matching record | All resolution/history/geometry rerun |
| Content or cross-file exports/imports | Parse changed file | Global resolution rerun; `content_changed` |
| Addition/deletion | New file parsed; deleted file omitted | Global file set rebuilt; `file_added` / `file_deleted` |
| Rename | Path is part of key; new path parsed | Global resolution/history rerun; `file_renamed` for equal bytes |
| Source scope/config/exclusions | Full parse fallback | `config_or_exclusions_changed` |
| Extractor source/Cargo.lock, schema, resolver version or indexer version | Full parse fallback | `extractor_changed` / `schema_changed` / `resolver_changed` / `indexer_changed` |
| Missing/corrupt entry | Parse that file | `cache_entry_missing_or_corrupt` if otherwise equivalent |
| History revision/horizon | Raw syntax may be reused | Always walk requested history with existing 4000-commit algorithmic window; `history_window_advanced` |
| Prior snapshot/naming input | Raw syntax may be reused | No downstream cache: use explicit prior membership and names as existing warm start does |
| Revoked authority/different owner | Deny / distinct namespace | `cache_authority_revoked`; no cached permission |

No graph, blend, partition, layout or naming result is cached. Their complete
inputs (including the extracted set, history revision/window, prior snapshot and
name cache) are consumed again every run. Thus cross-file changes and sliding
co-change history cannot keep an old graph. The map schema, SEED=7, algorithms,
mass normalization, thresholds and naming-continuity rules are unchanged.
`index_reuse` JSON log observations record actual parsed/reused counts and reasons;
`index_timing` observations record wall seconds and completed units outside maps.

Service publication writes validated map and symbol artifacts into a unique,
immutable directory, flushes them, then switches the map pointer and name cache
in one SQLite transaction. Commit filenames remain full object IDs. The switch
checks durable attempt epoch/status and serializes against registry cancellation.
Same-host process results and remote-agent results converge on this same
`Store::publish` switch. The private-checkout local viewer uses its own fenced
complete-snapshot metadata switch.
Failure before the switch leaves the last complete row readable; unreferenced
crash leftovers are not snapshots. A process crash after the service renames
`<commit>-<job-id>` into place but before SQLite commits can leave that
unreferenced directory behind; there is no startup garbage collector yet, so it
is a disk leak until operator cleanup. Never infer that a directory is safe to
delete without checking that no map row references it. Local publication uses
the existing complete snapshot metadata boundary, now flushed and fenced by
the current local job ID.
Source remains pinned to the immutable prepared Git revision in that metadata.
An early map is explicitly provisional and never a complete snapshot.

Extraction-cache transport is currently available to the local/loopback process
worker, where the service copies a validated cache into that job's private
output and adopts it with the completed snapshot. The remote-worker artifact
protocol transfers map, symbols, names and index metadata, but not extraction
records. Remote workers therefore perform a full extraction build; no remote
cache reuse is claimed. This is a documented fallback until extraction records
have an owner-scoped transport and the same validation and publication checks.

`GET /api/maps/{owner}/{repo}/freshness?commit=<loaded-commit>` reports indexed
and requested commits separately, queued/indexing/complete/failed/stale state,
failure code/text and retry availability. `complete` means a complete saved
snapshot, not a claim that the provider's HEAD is current; unknown requested
commit stays null. Map responses carry `X-Tolmap-Commit`; viewer detail/source
requests use that returned commit to avoid following a different latest map.
