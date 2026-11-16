# Personal library API

`tolmap web` is the only HTTP entry point. CLI examples and resolution rules are in [QUERY.md](QUERY.md); setup is in [LOCAL.md](LOCAL.md).

## Personal shared store and application configuration

The CLI and browser share `service::store::Store`: a repository registry, durable attempts and UUID snapshots. History is retained until removal or an explicit retention setting. Existing data is never discovered or imported; re-add repositories to start new history. Files outside the selected personal root are left alone.

The shared CLI/web loader reads a flat TOML table at
`$XDG_CONFIG_HOME/tolmap/config.toml` or `$HOME/.config/tolmap/config.toml`:

```toml
data_dir = "/private/tolmap"
# db_path = "/private/tolmap/tolmap.sqlite3"
# cache_dir = "/private/tolmap/cache"
bind_addr = "127.0.0.1"
port = 8787
refs = "hand"
prune_variant = "node-relative"
namer = "idf"
scip_install = "off"
max_concurrent_jobs = 1
max_queued_jobs = 16
clone_cache_bytes = 2147483648
```

Settings use the service vocabulary, with lowercase keys corresponding to
`TOLMAP_*` variables, including `static_dir`, `namer_model`
and `retain_commits_per_repo`. Credentials do not belong in this file. Precedence
is flag > environment > application file > default. A higher-priority root
beats lower-priority `db_path`/`cache_dir`; at equal priority individual paths
win. Personal history has no default retention limit.
Settings are read at invocation/startup. Repository intent remains in
`.tolmap/config.toml`; it cannot relocate the application database.

## Personal browser API (`tolmap web`)

`tolmap web [--addr IP:PORT] [--data DIR]` uses the same application loader,
registry and retained snapshots as the CLI. It binds to `127.0.0.1:8787` by
default. `bind_addr` / `TOLMAP_BIND_ADDR` accepts an IP or `IP:PORT`; a separate configured `port` overrides its port, and `--addr` overrides both. There is no authentication.
Loopback requests require `Host: localhost:PORT`, `127.0.0.1:PORT` or
`[::1]:PORT`; other hosts are rejected. Mutations require
`Content-Type: application/json` to reject cross-site simple requests.
An off-loopback bind prints once at startup:

```text
No authentication: anyone who can reach this address can read source and manage repositories.
```

Set `static_dir` / `TOLMAP_STATIC_DIR` to an existing production `web/dist`
bundle (the default is `web/dist` relative to the launch directory). A missing
bundle is a startup error. Web reuses the service's API-safe SPA/static handler
and the existing React host composition; public admission/catalogue handlers
are removed.
No old stores are discovered or imported. GitHub acquisition uses the anonymous-first
git/gh credential-helper flow described in [QUERY.md](QUERY.md); helpers never
prompt, and credentials are never stored in jobs or the database. Off-loopback
access includes private source and credential-backed management.

| Method and route | Personal behavior |
| --- | --- |
| `GET /api/maps` | One `CatalogueEntry` per registration, including repositories without snapshots; all repositories, newest indexed activity first. |
| `GET /api/repositories/{id}/history` | Shared `RepositoryHistory`, newest snapshots first, with open/refresh/remove actions. |
| `POST /api/repositories/{id}/refresh` | Refresh committed HEAD through the same library as CLI; returns `202` with `job_id`. |
| `DELETE /api/repositories/{id}` | Remove registration and tolmap-owned history/source; preserve the original checkout. Busy repositories must finish/cancel first. |
| `GET /api/personal/scan?path=...` | Sorted checkout roots without following symlinks or entering discovered checkouts, using `OfferedRepository` rows and registered markers. Local rows do not display provider visibility. |
| `POST /api/personal/index` | Existing `BatchIndexRequest` (`provider_ids` are local paths or GitHub source strings); returns `BatchIndexResponse`. Sources register before jobs start. |
| `GET /api/personal/batches/{id}` | Persisted `BatchProgress`; reloadable batch URL `/new/batches/{id}`. |
| `GET /api/jobs/{id}` | Re-read durable `JobSnapshot`, including worker phase/progress and terminal errors. |
| `POST /api/jobs/{id}/cancel` | Cancel queued work or stop its isolated indexer; completed maps remain available. |
| `POST /api/jobs/{id}/retry` | Start a fresh attempt for that job's registered repository, returning a one-job batch. |
| `GET /api/maps/snapshot/{snapshot_id}` | Exact saved map bytes and `x-tolmap-commit`. Browser URL is `/snapshot/{snapshot_id}`. |
| `GET /api/maps/personal/{repository_id}` | Latest complete map, optionally selecting a retained `commit`. History always links exact snapshot IDs. |

