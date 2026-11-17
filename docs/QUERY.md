# Compact map queries

`tolmap overview`, `where`, `deps` and `route` answer small structural questions
from an existing local or CI-built map. They do not index, refresh, read a
checkout contents for extraction, access the hub or make model calls. Registered
queries may inspect Git status to disclose stale/dirty state on stderr. Issue #303 defines this slice.

```sh
tolmap overview --map out
tolmap where src/main.rs --map data/tolmap.json
tolmap where 'src/bin/generate_types.rs::main' --map data/tolmap.json
tolmap deps src/main.rs --direction both --map data/tolmap.json
tolmap route src/main.rs src/schema.rs --map data/tolmap.json
```

## Inputs and flags

All commands select a registered repository with `--repo NAME`, or infer the
nearest registered checkout containing the current directory. Exact names win;
unique short names are accepted, and ambiguity lists names and IDs. Reads use
the latest successful publication, never index or fetch. `--commit SHA` selects
a retained commit/prefix and uses its latest snapshot; `--snapshot ID` selects
an exact retained snapshot. `--map <file-or-directory>` remains an explicit input and
conflicts with repository/commit/snapshot selectors. All accept `--format json|text`
(default `json`) and `--limit N` (zero requests totals alone). `overview`
returns every district by default. `deps` defaults to 25 rows per direction;
`where` and `route` default to 25 rows overall.
An explicit limit bounds any command. A map
file can have any name. A directory must contain exactly one top-level map JSON;
this accepts `build --out`'s `<name>.json` beside `<name>.names.json` and
`<name>.symbols.json`. Several maps require an explicit file, never a guess.

Paths are exact, case-sensitive repository-relative paths as stored in `F`,
using `/`; they are not filesystem paths relative to the current directory.
`where` first matches a file, then symbols. It accepts a simple symbol name,
`Parent::child`, or `path/to/file.rs::Parent::child`. Every matching definition
is a candidate; `query.ambiguous` is true when there is more than one, even if
the limit hides all but one. Use a returned qualified name (and inspect line
ranges if that still names several definitions). No candidate is selected for
the caller. A missing path-shaped target (contains `/` or an extension, without
`::`) returns zero matches with `query.missing_path` and up to three
`query.suggestions`: mapped paths ordered by character edit distance, then path.
Text says the path is absent and lists those suggestions. Suggestions are
advisory metadata, independent of `--limit`, and are never counted as matches.

Only a symbol lookup reads the map's `<stem>.symbols.json` sibling, when present.
A registered personal snapshot uses its retained
`symbols.json` instead.
A missing sibling uses the older compact map `S` table, which has no parent
hierarchy; only simple names and `path::name` work there. Compact names are
stored truncated to 44 characters, so full long-name searches require a symbols
sibling and compact qualified names are not guaranteed unique. A malformed or
unreadable sibling is an input error rather than silent fallback. The sibling
must belong to the same build as the map; legacy files contain no shared
identity with which to verify this association.

`deps` defaults to `--direction both`; `in` lists importers and `out` lists
imports, using map `E` edges only. A reciprocal relationship yields two rows,
one per direction. Duplicate edges yield one row per path and direction.
`--limit N` applies independently to each requested direction: `both` can return
up to N inbound and N outbound rows, so a busy direction cannot hide the other.
This is direct adjacency, not a transitive blast radius or symbol references.

## Versioned JSON

The Rust definitions in `src/query_contract.rs` are the source of truth;
`generate-types` exports their TypeScript bindings. Every successful response
uses the same envelope, also available as generic `QueryEnvelope<T>` for future
snapshot queries:

```json
{"schema":"tolmap.query/v3","query":{"kind":"where","target":"src/main.rs","ambiguous":false},"map":{"repo":"tolmap","commit":null,"sha256":"<64 lowercase hex digits>"},"coverage":{"refs":"hand","zero_edge_files":null,"calls_total":null,"calls_resolved":null,"unresolved":null,"lower_bound":true},"limit":25,"total":1,"truncated":false,"omitted":0,"next_limit":null,"results":[{"kind":"file","path":"src/main.rs","district":"<district name>","symbol":null}]}
```

The example's angle-bracket values are placeholders. JSON is compact UTF-8 with
one trailing newline. Paths and district names replace all internal indices.

