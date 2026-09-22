# tolmap web

React + TypeScript + Vite port of `viewer/template.html`. Read `../CLAUDE.md`, `../HANDOFF.md` and `../docs/ARCHITECTURE.md` before changing anything here — `viewer/` is frozen, this is the port.

## Run it

```
pnpm install
pnpm dev      # http://localhost:5173, one map per catalogue entry at /:owner/:repo
pnpm build    # tsc -b && vite build
```

`predev`/`prebuild` run `scripts/collect-maps.mjs`, which copies map JSON into `public/maps/` (gitignored, regenerated every run) and writes `public/maps/index.json`. Source directory precedence: `$TOLMAP_MAPS_DIR`, then `maps.config.json`'s `mapsDir`, then `../data` (the committed fixtures — fewer geometry modes, since only two of nine carry parcel polygons).

Set `$TOLMAP_CORPUS_DIR` to add the pinned evaluation maps from its `maps/` directory using `../eval/corpus.toml`; `$TOLMAP_CORPUS_MANIFEST` can point at another manifest. With neither variable set, corpus discovery is disabled and the baseline catalogue above is unchanged.

## Layout

- `src/map/MapRenderer.ts` — the imperative SVG renderer. Not React; see `docs/ARCHITECTURE.md`, "Rendering: do not put nodes in the React tree".
- `src/map/geometry.ts`, `graph.ts`, `search.ts` — pure functions shared between the renderer and the chrome (panels, search, footer stats).
- `src/map/MapCanvas.tsx` — the one React component that owns the renderer, behind a ref.
- `src/routes/` — TanStack Router routes (code-based, not file-based). `reserved.ts` lists top-level names that never resolve as a GitHub owner.
- `src/types/` — re-exports the generated bindings in `../bindings/`. Never hand-write a second copy of the map schema.
