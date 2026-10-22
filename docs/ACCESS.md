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
Protected source reading and comparisons are future surfaces under #221/#224;
their adapters must use this same contract before delivery. Local private work
requires no policy adapter, account, hosted endpoint or subscription.

Consumer conformance must exercise all operations, foreign domains, revoked and
expired principals, warm caches and stale publication attempts. Unit conformance
is not evidence of a consumer's database/storage isolation. This is a narrow
first slice of #222; protected reusable HTTP route adapters remain future work.
