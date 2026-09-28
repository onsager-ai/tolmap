# tolmap UX: a phone-first design for the map, navigation, search and indexing pages

Status: **proposal, awaiting the owner's approval**. Mockups: the design canvas at https://claude.ai/artifact/DL11XF1y9HYSV3PFKJhrHV (thirteen artboards in four rows: Phone map, Phone states, Indexing & errors, Desktop). This document is the written spec implementers follow. Where the canvas and this text disagree, this text wins and the canvas is fixed.

Why this exists: the viewer grew one complaint at a time (see the traceability table at the end), and on 2026-09-27 the owner said the result reads as patched: "Let's do the UI/UX design properly. I don't like patches and scattering fixes." A mobile code audit the same day found about 25 defects. Those defects are inputs to this design, not a work list; every one of them is resolved by a rule below, not by a local fix.

## 1. Principles

These stand. The first six are earlier owner decisions this design keeps.

1. **Keep the app shell** (owner, 2026-09-23). On a phone the map is full screen and one finger pans it. There is no scrolling page layout on the map route. Chrome floats over the map or lives in one bottom sheet.
2. **Symbols appear only when there is room** (owner decision D1, finding 27). The overview shows district outlines and roads only. A file draws its symbols at ≥ 40 px on screen; a class expands its members at ≥ 110 px. Nothing in this design puts symbol detail on screen below those gates.
3. **Hover is a desktop affordance** (owner, 2026-09-22). Hover gives an instant highlight and a styled card on desktop. Nothing is reachable only by hover: every hover card has a tap equivalent.
4. **Theme follows the system, with a toggle** (owner, 2026-09-24): System, Light, Dark, remembered per browser. On a phone the control lives in the Layers sheet (§4.6); on desktop in the top bar.
5. **Terrain stays removed** (owner, 2026-09-23). No design surface assumes a terrain layer.
6. **Terms follow `docs/GLOSSARY.md`.** Code objects keep their code names (file, class, function, import). Map words (district, neighborhood, road, hub, landmark, island) are only for structure tolmap computes. UI copy uses the glossary's English spelling.
7. **The map palette belongs to the renderer.** District hues, `--canvas`, `--coast` and the link colours are read by `MapRenderer` with `getComputedStyle` (`web/src/index.css`, top comment). Chrome never derives its colours from them, and they never derive from chrome.
8. **Numbers are a lower bound, and the UI says so plainly.** "Heuristic references" and the unconnected-file count are facts about the map's quality, shown once in a fixed place (§4.2), not as floating chips.
9. **Plain words first, detail on request.** Every screen leads with what a person needs to act (how long, what's selected, why it failed). Pipeline stage names, evidence strings and stage counts live behind a "Technical details" disclosure.
10. **One component, two containers.** Card content (district summary, file detail, district index, layers) is written once and hosted in the phone's bottom sheet or the desktop's rail and inspector. The phone is not a squeezed desktop and the desktop is not a stretched phone; they share components, not layouts.

## 2. Information architecture

```
/                          Home: map a repository, then the catalogue
/index?job=<id>            Indexing: queued → running → (done → redirect to the map) | failed
/<owner>/<repo>            Map
   selection state         none | district | neighborhood | file | symbol | road   (in the URL: ?d= ?file= …)
   overlays                search | layers | route
```

Screen inventory (each has an artboard unless noted):

| Screen / state | Phone artboard | Desktop artboard |
|---|---|---|
| Home | Home | (same content, centred column; not drawn) |
| Indexing, queued | Indexing · queued | (same frame as running) |
| Indexing, running | Indexing · running | Desktop indexing · details open |
| Indexing, failed (deterministic) | Couldn't map | (same content, centred) |
| Indexing, failed (transient) | described in §6.4 | described in §6.4 |
| Map, nothing selected | Map overview · peek | (rail + map, no inspector) |
| Map, district index open | District index · half | the left rail |
| Map, district selected | District selected · peek | inspector with the district summary |
| Map, file selected | File selected · half (dark and light) | Desktop map · file selected |
| Search | Search active | top-bar search with a dropdown |
| Layers and appearance | Layers and appearance | top-bar segmented control + theme button |
| Route (from/to) | a sheet state, §4.7 (not drawn) | inspector section (not drawn) |

## 3. Phone shell

Frame: 390 × 844 is the design size; §8 gives the other sizes.

- **Search pill** at the top (inset 12 px from the sides, below the safe-area top inset): a 48 px pill holding a search button ("Search") and the repository as its own 44 px button (`owner/repo` ⌄). It replaces today's separate repo select, layer button and search row, which together took ~190 px.
- **Repository sheet** (owner, 2026-09-28: "no way to go back or switch repos"): the pill's repository button opens a modal sheet, like the Layers sheet, holding the repository on screen, then "Home" and "Map another repository" (Home's field, focused), then the other mapped repositories as 56 px rows (the catalogue Home lists). The way home sits above the list so a long catalogue never pushes it off screen. It replaced a native select laid invisibly over a ↓↑ icon, which read as sort rather than switch and could never lead Home. Leaving through any row replaces the sheet's history entry.
- **Control column** on the right edge, below the pill: 44 × 44 buttons for zoom in, zoom out, fit (inward-corners icon, owner 2026-09-24) and layers. There is **no fullscreen button on phones**: the Fullscreen API does not exist on iPhone Safari, and the shell already fills the screen.
- **One bottom sheet** (owner, 2026-09-27: "One sheet, 3 heights"). It replaces the district drawer, the selection panel, the Folders panel and the floating coverage chips.
- **Nothing else floats over the map.** RouteBox, legends and the road card are sheet content (§4).

