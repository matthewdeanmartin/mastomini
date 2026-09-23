//! ISO 8601 timestamps without a date library.

/// Days since 1970-01-01 to (year, month, day). Howard Hinnant's algorithm.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `2026-09-22T13:45:07.123Z`
pub fn iso(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (y, mo, d) = civil(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        ms % 1000
    )
}

/// Midnight of the day, as Mastodon reports `Account.created_at`.
pub fn iso_day(ms: u64) -> String {
    iso(ms - ms % 86_400_000)
}

/// `2026-09-22`, as Mastodon reports `last_status_at`.
pub fn date(ms: u64) -> String {
    iso(ms)[..10].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_instants() {
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(iso(1_790_000_000_123), "2026-09-21T14:13:20.123Z");
        assert_eq!(iso_day(1_790_000_000_123), "2026-09-21T00:00:00.000Z");
        assert_eq!(date(1_790_000_000_123), "2026-09-21");
    }
}
