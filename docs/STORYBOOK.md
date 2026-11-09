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
| Overlays | Search/results/empty, layers, repository menu, quality, keyboard, path picking, lenses loaded/unresolved/empty/error, lens comparison commits/overlap/loading/stale/error | UX §§4–5; preserve |
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
the preview router's actual path plus the production generic reserved
placeholder copy and back link. The production placeholder does not print the
reserved name; the preview assertion does not invent route-specific UI. OSS captures include
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

Captures are candidates until the owner approves them. The owner approved the
set from run `42169568634` (source `fd1d6c5`) on 2026-11-06;
`web/storybook/baselines/approved.json` records it with `status: "approved"`
and CI compares exact pixels against those images. The owner approved the 32
Debt lens captures on 2026-11-09 from run `42577358027` (source `15d06e2`). A registry with
`status: "pending-review"`, null source revision/review/provenance and an empty
`images` array is the no-approval state; in it CI writes
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
reference and diff PNGs plus a failed report. With `STORYBOOK_PIXELS=advisory`
(set by the Storybook workflow) a pixel or dimension mismatch is instead
recorded as `mismatched` in `visual-regression.json`, listed in the job summary
and does not fail the job; capture or render failures, a missing or invalid
reference, matrix or provenance changes still fail. Exact pixel rendering proved
nondeterministic across runs (#219), so CI pixel differences are review
evidence, not a gate. The default, `strict`, keeps them a hard failure.
CI never accepts or rewrites a reference. Review
must happen outside capture CI and must identify the source revision and
families accepted before the registry is changed. A green candidate capture
gate establishes rendering/behavior only, not visual acceptance or
physical-device gesture qualification. The zero-pixel comparison is not
relaxed; advisory mode changes only whether a difference blocks the job.

Lens states introduced for #241 (32 identities) and the 28 existing captures
that the Lenses entry changed were approved by the owner on 2026-11-08 from run
`42441604311` (source `1704e0f`); `approved.json` records them under
`recaptures`. A complete capture matrix is rendering evidence, not visual
approval. The owner approved the 32 lens walkthrough captures and re-approved the 12 map-lens-loaded/unresolved captures (Start control) on 2026-11-09 from run `42583567170` (source `710f94a`). The Flask
viewer-check lane additionally uploads overview, zoomed stops, declared-hop
explanations and unresolved reports for phone and desktop in both themes under
`lens-review-samples` (also included in `viewer-shots/lenses/`). Its map is `data/flask.json` unchanged; Rust resolves
`data/lenses/flask-request.toml` and generates symbols from the fixture's pinned
source. The unresolved sample intentionally adds a missing member and stop.
Lens comparison states (40 identities: five states × four viewports × two
themes, OSS host), the 16 loaded/unresolved lens captures that gained the
comparison action and phone `map-path` (light and dark, changed by bottom-sheet
sizing) were approved by the owner on 2026-11-09 from run `42491269339`
(source `d5470c7`). The 16 commit/overlap comparison captures were re-approved
from run `42549386311` (source `9c012e7`) after route lines and district badges
were moved onto their stop markers; `approved.json` records both under `recaptures`. The Flask browser
lane includes commit comparison, zoomed comparison stops, overlap and stale
snapshot captures in both themes on phone and desktop. Those comparison edits
are explicitly synthetic resolver scenarios over the unchanged pinned Flask
map, not a fabricated Flask commit pair.
Lens walkthrough states for #248 (first, middle, unresolved and last steps,
restored from `?lens=overview&path=request-flow&step=<n>`) are candidates
awaiting owner review; the Flask browser lane uploads stepped captures under
`lens-walkthrough-samples`.
Source-reader responses in that browser lane are explicitly synthetic; the
native local/hosted service tests own git-object pinning evidence.
