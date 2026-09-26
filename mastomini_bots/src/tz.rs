//! Local time from POSIX `TZ` rules (`EST5EDT,M3.2.0,M11.1.0`), with no
//! time zone database: the board has none, and one rule string per zone is
//! all a bot schedule needs. Only the `Mm.w.d` rule form is supported, which
//! is what every zone with daylight saving uses in practice.

/// Days since 1970-01-01 from a civil date. Howard Hinnant's algorithm.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// (year, month, day) from days since 1970-01-01.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// 0 = Sunday.
pub fn weekday(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}

fn days_in_month(y: i64, m: u32) -> u32 {
    let next = if m == 12 {
        days_from_civil(y + 1, 1, 1)
    } else {
        days_from_civil(y, m + 1, 1)
    };
    (next - days_from_civil(y, m, 1)) as u32
}

/// `Mm.w.d/time`: day `d` (0 = Sunday) of week `w` (5 = last) of month `m`,
/// at `time` seconds past local midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rule {
    month: u32,
    week: u32,
    day: u32,
    time: i64,
}

impl Rule {
    /// Days since the epoch of this rule's date in `year`.
    fn date(&self, year: i64) -> i64 {
        let first = days_from_civil(year, self.month, 1);
        let mut day = 1 + (self.day + 7 - weekday(first)) % 7 + (self.week - 1) * 7;
        while day > days_in_month(year, self.month) {
            day -= 7;
        }
        first + i64::from(day) - 1
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Dst {
    name: String,
    /// Seconds east of UTC.
    offset: i64,
    start: Rule,
    end: Rule,
}

/// A zone: standard time and, optionally, its daylight saving rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tz {
    std_name: String,
    /// Seconds east of UTC (POSIX writes them west: `EST5` is -5 h).
    std_offset: i64,
    dst: Option<Dst>,
}

/// A local time and the offset in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// 0 = Sunday.
    pub weekday: u32,
    pub offset: i64,
    pub dst: bool,
}

struct Cursor<'a> {
    s: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn eat(&mut self, c: u8) -> bool {
        let hit = self.peek() == Some(c);
        self.i += usize::from(hit);
        hit
    }
    fn name(&mut self) -> Option<String> {
        let start = self.i;
        if self.eat(b'<') {
            while self.peek().is_some_and(|c| c != b'>') {
                self.i += 1;
            }
            let name = std::str::from_utf8(&self.s[start + 1..self.i]).ok()?;
            self.eat(b'>').then(|| name.to_string())
        } else {
            while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                self.i += 1;
            }
            (self.i - start >= 3).then(|| String::from_utf8_lossy(&self.s[start..self.i]).into())
        }
    }
    fn number(&mut self) -> Option<i64> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        std::str::from_utf8(&self.s[start..self.i])
            .ok()?
            .parse()
            .ok()
    }
    /// `[+-]hh[:mm[:ss]]` in seconds.
    fn hms(&mut self) -> Option<i64> {
        let sign = if self.eat(b'-') {
            -1
        } else {
            self.eat(b'+');
            1
        };
        let mut total = self.number()? * 3600;
        if self.eat(b':') {
            total += self.number()? * 60;
            if self.eat(b':') {
                total += self.number()?;
            }
        }
        Some(sign * total)
    }
    fn rule(&mut self) -> Option<Rule> {
        if !self.eat(b'M') {
            return None;
        }
        let month = self.number()? as u32;
        self.eat(b'.').then_some(())?;
        let week = self.number()? as u32;
        self.eat(b'.').then_some(())?;
        let day = self.number()? as u32;
        let time = if self.eat(b'/') { self.hms()? } else { 7200 };
        ((1..=12).contains(&month) && (1..=5).contains(&week) && day <= 6).then_some(Rule {
            month,
            week,
            day,
            time,
        })
    }
}

impl Tz {
    pub const UTC: &'static str = "UTC0";

    /// Parse a POSIX `TZ` string such as `EST5EDT,M3.2.0,M11.1.0`.
    pub fn parse(text: &str) -> Result<Tz, String> {
        let bad = || format!("Unsupported time zone rule: {text}");
        let mut c = Cursor {
            s: text.as_bytes(),
            i: 0,
        };
        let std_name = c.name().ok_or_else(bad)?;
        let std_offset = -c.hms().ok_or_else(bad)?;
        let dst = match c.peek() {
            None => None,
            Some(_) => {
                let name = c.name().ok_or_else(bad)?;
                let offset = if c.peek() == Some(b',') {
                    std_offset + 3600
                } else {
                    -c.hms().ok_or_else(bad)?
                };
                c.eat(b',').then_some(()).ok_or_else(bad)?;
                let start = c.rule().ok_or_else(bad)?;
                c.eat(b',').then_some(()).ok_or_else(bad)?;
                let end = c.rule().ok_or_else(bad)?;
                Some(Dst {
                    name,
                    offset,
                    start,
                    end,
                })
            }
        };
        if c.i != text.len() {
            return Err(bad());
        }
        Ok(Tz {
            std_name,
            std_offset,
            dst,
        })
    }

