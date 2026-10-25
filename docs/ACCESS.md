# Embedded resource policy v1

`access::{Policy, Request, Resource, Operation, enforce}` is an opt-in contract
for trusted embedding adapters. Context is deliberately not deserializable: the
adapter authenticates the principal and resolves ownership from trusted storage
before constructing a request. A supplied ID or header proves no authority.

Check before admission, source materialization, every status/event delivery,
cancel, provenance/map/symbol/comparison read, artifact download, cache
lookup/delivery and atomic result publication. Read covers provenance, map,
symbol and comparison resources; Source gates Git blob materialization and
delivery; Artifact covers exports. Resource identity includes the access domain,
stable repository and immutable revision. Domain must be part of private cache
keys, including previous-map/naming inputs. No reusable positive decision cache
is provided. Revocation is checked on each call; stream adapters own a documented
poll bound and must close on denial. Already-delivered bytes cannot be recalled.

This is an opt-in library boundary, not authentication middleware. Standalone
`tolmap serve` remains one trust domain. Its public catalogue/cache and loopback-
first API are not a protected multi-tenant backend. The policy does not retrofit
protection onto the router. A consumer must authenticate and authorize its own
routes, bind resource identity from trusted state, and apply checks before
storage/cache access and each delivery checkpoint.

## Core handler inventory

The standalone routes are registered by `service::http::router` in
`src/service/http.rs`. This is the current handler inventory; the route router
does not accept a `Policy` and none of these handlers calls `access::enforce`.

| Surface | Current Core route / handler | Current behavior and v1 consumer duty |
| --- | --- | --- |
| Job admission / warm index hit | `POST /api/index` → `service::http::post_index` | Resolves repository/HEAD, checks the public store for an existing commit, then returns cached completion or calls `jobs::spawn_job`. No `Admit` hook is wired; an embedding adapter must check before source acquisition, cache lookup or enqueue. |
| Job status | `GET /api/jobs/{job_id}` → `service::http::get_job` | Reads the in-memory or persisted snapshot. No per-job `Read` hook; the consumer must authorize guessed foreign job IDs before lookup/delivery. |
| Job cancellation | `POST /api/jobs/{job_id}/cancel` → `service::http::post_cancel_job` | Resolves the job slug and calls `jobs::cancel_job`. No `Cancel` hook; the consumer must resolve and authorize job ownership before mutation. |
| SSE | `GET /api/jobs/{job_id}/events` → `service::http::get_job_events` | Opens a snapshot stream. No `Events` hook, event checkpoint or live-policy monitor is wired; the consumer must authorize open and each event and close idle streams within its documented revoke bound. |
| Public catalogue | `GET /api/maps` → `service::http::get_maps` | Lists latest public entries. This is not a private catalogue and has no tenant filter. |
| Map / early map | `GET /api/maps/{owner}/{repo}` → `service::http::get_map` | Calls `Store::get`/`Store::latest`, may return the active job's early map, then reads the stored map. No `Read` hook; a protected adapter must check before every lookup and delivery. |
| Symbols | `GET /api/maps/{owner}/{repo}/symbols` → `service::http::get_symbols` | Looks up a map and reads a district symbol file. No `Read` hook; consumer responsibility / contract-only (Hub evidence: tolmap-hub PR13). |
| Hosted source / provenance | No route in `service::http::router` | `source::read` is a library function, not a hosted route. Unmatched `/api/*` paths return JSON 404 (`tests/static_serving.rs::static_dir_set_api_404_is_never_html`); no arbitrary path reaches a file handler. There is no standalone hosted source/provenance handler or `Source` hook. Consumer responsibility / contract-only (Hub evidence: tolmap-hub PR13). |
| Artifact / download | No standalone hosted artifact route | `local::dispatch` serves `/api/maps/snapshot/{id}/artifact` as a map-only local export. There is no hosted `Artifact` hook or storage adapter. Consumer responsibility / contract-only (Hub evidence: tolmap-hub PR13). |
| Comparison | No comparison route or HTTP handler in `service::http::router` | `Read`, `Artifact` and `Publish` are contract operations for a future/embedding comparison flow. Consumer responsibility / contract-only (Hub evidence: tolmap-hub PR13). |
| Local snapshot map/source/symbols | `local::handle` → `local::dispatch` in `src/local.rs` | `/api/maps/snapshot/{id}`, `/provenance`, `/source`, `/symbols?district=…`, and `/artifact` resolve through a saved local snapshot. This is the owner-local single-user service, not a protected tenant route. Source paths are Git-relative and bounded by `source::read`; the request cannot name an arbitrary host file. |
| Result publication | No HTTP route; library boundary is `service::worker_result::adopt` | Embedders check `Publish` before atomic adoption. The standalone service does not call the opt-in policy. |

`service::http::post_index` also returns an existing store entry without
creating a job. `get_map` consults the store and may read an early map; these are
public-service cache/storage reads, not private caches. Any consumer-owned cache
must include access domain, repository, immutable revision and all comparison
inputs in its key, and must recheck current authority before a warm hit is
delivered.

The local reader route and source contract are separate from the standalone
hosted router. `local::dispatch` serves the persisted local snapshot; its local
pages check Host/Origin for mutations, use prepared IDs instead of arbitrary
checkout paths, and keep checkout/storage disclosure on the owner-local page.
This makes the local single-user flow usable without a login or provider upload;
it does not qualify hosted session, database or object-storage isolation.

## Library conformance and limits

`tests/access.rs` exercises `enforce` directly. Its policy fixture grants one
operation at a time and denies mismatched operations, principal, access domain,
repository, guessed immutable revision, revoked authority and unavailable
policy before its cache closure can run. Its lifecycle case rechecks `Source`,
`Events`, `Publish` and `Artifact` after revocation. It does not call HTTP
handlers, parse spoofed headers, authorize job IDs, monitor a live process, or
prove hosted route coverage. `Request` and `Resource` have no `Deserialize`
implementation; the fixture does not claim to test an adapter's header parser.

The existing `service::executor::{execute, EventSink, CancelProbe}` and
versioned `worker::{JobSpec, WorkerSpec, WorkerEvent}` remain the processing
boundary. An embedding adapter checks before execute, monitors live authority
independently while it blocks, kills a cancelled child process group through
CancelProbe, and checks again before publication. Worker leases authenticate
computation, not the source principal. `service::worker_result::{adopt,
Reported, Adopted}` exposes the existing checked, no-symlink adoption path so
consumers need not copy validation. No engine implementation, wire schema or
default service behavior is changed.

Actual hosted qualification still requires consumer evidence for sessions,
membership/token expiry, spoofed headers against real routes, guessed foreign
job/resource IDs, cross-domain database and object storage, idle-stream revoke
latency, cancellation during clone, cache isolation, signed URL bypass and
provider revoke. No Core fixture or CI result certifies hosted production tenant
isolation. Public manifests/generated bindings are checked for accidental
private-app dependencies without importing private code.
