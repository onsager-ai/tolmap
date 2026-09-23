# Handoff

You are picking up a measured prototype, not a product.

**The Python and vanilla-JS code in this package is a reference implementation and a test oracle. Do not build on it.** The product is Rust + React/TypeScript, and the first thing to ship is a **website that maps any public repository**, in the shape of DeepWiki. `docs/ARCHITECTURE.md` has the target stack, the MVP scope, the module-by-module port map and the two places real risk lives. Freeze `src/tolmap/` and `viewer/` at v0.1.

Read `docs/FINDINGS.md` before writing any of the port. Several obvious-looking "improvements" have already been tried and falsified there, and the reference implementation is the thing the port is checked against.

---

## What is solid

- **The pipeline runs end to end** on Python, Go and TypeScript, and was validated on nine real repositories.
- **Mass normalisation** of the blended signal (finding 1). Do not revert this to per-edge coefficients, however natural that looks in the code.
- **Warm-started partitioning** (finding 4). This is the single highest-leverage step in the pipeline.
- **The viewer.** One HTML file, no build step, works on a phone, and taps, drags and pinches are correctly separated (see the pointer-handling comments in `viewer/template.html` — three real bugs live there, all of which only appear on touch).

## What is scaffolding

| thing | state | what it needs |
|---|---|---|
| `naming.py` | deterministic fallback only | the model hook is defined and unused — wire it up, keep the cache |
| reference graph | recovered from import statements | misses calls through variables; numbers are a **lower bound**, never inflated. SCIP/LSP would make it exact |
| previous layout | not persisted at all | the warm start has nothing to read; store maps by `(repo, commit_sha)` |
| auto-detection | none — `--pkg` and `--lang` are required | the single largest piece of unbuilt MVP work; a wrong guess fails silently |
| CLI | argparse, single process | no incremental mode, no daemon, no watch |
| store | none — JSON files on disk | SQLite, with the committed layout still authoritative |
| API | none | axum; the viewer currently has its data inlined at build time |
| URL state | none | the map cannot be linked to; TanStack Router closes this |
| tests | none | the eval scripts in `eval/` are the closest thing; the nine maps in `data/` are the port's acceptance fixtures |

---

## Build order

Milestones for the port are in `docs/ARCHITECTURE.md`. What follows is the product substance those milestones carry — the order is the same.

### 1. Make the previous layout available to the next run

Finding 4 is the highest-leverage result in the project: seeding Leiden with the previous membership takes district retention from 46% to 88% on django at no cost in modularity. Nothing else in the pipeline pays like that.

In the hosted MVP this means storing every indexed map keyed by `(repo, commit_sha)` and warm-starting from the most recent prior commit on the same branch. Nothing derived is ever written into the mapped repository: `.tolmap/` holds configuration only. `docs/ARCHITECTURE.md` has the rule — the repository holds intent, the store holds derivation — and the config schema.

Schema sketch is in `docs/PIPELINE.md`.

### 2. `tolmap check` — the CI command, after the site

Given a diff, print two numbers: **how many districts the change crosses**, and **the change in modularity**. Exit non-zero past a threshold.

This is the highest-value thing for a team that already has a repository, and it needs no UI. It is also the only feature that costs the reviewer no attention until it fires, which matters: in a world where agents write most of the code, anything that *demands* attention loses. It is deferred behind the site only because the site is how anyone finds out this exists.

### 3. Incremental build

Full rebuild is O(minutes) on django. A map that is regenerated per commit must be incremental: reuse the previous graph, re-parse only changed files, warm-start everything. The anchoring machinery already assumes this.

### 4. Wire the naming model

`naming.naming_prompt(ctx)` is the contract. Requirements, in order of importance: cache against the membership fingerprint; pass the previous name so the model renames rather than re-invents; never let two districts share a name. Name drift invalidates every spatial memory the team has built, which is worse than a mediocre name.

### 5. Exact references

Replace the import-derived reference graph with SCIP or an LSP index. Blast radius is the feature users will trust or not trust, and it currently under-reports.

---

## Things deliberately not built

- **Agent context.** Feeding the graph to an LLM is a solved and crowded space (`CodeGraph`, `code-graph`, `gograph`, …). Agents need an index, not a picture. The map's value is for the human who is now accountable for code they did not write.
- **Symbols on the map at every zoom.** Tried twice, reverted twice. Symbols now appear only when zoomed in far enough (finding 27); see README.
- **An IDE plugin.** The map is not a code browser; the editor wins that.

## The idea worth considering next

**Scope as a permission surface.** Before an agent starts, a human marks a district as its allowed work area; edits outside it require approval. Directories are not scopes (concerns cross them) and file lists are too brittle (agents legitimately touch new files), but a district *is* an expressible scope, derived from the code's actual structure rather than someone's guess. This may matter more than the visualisation.

---

## Gotchas

- `--pkg` is the source root *inside* the repo (`scrapy`, `lib/sqlalchemy`, `src/flask`, `packages` for vue, `.` for prometheus). Getting it wrong silently produces a sparse graph — see finding 7. These nine known-correct answers are the first test fixtures for auto-detection.
- Non-source trees must stay excluded (`templates/`, `vendor/`, `testdata/`, `node_modules/`). Scaffolding is not territory.
- `git_cochange` skips commits touching more than 40 files; sweeping refactors otherwise couple everything to everything.
- The viewer inlines all data. Past ~8 MB, move to fetching per-repo JSON.
- Everything is seeded (`SEED = 7`). Keep it that way; reproducibility is the product.
