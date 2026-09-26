//! A bot that writes with a language model (OpenRouter): new posts, or
//! replies to people who mention it.
//!
//! The admin sets the system instructions and prompt templates. Templates
//! use ready-made variables, all plain text:
//!
//! | Variable | Holds |
//! |---|---|
//! | `{{bot_name}}`, `{{bot_acct}}` | The bot's display name and `@name` |
//! | `{{bio}}` | The bio setting, else the bot's profile bio |
//! | `{{date}}`, `{{weekday}}`, `{{time}}`, `{{zone}}` | Now, in the bot's time zone |
//! | `{{max_characters}}` | The server's post length limit (mastomini: 140) |
//! | `{{recent_posts}}` | The bot's latest posts, one per line, so it doesn't repeat itself |
//! | `{{conversation}}` | The thread being answered, oldest first, one `@name: text` line per post |
//! | `{{reply_to}}`, `{{reply_to_author}}` | The post being answered and its author's `@name` |
//!
//! Guard rails: it never answers itself, answers other bots only if told
//! to (two bots can talk forever), answers at most a few mentions per run,
//! starts from "now" rather than the backlog the first time, and can't
//! notify anyone the conversation didn't already include. Replies are no
//! more public than the post they answer; direct messages are left alone
//! (on mastomini a bot's API key can't read them).

use crate::bot::{Bot, BotInfo, Run, RunError};
use crate::bots::good_morning::visibility_setting;
use crate::mastodon::{Account, Mention, NewStatus, Visibility};
use crate::openrouter::Prompt;
use crate::schedule::Schedule;
use crate::settings::{schedule_from, schedule_settings, Kind, Setting, Settings};
use crate::template::render;
use crate::text::{defuse_mentions, fit};

pub const VARIABLES: [&str; 12] = [
    "bot_name",
    "bot_acct",
    "bio",
    "date",
    "weekday",
    "time",
    "zone",
    "max_characters",
    "recent_posts",
    "conversation",
    "reply_to",
    "reply_to_author",
];

const SYSTEM: &str = "You are {{bot_name}} ({{bot_acct}}), a bot on a small, friendly Mastodon server for one household.
{{bio}}
Write plain text only: no markdown, no quotation marks around your answer, no hashtags unless asked. Never claim to be a person. Stay under {{max_characters}} characters.";

const POST_PROMPT: &str = "It is {{weekday}}, {{date}}, {{time}}.
Write one new post in your own voice.
Your recent posts (do not repeat them):
{{recent_posts}}";

const REPLY_PROMPT: &str = "The conversation so far, oldest first:
{{conversation}}

Write your reply to {{reply_to_author}}. Reply with the text only.";

/// How much of a thread goes into a prompt.
const MAX_CONVERSATION_LINES: usize = 20;
const MAX_CONVERSATION_CHARS: usize = 4000;
/// Mentions fetched per run; at most `replies_per_run` are answered.
const FETCH: usize = 40;

pub struct LlmBot {
    pub id: &'static str,
    pub name: &'static str,
    /// `post` or `reply`.
    pub mode: &'static str,
}

impl Bot for LlmBot {
    fn info(&self) -> BotInfo {
        BotInfo {
            id: self.id,
            name: self.name,
            description: if self.mode == "reply" {
                "Answers people who mention it, using a language model (OpenRouter)."
            } else {
                "Writes new posts with a language model (OpenRouter)."
            },
            schedule: Schedule::Manual,
            grace_minutes: 30,
            default_instance: "https://mastomini.local",
            uses_llm: true,
        }
    }

