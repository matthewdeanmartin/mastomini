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

/// Strict RFC3339, including numeric offsets and fractional seconds. No time
/// zone database is needed; persisted announcement windows are UTC milliseconds.
pub fn parse_iso(text: &str) -> Option<u64> {
    if !text.is_ascii() || text.len() < 20 {
        return None;
    }
    let b = text.as_bytes();
    if b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let n = |a, b| {
        let part: &str = text.get(a..b)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse::<i64>().ok())
            .flatten()
    };
    let (y, m, d, h, min, s) = (
        n(0, 4)?,
        n(5, 7)?,
        n(8, 10)?,
        n(11, 13)?,
        n(14, 16)?,
        n(17, 19)?,
    );
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&m) || h > 23 || min > 59 || s > 59 {
        return None;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if d < 1 || d > days[(m - 1) as usize] {
        return None;
    }
    let mut pos = 19;
    let mut fraction = 0;
    if b.get(pos) == Some(&b'.') {
        pos += 1;
        let start = pos;
        while b.get(pos).is_some_and(u8::is_ascii_digit) {
            pos += 1;
        }
        if pos == start || pos - start > 9 {
            return None;
        }
        for i in 0..3 {
            fraction = fraction * 10
                + b.get(start + i)
                    .filter(|_| start + i < pos)
                    .map_or(0, |c| (c - b'0') as i64);
        }
    }
    let zone = &text[pos..];
    let offset = if matches!(zone, "Z" | "z") {
        0
    } else {
        let z = zone.as_bytes();
        if z.len() != 6 || !matches!(z[0], b'+' | b'-') || z[3] != b':' {
            return None;
        }
        if !z[1..3].iter().chain(&z[4..6]).all(u8::is_ascii_digit) {
            return None;
        }
        let hh = zone[1..3].parse::<i64>().ok()?;
        let mm = zone[4..6].parse::<i64>().ok()?;
        if !(0..=23).contains(&hh) || !(0..=59).contains(&mm) {
            return None;
        }
        (hh * 60 + mm) * 60 * if z[0] == b'-' { -1 } else { 1 }
    };
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yo = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let days = era * 146097 + yo * 365 + yo / 4 - yo / 100 + doy - 719468;
    u64::try_from(((days * 86400 + h * 3600 + min * 60 + s) - offset) * 1000 + fraction).ok()
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
