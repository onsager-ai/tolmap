//! Version 1 integration contract for an embedding service's resource policy.
//! Context must be constructed by the trusted adapter, never deserialized from
//! request headers. Standalone CLI/service operation requires no policy owner.

pub const CONTRACT_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Read,
    Source,
    Admit,
    Cancel,
    Events,
    Artifact,
    Publish,
}

#[derive(Clone, Copy, Debug)]
pub struct Resource<'a> {
    pub access_domain: &'a str,
    pub repository: &'a str,
    pub revision: Option<&'a str>,
}

#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub principal: &'a str,
    pub operation: Operation,
    pub resource: Resource<'a>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Denied;

/// Implementations resolve *current* authority, including expiry/revocation.
/// An unavailable policy backend must return Denied, never cached permission.
pub trait Policy {
    fn check(&self, request: Request<'_>) -> Result<(), Denied>;
}

/// Check before touching even a warm cache. The closure cannot run on denial.
/// Long-lived consumers call this for every delivery and again at publication;
/// a successful call grants no continuing authority to later operations.
pub fn enforce<T>(
    policy: &dyn Policy,
    request: Request<'_>,
    action: impl FnOnce() -> T,
) -> Result<T, Denied> {
    policy.check(request)?;
    Ok(action())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Fixture(Cell<bool>);
    impl Policy for Fixture {
        fn check(&self, r: Request<'_>) -> Result<(), Denied> {
            if self.0.get() && r.principal == "reader" && r.resource.access_domain == "a" {
                Ok(())
            } else {
                Err(Denied)
            }
        }
    }

    #[test]
    fn every_delivery_and_cache_hit_requires_current_authority() {
        let policy = Fixture(Cell::new(true));
        for operation in [
            Operation::Read,
            Operation::Source,
            Operation::Admit,
            Operation::Cancel,
            Operation::Events,
            Operation::Artifact,
            Operation::Publish,
        ] {
            let mut request = Request {
                principal: "reader",
                operation,
                resource: Resource {
                    access_domain: "a",
                    repository: "same",
                    revision: Some("commit"),
                },
            };
            policy.0.set(true);
            assert_eq!(enforce(&policy, request, || 7), Ok(7));
            request.resource.access_domain = "b";
            assert_eq!(
                enforce(&policy, request, || panic!("foreign cache delivered")),
                Err(Denied)
            );
            request.resource.access_domain = "a";
            policy.0.set(false);
            assert_eq!(
                enforce(&policy, request, || panic!("revoked delivery")),
                Err(Denied)
            );
        }
    }
}