    fn settings(&self) -> Vec<Setting> {
        let (run, time, every) = if self.mode == "reply" {
            ("every", "09:00", "5")
        } else {
            ("daily", "12:00", "240")
        };
        let mut s = schedule_settings(run, time, "us_eastern", every);
        let vars = || Kind::LongText {
            placeholders: VARIABLES.to_vec(),
        };
        s.extend([
            Setting::new(
                "mode",
                "Does",
                Kind::Choice {
                    options: vec![("reply", "Answers mentions"), ("post", "Writes new posts")],
                },
                self.mode,
            ),
            Setting::new("system", "System instructions", vars(), SYSTEM)
                .help("Who the bot is and how it writes. Sent as the system message."),
            Setting::new("post_prompt", "Prompt for a new post", vars(), POST_PROMPT),
            Setting::new("reply_prompt", "Prompt for a reply", vars(), REPLY_PROMPT)
                .help("{{conversation}} is the thread, one “@name: text” line per post."),
            Setting::new("bio", "Bio for prompts", vars(), "")
                .optional()
                .help("Fills {{bio}}. Empty: the bot account's profile bio."),
            Setting::new("model", "Model", Kind::Text, "")
                .optional()
                .help("An OpenRouter model id. Empty: the device's default (Device page)."),
            Setting::new(
                "max_tokens",
                "Longest answer (tokens)",
                Kind::Number { min: 16, max: 2000 },
                "300",
            )
            .help("A cost guard. The post itself is cut to the server's limit."),
            Setting::new(
                "replies_per_run",
                "Replies per run",
                Kind::Number { min: 1, max: 10 },
                "3",
            ),
            Setting::new("reply_to_bots", "Answer other bots", Kind::Toggle, "no")
                .help("Two bots answering each other never stop."),
            visibility_setting(),
        ]);
        s
    }

    fn schedule(&self, settings: &Settings) -> Schedule {
        schedule_from(settings)
    }

    fn run(&self, run: &mut Run<'_>) -> Result<String, RunError> {
        let me = run.mastodon().me()?;
        let max_characters = run.mastodon().max_characters().unwrap_or(500);
        let base = Vars::new(run, &me, max_characters);
        match run.settings.get("mode") {
            "post" => post(run, &me, base),
            _ => reply(run, &me, base),
        }
    }
}

/// The variables every prompt has.
struct Vars {
    list: Vec<(&'static str, String)>,
    max_characters: usize,
}

impl Vars {
    fn new(run: &Run<'_>, me: &Account, max_characters: usize) -> Vars {
        let s = run.settings;
        let (_, zone) = s.zone("zone");
        let now = s.tz("zone").local(run.now_ms);
        let bio = if s.is_set("bio") {
            s.get("bio").to_string()
        } else {
            me.note.clone()
        };
        Vars {
            list: vec![
                ("bot_name", me.name().to_string()),
                ("bot_acct", format!("@{}", me.acct)),
                ("bio", bio),
                ("date", now.long_date()),
                ("weekday", now.weekday_name().to_string()),
                ("time", now.clock()),
                ("zone", zone.to_string()),
                ("max_characters", max_characters.to_string()),
            ],
            max_characters,
        }
    }

    fn with<'s>(&'s self, extra: &'s [(&'static str, String)]) -> Vec<(&'s str, &'s str)> {
        self.list
            .iter()
            .chain(extra)
            .map(|(k, v)| (*k, v.as_str()))
            .collect()
    }
}

fn ask(run: &mut Run<'_>, vars: &[(&str, &str)], prompt_key: &str) -> Result<String, RunError> {
    let s = run.settings;
    let system = render(s.get("system"), vars);
    let user = render(s.get(prompt_key), vars);
    let model = if s.is_set("model") {
        s.get("model").to_string()
    } else {
        run.default_model().to_string()
    };
    let max_tokens = s.number("max_tokens") as u32;
    run.llm()?.complete(&Prompt {
        system: &system,
        user: &user,
        model: &model,
        max_tokens,
    })
}

fn recent_posts(run: &mut Run<'_>, me: &Account) -> String {
    match run.mastodon().recent_posts(&me.id, 5) {
        Ok(posts) if !posts.is_empty() => posts
            .iter()
            .map(|p| p.text.replace('\n', " "))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => "(none yet)".into(),
    }
}

fn post(run: &mut Run<'_>, me: &Account, vars: Vars) -> Result<String, RunError> {
    let recent = recent_posts(run, me);
    let text = ask(run, &vars.with(&[("recent_posts", recent)]), "post_prompt")?;
    // Nobody is in the conversation: the model mentions no one.
    let text = fit(&defuse_mentions(&text, &[]), vars.max_characters);
    let posted = run.post(&NewStatus {
        text,
        visibility: Visibility::parse(run.settings.get("visibility")),
        spoiler_text: None,
        in_reply_to_id: None,
    })?;
    Ok(format!("Posted {}", posted.url))
}

