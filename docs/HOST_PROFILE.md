# Shared viewer host composition v1

The same production Home, indexing progress, map and CM6 source reader can be
served by a local service or an authenticated host. A host may insert a JSON
script with `id="tolmap-host"`, `type="application/json"` and this shape before
the module script:

```json
{"version":1,"private":true,"workspace":"Personal workspace","repositories":[{"value":"opaque-repository-id","label":"owner/repository"}],"actions":[{"label":"Connections and history","path":"/workspace"},{"label":"Sign out","path":"/logout","method":"post"}]}
```

Serialize it safely for HTML (escape `<` as `\u003c`). It selects authorized
repository options instead of the public repository input. POST `/api/index`
still returns the public `IndexResponse`; progress uses the generated
`JobSnapshot` contract and real worker stage events. Unknown counts and ETA stay
null. Completion may change the job's slug to an immutable snapshot route.

Private profiles disable bundled catalogue/map fallback and public issue links
that would include private repository or error context. No profile means the
existing OSS behavior. The profile supplies presentation, never authority:
the hosting server must validate sessions, grants, provider permission, immutable
revision, cache hits, streams, source and downloads independently.

The map remains full viewport, with the existing Home navigation and camera,
selection, source-reader and visibility-gate behavior. There is no second
reader or map implementation in the hosting application.

## Repository catalogue and history

`src/catalogue.rs` defines `CatalogueEntry`, `RepositoryHistory` and their action,
status and snapshot types. `generate-types` exports the TypeScript bindings;
web code imports those bindings rather than defining another wire schema.

`GET /api/maps` keeps the existing fields and adds optional `repository_id`
(stable, opaque), `latest_snapshot_id`, `indexed_at`, `snapshot_count`, and
`status`. Status is `{kind:"indexing",phase,job_id}`, `{kind:"failed",message}`
or `{kind:"reconnect_needed"}`. Missing status means no reported connection/job
state. Hosts return all connected repositories, once each, including repositories
without snapshots; core also collapses entries sharing a stable identity and
orders them by indexed activity. Catalogue `owner`/`repo` identify the latest
map's existing route, which may differ from the display name. `latest_snapshot_id`
is absent when no map is available. Entries without `repository_id` retain the
public catalogue's approved layout/navigation.

`GET /api/repositories/<encodeURIComponent(repository_id)>/history` returns:

```json
{"repository_id":"repo-1","name":"acme/platform","status":null,"current_core_revision":"abc123","actions":{"reopen":{"path":"/snapshot/map-1","method":"GET"},"reindex":{"path":"/host/repositories/repo-1/reindex","method":"POST"},"disconnect":null,"delete_repository":null},"snapshots":[{"id":"map-1","commit":"0123456789abcdef","indexed_at":"2026-11-11T08:00:00Z","files":120,"districts":8,"core_revision":"abc123","reopen":{"path":"/snapshot/map-1","method":"GET"},"delete_snapshot":null}]}
```

Snapshots are newest first. `core_revision` and `current_core_revision` are null
when unknown; the UI labels differing known revisions. Optional actions are null
when unavailable. Reopen uses GET; mutation actions use the host's advertised
POST or DELETE. All paths must be same-origin absolute paths. Core does not
construct Hub mutation routes. Re-index returns the existing `IndexResponse`;
accepted jobs show progress inline via the existing job endpoints, and a cached
result opens its returned map. Mutation failures stay visible and retain history;
success invalidates catalogue/history, so deleting the latest exposes its predecessor.
The server owns principal checks, CSRF protection, typed-confirmation enforcement
if required, auditing and artifact purge. Capabilities only govern presentation.

`tolmap serve` exposes retained commit history at the same endpoint, with the
lowercase `owner/repo` slug as identity (URL-encoded into one segment). It advertises
reopen only, and unknown core provenance stays null. Its public catalogue omits
`repository_id` to preserve existing direct-map navigation. A repository with only
one retained map returns a one-item history. Native local history and same-commit
reuse are deferred: existing local metadata does not persist core revision or
analysis scope, and changing the approved native template requires owner review.