### 3.1 Sheet detents

| Detent | Height | Used for |
|---|---|---|
| Peek | 156 px + bottom safe inset (190 px on a 390 × 844 phone) | the default; the summary of whatever is selected |
| Half | 480 px, or 57% of the visible height if smaller | lists and file detail |
| Full | visible height − (search pill bottom + 8 px) | long lists, all symbols, folders |

Heights are computed from `window.visualViewport.height` and `env(safe-area-inset-*)`, never from `vh` (iOS `vh` measures the large viewport, which put the old 58vh sheet over the lower zoom buttons). The sheet drags between detents with a grabber (a real button: tapping it cycles peek → half → full → peek) and by dragging its header. Content scrolls only at Full; at Peek and Half, a vertical drag on the sheet body moves the sheet, not the content, so there is no scroll-inside-scroll (the old folder list at max-height 300 px inside a 44vh body). At Full the standard bottom-sheet rule decides who owns a drag (owner, 2026-09-28: "when details opened, unable to drag down to collapse because of scrolling"; `dragOwner` in `web/src/map/phoneShell.ts`): the grabber and the header always move the sheet; a downward drag that starts with the content at its top moves the sheet; one that starts mid-scroll scrolls the content, and the next drag moves the sheet; an upward drag scrolls the content. Below Full an upward drag raises the sheet first.

### 3.2 Sheet content by state

| Map state | Peek shows | Half/Full shows |
|---|---|---|
| Nothing selected | repo, "19 districts · 6,347 files", the map-quality row | the District index, with a Districts / Folders tab |
| District selected | "District", name, files, mostly-folder; Zoom to district, Details | the district card ("Summary + collapsed", owner 2026-09-23): near neighbours, collapsed folders and key files |
| Neighborhood selected | name, files, parent district link | its files |
| File selected | breadcrumb (district), file name, path | imports / imported-by (keyed to the map rings), landmark, lines, symbols, commits, key symbols, Path from/to |
| Symbol selected | symbol, kind, file:lines | references, members |
| Road/street tapped | the two ends and the link count | the file pairs behind it |
| Route active | from → to, hop count | the path's files in order |

Selection opens the sheet at **Peek**, never higher. Half is one swipe or one tap on Details away. This inverts today's behaviour (every selection opened 58vh over what was tapped).

### 3.3 Re-centring above the sheet

