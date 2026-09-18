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

---

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

## Why it stops at the file

Google Maps draws building footprints and stops. Below that, addressing is textual and you switch representation entirely. Two attempts at drawing symbols on the map — as towers, then as rooms inside Voronoi plots — were built, measured and reverted. The failure was not rendering: there is nothing at that level for spatial memory to hold, and tiling a district with 200 cells erases the silhouette that made the district memorable.

Symbols earn their place as the unit of the **query**, not of the map: search by symbol, and blast radius by symbol. The cadastral plot view survives as an opt-in layer.

---

## Layout

```
src/tolmap/     pipeline
viewer/         the map UI, a single HTML template
eval/           stability measurement across commits
data/           nine prebuilt maps — render these without indexing anything
docs/           pipeline, findings, decisions
```

## State

Working prototype, measured. Not yet a product: see `HANDOFF.md` for what is missing and in what order it should be built.
