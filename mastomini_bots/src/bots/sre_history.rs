//! Seven UTC days, last observation per day, at most twelve current boards.
//! One compact state value (at most 4 KiB), no summaries or extra resident
//! cache. It uses the normal once-per-run persistence, even if posting fails.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const KEY: &str = "health_history";
const DAY_MS: u64 = 86_400_000;
const MAX_BYTES: usize = 4096;

#[derive(Default, Serialize, Deserialize)]
pub struct History {
    pub hosts: Vec<String>,
    pub days: Vec<Day>,
}

#[derive(Serialize, Deserialize)]
pub struct Day {
    pub day: u64,
    pub at_ms: u64,
    /// One byte per host: o=ok, w=warn, f=fail, d=down, n=no endpoint,
    /// -=not observed (e.g. a newly configured target).
    pub statuses: String,
}

pub fn read(state: &BTreeMap<String, String>, now: u64) -> History {
    let mut history: History = state
        .get(KEY)
        .filter(|s| s.len() <= MAX_BYTES)
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    if history.hosts.len() > 12
        || history.hosts.iter().any(|h| h.is_empty() || h.len() > 253)
        || history.days.len() > 7
        || history.days.iter().any(|d| {
            d.statuses.len() != history.hosts.len()
                || !d.statuses.bytes().all(|b| b"owfdn-".contains(&b))
                || d.at_ms / DAY_MS != d.day
        })
        || history.days.windows(2).any(|d| d[0].day >= d[1].day)
    {
        return History::default();
    }
    let today = now / DAY_MS;
    history.days.retain(|d| d.day <= today && today - d.day < 7);
    history
}

pub fn record(state: &mut BTreeMap<String, String>, now: u64, boards: &[(&str, &str)]) {
    if now == 0 || boards.len() > 12 || boards.iter().any(|(h, _)| h.is_empty() || h.len() > 253) {
        return;
    }
    let mut history = read(state, now);
    let hosts: Vec<String> = boards.iter().map(|(h, _)| h.to_string()).collect();
    // Carry observations by host, not by position, when targets are reordered.
    for day in &mut history.days {
        day.statuses = hosts
            .iter()
            .map(|h| {
                history
                    .hosts
                    .iter()
                    .position(|old| old == h)
                    .map_or('-', |i| day.statuses.as_bytes()[i] as char)
            })
            .collect();
    }
    history.hosts = hosts;
    let today = now / DAY_MS;
    history.days.retain(|d| d.day != today);
    history.days.push(Day {
        day: today,
        at_ms: now,
        statuses: boards
            .iter()
            .map(|(_, s)| match *s {
                "ok" => 'o',
                "warn" => 'w',
                "fail" => 'f',
                "down" => 'd',
                "none" => 'n',
                _ => 'f',
            })
            .collect(),
    });
    if let Ok(encoded) = serde_json::to_string(&history) {
        if encoded.len() <= MAX_BYTES {
            state.insert(KEY.into(), encoded);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seven_days_survive_serialization_and_repeat_runs_replace_one_day() {
        let mut state = BTreeMap::new();
        for day in 1..=10 {
            record(&mut state, day * DAY_MS, &[("a.local", "warn")]);
        }
        record(&mut state, 10 * DAY_MS + 1, &[("a.local", "ok")]);
        let reopened = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        let history = read(&reopened, 10 * DAY_MS + 2);
        assert_eq!(history.days.len(), 7);
        assert_eq!(history.days[0].day, 4);
        assert_eq!(history.days[6].statuses, "o");
        assert!(read(&state, 17 * DAY_MS).days.is_empty());
    }

    #[test]
    fn changing_targets_never_relabels_old_observations() {
        let mut state = BTreeMap::new();
        record(&mut state, DAY_MS, &[("a", "fail"), ("b", "warn")]);
        record(
            &mut state,
            2 * DAY_MS,
            &[("b", "ok"), ("c", "none"), ("a", "down")],
        );
        let h = read(&state, 2 * DAY_MS);
        assert_eq!(h.days[0].statuses, "w-f");
        assert_eq!(h.days[1].statuses, "ond");
    }

    #[test]
    fn maximum_history_fits_budget_and_invalid_state_is_not_trusted() {
        let mut state = BTreeMap::new();
        let hosts: Vec<String> = (0..12)
            .map(|i| format!("{i:02}{}", "a".repeat(251)))
            .collect();
        let boards: Vec<_> = hosts.iter().map(|h| (h.as_str(), "ok")).collect();
        for day in 20000..20010 {
            record(&mut state, day * DAY_MS, &boards);
        }
        assert!(state[KEY].len() <= MAX_BYTES);
        assert_eq!(read(&state, 20009 * DAY_MS).days.len(), 7);
        state.insert(
            KEY.into(),
            r#"{"hosts":["a"],"days":[{"day":1,"at_ms":86400000,"statuses":"oo"}]}"#.into(),
        );
        assert!(read(&state, DAY_MS).days.is_empty());
    }
}
