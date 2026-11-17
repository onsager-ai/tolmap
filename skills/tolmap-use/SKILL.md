---
name: tolmap-use
description: Use tolmap for repository orientation, change boundaries, diff impact and structural debt; hand exact references and renames to LSP or grep.
---

# Using tolmap

This skill ships with the tolmap CLI (currently 0.1.0). Use the skill from the
same checkout/release as the binary. Compact query JSON uses `tolmap.query/v3`;
check and debt have their own report versions. See [QUERY](../../docs/QUERY.md),
[CHECK](../../docs/CHECK.md) and [API](../../docs/API.md) for the full contracts.

## Reach for a map when

- Orienting in an unfamiliar repository: main areas, entry files, district names.
- Planning an edit: which areas and import boundaries does this file cross?
- Reviewing a diff: affected districts, changed edges and files outside coverage.
- Choosing structural work: hotspots, district cycles and hidden coupling
  (files that change together without a static import relationship).

Use LSP, grep or the code-graph skill for exact references, renames, type-aware
callers and symbol correctness. A map supplies structural context, not proof
that all callers or dependencies were found.

## First check freshness

```sh
# Register once (committed source only), then infer from the current checkout:
tolmap add . --all-sources
tolmap status
# Explicitly refresh only when requested:
tolmap refresh --all-sources
tolmap status
```

Read `freshness`, `checkout_commit`, `snapshot.commit`, `snapshot.map_path`,
`attempt` and `dirty`. From elsewhere use `--repo NAME`; `tolmap repos` lists
names and last observed state. Reads never index or fetch. `--commit SHA`
selects the latest snapshot at a retained revision; `--snapshot ID` selects an exact history entry.
`--map FILE` remains available and conflicts with database selectors. Freshness
and dirty state are disclosed on stderr without changing compact query JSON.
Dirty and untracked files are excluded from snapshots; `check` without `--head`
compares working-tree changes separately. CLI refresh can coexist with readers.
The shared root comes from `--data`, `TOLMAP_DATA_DIR`, application config or
XDG/HOME defaults. Existing data is never discovered or imported; re-add repositories to start new history. Set
`MAP` from `snapshot.map_path` for the explicit-file examples below, or omit
`--map "$MAP"` to use the registered checkout's latest published snapshot.

For a CI-built map, verify its recorded commit/build provenance against the
checkout instead; status describes the registered library, not arbitrary files.
Ordinary `build` maps can have `map.commit: null`; don't infer freshness from it.

## Ask a small question, then inspect the source

```sh
# Main areas, then discover the Rust CLI from its manifest and locate it:
tolmap overview --map "$MAP"
rg -n -A 2 '^\[\[bin\]\]' Cargo.toml
tolmap where src/main.rs --map "$MAP"
# Direct imports and importers before editing:
tolmap deps src/check.rs --direction both --map "$MAP"
# Narrow a symbol lookup when a simple name is ambiguous:
tolmap where 'src/bin/generate_types.rs::main' --map "$MAP"
# Explain a structural connection between two files:
tolmap route src/main.rs src/schema.rs --map "$MAP"
```

`overview` lists districts, not entry landmarks; use manifests/source to discover
entry paths. Paths are exact repository-relative paths, with `/`. JSON is the default;
`--format text` is for people. `deps` is direct adjacency, not transitive impact.
`route` is an **import path**, potentially reversed or undirected as its
`edge_kind` reports, not a runtime call chain. An empty result is not evidence
that no runtime relationship exists.

Every compact response includes map identity, `coverage`, `limit`, `total` and
`truncated`, `omitted` and `next_limit`. `overview` returns all districts;
`where` and `route` default to 25 rows; `deps` defaults to 25 rows per requested
direction. With `--direction both`, each direction has its own limit, so up to
50 rows can be returned. Read `query.directions.in` and `.out` for each direction's
`total`, `returned` and `omitted` counts (an unrequested direction is null). If truncated, use the returned
`next_limit` as `--limit` to get all rows
or narrow the question; a truncated route is only a prefix. `where` can return
several symbol candidates: inspect qualified names and line ranges.

