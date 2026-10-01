//! The NanaCoin news bot: twice a day, posts what happened in the household
//! economy (a lotto opened, someone is selling a bike, a loan request).
//!
//! It reads NanaCoin's activity feed after its saved cursor, keeps the
//! categories the admin ticked, and posts one short line per event. A
//! crowded category becomes one roll-up line ("🛒 5 new for sale: lamp,
//! bike, +3 more"), and a run posts at most "Most posts per run".
//!
//! Retries are safe: the first attempt at a slot pins how far into the feed
//! that slot reaches (`upto`), so a retry composes the same posts with the
//! same keys, and the cursor moves only once every post is out.

use crate::bot::{Bot, BotInfo, Run, RunError};
use crate::bots::good_morning::visibility_setting;
use crate::mastodon::{NewStatus, Visibility};
use crate::nanacoin::{self, format_nc, Event};
use crate::schedule::Schedule;
use crate::settings::{schedule_from, schedule_settings, Kind, Setting, Settings};
use crate::text::fit;
use crate::tz::Tz;

pub struct NanaNews;

/// A category of news: its checkbox setting (`c_…`), label, whether it is
/// ticked by default, and how a roll-up names it.
struct Category {
    key: &'static str,
    label: &'static str,
    on: bool,
    emoji: &'static str,
    plural: &'static str,
}

const fn cat(
    key: &'static str,
    label: &'static str,
    on: bool,
    emoji: &'static str,
    plural: &'static str,
) -> Category {
    Category {
        key,
        label,
        on,
        emoji,
        plural,
    }
}

/// In posting order. `summary` and `bots` are not feed kinds: one adds a
/// closing summary line, the other lets bot members' events through.
const CATEGORIES: [Category; 16] = [
    cat("c_lotto_open", "Lotto opened", true, "🎟", "new lottos"),
    cat("c_lotto_won", "Lotto winners", true, "🎉", "lottos drawn"),
    cat("c_for_sale", "New for sale", true, "🛒", "new for sale"),
    cat("c_wanted", "New wanted", true, "🙋", "new wanted"),
    cat("c_deeds", "Good deeds", true, "⭐", "new good deeds"),
    cat("c_loan_ask", "Loan requests", true, "🏦", "loan requests"),
    cat("c_gift_ask", "Gift requests", true, "🙏", "gift requests"),
    cat("c_joined", "New members", true, "👋", "new members"),
    cat("c_summary", "Summary line", true, "📊", ""),
    cat("c_sold", "Sold", false, "✅", "sold"),
    cat(
        "c_fx_quote",
        "Forex quotes",
        false,
        "💱",
        "new forex quotes",
    ),
    cat("c_fx_trade", "Forex trades", false, "💱", "forex trades"),
    cat("c_loan_made", "Loans made", false, "🤝", "loans made"),
    cat(
        "c_loan_paid",
        "Loans paid off",
        false,
        "✔",
        "loans paid off",
    ),
    cat("c_art", "Art", false, "🎨", "new art"),
    cat("c_bots", "Bots' own activity", false, "🤖", ""),
];

/// More events than this in one category become one roll-up line.
const ROLL_UP_OVER: usize = 3;
/// Feed pages (of 50) one run reads at most.
const PAGES: usize = 4;

fn category(e: &Event) -> Option<&'static str> {
    Some(match (e.kind.as_str(), e.side.as_str()) {
        ("lotto_opened", _) => "c_lotto_open",
        ("lotto_drawn", _) => "c_lotto_won",
        ("listing_opened", "BUY") => "c_wanted",
        ("listing_opened", _) => "c_for_sale",
        ("good_deed_posted", _) => "c_deeds",
        ("loan_requested", _) => "c_loan_ask",
        ("gift_request_opened", _) => "c_gift_ask",
        ("member_joined", _) => "c_joined",
        ("listing_sold", _) => "c_sold",
        ("quote_posted", _) => "c_fx_quote",
        ("quote_taken", _) => "c_fx_trade",
        ("loan_funded", _) => "c_loan_made",
        ("loan_paid", _) => "c_loan_paid",
        ("art_minted", _) => "c_art",
        _ => return None,
    })
}

