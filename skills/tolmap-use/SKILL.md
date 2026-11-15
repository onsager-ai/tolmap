---
name: tolmap-use
description: Use tolmap for repository orientation, change boundaries, diff impact and structural debt; hand exact references and renames to LSP or grep.
---

# Using tolmap

This skill ships with the tolmap CLI (currently 0.1.0). Use the skill from the
same checkout/release as the binary. Compact query JSON uses `tolmap.query/v1`;
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
tolmap status --repo .
# If absent/stale, explicitly request a committed-HEAD snapshot:
tolmap refresh --repo . --all-sources
tolmap status --repo .
```

Read the status row's `freshness`, `checkout_commit`, `indexed_commit`, source
scope and `dirty`. Status never indexes; refresh is explicit. Use the returned
`map_path` with the queries below, for example `MAP=/absolute/path/to/map.json`.
A fresh map can still have dirty files: **dirty and untracked files are never
in a committed snapshot**. Commit them before refreshing to include them.
`check` without `--head` separately analyses dirty-tree changes. Stop a local
server before refreshing its store. Use `--data /private/store` consistently
when sharing a nondefault local store.

For a CI-built map, verify its recorded commit/build provenance against the
checkout instead; local status describes the local store, not arbitrary files.
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
`truncated`. The default limit is 8. If truncated, raise `--limit` deliberately
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
Exit 0 means a valid advisory report, not absence of risk; 2/3 mean input/internal
failure. Compact queries also return 0 for no match, so read `total`.

## Pick debt with evidence

```sh
tolmap debt --map "$MAP" --kind hotspot --top 5 --format json
tolmap debt --map "$MAP" --kind cycle --format json
tolmap debt --map "$MAP" --kind hidden-coupling --path src/ --format json
tolmap debt --map "$MAP" --district 'CLI · checking' --format json
# Use actual district names returned by overview; the name above is illustrative.
tolmap debt diff before-debt.json after-debt.json
```

`--kind` is repeatable; `--path` accepts a prefix or glob. `--top` limits ranked
hotspot/god findings, not cycles or every report section. Keep totals, filter
counts, measurement flags and lower-bound wording with the findings. Missing
history or generated-file evidence is unmeasured, not evidence of no debt.
Use full current-version reports for `debt diff`; it compares raw findings and
metric deltas, not maps. Read the files before proposing a structural edit.
