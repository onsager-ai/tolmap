# Page visual references

Storybook renders the production route tree, React components, map renderer,
fonts and tokens. `web/storybook/catalogue.mjs` is the page/state inventory.
No alternate display implementation or fixture path enters the product bundle.
The `host` toolbar previews OSS or synthetic private host composition; it is
not authorization or a test of the real Hub service.

Run on CI (do not run browsers on the thermally constrained maintainer host):

```sh
cd web
pnpm install --frozen-lockfile
pnpm exec tsc -p tsconfig.storybook.json
pnpm build-storybook
# Serve storybook-static on 127.0.0.1:6007, then:
pnpm check:storybook
```

For interactive review, `pnpm storybook` opens the workbench on port 6007.
Only use this on a host whose local policy permits browser/rendering work.

## Coverage and design ownership

| Family | States | Governing reference |
| --- | --- | --- |
| Home | Catalogue, empty, loading, unavailable | UX §4.9; preserve |
| Local checkout | Detect, scope/all-source confirmation, indexing/cancel, detection/index failure, saved history/export | LOCAL.md and production native template; preserve |
| Indexing | No job, queue, Fetch/Read/Map/Detail, every failure class | UX §6; preserve |
| Map | Overview, district/file/symbol/folder, four layers, early detail, loading/error | UX §§3–5, 7, 9; preserve |
| Overlays | Search/results/empty, layers, repository menu, quality, keyboard, path picking | UX §§4–5; preserve |
| Source | Read-only, loading, access error, missing/expired/binary/large/unsupported, long-line fallback, find/miss | UX §13; preserve |
| Reserved routes | Every reserved route except the separately covered `/new` | Existing placeholder implementation; preserve |

The local native template is compiled by a tiny std-only Rust exporter, without
building the engine, reading a checkout or starting a service. Its production
forms are retained; preview submissions and auto-refresh are contained. The
source fixture explicitly identifies synthetic contents. The map/symbol
geometry comes from the committed Rust acceptance fixture, not synthetic layout.
MSW handles only preview API/data requests, including SSE, and never contacts a
provider. Each story owns a new router/query cache. The private preview uses only
synthetic account labels and cannot confer source access. Native Hub pages remain
owned by Hub. Existing viewer stability and actual service/process gates remain.

## Capture and approval

Capture every listed state in light/dark at 390×844, 844×390, 820×1180 and
1440×1000, Chromium at scale 1, `en-US`, UTC, reduced motion and a fixed clock.
The Storybook fixture label is outside the product frame and remains visible.
Each story waits for fonts and its actual state/interaction before capture.
CI uploads `tolmap-storybook` and `tolmap-visual-candidates`; the latter contains
a filterable `index.html`, individual PNGs and a revision-stamped `manifest.json`.
The full page set is required; a failed story stays visible in the manifest.

All captures are **unapproved candidates**. There is no approved image set in
this change. Review the families above at matching theme/viewport/fixture before
selecting reference images. An explicit owner decision must identify the source
revision and families accepted before goldens or pixel comparisons are added.
Do not copy captures into a golden directory or accept them during CI. A green
capture gate establishes rendering/behavior only, not visual acceptance or
physical-device gesture qualification. No existing thresholds are relaxed.
