# Working in this repo

Read `HANDOFF.md`, then `docs/ARCHITECTURE.md`, then `docs/FINDINGS.md`.

The Python under `src/tolmap/` and the viewer under `viewer/` are a **frozen reference implementation**. The product is Rust (indexer, axum, SQLite/Postgres) and React + TypeScript + Tailwind + shadcn + TanStack. Port against the reference; do not extend it.

The findings document records what was tried and falsified by measurement; several natural-looking refactors are on that list.

## Rules that are not style preferences

- **Determinism.** `SEED = 7` everywhere. Same repo at same commit must produce a byte-identical map. Reproducibility is the product, not a nicety. This was a requirement the reference did not meet until finding 9 — a seeded stage is not a deterministic one if a set of strings is iterated anywhere upstream of it.
- **Normalise on mass, not per edge** when blending signals (finding 1).
- **Never rename a district without the previous name in hand** (finding 4 and `naming.py`). Name drift is worse than a mediocre name.
- **The map stops at the file.** Symbols are the unit of the query, not of the map. This was tried the other way twice.
- **Numbers must be a lower bound.** The reference graph under-reports; never make it guess upward to look better.

## Naming

The tool is `tolmap` — a portmanteau of **Tolman** (Edward Tolman, who coined *cognitive map*) and **map**. Lowercase `tolmap` everywhere for the package, binary, module and domain. Capitalised `Tolman` refers to the person and stays as it is in prose. Do not "correct" one to the other.

## Conventions

- Rust 2021 for the indexer and API; React + TypeScript strict for the frontend. The frozen Python is 3.11, stdlib-first.
- The map surface is **not** rendered as React elements. React owns chrome and state; the map is one ref with an imperative renderer. See `docs/ARCHITECTURE.md`.
- The map JSON schema is defined once in Rust and the TypeScript is generated from it. Two hand-written definitions will drift.
- Comments explain *why*, especially where a simpler approach was tried and failed. The existing comments are load-bearing documentation.
- Measurement changes go with their numbers: if you change the blend, the clustering or the layout, re-run `eval/batch_stability.py` and update `docs/FINDINGS.md` in the same change.

## Checks before a change lands

Clustering or layout changed: the port must still place ≥95% of files in the district the reference assigns, with modularity within 0.02, on the fixtures in `data/`. This check belongs in CI from the first commit of the clustering module.

Viewer changed: districts named, landmarks listed, and tapping a district, a file and a symbol each produce a card — on a phone as well as a desktop. Touch behaviour regresses easily; three touch-only bugs are documented in the reference renderer's comments and all three will recur if the port is written from the rendering logic alone.

The reference implementation still runs, and is how you generate a fresh fixture:

```
python -m tolmap.cli build <repo> --pkg <src> --lang py --name fixture --out /tmp/fx
```