Both map paths support `/symbols?district=N`, `/provenance`, `/source?commit=SHA&path=...`,
`/freshness`, `/debt`, `/lenses` and `/lenses/{name}` using retained artifacts/source.
A snapshot URL never substitutes a different snapshot at the same commit. Source
requires the snapshot's immutable commit and uses the existing bounded Git-blob
reader. Missing source remains explicit. Comparison/organization APIs and individual
snapshot deletion are not advertised by the personal adapter .

Home/history poll every two seconds during indexing or queueing, otherwise every
30 seconds. Active job views poll every two seconds while visible.
A CLI refresh can run while web serves the last complete snapshot; it appears
without restarting web. Web jobs have a bounded in-process FIFO and acquire the
same cross-process repository/slot locks as CLI refresh. Competing CLI writes
return busy rather than being silently queued. Web process interruption leaves
saved maps intact; reopening an interrupted web job explains how to retry.
Cancellation during source acquisition takes effect before indexing starts;
worker cancellation kills the current worker process group.


## Isolated indexing

Each job runs `tolmap worker` with a JSON spec on stdin and versioned progress events on stdout. The executor pins committed source, isolates child processes, supports cancellation and validates artifacts before publication. SCIP dependency installation keeps the sandbox in [SCIP_SANDBOX.md](SCIP_SANDBOX.md). There is no network worker listener, worker token or public admission API.

`GET /api/healthz` returns `{"ok":true}`. Unmatched API paths return JSON errors and never the SPA fallback. Static assets support gzip. Organization and comparison HTTP endpoints are retired; use `tolmap org check/build` and `tolmap check --repo NAME`.

### `tolmap lens suggest --map <map.json> [--out <dir>]`

Starting lenses proposed from a stored map's landmarks and edges (issue #249;
`src/lens_suggest.rs`), for a reader who does not yet know which paths matter.
They are ordinary lens files (format v1) to review, edit and commit to
`.tolmap/lenses/`; the viewer lists a committed one in its Lenses picker. Up to
three files, each `<name>.toml`:

- `entry-routes`: from each `entry` landmark, a walk of at most 4 hops through
  the highest-fan-in files it imports, directly or not, ending in another
  district where one is reachable; a note on each entry;
- `district-tour`: one area per district (largest first), with its `capital`
  landmark as a note anchor;
- `crossings`: each `bridge` landmark between its highest-fan-in importer and
  import (other districts preferred), and for each of the 3 heaviest
  cross-district roads the file edge whose ends carry the most fan-in.

Every hop is `computed`, never `declared` (suggest cannot know a reason), and
each stop is reachable from the previous one by imports, so `tolmap lens check`
resolves every suggestion with no unresolved item and every hop as `imports`.
Routes are lower bounds over the map's edges. Each file starts with a comment
header saying it was generated, from which map (its file name and SHA-256) and
that its routes are lower bounds; titles say "generated". Nothing is
randomised: ties break by fan-in, then path, and the same map gives
byte-identical files. With `--out` the files are written there (the directory
is created); without it they are printed to stdout and nothing is written, so
suggest never writes into an analysed repository unasked. A lens with nothing
to suggest is not written, and a message on stderr says why (a map with no
`entry` landmark: `no entry landmark in the map: entry-routes not written`).
Exit 0, or 2 when the map cannot be read or a file cannot be written.
`data/lenses/flask-suggest/` holds the suggestions for the pinned flask fixture.

### `tolmap debt --map <map.json> [--format text|json|lens] [--kind <kind>...] [--path <prefix-or-glob>] [--district <name>] [--top <n>] [--cycle-evidence compact|full]`

