//! Generic policy conformance only. This fixture is not a hosted HTTP adapter
//! and cannot qualify a consumer's session/RLS/object-storage isolation.
use std::cell::Cell;
use tolmap::access::{enforce, Denied, Operation, Policy, Request, Resource};

const OPERATIONS: [Operation; 7] = [
    Operation::Read,
    Operation::Source,
    Operation::Admit,
    Operation::Cancel,
    Operation::Events,
    Operation::Artifact,
    Operation::Publish,
];

struct Owner {
    active: Cell<bool>,
    available: Cell<bool>,
    allowed: Cell<Option<Operation>>,
}

impl Policy for Owner {
    fn check(&self, request: Request<'_>) -> Result<(), Denied> {
        if self.active.get()
            && self.available.get()
            && self.allowed.get() == Some(request.operation)
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
fn each_operation_has_an_independent_allow_and_deny_boundary() {
    let owner = Owner {
        active: Cell::new(true),
        available: Cell::new(true),
        allowed: Cell::new(None),
    };
    // One operation grant cannot be reused as another operation. This is a
    // library contract fixture; route adapters must still wire each handler.
    for operation in OPERATIONS {
        let resource = request(operation);
        owner.allowed.set(Some(operation));
        let warm_cache = Cell::new(0);
        assert_eq!(enforce(&owner, resource, || warm_cache.replace(1)), Ok(0));
        for other in OPERATIONS.into_iter().filter(|candidate| *candidate != operation) {
            assert_eq!(
                enforce(
                    &owner,
                    Request { operation: other, ..resource },
                    || warm_cache.set(9)
                ),
                Err(Denied),
                "operation {other:?} cannot reuse {operation:?} authority"
            );
            assert_eq!(warm_cache.get(), 1, "denied operation touched cache");
        }
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
                // A guessed immutable foreign-resource ID is not authority.
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
                "{operation:?}"
            );
            assert_eq!(warm_cache.get(), 1, "denied {operation:?} touched cache");
        }
        owner.active.set(false);
        assert_eq!(
            enforce(&owner, resource, || warm_cache.set(9)),
            Err(Denied),
            "revoked {operation:?}"
        );
        owner.active.set(true);
        owner.available.set(false);
        assert_eq!(
            enforce(&owner, resource, || warm_cache.set(9)),
            Err(Denied),
            "unavailable policy {operation:?}"
        );
        assert_eq!(warm_cache.get(), 1);
        owner.available.set(true);
    }
}

#[test]
fn untrusted_principal_and_guessed_resource_are_denied() {
    let owner = Owner {
        active: Cell::new(true),
        available: Cell::new(true),
        allowed: Cell::new(Some(Operation::Read)),
    };
    let foreign_principal = Request {
        principal: "foreign-reader",
        ..request(Operation::Read)
    };
    assert_eq!(
        enforce(&owner, foreign_principal, || {
            panic!("foreign principal delivered bytes")
        }),
        Err(Denied)
    );
    let guessed_id = Request {
        resource: Resource {
            revision: Some("guessed-snapshot"),
            ..request(Operation::Read).resource
        },
        ..request(Operation::Read)
    };
    assert_eq!(
        enforce(&owner, guessed_id, || panic!("guessed resource ID delivered bytes")),
        Err(Denied)
    );
}

#[test]
fn revocation_between_admission_processing_events_and_publish_stops_later_actions() {
    let owner = Owner {
        active: Cell::new(true),
        available: Cell::new(true),
        allowed: Cell::new(Some(Operation::Admit)),
    };
    assert_eq!(
        enforce(&owner, request(Operation::Admit), || "admitted"),
        Ok("admitted")
    );
    owner.allowed.set(Some(Operation::Source));
    assert_eq!(
        enforce(&owner, request(Operation::Source), || "materialized"),
        Ok("materialized")
    );
    owner.allowed.set(Some(Operation::Events));
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
        owner.allowed.set(Some(operation));
        assert_eq!(
            enforce(&owner, request(operation), || panic!("continued after revoke")),
            Err(Denied)
        );
    }
    // No process or stream hook is part of this library. An embedding adapter
    // must translate live policy denial into CancelProbe and close its stream.
}
