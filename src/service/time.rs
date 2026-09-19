//! A minimal RFC3339 (UTC) timestamp formatter, so the job service does not
//! need a date/time crate for the one thing it needs one for: stamping
//! `started_at`/`finished_at`/`indexed_at` in API responses. The civil-date
//! algorithm is Howard Hinnant's `civil_from_days`
//! (<http://howardhinnant.github.io/date_algorithms.html>), which is exact
//! and branch-free for the proleptic Gregorian calendar -- there is no
//! reason to trust a hand-rolled version less than a dependency for this.

use std::time::{SystemTime, UNIX_EPOCH};

/// `SystemTime::now()` formatted as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_rfc3339() -> String {
    format_unix(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    )
}

fn format_unix(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let secs_of_day = unix_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_instants_format_correctly() {
        assert_eq!(format_unix(0), "1970-01-01T00:00:00Z");
        // 2026-09-19T14:12:04Z, cross-checked with Python's
        // `datetime.fromtimestamp(..., tz=timezone.utc)` during development.
        assert_eq!(format_unix(1_789_827_124), "2026-09-19T14:12:04Z");
        assert_eq!(format_unix(946_684_799), "1999-12-31T23:59:59Z");
    }
}
