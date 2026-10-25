# Embedded resource policy v1

`access::{Policy, Request, Resource, Operation, enforce}` is an opt-in contract
for trusted embedding adapters. Context is deliberately not deserializable.
The adapter authenticates the principal and resolves ownership from trusted
storage before calling the policy. A supplied ID or header proves no authority.

Check before admission, source materialization, every status/event delivery,
cancel, map/symbol/comparison read, artifact download, cache lookup/delivery and
atomic result publication. Read covers map, symbol and comparison resources;
Artifact covers their exports. Resource identity includes the access domain,
stable repository and immutable revision. Domain must be part of private cache
keys, including previous-map/naming inputs. No reusable positive decision cache
is provided. Revocation is checked on each call; stream adapters own a documented
poll bound and must close on denial. Already-delivered bytes cannot be recalled.

The existing `service::executor::{execute, EventSink, CancelProbe}` and versioned
`worker::{JobSpec, WorkerSpec, WorkerEvent}` remain the processing boundary. An
embedding adapter checks before execute, monitors live authority independently
while it blocks, kills a cancelled child process group through CancelProbe, and
checks again before publication. Worker leases authenticate computation, not the
source principal. `service::worker_result::{adopt, Reported, Adopted}` exposes the
existing checked, no-symlink adoption path so consumers need not copy validation.
No engine implementation, wire schema or default service behavior is changed.

Inventory: standalone `tolmap build/check` reads the caller's local checkout;
the loopback-first public `tolmap serve` API is a single trust domain, **not a
multi-tenant protected server**. Its public URL catalogue/cache are not a private
storage backend. Embedders must not forward arbitrary hosted requests to that
router. The optional contract does not retrofit protection onto that router.
Source reading is implemented by `source::read`, the public source routes and
`local` snapshot routes (#221/#226). It does not implicitly authorize protected
reads. Protected embedding adapters must apply this contract before source
materialization/delivery, including every warm cache hit. Comparisons follow the
same Read/Artifact/Publish contract; comparison feature qualification belongs to
#224. Local private work
requires no policy adapter, account, hosted endpoint or subscription.

Consumer conformance must exercise all operations, foreign domains, revoked and
expired principals, warm caches and stale publication attempts. Unit conformance
is not evidence of a consumer's database/storage isolation. This is a narrow
first slice of #222; protected reusable HTTP route adapters remain future work.


## Checkpoint and coverage matrix

The v1 policy is a trusted in-process hook, not authentication middleware. Audit
at Core `ec3455626e1d141b0c95f8df6770e9eb45e220ca`: the standalone HTTP router has
no injected resource policy. Do not expose it as a protected multi-tenant server.
No new enforcement hole is implied by a public/local route that never promised
protected operation; a consumer must own its protected route adapters.

| Surface | Required v1 operation / checkpoint | Core evidence / remaining consumer duty |
| --- | --- | --- |
| Map / symbol / comparison | Read before lookup and delivery | `tests/access.rs` named cases; authenticate, resolve ownership and both comparison inputs in consumer |
| Source / provenance | Read for provenance; Source before Git materialization and delivery | `tests/source.rs` pins/path bounds; `tests/access.rs` policy denial; consumer wires both checks |
| Job admission | Admit before source acquisition/enqueue | Contract fixture; consumer supplies trusted resource/domain, never browser headers |
| Job status / cancellation | Read for each status; Cancel before mutation | Contract fixture; consumer must also authorize guessed job IDs |
| SSE | Events before opening and every event; live monitor while idle | Contract fixture checks post-revoke event; consumer closes idle stream within its documented poll bound |
| Artifact/download | Artifact before opening storage and each delivery boundary | Contract fixture; consumer denies signed URL/cache bypass and stale exports |
| Cache / previous-map inputs | Same resource operation before lookup and delivery | Warm-cache denial test; domain/repo/revision in consumer cache keys |
| Processing / publication | Source at materialization; live CancelProbe; Publish before atomic adoption | Revocation sequence fixture + real local cancellation child kill/reap; consumer must wire policy denial to CancelProbe independently of events |

`tests/access.rs` varies principal, domain, repository, revision, current grant and
policy availability. It enumerates reads/jobs/streams/artifacts/comparison/cache
checkpoints and denies stale publication. Its spoof case demonstrates a trusted
Request cannot be replaced by supplied header strings; it **does not** exercise a
hosted HTTP/session implementation. Real source and local worker tests cover
immutable A/B and cancellation separately. An already running synchronous action
inside `enforce` is not interrupted by later revocation: embedders must monitor
live authority, stop the child through CancelProbe and recheck at delivery and
publication. Already-delivered bytes cannot be recalled.

Remaining hosted qualification includes actual session/membership/token expiry,
spoofed headers against real routes, guessed foreign IDs, cross-domain database
and object storage, idle-stream revocation latency, cancellation during clone,
cache isolation and provider revoke. No Core fixture or CI success certifies
hosted production tenant isolation. Public manifests/generated bindings are
checked for accidental private-app dependencies without importing private code.