fn quoted(title: &str) -> String {
    format!("“{}”", fit(title.trim(), 48))
}

/// One event's line, without the hashtag.
fn line(e: &Event, decimals: u8, tz: &Tz) -> String {
    let nc = |v: Option<i64>| format_nc(v.unwrap_or(0), decimals);
    let who = e.actor_name.as_str();
    let other = e.other_name.as_str();
    let title = quoted(&e.title);
    match category(e).unwrap_or("") {
        "c_lotto_open" => {
            let closes = e
                .closes_at
                .map(|s| {
                    let l = tz.local(s.saturating_mul(1000));
                    format!(", closes {} {}", &l.weekday_name()[..3], l.clock())
                })
                .unwrap_or_default();
            format!(
                "🎟 New lotto: {title}. {} NC a ticket{closes}.",
                nc(e.amount)
            )
        }
        "c_lotto_won" => format!("🎉 {other} won {title}: {} NC!", nc(e.amount)),
        "c_for_sale" => format!("🛒 {who} is selling {title} for {} NC.", nc(e.amount)),
        "c_wanted" => format!("🙋 {who} wants {title}, pays {} NC.", nc(e.amount)),
        "c_deeds" => format!("⭐ Good deed: {title}. Reward {} NC.", nc(e.amount)),
        "c_loan_ask" => format!(
            "🏦 {who} wants to borrow {} NC at {}%/yr.",
            nc(e.amount),
            format_nc(i64::from(e.apr_bps.unwrap_or(0)), 2)
        ),
        "c_gift_ask" => format!("🙏 {who} is asking: {title}."),
        "c_joined" => format!("👋 Welcome, {who}!"),
        "c_sold" => format!("✅ {title} sold for {} NC.", nc(e.amount)),
        "c_fx_quote" => format!(
            "💱 {who} {} {} NC at {}¢.",
            if e.side == "BID" { "buys" } else { "sells" },
            nc(e.coins),
            e.rate.unwrap_or(0)
        ),
        "c_fx_trade" => format!("💱 {} NC traded at {}¢.", nc(e.coins), e.rate.unwrap_or(0)),
        "c_loan_made" => format!("🤝 {who} lent {other} {} NC.", nc(e.amount)),
        "c_loan_paid" => format!("✔ {who} paid off a loan."),
        "c_art" => match e.editions {
            Some(n) if n > 1 => format!("🎨 {who} made {title} ({n} editions)."),
            _ => format!("🎨 {who} made {title}."),
        },
        _ => String::new(),
    }
}

/// "🛒 5 new for sale: lamp, bike, cookies, +2 more."
fn roll_up(c: &Category, events: &[&Event]) -> String {
    let names: Vec<String> = events
        .iter()
        .map(|e| {
            let name = if e.title.is_empty() {
                &e.actor_name
            } else {
                &e.title
            };
            fit(name.trim(), 24)
        })
        .collect();
    let shown = &names[..names.len().min(3)];
    let more = names.len() - shown.len();
    let more = if more > 0 {
        format!(", +{more} more")
    } else {
        String::new()
    };
    format!(
        "{} {} {}: {}{more}.",
        c.emoji,
        events.len(),
        c.plural,
        shown.join(", ")
    )
}

/// "📊 Since last time: 3 sold, 2 NC trades, 1 NC = 12¢."
fn summary(events: &[&Event]) -> Option<String> {
    let count = |kind: &str| events.iter().filter(|e| e.kind == kind).count();
    let mut parts = Vec::new();
    let listed = count("listing_opened");
    if listed > 0 {
        parts.push(format!("{listed} listed"));
    }
    let sold = count("listing_sold");
    if sold > 0 {
        parts.push(format!("{sold} sold"));
    }
    let trades = count("quote_taken");
    if trades > 0 {
        parts.push(format!(
            "{trades} NC trade{}",
            if trades == 1 { "" } else { "s" }
        ));
    }
    if let Some(rate) = events
        .iter()
        .rev()
        .find(|e| e.kind == "quote_taken")
        .and_then(|e| e.rate)
    {
        parts.push(format!("1 NC = {rate}¢"));
    }
    (!parts.is_empty()).then(|| format!("📊 Since last time: {}.", parts.join(", ")))
}

