<picture>
  <source media="(prefers-color-scheme: dark)" srcset="web/public/assets/brand/tolmap-lockup-dark.svg">
  <img src="web/public/assets/brand/tolmap-lockup-light.svg" width="240" alt="Tolmap">
</picture>

# tolmap

Turn a source repository into a map a person can hold in their head.

**Tolman + map.** Edward Tolman coined *cognitive map* in 1948, and in the same paper drew the distinction this tool exists to move people across.

> A **strip map** is a single route. It works until the route is blocked. A **comprehensive map** supports detours, shortcuts, and knowing where one thing sits relative to another.
>
> — *Cognitive maps in rats and men*, Tolman 1948

A developer who knows which three files to touch for a feature is holding a strip map. It is enough until someone refactors. And in a codebase where agents write an increasing share of the code, a strip map is all a human gets by default, because they were not there when it was built.

Tolman's further observation was that **stress and frustration push an organism toward the narrower map** — which is exactly the wrong direction when you are the one accountable for code you did not write.

```
pip install -r requirements.txt

python -m tolmap.cli build ~/src/scrapy --pkg scrapy --lang py --name scrapy
python -m tolmap.cli render scrapy -o atlas.html
```

`build` writes one self-contained `out/<name>.json`. `render` inlines any number of those into a standalone HTML page with no runtime dependencies.

### Rust CLI / npm package

The prepared first npm distribution is for Linux x64 with glibc 2.35 or newer. It bundles the native CLI, runtime libraries and production viewer; consumer machines do not need Rust or C++ build tools. After publication, install it with:

```sh
npm install -g tolmap
tolmap --help
npx tolmap local /path/to/private-repository --data "$HOME/.local/share/tolmap/private"
```

See [docs/NPM.md](docs/NPM.md) for platform limits, `tolmap local` options, licenses and the gated release process. CI builds and verifies a SHA-tied tarball; it never publishes automatically.

---

Coding agents: start with the versioned [tolmap-use skill](skills/tolmap-use/SKILL.md)
for compact queries, freshness, advisory diff checks and structural debt triage.

## What it does

Nine open-source repositories across Python, Go and TypeScript were indexed and measured while this was built. The numbers in `docs/FINDINGS.md` come from those runs, not from a whiteboard.

**The pipeline** (`docs/PIPELINE.md` has the detail)

| stage | module | output |
|---|---|---|
| 1 extract | `extract.py` (py) · `multi.py` (go, ts) | symbols, imports, co-change, churn |
| 2 weight | `extract.py` | one blended graph, normalised **on mass** |
| 3 partition | `pipeline.py` | districts, via seeded Leiden |
| 4 name | `naming.py` | district names — the one model-shaped stage |
| 5 place | `pipeline.py` | two-tier layout, anchored to the previous run |
| 6 landmark | `pipeline.py` | entry · bridge · hub · capital · hazard |
| 7 geometry | `blobs.py`, `parcels.py` | region outlines, weighted-Voronoi plots |

**The viewer** (`viewer/template.html`, one file, no build step)

Pan, zoom, search files *and* symbols, route between two files along the import graph, and select any symbol to see every file that references it — with the number of districts that change would cross.

---

## Symbols appear only when there is room for them

Google Maps draws building footprints and stops. Below that, addressing is textual and you switch representation entirely. Two attempts at drawing symbols on the map — as towers, then as rooms inside Voronoi plots — were built, measured and reverted. They failed because symbols were drawn at every zoom, and tiling a district with 200 cells erases the silhouette that made the district memorable.

The redesign (owner decision D1, `docs/FINDINGS.md` finding 27) keeps the silhouette by gating symbols on screen size. The overview shows district outlines and roads. A file draws its classes and functions once its footprint is at least 40 px on screen, and a class expands its methods once its short side reaches 110 px. Symbols remain the unit of the **query** too: search by symbol, and blast radius by symbol.

---

## Layout

```
src/tolmap/     pipeline
viewer/         the map UI, a single HTML template
eval/           stability measurement across commits
data/           nine prebuilt maps — render these without indexing anything
docs/           pipeline, findings, decisions
```

## Personal repository library

The Rust CLI and `tolmap web` share one personal database and configuration:

```sh
tolmap add /path/to/repository
tolmap add owner/public-repository
tolmap repos
TOLMAP_STATIC_DIR=/path/to/tolmap/web/dist tolmap web
```

Use an existing production viewer bundle (or build `web/` with its declared pnpm
workflow). Web defaults to `127.0.0.1:8787`; `--addr IP:PORT` overrides it. There
is no authentication. Anyone reaching an off-loopback listener can read source
and manage repositories. Home offers local paths, folder scan/multi-select and
public GitHub; repository pages offer history, Refresh and removal. CLI refreshes
appear without restarting web. Removal leaves the original checkout alone.
See [the personal API and configuration](docs/API.md#personal-browser-api-tolmap-web).
`local` and `serve` remain available for the later retirement slice; their old
stores are never imported into the personal library.

## Self-hosting

Build the `Dockerfile` and run `tolmap serve`. Configure it with the `TOLMAP_*` environment variables in `docs/API.md`'s configuration table, and persist `TOLMAP_CACHE_DIR` and `TOLMAP_DB_PATH` on a volume so the store and clone cache survive a restart.

The maintainers' own production/staging hosting config (Fly, Railway, pricing and machine sizing) is not in this repo.

## State

Working prototype, measured. Not yet a product: see `HANDOFF.md` for what is missing and in what order it should be built.

Private checkout → map → same-commit source: see [local reading](docs/LOCAL.md).

For compact structural answers from an existing map, see [CLI queries](docs/QUERY.md):
`overview`, `where`, `deps` and file-to-file `route`, with versioned JSON or short text.
