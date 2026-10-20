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

The catalogue is checked against the built Storybook `index.json`: every
candidate must have exactly one exported story, no unlisted story may appear,
and a separate required-state list prevents the catalogue and exports from
shrinking together. Production `RESERVED_NAMES` must match the reserved-route
stories; the route-tree contract is checked and each reserved story waits for
the production reserved-page copy naming its own path. OSS captures include
local checkout and reserved routes. The synthetic Hub host explicitly omits
those host-owned routes, and phone/landscape keyboard-dialog captures are
excluded by the shared `captureApplies` matrix function.

The four Storybook viewports are representative capture sizes, not the full
breakpoint boundary suite. `web/scripts/screenshots.mjs` owns rendered evidence
at 320×568 and 360×640, 667×375 landscape, 768×1024 and 1024×768 tablet, plus
1440×900 desktop and the 1100 px icon-only toolbar. `web/scripts/check-view-stability.mjs`
owns interaction and geometry checks for sheet detents/drag, path mode,
selection cards, keyboard commands and linked hover. Those suites remain the
owners of their existing thresholds; the Storybook state matrix does not
multiply every story across every boundary size.

Readiness is state-specific: home stories assert their actual catalogue row,
empty message, loading text or service error; indexing stories check the job
status and running/pending phase; refusal stories check the exact error code;
map stories check load/error UI, selected-card or layer state, and the
early-detail notice; source stories check exact refusal/loading copy or a
read-only editor, including the long-line plain-text fallback. The source
unsupported fixture exercises the production message for symlinks, submodules
and non-regular files as one refused state. It does not claim separate fixture
coverage for each filesystem entry type or test real repository access.

## Capture and approval

Capture every listed state in light/dark at 390×844, 844×390, 820×1180 and
1440×1000, Chromium at scale 1, `en-US`, UTC, reduced motion and a fixed clock.
The Storybook fixture label is outside the product frame and remains visible.
Each story waits for fonts and its actual state/interaction before capture.
CI uploads `tolmap-storybook` and `tolmap-visual-candidates`; the latter contains
a filterable `index.html`, individual PNGs and a `manifest.json` that records
both the viewer checkout revision and the consuming CI revision. When Hub runs
the viewer from its gitlink, the viewer revision identifies that immutable
checkout while the tested revision identifies the Hub commit.
The full page set is required; a failed story stays visible in the manifest.

All captures are **unapproved candidates**. There is no approved image set in
this change. `web/storybook/baselines/approved.json` starts with schema version
1 and `status: "pending-review"`, with null source revision/review/provenance
and an empty `images` array. In this state CI writes
`visual-regression.json` with `status: "pending-review"` and explicitly says
that candidate pixels were not compared; no pixel-pass claim is made. The
capture manifest includes every expected row, including `not-run` rows after
an early stop.

The reusable CI helper is
`verifyVisualBaselines({ browser, captures, evidenceDir, baselineDir, provenance, sourceRevision })`
from `web/scripts/visual-baselines.mjs`. A future owner review may replace the
pending registry with `status: "approved"`, a reviewed `sourceRevision`, a
non-null `review` record, the pinned browser/environment/fixture provenance,
and one image record for every exact `{id, host, viewport, theme}` identity.
Each image's `file` is relative to the baseline directory. Provenance pins the
Chromium version, locale, timezone, scale, reduced motion, fixed clock, every
viewport's dimensions and touch setting, and the fixture SHA256. The viewer
revision is recorded separately, so an approved reference can be compared
across code revisions without treating the consumer's commit as the viewer.
CI then requires a complete passing capture matrix, exact provenance, valid
PNGs and dimensions, and zero differing RGBA pixels. A mismatch writes actual,
reference and diff PNGs plus a failed report. CI never accepts or rewrites a reference. Review
must happen outside capture CI and must identify the source revision and
families accepted before the registry is changed. A green candidate capture
gate establishes rendering/behavior only, not visual acceptance or
physical-device gesture qualification. No existing thresholds are relaxed.
