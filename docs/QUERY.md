# Compact map queries

`tolmap overview`, `where`, `deps` and `route` answer small structural questions
from an existing local or CI-built map. They do not index, refresh, read a
checkout, access the hub or make model calls. Issue #303 defines this slice.

```sh
tolmap overview --map out
tolmap where src/main.rs --map data/tolmap.json
tolmap where 'src/bin/generate_types.rs::main' --map data/tolmap.json
tolmap deps src/main.rs --direction both --map data/tolmap.json
tolmap route src/main.rs src/schema.rs --map data/tolmap.json
```

## Inputs and flags

All commands require `--map <file-or-directory>` and accept `--format json|text`
(default `json`) and `--limit N` (default 8; zero requests totals alone). A map
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
the caller.

Only a symbol lookup reads the map's `<stem>.symbols.json` sibling, when present.
A missing sibling uses the older compact map `S` table, which has no parent
hierarchy; only simple names and `path::name` work there. A malformed or
unreadable sibling is an input error rather than silent fallback. The sibling
must belong to the same build as the map; legacy files contain no shared
identity with which to verify this association.

`deps` defaults to `--direction both`; `in` lists importers and `out` lists
imports, using map `E` edges only. A reciprocal relationship yields two rows,
one per direction. Duplicate edges yield one row per path and direction.
This is direct adjacency, not a transitive blast radius or symbol references.

## Versioned JSON

The Rust definitions in `src/query_contract.rs` are the source of truth;
`generate-types` exports their TypeScript bindings. Every successful response
uses the same envelope, also available as generic `QueryEnvelope<T>` for future
snapshot queries:

```json
{"schema":"tolmap.query/v1","query":{"kind":"where","target":"src/main.rs","ambiguous":false},"map":{"repo":"tolmap","commit":null,"sha256":"<64 lowercase hex digits>"},"coverage":{"refs":"hand","zero_edge_files":null,"calls_total":null,"calls_resolved":null,"unresolved":null,"lower_bound":true},"limit":8,"total":1,"truncated":false,"results":[{"kind":"file","path":"src/main.rs","district":"<district name>"}]}
```

The example's angle-bracket values are placeholders. JSON is compact UTF-8 with
one trailing newline. Paths and district names replace all internal indices.

| Field | Meaning |
|---|---|
| `schema` | `tolmap.query/v1`. Breaking field or semantic changes require a new version. Consumers must tolerate additive fields and query/result kinds. |
| `query` | Tagged by `kind`: `where`, `deps`, `overview`, or `route`; includes inputs and the command-specific facts below. |
| `map.repo` | Repository label recorded by the map, not a recovered remote URL. |
| `map.commit` | Stored top-level `commit`, otherwise a full 40/64 hex object ID from the map filename; otherwise `null`. Ordinary `build` and legacy fixtures do not record it. Never inferred from the caller's checkout. Renaming a file is not provenance verification. |
| `map.sha256` | SHA-256 of the exact map file bytes, including whitespace. Symbols bytes are not included. |
| `coverage.refs` | `hand` when reference coverage is absent or all recorded languages use hand; `scip` if all recorded languages use SCIP; `mixed` when both occur. |
| `coverage.zero_edge_files` | Map's recorded kept-edge coverage count; `null` when absent. |
| `coverage.calls_total`, `calls_resolved`, `unresolved` | Counts from the full symbols sibling, only when loaded. `unresolved` is a map of reason to count. `null` means unknown/unread, never zero. Compact `S` does not measure them. |
| `coverage.lower_bound` | Always `true`: unresolved/dynamic references can be absent. |
| `limit` | Requested maximum result rows. |
| `total` | All matching rows before limiting; route counts file steps including endpoints. |
| `truncated` | Exactly `total > limit`. A truncated route is a prefix, not a complete connection to the destination. |
| `results` | Ordered rows, length `min(limit,total)`. |

## Query and result shapes

- **where**: `query` has `target` and `ambiguous`. Rows are
  `{"kind":"file","path":...,"district":...}` with optional
  `symbol: {"name":...,"qualified_name":...,"start":...,"end":...}`.
  Line ranges are the stored one-based inclusive ranges. Symbol candidates sort
  by path, start line, qualified name, then end line.
- **deps**: `query` has `path` and `direction` (`in`, `out`, `both`). Rows are
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

Typical default results target less than 2 KB on the tolmap self-map. The limit
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
