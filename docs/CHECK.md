# `tolmap check`

`tolmap check` tells you what a change does to the map. It reports two numbers: how many districts the change crosses, and how much it changes modularity when the base partition is held fixed. It also lists the cross-district edges behind those numbers. It is meant to run in CI, and in the tools that orchestrate and verify agent work (Ostrom, Duhem), so its contract is its exit code and its versioned JSON. This document is that contract. Issue #170 is the plan it implements; HANDOFF.md item 2 is why it exists.

```
tolmap check <repo> --base <ref> [--head <ref>] [--base-map <map.json>]
             [--max-districts N] [--max-dq X | --report-only] [--format text|json]
```

## Flags

| flag | meaning |
|---|---|
| `<repo>` | any path inside the git repository; the check runs on its top level |
| `--base <ref>` | the commit the change is measured against. Its map supplies the districts. Any git revision that names a commit |
| `--head <ref>` | the changed commit. Omitted, head is the working tree: staged, unstaged and untracked (not ignored) files are all included |
| `--base-map <map.json>` | a stored map of `--base`, used as the base partition instead of building one. See "The base map" below |
| `--max-districts N` | fail (exit 1) when the change crosses more than `N` districts |
| `--max-dq X` | fail (exit 1) when modularity drops by more than `X`, that is when `delta_q < -X`. `X` is a number ≥ 0 |
| `--report-only` | apply no threshold: report, and exit 0 whatever the numbers. Cannot be combined with `--max-districts` or `--max-dq` (exit 2) |
| `--format text\|json` | `text` (the default) or `json` on stdout. Progress and timings go to stderr |

## Default thresholds

**With no threshold flag the check applies `--max-districts 4 --max-dq 0.01`.** Both are absolute: neither scales with the size of the repository.

- **Flags replace the defaults as a set.** Pass either threshold flag and only the flags you pass apply. `--max-dq 0.002` alone checks Δq and nothing else; no default mixes in.
- **`--report-only` applies none.** That is the behaviour the check had before the defaults existed.
- **The report says which applied.** `thresholds.source` is `default`, `flags` or `report_only`.

The defaults come from replaying 600 real commits through the check on GitHub Actions: the last 200 first-parent commits of django, prometheus and vuejs/core at their corpus pins (docs/FINDINGS.md finding 60, `remote-build.yml` `command=check-calibrate`). On those commits they would have failed 2, 2 and 1 of the 200, every one of them a change that crossed 5 or more districts.

- **`--max-districts 4`** sits above every repository's p99 (4, 3 and 3) and below a clear knee: above 4, the sample holds one commit at 5 and four at 7.
- **Absolute, not a share of the map.** The three maps have 11–15, 11–14 and 6–8 districts, yet their tails are the same in absolute terms. A share of the map's districts would fire most on the map with the fewest.
- **`--max-dq 0.01`** is set above a noise floor. Δq moves even on commits that change no file on the map, because the head's co-change history is one commit longer; on django that drift reached −0.0042. Below the floor, Δq cannot tell a small real coupling change from drift. 0.01 sits in the knee of django's tail (−0.0173, then −0.0059). In the sample it fires on one commit alone, and that commit also crossed 7 districts.

Read a Δq failure with that floor in mind. A drop of a few thousandths can come from history rather than from the diff.

## Exit codes

| code | meaning |
|---|---|
| 0 | pass: no threshold crossed, or `--report-only` |
| 1 | a threshold was crossed; the report is still printed |
| 2 | usage or input error: a bad flag or value, a path that is not in a git repository, a ref that names no commit, a `--base-map` that is missing, unreadable, not a map, or not verifiably from `--base`, detection refused (low confidence) or failed at base, the detected source root missing at head |
| 3 | internal error: the check itself failed (extraction, the base build, git failing mid-run, a panic) |

On 2 and 3 nothing is printed on stdout; the reason is on stderr. Consumers depend only on these codes and on the JSON `version`.