/// A post to make: its idempotency suffix and text (without hashtag).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub key: String,
    pub text: String,
}

/// The posts for these events, in order, at most `max_posts`.
fn compose(events: &[Event], decimals: u8, settings: &Settings, tz: &Tz) -> (Vec<Draft>, usize) {
    let on = |key: &str| settings.yes(key);
    let kept: Vec<&Event> = events
        .iter()
        .filter(|e| on("c_bots") || !e.actor_bot)
        .collect();
    let mut drafts = Vec::new();
    for c in CATEGORIES
        .iter()
        .filter(|c| !c.plural.is_empty() && on(c.key))
    {
        let these: Vec<&Event> = kept
            .iter()
            .copied()
            .filter(|e| category(e) == Some(c.key))
            .collect();
        if these.len() > ROLL_UP_OVER {
            drafts.push(Draft {
                key: format!("{}-{}", &c.key[2..], these[0].seq),
                text: roll_up(c, &these),
            });
        } else {
            drafts.extend(these.iter().map(|e| Draft {
                key: format!("{}-{}", &c.key[2..], e.seq),
                text: line(e, decimals, tz),
            }));
        }
    }
    let max = settings.number("max_posts").max(1) as usize;
    let summary = on("c_summary")
        .then(|| summary(&kept))
        .flatten()
        .map(|text| Draft {
            key: format!("summary-{}", kept.last().map_or(0, |e| e.seq)),
            text,
        });
    let room = max - usize::from(summary.is_some());
    let dropped = drafts.len().saturating_sub(room);
    drafts.truncate(room);
    drafts.extend(summary);
    (drafts, dropped)
}

impl Bot for NanaNews {
    fn info(&self) -> BotInfo {
        BotInfo {
            id: "nana_news",
            name: "NanaCoin news",
            description:
                "Posts what happened in the NanaCoin economy: lottos, listings, loans and more.",
            schedule: Schedule::Manual,
            // News from the morning is still news by lunch.
            grace_minutes: 180,
            default_instance: "https://mastomini.local",
            uses_llm: false,
            needs_mastodon: true,
        }
    }

    fn settings(&self) -> Vec<Setting> {
        let mut s = schedule_settings("daily", "08:00, 20:00", "us_eastern", "720");
        s.extend(nanacoin::nanacoin_settings());
        s.push(
            Setting::new(
                "max_posts",
                "Most posts per run",
                Kind::Number { min: 1, max: 20 },
                "6",
            )
            .help("Crowded categories are rolled up into one line first; the summary line is always last."),
        );
        s.push(visibility_setting().help(
            "Everything here is visible to every NanaCoin member. On a server outside the household, it becomes public.",
        ));
        s.push(
            Setting::new("hashtag", "Hashtag", Kind::Text, "#nanacoin")
                .optional()
                .help("Added to every post. Empty for none."),
        );
        for c in &CATEGORIES {
            s.push(
                Setting::new(
                    c.key,
                    c.label,
                    Kind::Toggle,
                    if c.on { "yes" } else { "no" },
                )
                .group("Post about"),
            );
        }
        s
    }

    fn schedule(&self, settings: &Settings) -> Schedule {
        schedule_from(settings)
    }

    fn ready(&self, settings: &Settings) -> Result<(), String> {
        nanacoin::ready(settings)
    }

    fn check(&self, run: &mut Run<'_>) -> Result<String, RunError> {
        let acct = run.mastodon().verify_credentials()?;
        let me = run.nanacoin()?.me()?;
        Ok(format!(
            "Mastodon {acct}; NanaCoin reads as {}",
            me.display_name
        ))
    }