    /// Seconds east of UTC in force at `utc` (seconds), and whether that is
    /// daylight saving time.
    pub fn offset_at(&self, utc: i64) -> (i64, bool) {
        let Some(dst) = &self.dst else {
            return (self.std_offset, false);
        };
        let (year, _, _) = civil_from_days((utc + self.std_offset).div_euclid(86_400));
        // Transitions are written in the local time in force just before them.
        let start = dst.start.date(year) * 86_400 + dst.start.time - self.std_offset;
        let end = dst.end.date(year) * 86_400 + dst.end.time - dst.offset;
        let on = if start < end {
            start <= utc && utc < end
        } else {
            // Southern hemisphere: daylight saving spans the new year.
            utc >= start || utc < end
        };
        if on {
            (dst.offset, true)
        } else {
            (self.std_offset, false)
        }
    }

    pub fn abbreviation(&self, dst: bool) -> &str {
        match (&self.dst, dst) {
            (Some(d), true) => &d.name,
            _ => &self.std_name,
        }
    }

    pub fn local(&self, utc_ms: u64) -> Local {
        let utc = (utc_ms / 1000) as i64;
        let (offset, dst) = self.offset_at(utc);
        let t = utc + offset;
        let days = t.div_euclid(86_400);
        let secs = t.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Local {
            year,
            month,
            day,
            hour: (secs / 3600) as u32,
            minute: (secs % 3600 / 60) as u32,
            second: (secs % 60) as u32,
            weekday: weekday(days),
            offset,
            dst,
        }
    }

    /// UTC milliseconds of a local wall-clock time. A time skipped by the
    /// spring-forward jump happens an hour later (02:30 becomes 03:30); a
    /// time that happens twice in the autumn is the first one.
    pub fn utc_ms(&self, year: i64, month: u32, day: u32, hour: u32, minute: u32) -> u64 {
        let wall = days_from_civil(year, month, day) * 86_400
            + i64::from(hour) * 3600
            + i64::from(minute) * 60;
        let mut offsets = vec![self.std_offset];
        if let Some(d) = &self.dst {
            offsets.push(d.offset);
        }
        let valid: Vec<i64> = offsets
            .iter()
            .map(|o| wall - o)
            .filter(|&utc| self.offset_at(utc).0 == wall - utc)
            .collect();
        let utc = match valid.iter().min() {
            Some(&utc) => utc,
            // In the gap: the standard-time reading lands after the jump.
            None => wall - self.std_offset,
        };
        (utc.max(0) as u64) * 1000
    }
}

/// A time zone the admin can pick by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zone {
    pub id: &'static str,
    pub label: &'static str,
    pub posix: &'static str,
}

/// The zones bot settings offer (`Kind::Zone`). The first is the default.
pub const ZONES: [Zone; 14] = [
    Zone {
        id: "us_eastern",
        label: "US Eastern",
        posix: "EST5EDT,M3.2.0,M11.1.0",
    },
    Zone {
        id: "us_central",
        label: "US Central",
        posix: "CST6CDT,M3.2.0,M11.1.0",
    },
    Zone {
        id: "us_mountain",
        label: "US Mountain",
        posix: "MST7MDT,M3.2.0,M11.1.0",
    },
    Zone {
        id: "us_arizona",
        label: "US Arizona",
        posix: "MST7",
    },
    Zone {
        id: "us_pacific",
        label: "US Pacific",
        posix: "PST8PDT,M3.2.0,M11.1.0",
    },
    Zone {
        id: "us_alaska",
        label: "US Alaska",
        posix: "AKST9AKDT,M3.2.0,M11.1.0",
    },
    Zone {
        id: "us_hawaii",
        label: "US Hawaii",
        posix: "HST10",
    },
    Zone {
        id: "utc",
        label: "UTC",
        posix: "UTC0",
    },
    Zone {
        id: "uk",
        label: "UK and Ireland",
        posix: "GMT0BST,M3.5.0/1,M10.5.0",
    },
    Zone {
        id: "central_europe",
        label: "Central Europe",
        posix: "CET-1CEST,M3.5.0,M10.5.0/3",
    },
    Zone {
        id: "eastern_europe",
        label: "Eastern Europe",
        posix: "EET-2EEST,M3.5.0/3,M10.5.0/4",
    },
    Zone {
        id: "india",
        label: "India",
        posix: "<+0530>-5:30",
    },
    Zone {
        id: "japan",
        label: "Japan",
        posix: "JST-9",
    },
    Zone {
        id: "australia_east",
        label: "Australia East",
        posix: "AEST-10AEDT,M10.1.0,M4.1.0/3",
    },
];

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