## What each number means

The check builds two graphs, one for base and one for head. Each goes through the stages a map build runs before partitioning, with the build's defaults:

1. **Detection** runs on the base checkout. At low confidence it is refused, as the service refuses it. The source it picks is extracted on both sides, so the two graphs describe the same codebase.
2. **Extraction.**
3. **Blend and prune:** mass-normalised blending (finding 1) and the `node-relative` prune.

This is the graph the partitioner sees. The base partition `P` is the base map's district of every file.

- **`districts_crossed`**: the number of distinct base districts that hold a changed file. A changed file is one that `git diff --name-status -M` reports as added, modified, deleted or renamed, plus untracked files when head is the working tree. A modified or deleted file counts in its base district, and a renamed file in the base district of its old path. A new file is **placed** in the district that holds the plurality of its resolved imports' targets. Each target is weighted by the blended weight of its edge before pruning, and a tie goes to the lowest district id. A new file with no resolved import into a base district is **unplaced**. It counts toward no district and is listed in `unplaced_files`, never guessed. Changed files that the map does not index (docs, tests, other languages) are ignored.
- **`delta_q`**: `Q(P, G_head) − Q(P, G_base)`. `P` is the base partition extended with the placed new files, and it is the same on both sides; only the graph changes, and nothing is re-partitioned. A negative `delta_q` means the change couples districts to each other more than it couples files within them.
  - **`Q`** is the partition stage's own objective: weighted modularity with the configuration null model, at the build's resolution γ = 1.1. That is `Q = Σ_c [ w_c / m − γ (K_c / 2m)² ]`, where `m` is the total edge weight, `w_c` the weight inside district `c`, and `K_c` the summed weighted degree of its files.
  - **Files that leave the partition.** Deleted files drop out of both sides, together with their edges. Unplaced files drop out of the head side.
  - **Not the map's `q`.** `Q` is not comparable with the map's `q`, which is reported topology-only (unweighted, γ = 1) on the partition before small districts are merged.
- **`modularity_base`, `modularity_head`**: the two `Q`s.
- **`edges_added`, `edges_removed`**: the edges between two different districts that are in the head graph and not in the base graph, or the other way round. These are the "why" behind `delta_q`.
  - **Not only the diff's own edges.** Pruning keeps each file's strongest edges relative to its others (finding 23), so an edge can enter or leave the pruned graph without either of its files changing.
  - **Matching.** Edges are matched across a rename by the file's base path.
  - **Direction.** `source → target` follows the import when the edge carries one, and path order otherwise.
  - **`weight`** is the edge's blended weight in its own graph. It is rescaled so the heaviest edge in that graph is 1, so weights compare within one graph only.
  - **`static_import`** is false for an edge made only of co-change, directory proximity or naming similarity.
- **`landmark_touches`**: changed files that are base-map landmarks of kind `hazard` or `bridge`. They are context only and never count toward a threshold.

**Every number is a lower bound.** tolmap's graph holds only the references it can resolve. It misses calls through variables, dynamic imports and reflection, so a change couples at least as much as reported. The JSON says so in `lower_bound: true`, and the text says so in its last line.

## The base map

Without `--base-map`, `tolmap check` checks `--base` out into a temporary `git worktree` and builds its map there, cold, the way `tolmap build` would. It uses the same detection, resolution, prune and namer as `tolmap build`, but draws no parcels, since parcels never change membership. The worktree is used with hooks off and is removed afterwards. The user's checkout is never touched, and neither is its index. A worktree is used rather than an archive because extraction's co-change signal reads the checkout's own `git log`. A second worktree holds `--head` when it is given.

With `--base-map`, the stored map supplies the partition, and the base graph is still extracted, because a map keeps no edge weights. The map document records no commit (docs/ARCHITECTURE.md: the compact schema has no commit field). The service store records it in the file name, `<cache_dir>/maps/<owner>/<repo>/<commit>.json`, and so does the check:

- The file must be named `<full commit id>.json`.
- That id must equal the commit `--base` resolves to.

Any other name exits 2 rather than trusting an unverified map. A map from the service (`GET /api/maps/{owner}/{repo}?commit=<sha>`) saved as `<sha>.json` qualifies. A stored map may have been built warm, so its districts can differ from a cold build of the same commit. That is expected, and it is the point of passing it: the check then measures against the districts people actually see.

## JSON schema and version policy

`CheckReport` is defined once in Rust (`src/schema.rs`), and its TypeScript is generated (`bindings/CheckReport.ts` and the `Check*` types beside it).

- **`version` is 1.** Any change to an existing field's meaning or type, and any removal, bumps it. Adding a field does not, so a consumer should ignore fields it does not know, and refuse a version it does not know.
- **Stable output.** Key order is fixed, and every float is rounded to 6 decimal places. `delta_q` is computed before rounding, and the thresholds compare the rounded values that are printed. The same inputs give a byte-identical report (CLAUDE.md's determinism rule); it holds no timings and no temporary paths.
- **`head`** is `null` for the working tree.
- **Added in version 1.** `base_files`, `base_districts` and `thresholds.source` were added after the first release of the report (issue #170 PR 2). They are additions, so `version` stayed 1.

| field | type | meaning |
|---|---|---|
| `version` | number | 1 |
| `base` | string | full commit id of `--base` |
| `head` | string \| null | full commit id of `--head`, or `null` for the working tree |
| `base_files`, `base_districts` | number | the base map's size: its files, and its distinct districts |
| `districts_crossed` | number | see above |
| `districts` | `{id, name, changed_files}[]` | the districts crossed, by id; `name` is the base map's name for it |
| `files` | `{path, base_path, status, district, placed}[]` | the changed files on the map, by path. `status` is `added`, `modified`, `deleted` or `renamed`; `base_path` is set for a rename only; `district` is `null` for an unplaced file; `placed` is true when the district came from placement |
| `unplaced_files` | string[] | head files with no district, sorted |
| `modularity_base`, `modularity_head`, `delta_q` | number | see above |
| `edges_added`, `edges_removed` | `{source, target, source_district, target_district, weight, static_import}[]` | heaviest first, then by path |
| `landmark_touches` | `{file, kind, detail}[]` | `kind` is `hazard` or `bridge` |
| `thresholds` | `{max_districts, max_dq, source}` | the thresholds applied, `null` where one was not applied. `source` is `default` (no threshold flag), `flags` (the flags given, and only those) or `report_only` (both `null`) |
| `verdict` | `"pass" \| "fail"` | `fail` exactly when the exit code is 1 |
| `lower_bound` | `true` | always |

## Text format

The text format starts with three lines: districts crossed, Δq and the verdict. Then comes the edge list: added (`+`) and removed (`−`) cross-district edges merged, heaviest first, with at most 20 shown. Then come context lines for unplaced files and touched landmarks, and last the lower-bound note. JSON lists every edge.

## Examples

Both examples are real output, not illustrations. They come from GitHub Actions ([remote-build run 36324295341](https://github.com/onsager-ai/tolmap/actions/runs/36324295341), `command=check`, `extra_args=--base HEAD~30`). The repository is django at its corpus pin `dd6f6b1` (the working tree), checked against 30 commits earlier (`5f0293b`), under the default thresholds.

Three things in them are worth reading closely:

- **It fails, on breadth alone.** Thirty commits together cross 6 districts, past the default 4. Of the 600 single commits replayed for finding 60, 4 crossed 6 or more. With `--report-only` the numbers are the same, the verdict is `pass` and the exit code is 0.
- **Modularity rose.** Δq is positive: the change coupled the districts slightly less.
- **Not every listed edge touches a changed file.** `sql/query.py -> utils/warnings.py` left the pruned graph although neither file changed. Pruning keeps each file's strongest edges relative to its others (finding 23), so a new edge elsewhere can push an old one out. The edge lists describe the graph the partitioner sees, not only the diff's own lines.

```
$ tolmap check django --base HEAD~30
districts crossed: 6 (2 admin & contrib, 3 models & db, 4 db & backends, 6 gis & contrib, 7 template, 8 gdal & gis)
delta q: +0.000162 (base 0.574889 -> head 0.575051, base partition held fixed)
verdict: fail (districts 6 > 4, delta q +0.000162 >= -0.010000; default thresholds)
cross-district edges: 3 added, 4 removed
  + django/db/backends/base/features.py -> django/db/backends/sqlite3/schema.py (district 4 -> 3) weight 0.153985
  - django/db/backends/postgresql/features.py -> django/db/models/fields/__init__.py (district 4 -> 3) weight 0.148922
  - django/contrib/gis/db/backends/postgis/operations.py -> django/db/models/sql/compiler.py (district 6 -> 3) weight 0.118161
  - django/db/backends/base/operations.py -> django/db/backends/mysql/schema.py (district 4 -> 3) weight 0.112463
  + django/core/validators.py -> django/utils/deprecation.py (district 2 -> 0) weight 0.097092, import
  + django/core/validators.py -> django/utils/warnings.py (district 2 -> 0) weight 0.092354, import
  - django/db/models/sql/query.py -> django/utils/warnings.py (district 3 -> 0) weight 0.088049, import
landmark touched: django/db/models/query.py is a hazard (churn 92 x cplx 587)
numbers are a lower bound: tolmap's graph holds only the references it can resolve (calls through variables, dynamic imports and reflection are missed), so the change couples at least this much
$ echo $?
1
```

```
$ tolmap check django --base HEAD~30 --format json
{
  "version": 1,
  "base": "5f0293b3ab546f86d666524f07d17304ac785506",
  "head": null,
  "base_files": 851,
  "base_districts": 13,
  "districts_crossed": 6,
  "districts": [
    {
      "id": 2,
      "name": "admin & contrib",
      "changed_files": 2
    },
    {
      "id": 3,
      "name": "models & db",
      "changed_files": 6
    },
    {
      "id": 4,
      "name": "db & backends",
      "changed_files": 4
    },
    {
      "id": 6,
      "name": "gis & contrib",
      "changed_files": 1
    },
    {
      "id": 7,
      "name": "template",
      "changed_files": 1
    },
    {
      "id": 8,
      "name": "gdal & gis",
      "changed_files": 1
    }
  ],
  "files": [
    {
      "path": "django/contrib/admin/views/main.py",
      "base_path": null,
      "status": "modified",
      "district": 2,
      "placed": false
    },
    {
      "path": "django/contrib/gis/db/backends/postgis/operations.py",
      "base_path": null,
      "status": "modified",
      "district": 6,
      "placed": false
    },
    {
      "path": "django/contrib/gis/gdal/raster/source.py",
      "base_path": null,
      "status": "modified",
      "district": 8,
      "placed": false
    },
    {
      "path": "django/core/validators.py",
      "base_path": null,
      "status": "modified",
      "district": 2,
      "placed": false
    },
    {
      "path": "django/db/backends/base/features.py",
      "base_path": null,
      "status": "modified",
      "district": 4,
      "placed": false
    },
    {
      "path": "django/db/backends/base/schema.py",
      "base_path": null,
      "status": "modified",
      "district": 3,
      "placed": false
    },
    {
      "path": "django/db/backends/mysql/schema.py",
      "base_path": null,
      "status": "modified",
      "district": 3,
      "placed": false
    },
    {
      "path": "django/db/backends/oracle/features.py",
      "base_path": null,
      "status": "modified",
      "district": 4,
      "placed": false
    },
    {
      "path": "django/db/backends/postgresql/features.py",
      "base_path": null,
      "status": "modified",
      "district": 4,
      "placed": false
    },
    {
      "path": "django/db/models/fields/__init__.py",
      "base_path": null,
      "status": "modified",
      "district": 3,
      "placed": false
    },
    {
      "path": "django/db/models/fields/generated.py",
      "base_path": null,
      "status": "modified",
      "district": 4,
      "placed": false
    },
    {
      "path": "django/db/models/functions/json.py",
      "base_path": null,
      "status": "modified",
      "district": 3,
      "placed": false
    },
    {
      "path": "django/db/models/query.py",
      "base_path": null,
      "status": "modified",
      "district": 3,
      "placed": false
    },
    {
      "path": "django/forms/models.py",
      "base_path": null,
      "status": "modified",
      "district": 3,
      "placed": false
    },
    {
      "path": "django/utils/inspect.py",
      "base_path": null,
      "status": "modified",
      "district": 7,
      "placed": false
    }
  ],
  "unplaced_files": [],
  "modularity_base": 0.574889,
  "modularity_head": 0.575051,
  "delta_q": 0.000162,
  "edges_added": [
    {
      "source": "django/db/backends/base/features.py",
      "target": "django/db/backends/sqlite3/schema.py",
      "source_district": 4,
      "target_district": 3,
      "weight": 0.153985,
      "static_import": false
    },
    {
      "source": "django/core/validators.py",
      "target": "django/utils/deprecation.py",
      "source_district": 2,
      "target_district": 0,
      "weight": 0.097092,
      "static_import": true
    },
    {
      "source": "django/core/validators.py",
      "target": "django/utils/warnings.py",
      "source_district": 2,
      "target_district": 0,
      "weight": 0.092354,
      "static_import": true
    }
  ],
  "edges_removed": [
    {
      "source": "django/db/backends/postgresql/features.py",
      "target": "django/db/models/fields/__init__.py",
      "source_district": 4,
      "target_district": 3,
      "weight": 0.148922,
      "static_import": false
    },
    {
      "source": "django/contrib/gis/db/backends/postgis/operations.py",
      "target": "django/db/models/sql/compiler.py",
      "source_district": 6,
      "target_district": 3,
      "weight": 0.118161,
      "static_import": false
    },
    {
      "source": "django/db/backends/base/operations.py",
      "target": "django/db/backends/mysql/schema.py",
      "source_district": 4,
      "target_district": 3,
      "weight": 0.112463,
      "static_import": false
    },
    {
      "source": "django/db/models/sql/query.py",
      "target": "django/utils/warnings.py",
      "source_district": 3,
      "target_district": 0,
      "weight": 0.088049,
      "static_import": true
    }
  ],
  "landmark_touches": [
    {
      "file": "django/db/models/query.py",
      "kind": "hazard",
      "detail": "churn 92 x cplx 587"
    }
  ],
  "thresholds": {
    "max_districts": 4,
    "max_dq": 0.01,
    "source": "default"
  },
  "verdict": "fail",
  "lower_bound": true
}
```

## Cost

The check has no incremental build (HANDOFF.md item 3). It extracts both sides and builds the base map, skipping geometry and parcels, unless `--base-map` is given. Head needs extraction and blend only. The time a run spent on each stage is printed to stderr. `remote-build.yml`'s `check` command times it on corpus repositories on GitHub Actions: the pinned commit against its parent, or against the `--base` given in `extra_args`.

On django (851 files, co-change over its last 4000 commits), the example above took 5.6 s wall time with a 55 MB peak RSS, on a standard GitHub-hosted runner. The stages were base graph 1.8 s, base map 0.5 s and head graph 1.8 s; the rest was git and the two worktrees. Against the pinned commit's parent, the same run took 3.7 s.

The calibration replay (finding 60) timed 600 single-commit checks on the same runners: the median was 5.8 s on django, 5.3 s on prometheus (444 files) and 1.5 s on vuejs/core (239 files), and no check took longer than 6.4 s.