| Field | Meaning |
|---|---|
| `schema` | `tolmap.query/v3`. Breaking field or semantic changes require a new version. Consumers must tolerate additive fields and query/result kinds. |
| `query` | Tagged by `kind`: `where`, `deps`, `overview`, or `route`; includes inputs and the command-specific facts below. |
| `map.repo` | Repository label recorded by the map, not a recovered remote URL. |
| `map.commit` | Stored top-level `commit`, otherwise a full 40/64 hex object ID from the map filename; otherwise `null`. Ordinary `build` and legacy fixtures do not record it. Never inferred from the caller's checkout. Renaming a file is not provenance verification. |
| `map.sha256` | SHA-256 of the exact map file bytes, including whitespace. Symbols bytes are not included. |
| `coverage.refs` | `hand` when reference coverage is absent or all recorded languages use hand; `scip` if all recorded languages use SCIP; `mixed` when both occur. |
| `coverage.zero_edge_files` | Map's recorded kept-edge coverage count; `null` when absent. |
| `coverage.calls_total`, `calls_resolved`, `unresolved` | Counts from the full symbols sibling, only when loaded. `unresolved` is a map of reason to count. `null` means unknown/unread, never zero. Compact `S` does not measure them. |
| `coverage.lower_bound` | Always `true`: unresolved/dynamic references can be absent. |
| `limit` | Maximum rows per requested direction for deps; maximum result rows otherwise. An unlimited overview uses its complete district count. |
| `total` | All matching rows before limiting; route counts file steps including endpoints. |
| `truncated` | For deps, either requested direction has omitted rows; otherwise exactly `total > limit`. A truncated route is a prefix, not a complete connection to the destination. |
| `omitted` | `total - results.length`: number of rows omitted. |
| `next_limit` | When truncated, the maximum requested direction total for deps, or `total` otherwise; `null` when complete. Repeating the same query with `--limit <next_limit>` returns all matching rows. Truncated deps text prints one accounting line per requested direction, including directions with zero omitted rows; other queries print one omitted-count line. |
| `results` | Ordered rows. For deps, length is the sum of `min(limit,direction.total)` over requested directions; otherwise `min(limit,total)`. |

## Query and result shapes

- **where**: `query` has `target` and `ambiguous`. Rows are
  `{"kind":"file","path":...,"district":...,"symbol":null}` for files;
  symbol candidates set `symbol` to
  `{"name":...,"qualified_name":...,"start":...,"end":...}`.
  Line ranges are the stored one-based inclusive ranges. Symbol candidates sort
  by path, start line, qualified name, then end line.
- **deps**: `query` has `path`, `direction` (`in`, `out`, `both`), and
  `directions:{"in":counts|null,"out":counts|null}`. Each requested direction has
  `counts:{"total":N,"returned":N,"omitted":N}`; an unrequested direction is
  `null`. Counts deduplicate paths within each direction. Envelope `total` is
  the sum of direction totals, `omitted` is the sum omitted, and `next_limit`
  is the maximum direction total when truncated. Rows are
  `{"kind":"dependency","path":...,"district":...,"direction":"in|out"}`,
  sorted by path then direction (`in` first).
