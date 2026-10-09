//! Rebuild the aquarium from seed + local slot date. No saved board or
//! Mastodon history participates in the simulation.
use crate::bot::{Bot, BotInfo, Run, RunError};
use crate::bots::good_morning::visibility_setting;
use crate::mastodon::{NewStatus, Visibility};
use crate::schedule::Schedule;
use crate::settings::{schedule_from, schedule_settings, Kind, Setting, Settings};
use crate::tz::{civil_from_days, days_from_civil};

pub struct FishTank;

pub(crate) fn epoch(text: &str) -> Result<i64, String> {
    let bad = || "Start date must be a real YYYY-MM-DD date between 1970 and 9999".to_string();
    if text.len() != 10
        || text.as_bytes().iter().enumerate().any(|(i, b)| {
            if i == 4 || i == 7 {
                *b != b'-'
            } else {
                !b.is_ascii_digit()
            }
        })
    {
        return Err(bad());
    }
    let parts: Vec<_> = text.split('-').collect();
    let y = parts[0].parse::<i64>().map_err(|_| bad())?;
    let m = parts[1].parse::<u32>().map_err(|_| bad())?;
    let d = parts[2].parse::<u32>().map_err(|_| bad())?;
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(bad());
    }
    let date = days_from_civil(y, m, d);
    if civil_from_days(date) != (y, m, d) {
        return Err(bad());
    }
    Ok(date)
}