`--kind` is repeatable and accepts `cycle`, `misplaced`, `god`, `hotspot`,
`unreached`, `undeclared-rust` or `hidden-coupling` (the CLI spellings of the
finding kinds shown in the report).
`--path` matches a path prefix unless it contains `*` or `?`, when it is a glob.
`--district` matches the district name. `--top` limits ranked findings after
other filters (cycles, god files and hotspots). Cycle order is unchanged:
cross-package cycles first, then deterministic route order. Filters apply to
text, JSON and lens TOML;
all outputs identify the selections (lens TOML uses a leading comment, which is
valid TOML and ignored by `tolmap lens check`). Filtered JSON keeps the normal
DebtReport version and lower-bound evidence, with additive `unfiltered_counts`,
`filtered_counts` and `filters` fields. DebtReport is now v7: `cycle_evidence` is
`compact` by default for CLI text/JSON. Compact cycles retain member districts,
`cross_edges`, one `route` and its `closing_edges`, package names and measurement
flags. Their `package_edges`, `mutual_pairs` and `cut` arrays are empty;
`cycle_evidence` distinguishes omitted evidence from measured empty bundles.
`--cycle-evidence full` restores those bundles. Lens output always uses full
evidence because its areas and notes depend on bundles.

`tolmap debt diff <before-report.json> <after-report.json>` compares two unfiltered,
versioned DebtReports. It emits version 2 JSON with each finding marked `added`,
`removed` or `shared`; shared findings include numeric metric deltas. Finding
identity is its kind and path (or stable cycle/pair identity), so changing churn
or complexity on an unchanged finding remains shared. Reports must have the
current DebtReport version. Cycles are compared using their compact summary,
even when either input has full evidence; bundle-only changes are not reported.
Cycle identity uses the closing route's file paths, so compact and full inputs
for the same map compare equally. Filtered reports (including `--top`) are
rejected. This subcommand is used instead of a map comparison
because the requested input is a raw, reproducible comparison of two already
computed reports.

