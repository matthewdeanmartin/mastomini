//! A daily poll-driven civilization, with bounded durable memory.
use super::story;
use crate::{
    bot::{Bot, BotInfo, Run, RunError},
    schedule::Schedule,
    settings::{schedule_from, Setting, Settings},
};
use serde_json::json;

pub struct Shoebox;
const RULES: &str = include_str!("shoebox_rules.txt");
impl Bot for Shoebox {
    fn info(&self) -> BotInfo {
        BotInfo { id: "shoebox", name: "Civilization in a Shoebox", description: "Three tiny inhabitants build a civilization. Vote in the daily poll; no votes means a random choice. A victory or extinction starts a new game next day.", schedule: Schedule::Manual, grace_minutes: 720, default_instance: "https://mastomini.local", uses_llm: true, needs_mastodon: true }
    }
    fn settings(&self) -> Vec<Setting> {
        story::settings()
    }
    fn schedule(&self, s: &Settings) -> Schedule {
        schedule_from(s)
    }
    fn run(&self, run: &mut Run<'_>) -> Result<String, RunError> {
        let date = story::date(run);
        if run.state.get("date") == Some(&date) {
            return Ok("Today's shoebox turn is already posted".into());
        }
        let limit = run.mastodon().max_characters()?;
        let pending = if let Some(pending) = run.state.get("pending") {
            pending.clone()
        } else {
            let reset = run.state.get("ending").is_some_and(|x| x != "PLAY");
            let day = if reset {
                1
            } else {
                run.state
                    .get("day")
                    .and_then(|x| x.parse::<u32>().ok())
                    .unwrap_or(0)
                    + 1
            };
            let mut choice = "Found a civilization with exactly three inhabitants.".to_string();
            if !reset {
                if let Some(id) = run.state.get("post").cloned() {
                    let post = run.mastodon().call(
                        "GET",
                        &format!("/api/v1/statuses/{id}"),
                        None,
                        None,
                    )?;
                    let poll = &post["poll"];
                    if poll["expired"].as_bool() != Some(true) {
                        return Ok("Waiting for the current poll to close".into());
                    }
                    let options = poll["options"]
                        .as_array()
                        .ok_or_else(|| story::invalid("Current poll has no options"))?;
                    let counts: Vec<u64> = options
                        .iter()
                        .map(|o| {
                            o["votes_count"]
                                .as_u64()
                                .ok_or_else(|| story::invalid("Poll vote counts are unavailable"))
                        })
                        .collect::<Result<_, _>>()?;
                    let winner = choose(&counts)?;
                    choice = options[winner]["title"]
                        .as_str()
                        .ok_or_else(|| story::invalid("Poll option has no title"))?
                        .to_string();
                    run.log(format!("Poll choice: {choice}; votes: {}", counts[winner]));
                }
            }
            let memory = if reset {
                ""
            } else {
                run.state.get("memory").map(String::as_str).unwrap_or("")
            };
            let input = format!("Day: {day}\nSelected action: {choice}\nCurrent game's condensed history:\n{memory}\nPost maximum: {limit} characters. Follow the rule book. Output only the required lines.");
            let output = story::generate(run, RULES, &input)?;
            let turn = Turn::parse(&output)?;
            if day == 1 && turn.ending != "PLAY" {
                return Err(story::invalid("A new game must start with a poll"));
            }
            run.state.insert("pending_day".into(), day.to_string());
            run.state.insert("pending".into(), output.clone());
            output
        };
        let turn = Turn::parse(&pending)?;
        let day = run
            .state
            .get("pending_day")
            .cloned()
            .unwrap_or_else(|| "1".into());
        let text = format!("Day {day}: {}", turn.text);
        let status = story::status(run, &text, limit, None);
        let mut body = json!({"status": status.text, "visibility": run.settings.get("visibility")});
        if turn.ending == "PLAY" {
            // 23 hours also closes before tomorrow's run across spring DST.
            body["poll"] = json!({"options": turn.options, "expires_in": 82800, "multiple": false, "hide_totals": false});
        }
        let key = format!(
            "shoebox:{}:{day}",
            run.state
                .get("generation")
                .map(String::as_str)
                .unwrap_or("0")
        );
        let posted = run
            .mastodon()
            .call("POST", "/api/v1/statuses", Some(&body), Some(&key))?;
        let id = posted["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| story::invalid("Posted turn has no id"))?;
        run.state.insert("post".into(), id.into());
        run.state.insert("memory".into(), turn.memory);
        run.state.insert("day".into(), day);
        run.state.insert("ending".into(), turn.ending.clone());
        if turn.ending != "PLAY" {
            let generation = run
                .state
                .get("generation")
                .and_then(|x| x.parse::<u64>().ok())
                .unwrap_or(0)
                + 1;
            run.state
                .insert("generation".into(), generation.to_string());
        }
        run.state.insert("date".into(), date);
        run.state.remove("pending");
        run.state.remove("pending_day");
        Ok(format!("Shoebox turn posted ({})", turn.ending))
    }
}

fn choose(counts: &[u64]) -> Result<usize, RunError> {
    let max = counts
        .iter()
        .max()
        .ok_or_else(|| story::invalid("Empty poll"))?;
    let tied: Vec<_> = counts
        .iter()
        .enumerate()
        .filter(|(_, n)| *n == max)
        .map(|(i, _)| i)
        .collect();
    // Rejection sampling avoids modulo bias; all zero counts include every option.
    loop {
        let mut bytes = [0; 4];
        getrandom::getrandom(&mut bytes)
            .map_err(|_| story::invalid("Random source unavailable"))?;
        let n = u32::from_le_bytes(bytes);
        let ceiling = u32::MAX - u32::MAX % tied.len() as u32;
        if n < ceiling {
            return Ok(tied[n as usize % tied.len()]);
        }
    }
}
struct Turn {
    ending: String,
    text: String,
    memory: String,
    options: Vec<String>,
}
impl Turn {
    fn parse(text: &str) -> Result<Self, RunError> {
        if serde_json::to_string(text).map_or(true, |s| s.len() > 1200) {
            return Err(story::invalid("Game turn is too large"));
        }
        let lines: Vec<_> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let field = |index: usize, prefix: &str| -> Result<String, RunError> {
            lines
                .get(index)
                .and_then(|l| l.strip_prefix(prefix))
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .ok_or_else(|| story::invalid("Gemma returned an invalid shoebox turn"))
        };
        let ending = field(0, "RESULT: ")?;
        if !["PLAY", "WIN", "LOSS"].contains(&ending.as_str()) {
            return Err(story::invalid("Unknown game result"));
        }
        let text = field(1, "POST: ")?;
        let memory = field(2, "MEMORY: ")?;
        if serde_json::to_string(&memory).map_or(true, |s| s.len() > 600) {
            return Err(story::invalid("Game summary is too large"));
        }
        let mut options = Vec::new();
        if ending == "PLAY" {
            for line in lines.iter().skip(3) {
                let option = line.strip_prefix("OPTION: ").map(str::trim).unwrap_or("");
                if option.is_empty()
                    || option.chars().count() > 50
                    || options.iter().any(|x| x == option)
                {
                    return Err(story::invalid("Invalid or duplicate poll option"));
                }
                options.push(crate::text::defuse_mentions(option, &[]));
            }
            if !(2..=4).contains(&options.len()) {
                return Err(story::invalid("A turn needs two to four options"));
            }
        } else if lines.len() != 3 {
            return Err(story::invalid("A finished game cannot have options"));
        }
        Ok(Self {
            ending,
            text,
            memory,
            options,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn votes_and_abstentions() {
        for _ in 0..100 {
            assert_eq!(choose(&[0, 7, 1]).unwrap(), 1);
            assert!(choose(&[0, 0, 0]).unwrap() < 3);
            assert_ne!(choose(&[4, 1, 4]).unwrap(), 1);
        }
        assert!(choose(&[]).is_err());
    }
    #[test]
    fn model_output_is_validated() {
        assert!(Turn::parse("RESULT: PLAY\nPOST: Three citizens arrive.\nMEMORY: Population 3.\nOPTION: Build shelter\nOPTION: Elect a mayor").is_ok());
        assert!(
            Turn::parse("RESULT: PLAY\nPOST: Hi\nMEMORY: 3\nOPTION: Same\nOPTION: Same").is_err()
        );
        assert!(Turn::parse("RESULT: WIN\nPOST: Victory!\nMEMORY: A thriving city.").is_ok());
    }
    #[test]
    fn retries_reuse_generated_turn_and_closed_poll_advances() {
        use crate::{mastodon::testing::Scripted, openrouter::Config};
        let bot = Shoebox;
        let settings = Settings::new(bot.settings(), Default::default());
        let config = Config {
            key: "test".into(),
            model: String::new(),
        };
        let mut http = Scripted::default();
        let answer = "RESULT: PLAY\nPOST: Three citizens elect a crumb.\nMEMORY: Population 3; crumb mayor.\nOPTION: Farm\nOPTION: Build";
        http.answer(
            200,
            json!({"configuration":{"statuses":{"max_characters":140}}}),
        );
        http.answer(200, json!({"choices":[{"message":{"content":answer}}]}));
        http.fail("timeout");
        let mut run = Run::new(
            "shoebox",
            1800000000000,
            1800000000000,
            &settings,
            &mut http,
            "https://test",
            "key",
            Some(&config),
        );
        assert!(bot.run(&mut run).is_err());
        let state = run.state.clone();
        drop(run);
        http.answer(
            200,
            json!({"configuration":{"statuses":{"max_characters":140}}}),
        );
        http.answer(200, json!({"id":"10"}));
        let mut run = Run::new(
            "shoebox",
            1800000000000,
            1800000000000,
            &settings,
            &mut http,
            "https://test",
            "key",
            Some(&config),
        );
        run.state = state;
        bot.run(&mut run).unwrap();
        assert_eq!(run.state["day"], "1");
        assert!(!run.state.contains_key("pending"));
        bot.run(&mut run).unwrap(); // same date: no network or extra post
        let state = run.state.clone();
        drop(run);
        let sent = http.sent.lock().unwrap();
        let first = &sent[2];
        let retry = &sent[4];
        assert_eq!(first.body, retry.body);
        assert_eq!(first.headers, retry.headers);
        let body: serde_json::Value = serde_json::from_slice(&retry.body).unwrap();
        assert_eq!(body["poll"]["options"], json!(["Farm", "Build"]));
        drop(sent);
        http.answer(
            200,
            json!({"configuration":{"statuses":{"max_characters":140}}}),
        );
        http.answer(200, json!({"poll":{"expired":true,"options":[{"title":"Farm","votes_count":2},{"title":"Build","votes_count":0}]}}));
        http.answer(200, json!({"choices":[{"message":{"content":answer}}]}));
        http.answer(200, json!({"id":"11"}));
        let mut run = Run::new(
            "shoebox",
            1800086400000,
            1800086400000,
            &settings,
            &mut http,
            "https://test",
            "key",
            Some(&config),
        );
        run.state = state;
        bot.run(&mut run).unwrap();
        assert_eq!(run.state["day"], "2");
        let sent = http.sent.lock().unwrap();
        let prompt: serde_json::Value = serde_json::from_slice(&sent[7].body).unwrap();
        assert!(prompt["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("Selected action: Farm"));
    }
}