impl Local {
    pub fn weekday_name(&self) -> &'static str {
        DAYS[self.weekday as usize]
    }

    /// `07:30`
    pub fn clock(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// `Saturday, September 26, 2026`
    pub fn long_date(&self) -> String {
        format!(
            "{}, {} {}, {}",
            DAYS[self.weekday as usize],
            MONTHS[self.month as usize - 1],
            self.day,
            self.year
        )
    }
}

/// `2026-09-26T11:30:00.000Z`
pub fn iso(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        ms % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const EASTERN: &str = "EST5EDT,M3.2.0,M11.1.0";

    fn utc(y: i64, mo: u32, d: u32, h: u32, mi: u32) -> u64 {
        ((days_from_civil(y, mo, d) * 86_400 + i64::from(h) * 3600 + i64::from(mi) * 60) * 1000)
            as u64
    }

    #[test]
    fn civil_round_trips() {
        for days in [-1_000_000, -1, 0, 1, 19_000, 20_722, 2_000_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(weekday(days_from_civil(2026, 9, 26)), 6); // a Saturday
        assert_eq!(iso(utc(2026, 9, 26, 11, 30)), "2026-09-26T11:30:00.000Z");
    }

    #[test]
    fn eastern_time_follows_daylight_saving() {
        let tz = Tz::parse(EASTERN).unwrap();
        // 2026: DST from Sunday March 8 02:00 EST to Sunday November 1 02:00 EDT.
        assert_eq!(tz.local(utc(2026, 1, 15, 12, 30)).hour, 7);
        assert_eq!(tz.local(utc(2026, 7, 15, 11, 30)).hour, 7);
        assert!(!tz.offset_at((utc(2026, 3, 8, 6, 59) / 1000) as i64).1);
        assert!(tz.offset_at((utc(2026, 3, 8, 7, 0) / 1000) as i64).1);
        assert!(tz.offset_at((utc(2026, 11, 1, 5, 59) / 1000) as i64).1);
        assert!(!tz.offset_at((utc(2026, 11, 1, 6, 0) / 1000) as i64).1);
        // 07:30 local is 12:30 UTC in winter, 11:30 UTC in summer.
        assert_eq!(tz.utc_ms(2026, 1, 15, 7, 30), utc(2026, 1, 15, 12, 30));
        assert_eq!(tz.utc_ms(2026, 9, 26, 7, 30), utc(2026, 9, 26, 11, 30));
        assert_eq!(tz.abbreviation(true), "EDT");
        let local = tz.local(utc(2026, 9, 26, 11, 30));
        assert_eq!(local.long_date(), "Saturday, September 26, 2026");
    }

    #[test]
    fn gaps_and_repeats() {
        let tz = Tz::parse(EASTERN).unwrap();
        // 02:30 on the spring-forward day does not exist: 03:30 EDT.
        assert_eq!(tz.utc_ms(2026, 3, 8, 2, 30), utc(2026, 3, 8, 7, 30));
        // 01:30 on the fall-back day happens twice: the first (EDT) one.
        assert_eq!(tz.utc_ms(2026, 11, 1, 1, 30), utc(2026, 11, 1, 5, 30));
    }

    #[test]
    fn other_zones() {
        let utc0 = Tz::parse(Tz::UTC).unwrap();
        assert_eq!(utc0.utc_ms(2026, 9, 26, 7, 30), utc(2026, 9, 26, 7, 30));
        // Sydney: DST from the first Sunday of October to the first of April.
        let syd = Tz::parse("AEST-10AEDT,M10.1.0,M4.1.0/3").unwrap();
        assert!(syd.local(utc(2026, 1, 15, 0, 0)).dst);
        assert!(!syd.local(utc(2026, 7, 15, 0, 0)).dst);
        assert_eq!(syd.local(utc(2026, 7, 15, 0, 0)).hour, 10);
        let india = Tz::parse("<+0530>-5:30").unwrap();
        assert_eq!(india.local(utc(2026, 1, 1, 0, 0)).minute, 30);
        for zone in ZONES {
            assert!(Tz::parse(zone.posix).is_ok(), "{}", zone.id);
        }
        for bad in [
            "",
            "E5",
            "EST",
            "EST5EDT,J60,J300",
            "EST5EDT,M13.1.0,M11.1.0",
            "EST5x",
        ] {
            assert!(Tz::parse(bad).is_err(), "{bad}");
        }
    }
}