impl Bot for FishTank {
    fn info(&self) -> BotInfo {
        BotInfo {
            id: "fishtank", name: "Fishtank",
            description: "A daily aquarium: fish swim forward from a fixed seed and start date, with no history reads.",
            schedule: Schedule::Manual, grace_minutes: 120,
            default_instance: "https://mastomini.local", uses_llm: false, needs_mastodon: true,
        }
    }
    fn settings(&self) -> Vec<Setting> {
        let mut s = schedule_settings("daily", "10:00", "us_eastern", "1440");
        s.extend([
            Setting::new("seed", "Tank seed", Kind::Number { min: 0, max: u32::MAX as i64 }, "42")
                .help("Keep seed and start date fixed for continuity. Changing either starts a different tank."),
            Setting::new("epoch", "Start date (YYYY-MM-DD)", Kind::Text, "2026-10-08")
                .help("Local date in the selected zone. Start date posts iteration 1; 36 days later posts iteration 37."),
            visibility_setting(),
        ]);
        s
    }
    fn ready(&self, settings: &Settings) -> Result<(), String> {
        epoch(settings.get("epoch")).map(|_| ())
    }
    fn schedule(&self, settings: &Settings) -> Schedule {
        schedule_from(settings)
    }
    fn run(&self, run: &mut Run<'_>) -> Result<String, RunError> {
        let start = epoch(run.settings.get("epoch")).map_err(RunError::permanent)?;
        let local = run.settings.tz("zone").local(run.slot_ms);
        let elapsed = days_from_civil(local.year, local.month, local.day) - start;
        if elapsed < 0 {
            return Ok("The tank has not started yet".into());
        }
        // Bound replay work even if a broken clock supplies a far-future date.
        if elapsed > 100_000 {
            return Err(RunError::permanent("Tank replay exceeds 100,000 days"));
        }
        let iteration = elapsed as u32 + 1;
        let seed = run.settings.number("seed") as u32;
        let mut tank = Tank::new(seed);
        for _ in 0..iteration {
            tank.step();
        }
        let text = tank.render(seed, iteration);
        let limit = run.mastodon().max_characters()?;
        if text.chars().count() > limit {
            return Err(RunError::permanent(
                "The server's character limit is too small for this tank",
            ));
        }
        let posted = run.post_keyed(
            &NewStatus {
                text,
                visibility: Visibility::parse(run.settings.get("visibility")),
                spoiler_text: None,
                in_reply_to_id: None,
            },
            &format!("tank:{seed}:{start}:{iteration}"),
        )?;
        Ok(format!("Posted tank day {iteration}: {}", posted.url))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Tank {
    rng: u64,
    fish: [(i32, i32); 4],
}
impl Tank {
    fn random(&mut self) -> u32 {
        // Explicit arithmetic, stable across Rust versions and platforms.
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.rng >> 32) as u32
    }
    fn new(seed: u32) -> Self {
        let mut tank = Self {
            rng: u64::from(seed),
            fish: [(-1, -1); 4],
        };
        for i in 0..4 {
            loop {
                let pos = ((tank.random() % 8) as i32, (tank.random() % 4) as i32);
                if !tank.fish.contains(&pos) {
                    tank.fish[i] = pos;
                    break;
                }
            }
        }
        tank
    }
    fn step(&mut self) {
        for i in 0..4 {
            let (dx, dy) = match self.random() % 4 {
                0 => (1, 0),
                1 => (-1, 0),
                2 => (0, 1),
                _ => (0, -1),
            };
            let (x, y) = self.fish[i];
            let nx = if (0..8).contains(&(x + dx)) {
                x + dx
            } else {
                x - dx
            };
            let ny = if (0..4).contains(&(y + dy)) {
                y + dy
            } else {
                y - dy
            };
            if !self.fish.contains(&(nx, ny)) {
                self.fish[i] = (nx, ny);
            }
        }
    }
    fn render(&self, seed: u32, day: u32) -> String {
        let mut text = format!("Tank day {day} · seed {seed}\n🟫🟫🟫🟫🟫🟫🟫🟫🟫🟫\n");
        for y in 0..4 {
            text.push('🟫');
            for x in 0..8 {
                text.push(
                    self.fish
                        .iter()
                        .position(|p| *p == (x, y))
                        .map_or('🟦', |i| ['🐟', '🐠', '🐡', '🦐'][i]),
                );
            }
            text.push_str("🟫\n");
        }
        text.push_str("🟫🪨🌿🪨🪨🌿🪨🌿🪨🟫");
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replay_matches_uninterrupted_simulation_and_preserves_fish() {
        let mut live = Tank::new(42);
        for day in 1..=100 {
            live.step();
            let mut replay = Tank::new(42);
            for _ in 0..day {
                replay.step();
            }
            assert_eq!(replay, live);
            assert!(live
                .fish
                .iter()
                .all(|(x, y)| (0..8).contains(x) && (0..4).contains(y)));
            assert_eq!(
                live.fish
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len(),
                4
            );
            assert!(live.render(42, day).chars().count() <= 140);
        }
        assert_ne!(Tank::new(42), Tank::new(43));
    }
    #[test]
    fn dates_are_real_and_day_37_is_36_days_after_epoch() {
        assert!(epoch("2026-02-29").is_err());
        assert!(epoch("2024-02-29").is_ok());
        assert!(epoch("2026-13-01").is_err());
        assert_eq!(
            epoch("2026-11-13").unwrap() - epoch("2026-10-08").unwrap() + 1,
            37
        );
    }

    #[test]
    fn posting_replays_day_37_ignores_saved_state_and_uses_slot_on_late_retry() {
        use crate::mastodon::testing::Scripted;
        use serde_json::{json, Value};
        let settings = Settings::new(FishTank.settings(), [("zone".into(), "utc".into())].into());
        let slot = epoch("2026-11-13").unwrap() as u64 * 86_400_000 + 36_000_000;
        let mut requests = Vec::new();
        for now in [slot, slot + 86_400_000] {
            let mut http = Scripted::default();
            http.answer(
                200,
                json!({"configuration": {"statuses": {"max_characters": 140}}}),
            );
            http.answer(200, json!({"id": "p1", "url": "u"}));
            let mut run = Run::new(
                "fishtank",
                slot,
                now,
                &settings,
                &mut http,
                "https://mastomini.local",
                "K",
                None,
            );
            run.state
                .insert("old_board".into(), "deliberately wrong".into());
            assert_eq!(FishTank.run(&mut run).unwrap(), "Posted tank day 37: u");
            let sent = http.sent.lock().unwrap();
            assert_eq!(sent.len(), 2);
            assert!(sent[0].url.ends_with("/api/v2/instance"));
            assert_eq!(sent[1].method, "POST");
            let body: Value = serde_json::from_slice(&sent[1].body).unwrap();
            assert!(body["status"]
                .as_str()
                .unwrap()
                .starts_with("Tank day 37 · seed 42"));
            requests.push((sent[1].body.clone(), sent[1].headers.clone()));
        }
        assert_eq!(requests[0], requests[1]);
    }

    #[test]
    fn before_start_does_not_fetch_or_post() {
        use crate::mastodon::testing::Scripted;
        let settings = Settings::new(FishTank.settings(), [("zone".into(), "utc".into())].into());
        let mut http = Scripted::default();
        let mut run = Run::new(
            "fishtank",
            0,
            0,
            &settings,
            &mut http,
            "https://mastomini.local",
            "K",
            None,
        );
        assert_eq!(
            FishTank.run(&mut run).unwrap(),
            "The tank has not started yet"
        );
        assert!(http.sent.lock().unwrap().is_empty());
    }
}
