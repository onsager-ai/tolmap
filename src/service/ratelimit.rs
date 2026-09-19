//! Per-IP and per-repo rate limiting for `POST /api/index` (docs/ARCHITECTURE.md's
//! "Limits": "a per-IP and per-repo rate limit").
//!
//! Hand-rolled fixed-window counters rather than a crate (`governor` et
//! al.): the service is single-process and loopback-only, the limiter only
//! needs to survive this process's lifetime, and a `Mutex<HashMap<K,
//! Window>>` is the entire algorithm. Fixed windows (not sliding/token
//! bucket) allow a burst at the window boundary, which is an accepted
//! trade for a guard against accidental hammering, not a precise
//! product-facing quota (see `Limits`'s doc comments).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Window {
    started: Instant,
    count: u32,
}

pub struct RateLimiter {
    per_ip: Mutex<HashMap<IpAddr, Window>>,
    per_repo: Mutex<HashMap<String, Window>>,
}

pub enum Verdict {
    Allowed,
    Denied { message: String },
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    pub fn new() -> Self {
        RateLimiter {
            per_ip: Mutex::new(HashMap::new()),
            per_repo: Mutex::new(HashMap::new()),
        }
    }

    pub fn check_ip(&self, ip: IpAddr, limit: u32, window: Duration) -> Verdict {
        check(&self.per_ip, ip, limit, window, || {
            format!(
                "more than {limit} requests from this address in the last {}s",
                window.as_secs()
            )
        })
    }

    pub fn check_repo(&self, slug: &str, limit: u32, window: Duration) -> Verdict {
        check(&self.per_repo, slug.to_owned(), limit, window, || {
            format!(
                "more than {limit} index requests for {slug} in the last {}s",
                window.as_secs()
            )
        })
    }
}

fn check<K: std::hash::Hash + Eq>(
    table: &Mutex<HashMap<K, Window>>,
    key: K,
    limit: u32,
    window: Duration,
    message: impl FnOnce() -> String,
) -> Verdict {
    let mut table = table.lock().expect("rate limiter mutex poisoned");
    let now = Instant::now();
    let entry = table.entry(key).or_insert_with(|| Window {
        started: now,
        count: 0,
    });
    if now.duration_since(entry.started) >= window {
        entry.started = now;
        entry.count = 0;
    }
    entry.count += 1;
    if entry.count > limit {
        Verdict::Denied { message: message() }
    } else {
        Verdict::Allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn allows_up_to_the_limit_then_denies() {
        let limiter = RateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        for _ in 0..3 {
            assert!(matches!(
                limiter.check_ip(ip, 3, Duration::from_secs(60)),
                Verdict::Allowed
            ));
        }
        assert!(matches!(
            limiter.check_ip(ip, 3, Duration::from_secs(60)),
            Verdict::Denied { .. }
        ));
    }

    #[test]
    fn per_repo_and_per_ip_windows_are_independent() {
        let limiter = RateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert!(matches!(
            limiter.check_repo("a/b", 1, Duration::from_secs(60)),
            Verdict::Allowed
        ));
        assert!(matches!(
            limiter.check_repo("a/b", 1, Duration::from_secs(60)),
            Verdict::Denied { .. }
        ));
        // A different slug is unaffected, and so is the per-IP counter.
        assert!(matches!(
            limiter.check_repo("c/d", 1, Duration::from_secs(60)),
            Verdict::Allowed
        ));
        assert!(matches!(
            limiter.check_ip(ip, 5, Duration::from_secs(60)),
            Verdict::Allowed
        ));
    }
}