- **overview**: `query` includes `files`, `imports` (map edge rows), `districts`,
  and `language` (the map's language label). Rows are
  `{"kind":"district","name":...,"files":N,"imports_in":N,"imports_out":N}`.
  Counts describe cross-district imports only; internal imports are excluded.
  Rows sort by descending file count, then name, with stored district order
  breaking an otherwise equal tie. `total` counts populated districts.
- **route**: `query` has `from`, `to`, and `edge_kind` (`imports`, `imported-by`,
  `undirected`, `same-file`, or `null` if none was found). Rows are
  `{"kind":"step","path":...,"district":...}` in traversal order.

Route uses `lens::Routes`, the viewer-equivalent file-to-file search documented
in [API.md](API.md)'s lenses and [UX.md §4.7](UX.md#47-path). It prefers a
shortest forward directed path, then a shortest reverse path, then an undirected
path. Neighbors are visited in stored edge order, so ties match the viewer.
Reverse paths are emitted from the requested `from` to `to`, labeled
`imported-by`. `same-file` has one step and implies no import edge.
**This is a file import path, never a runtime call chain.** A missing path means
none was found among extracted imports, not that no dependency exists.

For example, a deps query with three importers and one import at `--limit 1`
returns one row from each direction. Its accounting fields are:

```json
{"query":{"kind":"deps","path":"src/parity.rs","direction":"both","directions":{"in":{"total":3,"returned":1,"omitted":2},"out":{"total":1,"returned":1,"omitted":0}}},"limit":1,"total":4,"truncated":true,"omitted":2,"next_limit":3}
```

This fragment omits map identity, coverage and the two result rows for brevity.
`tolmap.query/v3` changes deps limit semantics and adds direction accounting.
`where`, `route`, `overview` and `status` keep their previous semantics.

Small `where`/`deps` results target less than 2 KB on the tolmap self-map.
Complete overviews and larger dependency boundaries can exceed that target. The limit
bounds rows rather than bytes; very long paths, names or coverage reason maps
can exceed that target. `total` remains complete when rows are omitted. Text
is intended for people and is not a stable parsing contract.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Valid query, including an unknown file/symbol or no route: empty results, total 0, truncated false. Ambiguous symbols also succeed and report candidates. |
| 2 | Usage or input error: invalid flag/value, missing/unreadable/invalid map, multiple/no maps in a directory, invalid map indices, or invalid symbols when needed. |
| 3 | Internal serialization/output error, including a failed stdout write. |

On errors stdout contains no response (an output failure can leave a partial
write), and stderr explains the error. No advisory structural result causes a
failure exit. Exact queries are deterministic for identical stored files and
flags. No viewer baseline or extraction fixture is changed by these commands.

## Shared personal CLI

```sh
tolmap add /path/to/checkout --name engine --all-sources
tolmap add owner/name
tolmap add --scan /path/to/projects --all-sources
tolmap repos
tolmap status --repo engine
tolmap refresh engine
tolmap refresh --repo engine --commit <available-commit>
tolmap overview --repo engine
tolmap check --repo engine --report-only
tolmap debt --repo engine --format json
tolmap rm engine
```

`--data ROOT` is global; otherwise `TOLMAP_DATA_DIR`, application config, then
XDG/HOME defaults choose the shared root. Local identity is canonical checkout
root; identical origins never merge checkouts. Basename collisions receive a
path hash suffix. GitHub URL/name spellings deduplicate case-insensitively.
GitHub HEAD lookup, clone and fetch each try anonymous access first with
helpers/prompts disabled. An authentication/not-found refusal retries with your
existing Git credential helper, then `gh auth git-credential` if that helper
has no credential. Configure a helper externally or run `gh auth login` before
starting tolmap; no sign-in UI or interactive prompt is opened. Providers must
work noninteractively in the session running tolmap (including any keyring
session); each provider is limited to 15 seconds. `GH_TOKEN`, `GITHUB_TOKEN`
and `GH_HOST` are forwarded to authenticated Git and its helper bridge, then
only to the gh provider. Anonymous Git, configured Git helpers and indexing
workers do not receive those variables. Empty and relative PATH entries are
removed before starting authenticated Git or a credential provider. No
credential response is written to disk.

Credentials travel only through the helper protocol, never source URLs, command
arguments, application config, logs, jobs or database rows. tolmap does not call
helpers to store or erase credentials and does not modify global Git config.
Network/provider diagnostics are replaced with fixed errors before persistence.
Missing or revoked credentials leave prior maps/source available. Failed adds
remain registered and appear in `repos`. Retained clones are full clones: no
lazy network reads are needed when indexing or opening retained source.

`tolmap web` uses credentials available to the process that started it. Binding
outside loopback exposes private source and credential-backed repository
management to anyone who can reach the listener; there is no authentication.

A moved checkout can be relinked with `add NEW_PATH --name EXISTING` only when
the old path is missing and the new checkout contains its last indexed commit.
The registration ID and history survive. Otherwise the command names both paths
and fails. `rm` removes tolmap-owned history/source, never the original checkout.
Refresh reuses a complete snapshot for the same commit and source selection;
changed source selections may retain repeated commits. Source objects are independent of the original checkout and clone cache.
No existing stores are discovered or imported.

Refresh excludes dirty and untracked content; check without `--head` compares
against the local working tree including those changes. In database mode,
`--commit` selects check's base snapshot and `--base`, if given, must agree.
Remote checks require an explicit `--head` available in retained source.
Explicit `check PATH --base REV [--base-map FILE]` stays available (`--map`
is an alias for `--base-map`). Query/check/debt exit contracts and thresholds
remain unchanged. Source-oriented `build`, `detect` and lens commands also
accept repository/commit/snapshot selection; explicit graph/path inputs stay
available. `tolmap web` uses the same library.

Organization members without manifest paths or `--clones` resolve their qualified
registered names at the manifest's exact pinned commit. Explicit member paths,
clone roots and stored-map roots keep precedence; pin ambiguity fails.
