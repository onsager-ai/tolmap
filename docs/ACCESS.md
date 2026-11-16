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
The protected comparison wrapper authorizes both commits as `Read`, regardless
of the operation supplied by its caller.

This remains an opt-in library boundary for embedders. `tolmap web` is a personal,
single-user application with no authentication; its router does not call
`access::enforce`. Off-loopback binding explicitly exposes source and management.
A protected consumer must authenticate its own routes, resolve trusted resource
identity and check current policy before storage/cache access and each delivery.

## Core handler inventory

Routes are registered in `src/service/personal_web.rs`. Management calls the
same application library as the CLI. Job polling/cancel, registration/history,
map/symbol/provenance/source reads and publication have no tenant policy hooks.
They belong to the listener's one trust domain. Source reads require the saved
snapshot's commit and bounded Git-relative paths. `service::worker_result::adopt`
keeps artifact validation. Protected embeddings supply their own policy before
admission, reads, cancellation, adoption and delivery.

`comparison::protected` remains a library API that authorizes both commits
before cache hits and delivery. Comparison HTTP endpoints and public catalogue
routes are retired. Consumer-owned caches must include access domain, repository,
immutable revision and all comparison inputs and recheck authority on each hit.

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
CancelProbe, and checks again before publication. `service::worker_result::{adopt,
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
