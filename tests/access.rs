//! Generic policy conformance only. This fixture is not a hosted HTTP adapter
//! and cannot qualify a consumer's session/RLS/object-storage isolation.
use std::cell::Cell;
use tolmap::access::{enforce, Denied, Operation, Policy, Request, Resource};

struct Owner {
    active: Cell<bool>,
    available: Cell<bool>,
}
impl Policy for Owner {
    fn check(&self, request: Request<'_>) -> Result<(), Denied> {
        if self.active.get()
            && self.available.get()
            && request.principal == "authenticated-reader"
            && request.resource.access_domain == "workspace-a"
            && request.resource.repository == "private-repo"
            && request.resource.revision == Some("snapshot-a")
        {
            Ok(())
        } else {
            Err(Denied)
        }
    }
}
fn request(operation: Operation) -> Request<'static> {
    Request {
        principal: "authenticated-reader",
        operation,
        resource: Resource {
            access_domain: "workspace-a",
            repository: "private-repo",
            revision: Some("snapshot-a"),
        },
    }
}

#[test]
fn protected_surfaces_require_current_authority_before_even_warm_delivery() {
    let owner = Owner {
        active: Cell::new(true),
        available: Cell::new(true),
    };
    // Status, map, symbol and comparison reads share Read in v1. A cache hit
    // is the same operation on the same full resource identity, never a grant.
    for (surface, operation) in [
        ("map", Operation::Read),
        ("symbol", Operation::Read),
        ("comparison", Operation::Read),
        ("status", Operation::Read),
        ("source", Operation::Source),
        ("admit", Operation::Admit),
        ("cancel", Operation::Cancel),
        ("SSE event", Operation::Events),
        ("artifact/download", Operation::Artifact),
        ("publication", Operation::Publish),
    ] {
        let resource = request(operation);
        owner.active.set(true);
        owner.available.set(true);
        let warm_cache = Cell::new(0);
        assert_eq!(enforce(&owner, resource, || warm_cache.replace(1)), Ok(0));
        for invalid in [
            Request {
                principal: "guessed-reader",
                ..resource
            },
            Request {
                resource: Resource {
                    access_domain: "workspace-b",
                    ..resource.resource
                },
                ..resource
            },
            Request {
                resource: Resource {
                    repository: "guessed-repo",
                    ..resource.resource
                },
                ..resource
            },
            Request {
                resource: Resource {
                    revision: Some("guessed-snapshot"),
                    ..resource.resource
                },
                ..resource
            },
        ] {
            assert_eq!(
                enforce(&owner, invalid, || warm_cache.set(9)),
                Err(Denied),
                "{surface}"
            );
            assert_eq!(warm_cache.get(), 1, "denied {surface} touched cache");
        }
        owner.active.set(false);
        assert_eq!(
            enforce(&owner, resource, || warm_cache.set(9)),
            Err(Denied),
            "revoked {surface}"
        );
        owner.active.set(true);
        owner.available.set(false);
        assert_eq!(
            enforce(&owner, resource, || warm_cache.set(9)),
            Err(Denied),
            "unavailable policy {surface}"
        );
        assert_eq!(warm_cache.get(), 1);
    }
}

#[test]
fn untrusted_headers_do_not_replace_trusted_context() {
    let owner = Owner {
        active: Cell::new(true),
        available: Cell::new(true),
    };
    // There is deliberately no header deserializer on Request/Resource.
    // Model an adapter which holds a foreign authenticated principal while an
    // attacker supplies plausible header values: they cannot change its Request.
    let headers = [
        ("X-Principal", "authenticated-reader"),
        ("X-Workspace", "workspace-a"),
    ];
    assert_eq!(headers.len(), 2);
    let trusted = Request {
        principal: "foreign-reader",
        ..request(Operation::Read)
    };
    assert_eq!(
        enforce(&owner, trusted, || panic!("spoof delivered bytes")),
        Err(Denied)
    );
}

#[test]
fn revocation_between_admission_processing_events_and_publish_stops_later_actions() {
    let owner = Owner {
        active: Cell::new(true),
        available: Cell::new(true),
    };
    assert_eq!(
        enforce(&owner, request(Operation::Admit), || "admitted"),
        Ok("admitted")
    );
    assert_eq!(
        enforce(&owner, request(Operation::Source), || "materialized"),
        Ok("materialized")
    );
    assert_eq!(
        enforce(&owner, request(Operation::Events), || "first event"),
        Ok("first event")
    );
    owner.active.set(false);
    for operation in [
        Operation::Source,
        Operation::Events,
        Operation::Publish,
        Operation::Artifact,
    ] {
        assert_eq!(
            enforce(&owner, request(operation), || panic!(
                "continued after revoke"
            )),
            Err(Denied)
        );
    }
    // An embedding CancelProbe must translate live policy denial into process
    // cancellation too. Actual child kill/reap is qualified by local-reader CI;
    // this contract test does not invent an HTTP stream or a process monitor.
}