/// The thread as lines, trimmed from the old end to fit.
fn conversation(lines: Vec<String>) -> String {
    let mut kept: Vec<String> = Vec::new();
    let mut size = 0;
    for line in lines.into_iter().rev() {
        if kept.len() >= MAX_CONVERSATION_LINES || size + line.len() > MAX_CONVERSATION_CHARS {
            break;
        }
        size += line.len() + 1;
        kept.push(line);
    }
    kept.reverse();
    kept.join("\n")
}

fn reply(run: &mut Run<'_>, me: &Account, vars: Vars) -> Result<String, RunError> {
    let Some(cursor) = run.state.get("cursor").cloned() else {
        // First run: start from now, not from every mention there ever was.
        let newest = run.mastodon().mentions(None, 1)?;
        let cursor = newest.last().map_or("0".to_string(), |m| m.id.clone());
        run.state.insert("cursor".into(), cursor);
        return Ok("Watching for mentions from now on".into());
    };
    let mentions = run.mastodon().mentions(Some(&cursor), FETCH)?;
    let limit = run.settings.number("replies_per_run").max(1) as usize;
    let (mut answered, mut skipped) = (0, 0);
    for mention in mentions {
        if answered >= limit {
            break;
        }
        match skip_reason(run, me, &mention) {
            Some(why) => {
                run.log(format!(
                    "Skipped a mention from @{}: {why}",
                    mention.status.account.acct
                ));
                skipped += 1;
            }
            None => match answer(run, me, &vars, &mention) {
                Ok(url) => {
                    run.log(format!("Answered @{}: {url}", mention.status.account.acct));
                    answered += 1;
                }
                // Try this mention again next run; the cursor stays before it.
                Err(e) if e.retryable => return Err(e),
                Err(e) => {
                    run.log(format!(
                        "Could not answer @{}: {}",
                        mention.status.account.acct, e.message
                    ));
                    skipped += 1;
                }
            },
        }
        run.state.insert("cursor".into(), mention.id.clone());
    }
    Ok(match (answered, skipped) {
        (0, 0) => "No new mentions".to_string(),
        (a, 0) => format!("Answered {a} mention{}", if a == 1 { "" } else { "s" }),
        (a, s) => format!("Answered {a}, skipped {s}"),
    })
}

