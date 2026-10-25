# Reader qualification and visual/device handoff — 2026-10-25

Approval status: **pending human**. Physical iPhone/Safari: **not run**.
No approved-baseline registry is changed by this work. CI candidates and physical
acceptance are separate evidence. This document is the Core #226/#221/#222 slice
and the #219/#177 handoff; Hub/provider and #224/#225 feature delivery remain
separate tasks.

## Candidate provenance

| Candidate | Exact provenance | Review status / use |
| --- | --- | --- |
| Historical combined gallery | Core `b073260b203877a2b1bcf1f427752e66068a938e` (capture merge tree `be495d1`), Hub `86b87f918676ba4bdd594d1c5f5112f810d6ea26`; dataset `tolmap-hub-20261020`, SHA-256 `03a8c0714d84f1f9d45f73d234856ea3b458104ae5a476f4c43349e5a8df3c4a` | Historical, pending human; 532 Core, 200 Hub native and 396 Hub viewer captures. Not current Core main. |
| Logo-era Core candidate | Core `aa2b6cd7209de7b96bae37e7411c10caf0302b9b`; [run 39742883954](https://github.com/onsager-ai/tolmap/actions/runs/39742883954); visual artifact `12409484732`, Storybook artifact `12410610337` | Successful capture, pending human. Zero UI-path diff to baseline `ec3455626e1d141b0c95f8df6770e9eb45e220ca` in `web/`, `src/local_pages.rs`, `.github/workflows/storybook.yml` per Gateway audit. Does not include this reader fix. |
| This reader revision | Draft PR head and exact CI runs recorded in PR check evidence; `reader-evidence/browser.json` and `performance.json` carry PR-head `sourceSha`, tested merge `testedSha`, base SHA and run ID; `tolmap-reader-visual-review` contains Find-open PNGs plus the browser manifest | Pending CI and human image review; never promote an old capture as evidence for this head. |

The existing owner-private gallery is
<https://tolmap-visual-review.tikazyq.chatgpt.site>. Its historical candidate
identities include `tolmap-core|oss|map-file|phone|light`,
`tolmap-core|oss|source-search|phone|dark`, and
`tolmap-core|oss|source-ready|desktop|dark`.
Gateway's direct inspection of those PNGs found the defects below; this worker
has not treated that inspection as approval. The separate interactive Hub
Storybook at <https://tolmap-hub-design-review.tikazyq.chatgpt.site> is synthetic
and its exact source revision remains unverified. Staging Hub `1e9281e0dec8cf18435faa71d26e7f747daebd8b`
is older than this task; no deployment is authorized or performed here.

## Reasoned changes to inspect

| Prior candidate defect | Intended change / review focus | Automated evidence |
| --- | --- | --- |
| Phone Read selected source covered Zoom in | Action now belongs to the existing portrait/landscape selection sheet, retaining action ref and file/symbol range. Inspect Peek/Half card density and landscape sheet scroll; controls may hide when covered by the sheet. | Real local viewer action visibility/hit-testing and non-overlap at Peek and Half at 320×568 and 390×844, plus collapsed → toggle → action at 844×390, both themes. |
| Default CM6 purple/blue unreadable on dark chrome; selected line nearly invisible | Reader-owned HighlightStyle retains grammar tags using theme-specific tokens; the current Find match has an outline distinct from other matches and selected-word matches. Inspect actual syntax, selected range and Find match in both themes. Plain-text fallback remains. | Rendered token text ≥4.5:1 on editor and selection backgrounds; selection background ≥1.5:1 from editor; active Find and selection-match text ≥4.5:1; computed current-match style differs from both other highlight kinds. |
| Small Find controls / Safari keyboard zoom risk | Inputs ≥16 px; labels/buttons/close ≥44 px; wrapping/scrolling panel; full reader follows visual viewport. Inspect keyboard, last match and safe areas on physical Safari. | Computed font/target dimensions; search match/miss; keyboard/focus/Back. Chromium cannot qualify the iOS keyboard. |
| Find-open state absent from review screenshots; Escape did not dismiss Find from the reader header | Each profile now captures Find open with an active match; Escape closes Find first, then reading, and restores focus to the source action. | `reader-find-<width>x<height>-<theme>.png`, per-control dimensions, and two-step Escape/focus assertions in `reader-evidence/browser.json`. |

Storybook retains production states for source ready/search/miss/loading/access
failure/expired/missing/binary/large/unsupported/long-line across desktop, phone,
landscape and tablet in light/dark. Local process CI covers detect/confirm/index,
dirty/untracked exclusion, detection failure, low-confidence root confirmation,
indexing retry, cancellation and history. Existing viewer CI retains named
districts, landmarks, district/file/symbol cards and 40/110 px visibility gates.
The loopback confirmation forwards the exact detected scope for the pinned commit
through parent-owned child configuration; public and hosted jobs still reject
unconfirmed low-confidence sources, and job JSON cannot grant confirmation.
Actual local process CI adds A/B immutable source, refusal boundaries, map-only
export, no-login and no non-loopback AF_INET/AF_INET6 connect; performance uses
production >800 kB normal/long-line
source, not the historical 20,032-byte screenshot as a performance proxy.

The performance method now uses 14 mount/destroy cycles per file, excluding the
first two as warmup. After teardown it waits two animation frames plus idle,
forces CDP GC, records heap and probes live DIV objects for detached editor roots.
All probe handles are released each cycle. The artifact reports a least-squares
warm-cycle slope, residuals and an explicit plateau/growth classification rule;
that observation heuristic is not an acceptance SLA or proof about leaks.
The old `65f135d` three-cycle heap series grew monotonically and cannot qualify
this revision. New measurements and retention counts still require review.

The old npm consumer run `40172284928` timed out during local indexing while
native reader CI passed. Its cause remains open. CI now records stage/process
CPU observations, prepared/job state, bounded logs and worker-write tails on
timeout (`reader-index-timeout-<node>.json`). A separate packaged `dump-graph`
probe measures each large source with and without write tracing, bounded to
30 seconds each (`reader-extraction-cost-<node>.json`). These fresh-process pairs
diagnose extraction separately from reader load; a timeout is never a pass.
The traced local acceptance journey and its polling bound remain in place.

Review the exact new CI artifacts, not just green jobs. A reviewer must record
source SHA, workflow/run ID, capture identity, image/hash, observed diff/reason,
reviewer and decision before any approved baseline promotion. Approval remains
pending until that explicit record exists; no approval has been requested or
inferred by the worker.

## Revision-pinned iPhone/Safari checklist

Use a test URL whose deployed Core bundle revision is verified equal to the draft
PR head. A localhost URL cannot be opened on a separate iPhone; the owner must
supply an explicitly authorized reachable environment. No deployment is implied
by this handoff. Record Core SHA, actual Hub SHA if used, URL, immutable snapshot
commit, iPhone model, iOS/Safari versions, portrait/landscape dimensions, theme,
timestamp, each observed/expected result, and screenshot/recording references.
Do not enter a pass against an unknown or older bundle revision.

| Step | Expected observation | Result |
| --- | --- | --- |
| Open map; tap district, file, visible symbol near neighbors | Correct named district/card, landmarks and symbol range; no invisible-target selection or normal tap becoming drag | Pending physical |
| Select file at Peek/Half/Full, rotate | Read selected source and map controls usable, no overlap; selected file/range retained; sheet remains scrollable | Pending physical |
| Open/close search, layers, repository sheet, then reader; Safari edge-swipe Back | One current state dismissed at a time before leaving map; selection/camera retained as specified, no trap | Pending physical |
| Find in file with software keyboard; type match/miss, Next to last match, dismiss/reopen keyboard | No focus zoom; controls/active match visible above keyboard; text/results own scrolling; useful focus on close | Pending physical |
| Rotate map, search, sheet and reader portrait ↔ landscape | Notch/home-indicator safe areas clear; no clipped action or horizontal page overflow | Pending physical |
| Pinch map, release both fingers, immediately pan then tap file | Finger-centered pinch; no stuck pointer; one-finger pan and subsequent tap work | Pending physical |
| Select/copy source range; try typing/paste; close/reopen pinned link | Clipboard equals selected committed text; no mutation; immutable repository/commit/path/range and map camera preserved | Pending physical |
| Light/dark syntax, Find highlights and selected range | Readable text and controls in both themes, including plain-text fallback | Pending physical |

Hub connection/index/read/revoke needs an existing authorized disposable provider
repository and belongs to the Hub acceptance task. No new credentials, provider
grants, production data changes or baseline approval are authorized here.
