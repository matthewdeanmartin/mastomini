//! The time bot: posts a message with the date at a time of day. By default
//! "Good morning, it is Saturday, September 26, 2026" at 07:30 US Eastern.
//! The template for new bots: settings, a schedule from them, one post.

use crate::bot::{Bot, BotInfo, Run, RunError};
use crate::mastodon::{NewStatus, Visibility};
use crate::schedule::Schedule;
use crate::settings::{schedule_from, schedule_settings, Kind, Setting, Settings};
use crate::template::render;

pub struct GoodMorning;

pub fn visibility_setting() -> Setting {
    Setting::new(
        "visibility",
        "Who sees its posts",
        Kind::Choice {
            options: vec![
                ("public", "Everyone"),
                ("unlisted", "Everyone, but not in public timelines"),
                ("private", "Followers only"),
            ],
        },
        "public",
    )
}

impl Bot for GoodMorning {
    fn info(&self) -> BotInfo {
        BotInfo {
            id: "good_morning",
            name: "Good morning",
            description: "Posts a message with the date every day at a set time.",
            schedule: Schedule::Manual,
            // Still worth saying two hours late; after that it would be odd.
            grace_minutes: 120,
            default_instance: "https://mastomini.local",
            uses_llm: false,
        }
    }

    fn settings(&self) -> Vec<Setting> {
        let mut s = schedule_settings("daily", "07:30", "us_eastern", "1440");
        s.push(
            Setting::new(
                "message",
                "Message",
                Kind::LongText {
                    placeholders: vec!["date", "weekday", "time", "zone"],
                },
                "Good morning, it is {{date}}",
            )
            .help("{{date}}: Saturday, September 26, 2026. {{weekday}}, {{time}} (07:30) and {{zone}} too."),
        );
        s.push(visibility_setting());
        s
    }

    fn schedule(&self, settings: &Settings) -> Schedule {
        schedule_from(settings)
    }

    fn run(&self, run: &mut Run<'_>) -> Result<String, RunError> {
        let settings = run.settings;
        let (_, zone) = settings.zone("zone");
        // The slot's date, not the clock's: a retry says the same thing.
        let today = settings.tz("zone").local(run.slot_ms);
        let text = render(
            settings.get("message"),
            &[
                ("date", &today.long_date()),
                ("weekday", today.weekday_name()),
                ("time", &today.clock()),
                ("zone", zone),
            ],
        );
        if text.is_empty() {
            return Err(RunError::permanent("The message came out empty"));
        }
        let posted = run.post(&NewStatus {
            text,
            visibility: Visibility::parse(settings.get("visibility")),
            spoiler_text: None,
            in_reply_to_id: None,
        })?;
        Ok(format!("Posted {}", posted.url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastodon::testing::Scripted;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn run_with(values: &[(&str, &str)], slot: u64) -> serde_json::Value {
        let mut http = Scripted::default();
        http.answer(
            200,
            json!({"id": "1", "url": "https://mastomini.local/@bot/1"}),
        );
        let sent = http.sent.clone();
        let values: BTreeMap<String, String> = values
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let settings = Settings::new(GoodMorning.settings(), values);
        let mut run = Run::new(
            "good_morning",
            slot,
            slot,
            &settings,
            &mut http,
            "https://mastomini.local",
            "K",
            None,
        );
        assert_eq!(
            GoodMorning.run(&mut run).unwrap(),
            "Posted https://mastomini.local/@bot/1"
        );
        let body = serde_json::from_slice(&sent.lock().unwrap()[0].body).unwrap();
        body
    }

    #[test]
    fn says_the_eastern_date_of_its_slot() {
        // 2026-09-27 03:00 UTC is still Saturday the 26th in New York.
        let body = run_with(&[], 1_790_478_000_000);
        assert_eq!(
            body["status"],
            "Good morning, it is Saturday, September 26, 2026"
        );
        assert_eq!(body["visibility"], "public");
    }

    #[test]
    fn follows_its_settings() {
        let body = run_with(
            &[
                ("zone", "japan"),
                ("message", "{{weekday}} {{time}} {{zone}}"),
                ("visibility", "unlisted"),
            ],
            1_790_478_000_000,
        );
        // 03:00 UTC is noon Sunday in Tokyo.
        assert_eq!(body["status"], "Sunday 12:00 Japan");
        assert_eq!(body["visibility"], "unlisted");
        let settings = Settings::new(GoodMorning.settings(), BTreeMap::new());
        assert_eq!(
            GoodMorning.schedule(&settings).describe(),
            "Every day at 07:30 US Eastern"
        );
    }
}