fn skip_reason(run: &Run<'_>, me: &Account, mention: &Mention) -> Option<&'static str> {
    let author = &mention.status.account;
    if author.id == me.id {
        Some("that is the bot itself")
    } else if author.bot && !run.settings.yes("reply_to_bots") {
        Some("it is a bot (see “Answer other bots”)")
    } else if mention.status.visibility == "direct" {
        Some("direct messages are left alone")
    } else {
        None
    }
}

fn answer(
    run: &mut Run<'_>,
    me: &Account,
    vars: &Vars,
    mention: &Mention,
) -> Result<String, RunError> {
    let status = &mention.status;
    let mut thread = run.mastodon().ancestors(&status.id).unwrap_or_default();
    thread.push(status.clone());
    let mut allowed: Vec<String> = Vec::new();
    for post in &thread {
        allowed.push(post.account.acct.to_lowercase());
        allowed.extend(post.mentions.iter().map(|m| m.to_lowercase()));
    }
    allowed.retain(|a| *a != me.acct.to_lowercase());
    let lines = thread.iter().map(|p| p.line()).collect();
    let author = format!("@{}", status.account.acct);
    let extra = [
        ("conversation", conversation(lines)),
        ("reply_to", status.text.clone()),
        ("reply_to_author", author.clone()),
    ];
    let text = ask(run, &vars.with(&extra), "reply_prompt")?;
    // Models often open with the names they saw: drop the bot's own and the
    // author's, then address the author once, first, as Mastodon replies do.
    let mut body = text.trim_start();
    loop {
        let lower = body.to_lowercase();
        let lead = [
            format!("@{}", me.acct.to_lowercase()),
            author.to_lowercase(),
        ]
        .into_iter()
        .find(|m| {
            lower.starts_with(m.as_str())
                && !lower[m.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
        });
        match lead {
            Some(m) => body = body[m.len()..].trim_start_matches([',', ':', ' ']),
            None => break,
        }
    }
    let body = defuse_mentions(body, &allowed);
    let text = fit(&format!("{author} {body}"), vars.max_characters);
    let visibility = Visibility::parse(&status.visibility)
        .at_most(Visibility::parse(run.settings.get("visibility")));
    let posted = run.post_keyed(
        &NewStatus {
            text,
            visibility,
            spoiler_text: (!status.spoiler_text.is_empty()).then(|| status.spoiler_text.clone()),
            in_reply_to_id: Some(status.id.clone()),
        },
        &format!("reply:{}", status.id),
    )?;
    Ok(posted.url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastodon::testing::Scripted;
    use crate::openrouter::Config;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    fn me() -> Value {
        json!({"id": "9", "acct": "helper", "display_name": "Helper", "note": "<p>I help with chores.</p>", "bot": true})
    }

    fn instance() -> Value {
        json!({"configuration": {"statuses": {"max_characters": 140}}})
    }

    fn status(id: &str, acct: &str, text: &str, visibility: &str, reply_to: Option<&str>) -> Value {
        json!({"id": id, "account": {"id": format!("a{acct}"), "acct": acct, "bot": acct.ends_with("bot")},
               "content": format!("<p>{text}</p>"), "visibility": visibility, "spoiler_text": "",
               "in_reply_to_id": reply_to, "mentions": [{"acct": "helper"}]})
    }

    fn run(
        bot: &LlmBot,
        values: &[(&str, &str)],
        state: &[(&str, &str)],
        http: &mut Scripted,
    ) -> (
        Result<String, RunError>,
        BTreeMap<String, String>,
        Vec<String>,
    ) {
        let values = values
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let settings = Settings::new(bot.settings(), values);
        let config = Config {
            key: "sk-or-test".into(),
            model: String::new(),
        };
        let mut run = Run::new(
            bot.id,
            1_790_000_000_000,
            1_790_000_000_000,
            &settings,
            http,
            "https://mastomini.local",
            "K",
            Some(&config),
        );
        run.state = state
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let result = bot.run(&mut run);
        (result, run.state, run.log)
    }

    fn body(http: &Scripted, n: usize) -> Value {
        serde_json::from_slice(&http.sent.lock().unwrap()[n].body).unwrap()
    }

    const REPLY: LlmBot = LlmBot {
        id: "llm_reply",
        name: "Replies",
        mode: "reply",
    };
    const POST: LlmBot = LlmBot {
        id: "llm_post",
        name: "Poster",
        mode: "post",
    };

    #[test]
    fn first_run_starts_from_now() {
        let mut http = Scripted::default();
        http.answer(200, me());
        http.answer(200, instance());
        http.answer(200, json!([{"id": "50", "type": "mention", "status": status("s1", "alice", "hi", "public", None)}]));
        let (result, state, _) = run(&REPLY, &[], &[], &mut http);
        assert_eq!(result.unwrap(), "Watching for mentions from now on");
        assert_eq!(state["cursor"], "50");
        assert_eq!(http.sent.lock().unwrap().len(), 3, "no model call");
    }

    #[test]
    fn answers_a_mention_with_the_thread_as_lines() {
        let mut http = Scripted::default();
        http.answer(200, me());
        http.answer(200, instance());
        http.answer(200, json!([
            {"id": "52", "type": "mention", "status": status("s3", "alice", "@helper what should we cook?", "private", Some("s2"))},
            {"id": "51", "type": "mention", "status": status("s9", "otherbot", "@helper hi bot", "public", None)},
        ]));
        http.answer(
            200,
            json!({"ancestors": [status("s2", "bob", "Dinner at 7", "private", None)]}),
        );
        http.answer(
            200,
            json!({"choices": [{"message": {"content": "@helper Try soup! Ask @mallory too."}}]}),
        );
        http.answer(
            200,
            json!({"id": "r1", "url": "https://mastomini.local/@helper/r1"}),
        );
        let (result, state, log) = run(&REPLY, &[], &[("cursor", "50")], &mut http);
        assert_eq!(result.unwrap(), "Answered 1, skipped 1");
        assert_eq!(state["cursor"], "52");
        assert!(log[0].contains("otherbot"), "{log:?}");
        let sent = http.sent.lock().unwrap().clone();
        assert!(sent[2].url.contains("types%5B%5D=mention") && sent[2].url.contains("min_id=50"));
        let llm = body(&http, 4);
        let system = llm["messages"][0]["content"].as_str().unwrap();
        assert!(
            system.contains("Helper (@helper)")
                && system.contains("I help with chores.")
                && system.contains("140")
        );
        let user = llm["messages"][1]["content"].as_str().unwrap();
        assert!(
            user.contains("@bob: Dinner at 7\n@alice: @helper what should we cook?"),
            "{user}"
        );
        assert!(user.contains("reply to @alice"));
        let posted = body(&http, 5);
        // Addressed to the author, invented mentions defused, no more public
        // than the question.
        assert_eq!(
            posted["status"],
            "@alice Try soup! Ask @\u{200B}mallory too."
        );
        assert_eq!(posted["in_reply_to_id"], "s3");
        assert_eq!(posted["visibility"], "private");
        let key = sent[5]
            .headers
            .iter()
            .find(|(k, _)| k == "Idempotency-Key")
            .unwrap();
        assert_eq!(key.1, "mastomini-bots:llm_reply:reply:s3");
    }

    #[test]
    fn a_failing_model_leaves_the_mention_for_next_time() {
        let mut http = Scripted::default();
        http.answer(200, me());
        http.answer(200, instance());
        http.answer(200, json!([{"id": "52", "type": "mention", "status": status("s3", "alice", "@helper hi", "public", None)}]));
        http.answer(200, json!({"ancestors": []}));
        http.answer(503, json!({"error": {"message": "overloaded"}}));
        let (result, state, _) = run(&REPLY, &[], &[("cursor", "50")], &mut http);
        assert!(result.unwrap_err().retryable);
        assert_eq!(state["cursor"], "50");
    }

    #[test]
    fn posts_within_the_limit_and_names_no_one() {
        let mut http = Scripted::default();
        http.answer(200, me());
        http.answer(200, instance());
        http.answer(
            200,
            json!([status(
                "p1",
                "helper",
                "Yesterday's thought",
                "public",
                None
            )]),
        );
        let long = format!("@bob look: {}", "tidy up ".repeat(40));
        http.answer(200, json!({"choices": [{"message": {"content": long}}]}));
        http.answer(200, json!({"id": "p2", "url": "u"}));
        let (result, _, _) = run(
            &POST,
            &[
                ("model", "google/gemma-4-31b-it"),
                ("bio", "Loves tidiness."),
            ],
            &[],
            &mut http,
        );
        assert_eq!(result.unwrap(), "Posted u");
        let llm = body(&http, 3);
        assert_eq!(llm["model"], "google/gemma-4-31b-it");
        assert!(llm["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Loves tidiness."));
        assert!(llm["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("Yesterday's thought"));
        let text = body(&http, 4)["status"].as_str().unwrap().to_string();
        assert!(text.chars().count() <= 140, "{text}");
        assert!(text.starts_with("@\u{200B}bob"), "{text}");
    }

    #[test]
    fn needs_an_openrouter_key() {
        let mut http = Scripted::default();
        http.answer(200, me());
        http.answer(200, instance());
        http.answer(200, json!([]));
        let settings = Settings::new(POST.settings(), BTreeMap::new());
        let mut run = Run::new(
            "llm_post",
            0,
            0,
            &settings,
            &mut http,
            "https://x",
            "K",
            None,
        );
        let e = POST.run(&mut run).unwrap_err();
        assert!(e.message.contains("OpenRouter API key") && !e.retryable);
    }
}
