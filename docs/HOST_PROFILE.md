# Shared viewer host composition v1

The same production Home, indexing progress, map and CM6 source reader can be
served by a local service or an authenticated host. A host may insert a JSON
script with `id="tolmap-host"`, `type="application/json"` and this shape before
the module script:

```json
{"version":1,"private":true,"workspace":"Personal workspace","repositories":[{"value":"opaque-repository-id","label":"owner/repository"}],"actions":[{"label":"Connections and history","path":"/workspace"},{"label":"Sign out","path":"/logout","method":"post"}]}
```

Serialize it safely for HTML (escape `<` as `\u003c`). The profile is Rust-defined
in `src/catalogue.rs` and generated into `bindings/`. Hosted Home shows the
repository list and shared app shell, without the public repository form. POST `/api/index`
still returns the public `IndexResponse`; progress uses the generated
`JobSnapshot` contract and real worker stage events. Unknown counts and ETA stay
null. Completion may change the job's slug to an immutable snapshot route.

Private profiles disable bundled catalogue/map fallback and public issue links
that would include private repository or error context. No profile means the
existing OSS behavior. The profile supplies presentation, never authority:
the hosting server must validate sessions, grants, provider permission, immutable
revision, cache hits, streams, source and downloads independently.

The map fills the area below the shared hosted shell, with existing navigation and camera,
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

## Repository onboarding and batches (#297)

`HostProfile.onboarding` is optional; omit it to advertise no add capability.
`repositories` remains supported for existing profile producers, but is no longer
an input picker. Account `actions` populate the shell's ⋯ menu; labels and paths
are host-owned, and `method:"post"` submits a same-origin form.

Example onboarding capability (all paths here are illustrative, not core routes):

```json
{"offered_repositories_path":"/host/offered","batch_index_path":"/host/index-batch","batch_progress_path":"/host/batches/{batch_id}","open_sheet_parameter":"add","install":null,"manage_access":{"label":"Manage GitHub access","path":"/host/github/configure"},"sign_in":{"label":"Sign in","path":"/login"},"authorize":{"label":"Reconnect GitHub","path":"/host/github/start","method":"post"}}
```

- `GET offered_repositories_path` returns `OfferedRepository[]`, each with
  `provider_id` (opaque string), `full_name`, `visibility` (`public` or `private`),
  and nullable `connected_repository_id`. Connected rows are marked Mapped and
  cannot be selected. The host returns the complete offered list.
- `POST batch_index_path` accepts `BatchIndexRequest`: `{"provider_ids":["101","102"]}`.
  It validates provider access, connects the selection and starts indexing, returning
  `BatchIndexResponse`: `{"batch_id":"batch-1","jobs":[{"repository_id":"repo-1","job_id":"job-1"},{"repository_id":"repo-2","job_id":"job-2"}]}`.
  A successful response has at least one job. Errors must not silently drop selected
  repositories; the host owns transactional/idempotency policy.
- `GET batch_progress_path`, replacing its single `{batch_id}` placeholder with
  the encoded id, returns `BatchProgress`: `{"batch_id":"batch-1","jobs":[...]}`
  using the existing generated `JobSnapshot` shape. The host persists membership,
  authorizes every read and retains progress across browser reloads. Core polls
  every two seconds until all jobs are done or failed.
- `install` is a nullable `HostAction`: when present the empty workspace offers
  “Connect GitHub”; otherwise it offers “Add repository”. `manage_access` supplies
  the quiet provider access link, including POST form semantics when required.
- The provider callback returns to Home with `?<open_sheet_parameter>=1` (for
  example `/?add=1`). Opening the sheet always refreshes offered repositories;
  closing removes the marker. Core does not embed a provider callback route.

Endpoints are same-origin absolute paths and JSON. A 401/403 produces expired-session
copy and the optional `sign_in` HostAction as the primary recovery action. A 409
with a JSON error body (for example `{"error":"provider_authorization_needed"}`)
means provider authorization is missing or expired: core shows “Reconnect GitHub
to see your repositories.” and the optional `authorize` HostAction. Both actions
support `method:"post"` form submission; without the relevant action the sheet
shows the state’s copy without a recovery button. Other errors retain
provider-unavailable copy with Retry. Sessions and provider permission remain
server authority. Single jobs use the existing `/new` flow;
multiple jobs use the core route `/new/batches/<batch-id>`. No capability means
no sheet, and `tolmap serve` needs no changes to implement this optional contract.
