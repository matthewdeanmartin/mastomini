//! When a bot runs. A schedule produces *slots*: the instants a run is due.
//! The scheduler remembers the last slot it handled, so a restart neither
//! repeats a run nor, beyond the bot's grace period, catches up on stale ones.

use crate::tz::Tz;

const MINUTE_MS: u64 = 60_000;
const DAY_MS: u64 = 24 * 60 * MINUTE_MS;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schedule {
    /// Every day at `hour:minute` local time in the POSIX zone `tz`
    /// (`label` is how the admin page names it, e.g. "US Eastern").
    Daily {
        hour: u32,
        minute: u32,
        tz: String,
        label: String,
    },
    /// Every `minutes`, aligned to the clock (every 15 minutes: :00, :15...).
    Every { minutes: u32 },
    /// Only when the admin presses "Run now".
    Manual,
}

impl Schedule {
    pub fn check(&self) -> Result<(), String> {
        match self {
            Schedule::Daily {
                hour, minute, tz, ..
            } => {
                Tz::parse(tz)?;
                if *hour > 23 || *minute > 59 {
                    return Err(format!("{hour}:{minute:02} is not a time of day"));
                }
                Ok(())
            }
            Schedule::Every { minutes: 0 } => Err("Every 0 minutes".into()),
            _ => Ok(()),
        }
    }

    /// The first slot strictly after `after_ms`.
    pub fn next_after(&self, after_ms: u64) -> Option<u64> {
        match *self {
            Schedule::Daily {
                hour,
                minute,
                ref tz,
                ..
            } => {
                let tz = Tz::parse(tz).ok()?;
                let today = tz.local(after_ms);
                // Today's slot, else tomorrow's (a local calendar day, so
                // daylight saving days of 23 or 25 hours come out right).
                (0..3).find_map(|ahead| {
                    let days =
                        crate::tz::days_from_civil(today.year, today.month, today.day) + ahead;
                    let (y, m, d) = crate::tz::civil_from_days(days);
                    Some(tz.utc_ms(y, m, d, hour, minute)).filter(|&slot| slot > after_ms)
                })
            }
            Schedule::Every { minutes } => {
                let period = u64::from(minutes.max(1)) * MINUTE_MS;
                Some((after_ms / period + 1) * period)
            }
            Schedule::Manual => None,
        }
    }

    /// The latest slot at or before `now_ms`.
    pub fn latest_at(&self, now_ms: u64) -> Option<u64> {
        let mut slot = self.next_after(now_ms.saturating_sub(2 * DAY_MS))?;
        if slot > now_ms {
            return None;
        }
        while let Some(next) = self.next_after(slot).filter(|&n| n <= now_ms) {
            slot = next;
        }
        Some(slot)
    }

    /// "Every day at 07:30 US Eastern"
    pub fn describe(&self) -> String {
        match self {
            Schedule::Daily {
                hour,
                minute,
                label,
                ..
            } => {
                format!("Every day at {hour:02}:{minute:02} {label}")
            }
            Schedule::Every { minutes: 1 } => "Every minute".into(),
            Schedule::Every { minutes } if minutes % 60 == 0 => {
                format!("Every {} hours", minutes / 60)
            }
            Schedule::Every { minutes } => format!("Every {minutes} minutes"),
            Schedule::Manual => "Only when run by hand".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tz::days_from_civil;

    fn morning() -> Schedule {
        Schedule::Daily {
            hour: 7,
            minute: 30,
            tz: "EST5EDT,M3.2.0,M11.1.0".into(),
            label: "US Eastern".into(),
        }
    }

    fn utc(y: i64, mo: u32, d: u32, h: u32, mi: u32) -> u64 {
        ((days_from_civil(y, mo, d) * 86_400 + i64::from(h) * 3600 + i64::from(mi) * 60) * 1000)
            as u64
    }

    #[test]
    fn daily_slots_are_local_time_across_daylight_saving() {
        // Saturday 26 September 2026, EDT: 07:30 is 11:30 UTC.
        assert_eq!(
            morning().next_after(utc(2026, 9, 26, 11, 0)),
            Some(utc(2026, 9, 26, 11, 30))
        );
        assert_eq!(
            morning().next_after(utc(2026, 9, 26, 11, 30)),
            Some(utc(2026, 9, 27, 11, 30))
        );
        // Late evening Eastern is already tomorrow in UTC.
        assert_eq!(
            morning().next_after(utc(2026, 9, 27, 3, 0)),
            Some(utc(2026, 9, 27, 11, 30))
        );
        // Across the November change: 07:30 EST is 12:30 UTC.
        assert_eq!(
            morning().next_after(utc(2026, 10, 31, 12, 0)),
            Some(utc(2026, 11, 1, 12, 30))
        );
        // And the March one.
        assert_eq!(
            morning().next_after(utc(2026, 3, 7, 13, 0)),
            Some(utc(2026, 3, 8, 11, 30))
        );
        assert_eq!(
            morning().latest_at(utc(2026, 9, 26, 11, 29)),
            Some(utc(2026, 9, 25, 11, 30))
        );
        assert_eq!(
            morning().latest_at(utc(2026, 9, 26, 11, 30)),
            Some(utc(2026, 9, 26, 11, 30))
        );
        assert_eq!(morning().describe(), "Every day at 07:30 US Eastern");
    }

    #[test]
    fn periodic_and_manual() {
        let every = Schedule::Every { minutes: 15 };
        assert_eq!(
            every.next_after(utc(2026, 9, 26, 10, 7)),
            Some(utc(2026, 9, 26, 10, 15))
        );
        assert_eq!(
            every.latest_at(utc(2026, 9, 26, 10, 15)),
            Some(utc(2026, 9, 26, 10, 15))
        );
        assert_eq!(Schedule::Every { minutes: 120 }.describe(), "Every 2 hours");
        assert_eq!(Schedule::Manual.next_after(0), None);
        assert_eq!(Schedule::Manual.latest_at(u64::MAX / 2), None);
        assert!(Schedule::Every { minutes: 0 }.check().is_err());
        assert!(morning().check().is_ok());
    }
}