`coverage.refs` says hand/SCIP/mixed extraction, not reference completeness.
Null counts mean unknown or unread, never zero. `lower_bound: true` means
unresolved or dynamic references may be missing, even with SCIP. Confirm
important boundaries against imports/source and use LSP for exact callers.

## Review a diff without imposing a new gate

```sh
tolmap check . --base HEAD^ --head HEAD --all-sources --report-only --format json
# Omit --head to include staged, unstaged and untracked working-tree changes.
```

Read changed files, districts, edges, `lower_bound` and `outside_graph` together.
`outside_graph` lists changed paths beyond the selected graph, with reasons;
an empty mapped impact does not make those changes safe or irrelevant.
`--all-sources` uses detection's scope floor; explicit paired `--pkg`/`--lang`
can select sources when needed. Keep pilot checks advisory (`--report-only`).
Read `default_thresholds` for the default pass/fail comparison; positive
`districts_over` / `dq_over` means over the limit, negative means remaining margin.
For GitHub library entries, `--repo` uses retained source and requires `--head`;
use `check . --base <commit>` without `--repo` for local clone/worktree changes.
Exit 0 means a valid advisory report, not absence of risk; 2/3 mean input/internal
failure. Compact queries also return 0 for no match, so read `total`.

## Pick debt with evidence

```sh
tolmap debt --map "$MAP" --kind hotspot --top 5 --format json
tolmap debt --map "$MAP" --kind cycle --top 5 --format json
tolmap debt --map "$MAP" --kind cycle --cycle-evidence full --format json
tolmap debt --map "$MAP" --kind hidden-coupling --path src/ --format json
tolmap debt --map "$MAP" --district 'CLI · checking' --format json
# Use actual district names returned by overview; the name above is illustrative.
tolmap debt diff before-debt.json after-debt.json
```

To judge a refactor, hold both sides to the base partition. Independently
partitioned maps churn: files change district and flags appear or vanish on
files nobody touched.

```sh
tolmap build "$BASE_CHECKOUT" --name base --out maps    # one map per side
tolmap build "$HEAD_CHECKOUT" --name head --out maps
tolmap debt --map maps/base.json --partition-from maps/base.json --format json > before-debt.json
tolmap debt --map maps/head.json --partition-from maps/base.json --format json > after-debt.json
tolmap debt diff before-debt.json after-debt.json
```

Pin both reports to the same base: `debt diff` exits 2 for a pinned report
against an unpinned one, or two pinned to different bases. Read `pinned` in the
head report: `unplaced_new_files` are new files with no edge to a base file,
listed in `unplaced_paths` (file-level findings only, no district cycle or misplaced finding), and `placed_new_files` took the base district
holding most of their edges. A pinned report also takes its god and hotspot
cutoffs from the base (`thresholds`, `pinned.cutoffs_from_base`), so a file
nobody touched does not become a hotspot because the head has more files.
A god file's `fanin` leaves out imports from its own Rust submodules; `raw_fanin`
is the unadjusted value, and `descendant_fanin_measured: false` means the map
predates that measurement.

`--kind` is repeatable; `--path` accepts a prefix or glob. `--top` limits ranked
hotspot/god findings and cycles. Cycles default to compact summaries; use
`--cycle-evidence full` for package, mutual-pair and cut bundles. Keep totals, filter
counts, measurement flags and lower-bound wording with the findings. Missing
history or generated-file evidence is unmeasured, not evidence of no debt.
Use unfiltered current-version reports for `debt diff`; it compares raw findings
and metric deltas, not maps. Cycles compare compact summaries in both evidence
modes, so bundle-only differences are omitted. Read the files before proposing a structural edit.