Structural debt findings (issue #250; `src/debt.rs`), computed offline from a
stored map alone; the map is not changed and the same map gives byte-identical
output. `json` prints a `DebtReport` (`bindings/DebtReport.ts`): district
cycles (strongly connected district sets in the report-only district graph built
from `E`, each with one shortest route and the file edges that close it), their
mutual district pairs (the lighter direction) and a deterministic greedy
feedback-arc **small cut** (never claimed to be minimum). Before this report
graph is searched, each `D.generated` binding with a `generator` is collapsed
onto that generator; a resulting same-file or same-district edge is dropped.
`DebtEdge.via_generated` retains the binding's `F` index whenever an edge used
that collapse; when both ends are collapsed bindings, only the source binding is
recorded. Text and debt-lens output show its binding path. Bindings without a
generator, and maps without `D.generated`, retain their existing
behavior. This changes only the debt report: `E`, blending, clustering, layout,
and map JSON are unchanged. When `D.package_roots` is present, the report
projects the file edges of all of the cycle's collapsed cross-district bundles,
not only the cut or the mutual pairs, onto their nearest package roots. It calls
a cycle `crosses_package` only when those projected package edges form a cycle;
otherwise it is `within package`, a
clustering cycle that still lists its package edges. Maps without that sidecar
report package scope as unmeasured. The report also includes misplaced files,
god files, hotspots (churn × complexity; the top two include the map's
`hazard` landmarks), files with **no inbound edge found**, which excludes
entry files, and **hidden coupling**: a pair retained in the optional,
index-aligned map sidecar with at least three co-changes and weight ≥ 0.35 but
no file edge `E`, reference edge `U`, or declared Rust `mod` parent/child
pair. The sidecar records pair indices,
co-change counts and weights rounded to two decimals from the indexer's 4,000
non-merge-commit history window. It is absent from older maps; those reports
state `hidden_coupling_measured: false` (and text output says "hidden coupling
not measured"), never that no pair was found. Rust `mod` declaration pairs and
crate roots use the same optional, index-aligned sidecar: a declared `.rs`
module is not reported as no-inbound, while a non-target Rust source file no
root reaches is an **undeclared Rust file** (`no \`mod\` declares it; likely not
compiled`). `#[path]` targets are retained; macro-generated declarations may
be missed, so this too is a lower bound. Older maps report
`rust_mod_reachability_measured: false`, never an empty Rust result. Statically
readable build configurations retain literal bundler entries in the same sidecar;
older or dynamically configured maps report that entry evidence as unmeasured.
Test paths use the no-inbound test rule and are excluded from production hotspot
ranking, with the count reported. Files with no edge at all are coverage gaps:
they are counted in `isolated_files`, not listed. Every report
states its thresholds (named constants in `src/debt.rs`, plus the cuts they
give on this map) and echoes the map's `coverage`; every count is a lower
bound over the map's edges and history. Dynamic imports, reflection and
declared connections are invisible to the map, so a file with no inbound edge
found is not shown to be dead.

`lens` prints the equivalent lens TOML (format v1), which `tolmap lens check`
resolves fully: at most one area per finding kind, labelled by the kind alone
(`district cycle`, `misplaced file`, `god file`, `hotspot`, `no inbound edge
found`, `undeclared Rust file`, `hidden coupling`) and holding every file in a
finding of that kind, so a district is tinted at most once per kind. It emits
one contextual path per cycle and computed paths for the first 20 heuristic
cut bundles, named from file paths rather than district names; a note states
how many cut bundles the full report retains. It also emits declared paths for
the first 20 hidden-coupling pairs in report order (cross-district first, then
weight). The hidden-coupling area still contains every pair's files, while a note states how
many of the full `tolmap debt` report are shown as paths; each path's reason is
`co-changed N times; no static edge found`. Notes carry the measured values,
the misplaced file's per-district edge counts and the cycle's district names.
Labels and path names carry no measured value and no district name, so lens
comparison does not read a metric change or a renamed district as a removed
and an added finding. Exit 0, or 2 when the map cannot be read.

### `tolmap org check <name.org.toml>` and `tolmap org build <name.org.toml> [--clones <dir>] [--maps <dir>] [--out <file>]`

Organization maps (issue #255; `src/org.rs`), CLI only. The organization
manifest is passed to the CLI and never written into a member repository.
Schema (`bindings/OrganizationManifest.ts`):

```toml
version = 1
name = "semon"                       # [a-z0-9][a-z0-9._-]{0,63}

[[members]]
repo = "onsager-ai/semon"            # owner/repo, listed once
commit = "<full 40-character sha>"   # always required

[[members]]
repo = "onsager-ai/duhem"
commit = "<full 40-character sha>"
path = "../duhem"                    # optional local clone, relative to the manifest
```

`org check` exits 0 when the manifest is valid and 2 otherwise, with a distinct
error for an unknown key, a duplicate member (case-insensitive), an abbreviated
commit, a non-hex commit, a repository that is not `owner/repo`, and an empty
or absolute `path`.

`org build` reads no network. Each member's source is its commit checked out
into a detached temporary worktree of its local clone (`path`, else
`<clones>/<owner>/<repo>`), never a working tree; the build fails with exit 2
when the clone or the commit is missing. A member's map is the stored
`<maps>/<owner>/<repo>/<commit>.json` when present, otherwise it is built from
the worktree as `tolmap build` builds it with detection, the offline IDF namer
and SEED 7. Member maps are only read or built: never changed, re-clustered or
laid out again. The `OrganizationDocument` (`bindings/OrganizationDocument.ts`)
is pretty JSON on stdout or `--out`, byte-identical for the same manifest and
member commits (members sorted by repository, every list sorted):

- `members`: repository, commit, `path` as written in the manifest, `map`
  (`stored` or `built`), `map_digest` (`sha256:` of the map bytes), file and
  district counts from the member map, and the packages the member provides.
  No modularity is computed or combined across repositories; a member's own
  `q` stays in its map.
- `edges`: consumer → provider repository, each with its evidence: the
  consumer's package, manifest file and section, the provider package, the pin
  (`commit`, `tag`, `branch`, `ref`, `version`, `path`, `workspace` or `none`)
  and its status: `match`, `skew`, or `unjudged` with a reason (a branch, a
  path, a range this reader does not evaluate).
- `pin_skew`: every dependency pinning a different commit (a git rev, a tag or
  a Go pseudo-version naming another commit) or a version requirement that
  excludes the provider's version at the member commit.
- `external`: declared dependencies no member provides, listed, never dropped.
- `unresolved`: manifests or declarations that cannot be read, names provided
  by more than one member, and git dependencies on a member that provides no
  such package.

Dependencies are read from Cargo (`[dependencies]`, dev, build and `target`
tables, workspace inheritance; git, path and registry by crate name), npm/pnpm
`package.json` (dependencies, dev, peer, optional; `workspace:`, `file:`, git
and `npm:` aliases by package name), `go.mod` (`require` and `replace`, by
module path) and Python (`[project]` dependencies and extras, dependency
groups, Poetry tables, `[tool.uv.sources]` and `requirements*.txt`, by PEP 503
name). A git dependency resolves by repository first. Dependencies satisfied
inside the consumer's own repository are not edges. Every count is a lower
bound: `setup.py` `install_requires` is not evaluated, and HTTP calls, queues
and copied code link repositories without any declaration. Exit 0, 2 on an
invalid manifest or missing clone, commit or map input, 3 on an internal error.