The map's safe rectangle is `[left 12, top pillBottom + 8, right W − 12 − 44 − 8, bottom H − sheetHeight − 8]`, recomputed on every detent change and on resize. Every programmatic camera move uses it: selecting on the map, picking a search result, Zoom to district, fit, and the district index's row taps. The selection is centred in the safe rectangle, so the thing you picked is always visible above the sheet. When the user drags the sheet to Half, the camera eases so the selection stays in the rectangle, but only if it would otherwise be covered (no camera move the user didn't cause).

### 3.4 Back stack

Opening an overlay pushes exactly one history entry; OS back (Android back, the iOS edge swipe, the browser back button) pops overlays in this order before it leaves the map:

1. repository sheet open → close it
2. search open → close search
3. layers sheet open → close it
4. sheet at Half/Full → return to Peek
5. a selection → clear it (the camera stays)
6. otherwise → leave the map (the previous page: Home, when the map was opened from it). A map opened straight from a link has no page of ours behind it; the repository sheet's "Home" is the way there.

Selection changes themselves `replace` the URL (deep links keep working, and the back button is not a selection-history scrubber). The pushed entries carry a marker so a reload or a shared link never lands in an overlay.

### 3.5 Tap outside and dismissal

- A tap on empty map clears the selection and returns the sheet to Peek (one tap, one step; it does not also zoom).
- Panning the map with the sheet at Half/Full returns it to Peek once the pan exceeds 24 px, so the map you are moving is not covered.
- The sheet's close button (44 px, top right) clears the selection.
- Dragging the sheet below Peek is not a state; it springs back.

## 4. Phone screens

### 4.1 Map overview (artboard "Map overview · peek")

The first thing a phone shows after a map loads: pill, control column, the map fitted into the safe rectangle, the sheet at Peek with the repo summary. The overview follows the active layer: Peek shows that layer's headline, and Half/Full show its ranked index. The Districts / Folders tab remains available on the District layer only. No auto-selection (owner, 2026-09-22 decision for desktop, extended to phones by this design so both open clean; deep links keep their selection).

### 4.2 Map-quality row

One row in the Peek summary: "N files without links ⓘ". It opens a Full sheet with the Map quality card of §5 ("Some links may be missing", or the SCIP wording when the map carries SCIP references) and the list of files without links (the footer list from #81). It replaces the two floating chips, which overlapped the selection card on phones. The wording "Heuristic references" and "unconnected files" is retired (owner, 2026-09-28: "quite confusing").

### 4.3 District index (artboard "District index · half")

The District index (owner, 2026-09-24, "District index": one row per district, no colour chips) in the sheet. Rows are ≥ 64 px: name (Archivo 16/600), count (mono, right), one line "mostly `folder/` · key file `name`" (mono for the paths). The Folders tree (with percentages, owner 2026-09-23) is the second tab, not a separate panel.

### 4.4 District selected (artboard "District selected · peek")

The map eases to the district in the safe rectangle; other districts dim; folder labels appear at this zoom (#81). The sheet shows the summary at Peek with Zoom to district and Details.

### 4.5 File selected (artboard "File selected · half")

- The file keeps its place on the map above the sheet, with its import links drawn (#61's selection links).
- **The two counts are the legend.** "Imports 11" carries a solid ring in the link-out colour, "Imported by 687" a dashed ring in the link-in colour: the same stroke styles the map draws. This answers the owner's "what's the coloured circles?" (2026-09-24) without a separate legend. Tapping a count lists those files.
- Facts on one wrapping line: landmark badge (e.g. Bridge), lines, symbols, commits.
- Key symbols (3 at Half, all at Full), then Path from here / Path to here.

### 4.6 Layers and appearance (artboard "Layers and appearance")

Opened from the control column. A radio list (District, Churn, Complexity, Package), each with a one-line meaning; Churn and Complexity show the renderer's ramp (`#3E6E88 → #B8B06A → #C0472F`) with its range, which today's phone footer hides entirely. Below it, Appearance: System / Light / Dark. The layers sheet is modal over the map sheet and returns to it on close.

### 4.7 Path

"Path from here" puts the sheet into path mode (Peek: "From `a.py` · pick a destination", with Cancel); the next file tap sets the destination and the sheet shows the path. This replaces RouteBox, which sat under the drawer on phones (reference used `bottom: calc(58px + safe)`; the port lost it).

### 4.8 Search (artboard "Search active")

- Tapping the pill turns the screen into search: back button, a 16 px input (anything under 16 px makes iPhone Safari zoom the page on focus and stay zoomed), a clear button, and results grouped by kind: District, Files, Symbols. Rows are ≥ 56 px, the match is bold, file rows show the directory in mono beneath the name.
- The results list sits above the keyboard (sized from `visualViewport`), is a `role="listbox"`, and scrolls independently.
- Picking a result closes search, moves the camera into the safe rectangle and opens the sheet at Peek on that item. Back closes search and restores the previous selection.
- Nothing else can cover the results: the search layer is above every map control (today the zoom buttons and the package legend covered the dropdown's edges).

### 4.9 Home (artboard "Home")

A plain one-screen page: wordmark and theme button; "Map a codebase" with one sentence of what tolmap does; the repository field (16 px, mono) and a full-width "Map it" button; then "Mapped repositories" as 64 px rows with files, districts and language. On desktop the same content sits in a 640 px column.

## 5. Desktop layout: map-first, floating (prototype "tolmap desktop prototype")

This section was redesigned on 2026-09-28 after phase 5 shipped the first desktop layout (#179). The owner asked for the desktop to be "as modern and elegant as mobile". The #179 layout matched its artboard, but it read as a dashboard: a full-width top bar, a dense 320 px rail and a boxed inspector. The owner chose "Map-first, floating" and approved the clickable prototype after six rounds of changes (§12). The prototype is the reference for spacing, copy and behaviour. Where this text and the prototype disagree, this text wins.

The phone and the desktop now share one idea: **the map fills the window, and a few floating objects sit over it.** The desktop's one panel is the phone sheet, docked left.

- **Map**, full-bleed, edge to edge. There is no top bar, no rail and no footer strip.
- **Command bar**, floating top-left, 16 px from the edges, 44 px tall. It holds the wordmark (a link Home) and the repository menu (mono name and a chevron). The menu lists the mapped repositories from the catalogue (name, files, districts, languages; a check on the current one), then "Map another repository" (Home with the field focused) and "Home". Arrow keys move through it and Esc closes it.
- **Actions**, floating top-right, as two groups:
  - The **layer switch**: a segmented control with an icon and a label per layer: District (overlapping circles), Churn (a pulse line), Complexity (a branching graph), Package (a box). The active layer's icon takes the accent colour. Below 1180 px wide only the icons show, and each keeps a tooltip ("Churn  2") and an `aria-label`. No colour-palette squares.
  - **Search, theme and keyboard**: three 36 px icon buttons. Search is an icon only, with no placeholder text and no shortcut badge; its tooltip carries "Search  ⌘K" (Ctrl K off a Mac).
- **Panel**, floating left, 360 px wide, from 72 px below the top to 72 px above the bottom, sized to its content and scrolling inside itself. It carries the phone sheet's cards (principle 10) as a stack of views: overview, district, file, symbol, path, map quality.
  - The panel head is a **breadcrumb trail** of that stack (`langgenius/dify › workflow & components › types.ts`) with a back arrow when the stack is deeper than the overview. Every crumb is a button.
  - A hide button in the head collapses the panel off-screen. What remains is a small floating tab at the same place ("Districts 19", or the current card's name) that brings it back. `[` toggles it. The tab carries no key badge; the shortcut lives in its tooltip.
  - Card changes cross-fade with a 4 px rise, 180 ms (none under reduced motion).
- **The overview follows the layer.** The panel's overview shows that layer's headline numbers and a ranked index, so every layer has its own basic stats. The phone sheet follows the same layer at Peek, Half and Full; its Districts / Folders tab exists only on the District layer (§4.1, §4.3):
  - **District**: "19 districts · 6,347 files", then one quiet line, "modularity 0.737 · 126 files without links ⓘ", then the District index with the Districts / Folders tab (§4.3, rows 50 px on desktop: a swatch, the name, one "mostly `folder/` · `key file`" line, the count).
  - **Churn**: up to the last 4,000 non-merge commits in the indexer's history window, the most active district, districts ranked by their per-file commit counts (commits per file, a bar and the total), then the ten most-changed files. The map document keeps per-file counts but no commit IDs, so the overview does not claim a unique repository-wide commit total.
  - **Complexity**: the median map complexity per file, the most complex district, districts ranked by median complexity, then the ten most complex files. This is the indexer's syntax-derived `complexity` count, not a cross-language cyclomatic-complexity score.
  - **Package**: the top-level packages using the same grouping as the Package layer (language when the document identifies one language, file count, how many districts each spans), then the districts that mix packages. A polyglot document has no language per file, so its package rows omit language. Hovering a package row lights all its districts and dims the rest; clicking it frames them.
  - Every number is computed in the viewer from what the map document already carries per file (`churn`, `complexity`, the file's folder). A figure the document does not carry is left out, never estimated. Thresholds (for example "files above N") come from a named constant with a comment, not from the copy.
- **Linked hover.** Hovering an index row highlights its district on the map, and hovering the map shows a small hover card (district: name, files, mostly-folder; file: name, path, imported-by, landmark). The index and the map read as one object.
- **Map quality** moves into the panel (the footer strip is gone). "126 files without links ⓘ" opens the Map quality card: title "Some links may be missing"; one paragraph saying tolmap finds links by reading import statements with its own rules, not by compiling, so it can miss a link but never invents one, and every count is a minimum; "Imports it could not follow" per language; the files without links by folder, which are listed rather than placed on the map. When the map carries SCIP references the title and paragraph say so instead. The same copy replaces "Heuristic references" and "unconnected" on the phone (§4.2).
- **Legend**, bottom-left beside the panel (or at the left edge when the panel is hidden), only for Churn, Complexity and Package: the ramp with its range, or the package swatches.
- **Controls**, bottom-right: zoom in, zoom out, fit, fullscreen, 40 px, in one floating group.
- **Surfaces.** Floating chrome uses a translucent chrome token with a backdrop blur, a hairline border and a soft two-layer shadow; menus and dialogs use the solid chrome colour. No element has both a heavy border and a heavy shadow.
- **Selection and camera.** The fit safe rectangle is the window minus the command bar row, the panel when it is open, and the controls. A selection outside it, or too small to read, eases into it. Clicking empty map clears the selection and returns the panel to the overview (§3.5).
- **Map labels on desktop.** District labels are placed greedily, largest district first, with the selection and the hovered district first of all, and **no label overlaps another**. A label that would overlap is dropped, not shrunk. A big district's label sits in its upper part, clear of its file dots, with a "N files" subtitle when there is room. File dots and file labels appear only when their district has room on screen, or when the file is selected or linked to the selection. The symbol gates of D1 are unchanged.

### 5.1 Search and commands (⌘K)

Search is a centred command palette, 640 px wide, opened by ⌘K / Ctrl K, `/`, or the search icon. It groups results as Districts, Files, Symbols and **Commands** (switch layer, fit, map quality, theme, hide the panel, switch repository); with an empty query it shows the largest districts and the commands. Arrow keys move, Enter opens, Esc closes and returns focus. In path mode it lists files only and a "Path destination" tag shows in the field. §4.8's grouping and match highlighting apply.

### 5.2 Keyboard

| Key | Action |
|---|---|
| ⌘K, Ctrl K, `/` | Search and commands |
| Esc | Close the open menu or dialog; otherwise step back one card in the panel |
| ↑ ↓, Enter | Move through the overview's index (and light the district on the map); open the row |
| `[` | Hide or show the panel |
| 1 2 3 4 | District, Churn, Complexity, Package |
| + − | Zoom about the safe rectangle's centre |
| F | Fit the map |
| Z | Zoom to the selection |
| T | Cycle the theme (system, light, dark) |
| ? | The keyboard list |

Single-letter keys never fire while a text field has focus. Mouse: drag pans, the wheel (or a trackpad pinch) zooms at the cursor, Shift + wheel pans sideways, double-click zooms in.

## 6. Indexing, queue and error pages

### 6.1 What the page leads with

The ETA, big: "About 3–5 min left" (from `eta.low_s`/`high_s`, rounded to whole minutes independently, as `formatEtaRange` does). Beneath it: "Running 1m 12s · started 7:08 AM · estimate". While queued: "Starts in about 2 min" and "2nd in line · waiting 0m 40s" (from `eta_start_s` and `queue_position`).

### 6.2 Four phases instead of 22 stages

The page shows four plain-language phases, each a row with a state icon and a one-line meaning, and a four-segment overall bar. The running phase shows its current step in plain words with the live count and rate from `progress` (`done`, `total`, `rate_per_s`, `unit`). Finished phases show their duration (the sum of their stages' `duration_s`).

Stage → phase mapping, from `src/progress.rs` `StageId::ALL` (wire ids as in `bindings/StageId.ts`):

| Phase | Meaning shown | Stages (wire id → step text shown while running) |
|---|---|---|
| Fetch | Download the repository | `clone` "Contacting the host", `clone_objects` "Downloading", `clone_deltas` "Unpacking", `clone_checkout` "Writing files" |
| Read | Find the source, parse files, follow imports, read history | `detect` "Finding the source", `parse` "Parsing files", `resolve` "Following imports", `install` "Installing dependencies", `index_go` / `index_py` / `index_ts` "Indexing Go / Python / TypeScript", `history` "Reading history" |
| Map | Group files into districts and draw them | `blend_prune` "Weighing links", `partition` "Finding districts", `neighbourhoods` "Finding neighborhoods", `naming` "Naming districts", `regions` "Drawing districts", `footprints` "Sizing files", `write_map` "Saving the map" |
| Detail | Classes and functions for close zoom | `symbols` "Reading classes and functions", `symbol_cards` "Laying out symbols", `write` "Saving details" |

`install` and `index_*` only run for `--refs scip` jobs; as today, a pending `index_*` stage is not shown (`shownStage`). Phase progress is the share of its shown stages done, plus the running stage's `done/total` when `total` is known; an indeterminate stage shows an indeterminate bar.

**When the map is ready:** the page moves to the map when the Map phase finishes, and otherwise when the job reaches `done`. Phase 4 found that the service did not publish the map before symbols (it registered the map only with the job's whole result, after the Detail phase), so it now does: the map is read back when `write_map` finishes, served at the job's commit while the job runs on, and the snapshot says so with `map_ready` (`docs/API.md`, `GET /api/jobs/{job_id}`). The map view keeps `?job=<id>`, pins that commit, asks for symbols once the job is done, and says in one line that classes and functions are still coming until then.

### 6.3 Found so far

A line of facts as they arrive, taken only from what the snapshot carries: the `parse` stage's `total` ("6,347 source files"), `history`'s count ("N commits read"), and the `regions`/`naming` totals ("19 districts"). The job snapshot does not carry `RepoFeatures` (languages, clone bytes): those go to the master only. Showing languages would need an additive snapshot field; this design does not require it.

### 6.4 Actions and copy

- **Copy link** is the primary action on a running or queued job, with "You can close this page. The link keeps working, and the map opens here when it's ready."
- **Cancel mapping** (or **Leave the queue**) is a quiet text button at the bottom, in the link-out colour, with the existing confirm step.
- **Technical details** is a disclosure: the full stage list grouped under the four phases with durations, "stage 6 of 22", and the raw `stage` string. Open by default on desktop (artboard "Desktop indexing"), closed on phones.

### 6.5 Failure pages

A failed job gets a page with a plain title, one or two sentences of explanation, the service's message inside a "What tolmap saw" disclosure (mono), and actions chosen by `error_code`:

| `error_code` | Class | Title | Actions |
|---|---|---|---|
| `detection_uncertain` | deterministic | "tolmap couldn't tell where this repository's code lives" | Map another repository; Report this repository |
| `detection_failed` | deterministic | "tolmap found no Python, Go, TypeScript or Rust source here" | Map another repository |
| `cancelled` | user action | "Mapping was cancelled" | Map it again; Home |
| `clone_failed` | transient or input | "tolmap couldn't download this repository" | Try again; check the name |
| `worker_crashed` | transient | "The mapping job stopped during <phase>" | Try again |
| `busy`, `server_stopping`, `rate_limited` | transient | "tolmap is busy right now" | Try again (after `Retry-After` when present) |
| `index_failed`, `internal_error`, unknown | bug | "Something went wrong while mapping" | Try again; Report this repository |

A deterministic refusal never offers Try again: the same repository at the same commit gives the same answer, and the button implied otherwise (the owner's hindsight report, 2026-09-27, issue #162). The copy says so: "Trying again won't change the result."

## 7. Interaction model (rules, not suggestions)

### 7.1 Gestures

The gesture logic is one pure module (like `web/src/map/pinch.ts`) with unit tests, fed pointer events and returning intents (tap, double-tap, pan, pinch, cancel). The renderer applies intents; it never infers them.

1. **Tap slop.** A pointer sequence is a tap if the straight-line distance from pointerdown to pointerup is ≤ 10 px on touch and ≤ 4 px on a mouse, and it lasted < 500 ms. Distance is from the down point, not cumulative path length (4 px of cumulative travel turned ordinary finger taps into drags).
2. **Double-tap** zooms 2× about the tapped point. It counts only when both taps are single-pointer taps (never a pinch end or a `pointercancel`), within 300 ms and 24 px of each other. The first tap's selection stands; the second tap does not undo it or step it back.
3. **Pinch.** Both pinch pointers are captured on the map element (`setPointerCapture`), so a repaint that replaces child nodes cannot orphan them. `lostpointercapture`, `pointercancel` and any `isPrimary` pointerdown reset the gesture state, so a lost pointerup can never leave a ghost pointer that turns later one-finger drags into pinches. A third pointer is ignored, not treated as a new pan origin.
4. **No repaint mid-gesture.** While any pointer is down, and for 350 ms after the last pointerup, React-driven `render()` calls are queued and applied once the gesture ends; the renderer's own transform-only frame updates continue. This also closes the gap where a symbols fetch could repaint between pointerup and the click and deselect.
5. **Tap on empty map** clears the selection (§3.5). A tap on an invisible island never selects it (the #67 rule stands).
6. **Page gestures stay off the map.** `touch-action: none` on the map, `touch-action: manipulation` on `html` (no double-tap page zoom on buttons), `overscroll-behavior: none` on `html` and `body` (no pull-to-refresh or rubber-band behind the map), `-webkit-tap-highlight-color: transparent` on the map, `user-select: none` on map labels (#29).
7. **Road and street cards** move with the map or close on pan; they never stay pinned to a screen position while the map moves under them.

### 7.2 Search, focus and keyboard

- Search opens on a tap on the pill (phone) or `/` (desktop); it closes on back, Esc, the back button, picking a result, or a tap outside the results on desktop.
- Arrow keys move through results, Enter picks, Esc closes; focus returns to the element that opened search.
- Focus rings show for keyboard focus only (`:focus-visible`), never after a tap.
- The sheet grabber, sheet header and every list row are real `button`s; the sheet header has `aria-expanded`.
- Truncated names show in full on long-press (touch) and on hover (desktop), and in the card itself; nothing important lives only in a `title` attribute.

### 7.3 Hover on touch devices

Tailwind's `hoverOnlyWhenSupported` is turned on so a tap never leaves a stuck hover style. No hover style changes text to a colour that fails contrast in either theme (today `hover:text-white` makes text vanish on the light theme).

## 8. Tokens

### 8.1 Type

- **Archivo** for all UI text: headings, labels, buttons, body copy, district and neighborhood names on the map.
- **IBM Plex Mono** only for code identifiers and data: file names and paths, symbol names, repository slugs, line ranges, counts in data rows, evidence strings. Today almost everything is mono at 9.5–13 px, which reads as a debug dump.
- Scale (px / weight): 30/700 page title · 22/600 sheet title · 20/600 section title · 16/600 row title · 16/400 body and every input on phones · 14/400 secondary · 13/400 meta · 12/600 uppercase group labels (letter-spacing 0.06em). Nothing under 12 px anywhere; nothing under 16 px in an input on a phone.
- Numbers in headings use `font-variant-numeric: tabular-nums` in Archivo; numbers in data rows use mono.

### 8.2 Space, shape, targets

- Spacing steps: 4, 8, 12, 16, 20, 24, 32. Phone side gutter 20 px in sheets, 12 px for floating controls.
- Radii: 24 (pill), 20 (sheet top), 14 (cards, list groups, primary buttons), 12 (secondary buttons, control column), 10 (segmented controls), 6 (badges).
- **Touch targets ≥ 44 × 44 px** on phones, with no exceptions: breadcrumb segments, district-index rows, search rows, folder and key-file toggles, zoom-to-district, package-legend buttons, the theme button, zoom buttons, symbol rows. Symbol cards on the map stay hit-testable only when ≥ 14 px (the existing floor), with a 44 px hit area around small ones.
- Desktop pointer targets may be 32–40 px.

### 8.3 Chrome colours

Reuse the existing tokens (`web/src/index.css`, light values first, dark in the two dark blocks):

| Token | Light | Dark | Use |
|---|---|---|---|
| `--chrome` | #eef1ee | #151c21 | sheets, rail, top bar, pages |
| `--chrome2` | #ffffff | #1e272d | raised controls, rows, buttons |
| `--rule` | #d7dbd6 | #2b363d | dividers, borders |
| `--on` | #131a1e | #d9e1e3 | primary text |
| `--dim` | #5b6367 | #8b989e | secondary text (4.5:1 on `--chrome` in both themes) |

New tokens this design introduces:

| Token | Light | Dark | Use |
|---|---|---|---|
| `--accent` | #2f7d68 | #6fb39f | primary buttons, selection, focus; replaces the hard-coded `#6FB39F` scattered through `IndexJobView.tsx` and the catalogue |
| `--on-accent` | #ffffff | #0a1410 | text on `--accent` |
| `--link-out` | #c0472f | #e0664c | "imports" rings, lines and the count's glyph; destructive text buttons |
| `--link-in` | #3d7fb8 | #6aa6d8 | "imported by" rings (dashed), lines and glyph |
| `--grabber` | #b9c0bb | #3a474f | sheet grabber |

`--link-out`/`--link-in` supersede `--hot`/`--cold` for selection links. The dark values are lightened because `#c0472f` and `#4f8fc4` fall under 3:1 against the dark chrome and canvas. They stay renderer tokens (principle 7): the renderer reads them for the map, and the card reads the same token, which is what makes the counts a legend. Direction is never colour alone: out is solid, in is dashed, on the map and in the card.

## 9. Breakpoints

Layout is chosen by the available box, not by `vh` or the window width alone.

| Profile | Rule | Layout |
|---|---|---|
| Phone portrait | width ≤ 600 and height > width | §3–4: pill, control column, one sheet |
| Phone landscape | height ≤ 500 | pill and controls as portrait; the sheet becomes a **side sheet** on the left, 360 px wide, full height, with the same detents mapped to widths (collapsed 0, peek 360); map safe rectangle is the rest. The desktop layout is never used below 500 px of height (today an 844 × 390 phone got the 250 px rail and 28 px buttons) |
| Tablet | 600 < width ≤ 1100, height > 500 | the desktop layout of §5: the panel starts hidden below 900 px wide; below 1180 px the layer switch shows icons only; touch targets 44 px because tablets are touch |
| Desktop | width > 1100 | §5 |

Safe-area insets (`env(safe-area-inset-*)`, with `viewport-fit=cover`) apply on every side in every profile, including the CSS fullscreen fallback, which today hides the top bar that carried the top inset and puts the search box under the notch.

## 10. Traceability

Every defect from the 2026-09-27 mobile audit and every still-open owner complaint since 2026-09-20 (`viewer-ux-feedback` record), and the section that resolves it.

### 10.1 Audit defects

| # | Defect | Resolved by |
|---|---|---|
| 1 | Pinch pointers not captured; a mid-pinch repaint can leave a ghost pointer | §7.1 rule 3 |
| 2 | Double-tap zooms at the centre, eats the next selection, no distance check, counts pinch-end | §7.1 rule 2 |
| 3 | Tap threshold is 4 px of cumulative path | §7.1 rule 1 |
| 4 | Inputs under 16 px trigger iOS focus zoom | §4.8, §8.1 |
| 5 | Landscape phones get the desktop layout or a sliver; `VH` clamp | §9 |
| 6 | Every selection opens a 58vh sheet over the target; programmatic pans centre under it | §3.2, §3.3 |
| 7 | Back never closes a card or drawer | §3.4 |
| 8 | Search dropdown never closes on outside tap; covered by other chrome | §4.8, §7.2 |
| 9 | RouteBox hidden under the drawer on phones; ✕ is a span | §4.7 |
| 10 | Many tap targets under 44 px | §8.2 |
| 11 | Fit safe rectangle keyed on map width, not layout | §3.3, §5, §9 |
| 12 | Road/street tap card floats after a pan | §7.1 rule 7 |
| 13 | Churn/complexity ramp hidden on phones | §4.6 |
| 14 | Drawer has no drag and no tap-outside close | §3.1, §3.5 |
| 15 | Safe areas not handled in fullscreen or landscape | §9 |
| 16a | A third pointer makes the view jump | §7.1 rule 3 |
| 16b | Sheet header, rail rows and search results are non-focusable divs | §7.2 |
| 16c | Truncated paths only in `title` | §7.2 |
| 16d | `hover:text-white` vanishes on light; stuck hover after taps | §7.3 |
| 16e | Nested scrollers (folder list inside sheet body) | §3.1 |
| ref-2 gap | React repaint between pointerup and click deselects | §7.1 rule 4 |

### 10.2 Owner complaints still open

| Date | Complaint | Resolved by |
|---|---|---|
| 2026-09-24 | "what's the colored circles?" (no legend for selection rings) | §4.5, §8.3 |
| 2026-09-24 | Chrome and map themes misaligned; follow system + toggle | principle 4, §4.6, §8.3 |
| 2026-09-23 | District card "quite overwhelming" ("Summary + collapsed") | §3.2 (Peek is the summary) |
| 2026-09-24 | District legend not intuitive (District index) | §4.3, §5 |
| 2026-09-27 | Mobile: tap/selection, search/nav/controls, "Navigation is quite bad" | §3, §4, §7 |
| 2026-09-27 | "the loading page for indexing is quite poorly designed" | §6 |
| 2026-09-28 | iPhone pass: "No way to go back or switch repos" | §3 (repository sheet), §3.4 (#177) |
| 2026-09-28 | iPhone pass: "when details opened, unable to drag down to collapse because of scrolling" | §3.1 (who owns a drag at Full) (#177) |
| 2026-09-27 | "Some repos not indexable" (hindsight refused, "try again" offered) | §6.5 (the page); #162 (the detection fix) |
| 2026-09-21/22 | Large maps: too many markers | not a chrome question; the renderer's ranked pins and count badges (#61 lineage) keep their own track. This design only guarantees chrome never adds markers over the map |
| 2026-09-24 | Crosses and squares at deep zoom (cards too small to label) | renderer, `fix/card-min-size`; §8.2 keeps the 14 px floor |

## 11. Implementation phases

Each phase ships a coherent slice behind no flag, with the CI screenshot states it adds (`web/scripts/screenshots.mjs`; today the only phone profile is 390 × 844 portrait, and every selection is a deep link).

1. **Foundations: tokens, type and the gesture module.** Chrome tokens of §8.3 and the type scale of §8.1 applied app-wide; `hoverOnlyWhenSupported`; the page-gesture CSS of §7.1 rule 6; the pure gesture module with unit tests for rules 1–4 (replaces CDP pinches as the check); the no-repaint-mid-gesture queue. Screenshots: every existing state in both themes (a type and colour change touches all of them).
2. **The phone shell: pill, control column, one sheet, back stack.** §3 and §4.1–4.7 and the map-quality row, replacing the drawer, the selection panel, the Folders panel, the chips and RouteBox on phones. Screenshots: a selection made by **tapping the map** with the sheet at Peek, district selected, file selected at Half, district index at Half, layers sheet, route mode, back-button pops (a script asserting the order of §3.4), 360 × 640 and 320 × 568.
3. **Search.** §4.8 and §7.2 on both profiles. Screenshots: search focused with results on the district layer and the package layer; a result picked with the sheet at Peek and the target visible.
4. **Indexing and failure pages.** §6 in full, with the phase mapping as a tested pure function. Screenshots: queued, each phase running, done-redirect, every `error_code` row of §6.5 in both themes, on phone and desktop.
5. **Landscape, tablet and desktop alignment.** §5 and §9: the landscape side sheet, the tablet collapsible rail, the desktop inspector built from the same components, the safe rectangle from real chrome, the CSS fullscreen fallback with insets. Screenshots: 667 × 375, 844 × 390, 768 × 1024, 1024 × 768, fullscreen with `requestFullscreen` stubbed out.
6. **Home.** §4.9. Screenshots: home on phone and desktop, empty catalogue, service unavailable.
7. **Desktop, map-first (2026-09-28 redesign of §5).** Built in slices, each its own PR:
   - **7a. Floating chrome and the panel.** Remove the top bar, rail, inspector and footer strip on desktop and tablet; the command bar with the repository menu; the actions (icon layer switch, search, theme, keyboard); the left panel with the view stack, breadcrumbs and hide/show; the legend and controls; the safe rectangle; the Map quality copy on both profiles. Screenshots: 1440 × 900 and 1024 × 768 in both themes, with the overview, a district, a file and the panel hidden; the repository menu open; 1100 × 800 with icon-only layers.
   - **7b. Search and keyboard.** §5.1 and §5.2, linked hover and the hover card. Screenshots: the palette with a query, with commands, and in path mode; the keyboard list; an index row highlighted by the keyboard with its district lit.
   - **7c. Layer-aware overview.** The District, Churn, Complexity and Package overviews of §5, computed from the loaded map document, on desktop and in the phone sheet. The phone's Peek headline and Half/Full ranked index follow the active layer; Districts / Folders remains on District only. Screenshots: each layer's overview at 1440 × 900 and at 390 × 844 (Half), in both themes.
   - **7d. Desktop label placement.** The no-overlap label rule of §5 in the renderer, desktop and tablet profiles only. Screenshots: the dify and django overviews at fit and at two zoom steps, compared with today's.

After phases 2 and 3 the owner checks on an iPhone on staging, because some behaviour cannot be settled in CI: focus zoom, the edge-swipe back gesture, real finger jitter against the 10 px slop, pointer capture after node removal in WebKit, the keyboard's effect on `visualViewport`, and safe-area insets in landscape.

## 12. Owner decisions

Settled by the owner on 2026-09-27 (session `16030105-19f0-4a84-933b-c5953f23c6b3`, transcript line 11067, 08:01:09Z, AskUserQuestion), when this design was approved with "Approve, start phase 1":

- **"Route" in the UI:** renamed to **"Path from / Path to"** (and "path mode" for route mode). GLOSSARY keeps Route for an aggregated dependency between repositories. §4.7 and every "route" in the viewer's copy follow this. Code identifiers such as `RouteBox` may keep their names.
- **Opening the map before details finish:** yes. The indexing page hands over to the map when the Map phase finishes, and symbols fill in at close zoom when the Detail phase lands. Phase 4 first confirms against `src/service` that the map is published before symbols, and makes the smallest service change needed if it isn't.

Settled by the owner on 2026-09-28 (session `16030105-19f0-4a84-933b-c5953f23c6b3`), redesigning §5:

- **Direction:** "Map-first, floating (Recommended)" (AskUserQuestion, transcript line 13110, 02:41:43Z), after "i think we should support desktop web as well while keep mobile UI/UX" (line 13014, 02:35:25Z) and "basically we should make desktop web as modern and elegant as mobile" (line 13065, 02:35:50Z).
- **Process:** "Prototype first (Recommended)". The prototype was revised on the owner's notes: no key badge on the collapsed panel tab; icons, not colour squares, in the layer switch; search as an icon only, moved to the right-hand actions; the confusing "Heuristic references" footer replaced, then folded into the panel; basic stats for every layer, not only District. Approved with "i'm okay with the mockup now" (line 13601, 03:06:37Z).

Settled by the owner on 2026-09-28 (same session):

- **7c includes the phone sheet.** "Phone too (Recommended)" (AskUserQuestion, transcript line 14253, 07:25:54Z) extended the layer-aware overview from the desktop panel to the phone.