    fn run(&self, run: &mut Run<'_>) -> Result<String, RunError> {
        let settings = run.settings;
        let slot = run.slot_ms.to_string();
        let cursor: Option<u64> = run.state.get("cursor").and_then(|v| v.parse().ok());
        let incarnation = run.state.get("incarnation").cloned();
        let pinned: Option<u64> = (run.state.get("slot") == Some(&slot))
            .then(|| run.state.get("upto").and_then(|v| v.parse().ok()))
            .flatten();

        // Read everything from NanaCoin first; post afterwards.
        let mut nc = run.nanacoin()?;
        let first = nc.activity(cursor, 50)?;
        let decimals = first.decimals;
        let fresh = cursor.is_none()
            || incarnation
                .as_deref()
                .is_some_and(|i| i != first.incarnation.to_string());
        if fresh {
            let why = if cursor.is_none() {
                "First run: watching from now"
            } else {
                "The economy was reset: watching from now"
            };
            drop(nc);
            run.state
                .insert("cursor".into(), first.sequence.to_string());
            run.state
                .insert("incarnation".into(), first.incarnation.to_string());
            run.state.remove("slot");
            run.state.remove("upto");
            return Ok(why.into());
        }
        let upto = pinned.unwrap_or(first.sequence);
        let truncated = first.truncated;
        let mut events = first.events;
        for _ in 1..PAGES {
            let last = events.last().map(|e| e.seq);
            match last {
                Some(last) if last < upto && events.len() % 50 == 0 => {
                    let page = nc.activity(Some(last), 50)?;
                    if page.events.is_empty() {
                        break;
                    }
                    events.extend(page.events);
                }
                _ => break,
            }
        }
        drop(nc);
        events.retain(|e| e.seq <= upto);
        run.state.insert("slot".into(), slot);
        run.state.insert("upto".into(), upto.to_string());
        if truncated {
            run.log("Some activity was older than NanaCoin keeps; it was skipped");
        }
        // A page cap reached before `upto`: continue from there next time.
        let reached = if events.last().map(|e| e.seq) < Some(upto) && events.len() >= PAGES * 50 {
            events.last().map_or(upto, |e| e.seq)
        } else {
            upto
        };

        let (drafts, dropped) = compose(&events, decimals, settings, &settings.tz("zone"));
        if dropped > 0 {
            run.log(format!(
                "{dropped} more posts were left out (the most per run)"
            ));
        }
        let limit = if drafts.is_empty() {
            0
        } else {
            run.mastodon().max_characters()?
        };
        let tag = settings.get("hashtag").trim();
        let visibility = Visibility::parse(settings.get("visibility"));
        for d in &drafts {
            let text = if tag.is_empty() {
                fit(&d.text, limit)
            } else {
                let room = limit.saturating_sub(tag.chars().count() + 1);
                format!("{} {tag}", fit(&d.text, room))
            };
            run.post_keyed(
                &NewStatus {
                    text,
                    visibility,
                    spoiler_text: None,
                    in_reply_to_id: None,
                },
                &format!("news-{}", d.key),
            )?;
        }
        run.state.insert("cursor".into(), reached.to_string());
        run.state.remove("slot");
        run.state.remove("upto");
        Ok(match (drafts.len(), events.len()) {
            (_, 0) => "Nothing new".into(),
            (0, n) => format!("{n} events, none in a ticked category"),
            (p, n) => format!("Posted {p} from {n} events"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastodon::testing::Scripted;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    fn settings(values: &[(&str, &str)]) -> Settings {
        let mut map: BTreeMap<String, String> = values
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        map.entry("nc_key".into()).or_insert_with(|| "nc_R".into());
        Settings::new(NanaNews.settings(), map)
    }

    fn ev(seq: u64, kind: &str, title: &str) -> Event {
        Event {
            seq,
            kind: kind.into(),
            actor: "user-2".into(),
            actor_name: "Robin".into(),
            title: title.into(),
            side: "SELL".into(),
            amount: Some(50_000),
            ..Event::default()
        }
    }

    fn utc() -> Tz {
        Tz::parse("UTC0").unwrap()
    }

    #[test]
    fn templates_are_short_and_plain() {
        let tz = utc();
        let mut lotto = ev(1, "lotto_opened", "Friday pot");
        lotto.closes_at = Some(1_790_532_000); // Sunday 2026-09-27 18:00 UTC
        assert_eq!(
            line(&lotto, 4, &tz),
            "🎟 New lotto: “Friday pot”. 5 NC a ticket, closes Sun 18:00."
        );
        let mut won = ev(2, "lotto_drawn", "Friday pot");
        won.other_name = "Sam".into();
        won.amount = Some(1_234_500);
        assert_eq!(line(&won, 4, &tz), "🎉 Sam won “Friday pot”: 123.45 NC!");
        let mut ask = ev(3, "loan_requested", "");
        ask.apr_bps = Some(525);
        assert_eq!(
            line(&ask, 4, &tz),
            "🏦 Robin wants to borrow 5 NC at 5.25%/yr."
        );
        let mut quote = ev(4, "quote_posted", "");
        quote.side = "BID".into();
        quote.coins = Some(100_000);
        quote.rate = Some(12);
        assert_eq!(line(&quote, 4, &tz), "💱 Robin buys 10 NC at 12¢.");
        assert_eq!(
            line(&ev(5, "listing_opened", "Bike tune-up"), 4, &tz),
            "🛒 Robin is selling “Bike tune-up” for 5 NC."
        );
        for c in &CATEGORIES {
            assert!(c.label.chars().count() <= 20, "{}", c.label);
        }
    }

    #[test]
    fn ticked_categories_roll_ups_and_the_cap() {
        let tz = utc();
        let mut events: Vec<Event> = (1..=5)
            .map(|i| ev(i, "listing_opened", &format!("Thing {i}")))
            .collect();
        events.push(ev(6, "member_joined", ""));
        events.push(ev(7, "listing_sold", "Thing 1"));
        let mut bot = ev(8, "member_joined", "");
        bot.actor_bot = true;
        events.push(bot);

        let (drafts, dropped) = compose(&events, 4, &settings(&[]), &tz);
        let texts: Vec<&str> = drafts.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "🛒 5 new for sale: Thing 1, Thing 2, Thing 3, +2 more.",
                "👋 Welcome, Robin!",
                "📊 Since last time: 5 listed, 1 sold.",
            ]
        );
        assert_eq!(dropped, 0);
        assert_eq!(drafts[0].key, "for_sale-1");

        // Sold ticked, joined unticked, bots let through, at most 2 posts.
        let s = settings(&[
            ("c_sold", "yes"),
            ("c_joined", "no"),
            ("c_bots", "yes"),
            ("max_posts", "2"),
        ]);
        let (drafts, dropped) = compose(&events, 4, &s, &tz);
        assert_eq!(drafts.len(), 2);
        assert!(drafts[0].text.starts_with("🛒 5 new"));
        assert!(
            drafts[1].text.starts_with("📊"),
            "the summary is always last"
        );
        assert_eq!(dropped, 1);
    }

    fn feed(sequence: u64, events: Value) -> Value {
        json!({"incarnation": 1, "sequence": sequence, "decimals": 4,
               "truncated": false, "events": events})
    }

    fn run_once(
        state: BTreeMap<String, String>,
        answers: Vec<(u16, Value)>,
    ) -> (
        Result<String, RunError>,
        BTreeMap<String, String>,
        Vec<crate::mastodon::HttpRequest>,
    ) {
        let mut http = Scripted::default();
        for (status, body) in answers {
            http.answer(status, body);
        }
        let sent = http.sent.clone();
        let s = settings(&[]);
        let mut run = Run::new(
            "nana_news",
            1_790_000_000_000,
            1_790_000_000_000,
            &s,
            &mut http,
            "https://mastomini.local",
            "K",
            None,
        );
        run.state = state;
        let result = NanaNews.run(&mut run);
        let state = run.state.clone();
        drop(run);
        let sent = sent.lock().unwrap().clone();
        (result, state, sent)
    }

    #[test]
    fn first_run_watches_from_now_then_posts_once() {
        let (r, state, sent) = run_once(BTreeMap::new(), vec![(200, feed(40, json!([])))]);
        assert_eq!(r.unwrap(), "First run: watching from now");
        assert_eq!(state["cursor"], "40");
        assert_eq!(sent.len(), 1, "no posts");

        let events = json!([
            {"seq": 41, "kind": "lotto_opened", "actor_name": "Nana", "title": "Pot", "amount": 10000},
            {"seq": 42, "kind": "member_joined", "actor_name": "Sam"}
        ]);
        let instance = json!({"configuration": {"statuses": {"max_characters": 140}}});
        let posted = json!({"id": "1", "url": "u"});
        let (r, state, sent) = run_once(
            state,
            vec![
                (200, feed(42, events.clone())),
                (200, instance.clone()),
                (200, posted.clone()),
                (200, posted.clone()),
            ],
        );
        assert_eq!(r.unwrap(), "Posted 2 from 2 events");
        assert_eq!(state["cursor"], "42");
        assert!(!state.contains_key("upto"));
        assert_eq!(
            sent[0].url,
            "https://nanacoin.local/api/v1/activity?limit=50&after=40"
        );
        let body: Value = serde_json::from_slice(&sent[2].body).unwrap();
        assert_eq!(
            body["status"],
            "🎟 New lotto: “Pot”. 1 NC a ticket. #nanacoin"
        );
        assert_eq!(body["visibility"], "public");
        let key = sent[2]
            .headers
            .iter()
            .find(|(k, _)| k == "Idempotency-Key")
            .unwrap()
            .1
            .clone();
        assert_eq!(key, "mastomini-bots:nana_news:news-lotto_open-41");
    }

    #[test]
    fn a_retry_composes_the_same_posts() {
        let mut state = BTreeMap::new();
        state.insert("cursor".to_string(), "40".to_string());
        state.insert("incarnation".to_string(), "1".to_string());
        let events = json!([{"seq": 41, "kind": "member_joined", "actor_name": "Sam"}]);
        let instance = json!({"configuration": {"statuses": {"max_characters": 140}}});
        // The post fails with a server error: retryable, cursor unmoved, upto pinned.
        let (r, state, _) = run_once(
            state,
            vec![
                (200, feed(41, events)),
                (200, instance.clone()),
                (503, json!({"error": "busy"})),
            ],
        );
        assert!(r.unwrap_err().retryable);
        assert_eq!(state["cursor"], "40");
        assert_eq!(state["upto"], "41");
        // The retry sees a newer event too, but only posts up to 41.
        let events = json!([
            {"seq": 41, "kind": "member_joined", "actor_name": "Sam"},
            {"seq": 42, "kind": "member_joined", "actor_name": "Kim"}
        ]);
        let (r, state, sent) = run_once(
            state,
            vec![
                (200, feed(42, events)),
                (200, instance),
                (200, json!({"id": "1", "url": "u"})),
            ],
        );
        assert_eq!(r.unwrap(), "Posted 1 from 1 events");
        assert_eq!(state["cursor"], "41");
        assert_eq!(sent.len(), 3);
    }

    #[test]
    fn a_reset_economy_starts_over() {
        let mut state = BTreeMap::new();
        state.insert("cursor".to_string(), "900".to_string());
        state.insert("incarnation".to_string(), "1".to_string());
        let mut f = feed(3, json!([]));
        f["incarnation"] = json!(2);
        let (r, state, _) = run_once(state, vec![(200, f)]);
        assert_eq!(r.unwrap(), "The economy was reset: watching from now");
        assert_eq!(state["cursor"], "3");
        assert_eq!(state["incarnation"], "2");
    }

    #[test]
    fn needs_a_nanacoin_key_to_turn_on() {
        let s = Settings::new(NanaNews.settings(), BTreeMap::new());
        assert!(NanaNews.ready(&s).is_err());
        assert!(NanaNews.ready(&settings(&[])).is_ok());
        assert_eq!(
            NanaNews.schedule(&s).describe(),
            "Every day at 08:00 and 20:00 US Eastern"
        );
    }
}
