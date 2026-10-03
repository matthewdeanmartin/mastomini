//! The NanaCoin trader: a bot member of the NanaCoin economy that owns its
//! own money and trades by a strategy (forex band, lotto, lending...).
//!
//! One type, listed several times (`trader_1`...), each with its own NanaCoin
//! key, strategy and settings. A run reads the market, asks its strategy
//! what to do ([`strategy::decide`]), keeps only what [`guard`] allows, and
//! does it. Mastodon is optional: when the bot has a server and key it
//! posts its trades.
//!
//! **Dry run** is on until the admin turns it off: the activity log says
//! what the bot would do, and no money moves.

pub mod strategy;

use crate::bot::{Bot, BotInfo, Run, RunError};
use crate::bots::good_morning::visibility_setting;
use crate::mastodon::{NewStatus, Visibility};
use crate::nanacoin::{self, format_nc, minor, NanaError};
use crate::schedule::Schedule;
use crate::settings::{schedule_from, schedule_settings, Kind, Setting, Settings};
use strategy::{Action, Market, STRATEGIES};

pub struct Trader {
    pub id: &'static str,
    pub name: &'static str,
}

/// Forex rates kept for the strategies that follow the market.
const RATES_KEPT: usize = 20;

fn number(key: &'static str, label: &'static str, max: i64, default: &'static str) -> Setting {
    Setting::new(key, label, Kind::Number { min: 0, max }, default)
}

/// Money rules every strategy obeys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Minor units / cents never spent.
    pub reserve_nc: i64,
    pub reserve_cents: i64,
    /// NC (minor units) this run may still commit.
    pub run_nc: i64,
    /// What today's caps leave.
    pub day_nc: i64,
    pub day_cents: i64,
    pub allow_bots: bool,
}

/// The actions the limits allow, in order, and why the others were left out.
pub fn guard(actions: Vec<Action>, m: &Market, limits: &Limits) -> (Vec<Action>, Vec<String>) {
    let mut nc = m.me.balance;
    let mut cents = m.me.usd_cents;
    let (mut run_nc, mut day_nc, mut day_cents) = (limits.run_nc, limits.day_nc, limits.day_cents);
    let mut allowed = Vec::new();
    let mut refused = Vec::new();
    for action in actions {
        let what = action.describe(m.decimals);
        if let Some(other) = action.counterparty() {
            if other == m.me.account {
                refused.push(format!("{what}: that is the bot itself"));
                continue;
            }
            if !limits.allow_bots && m.bots.iter().any(|b| b == other) {
                refused.push(format!("{what}: the other side is a bot"));
                continue;
            }
        }
        let (out_nc, out_cents) = action.outflow(m.decimals);
        let reason = if nc - out_nc < limits.reserve_nc {
            Some("it would go under the NC reserve")
        } else if cents - out_cents < limits.reserve_cents {
            Some("it would go under the cash reserve")
        } else if out_nc > run_nc {
            Some("over the most NC per run")
        } else if out_nc > day_nc {
            Some("over the most NC per day")
        } else if out_cents > day_cents {
            Some("over the most cash per day")
        } else {
            None
        };
        if let Some(reason) = reason {
            refused.push(format!("{what}: {reason}"));
            continue;
        }
        nc -= out_nc;
        cents -= out_cents;
        run_nc -= out_nc;
        day_nc -= out_nc;
        day_cents -= out_cents;
        allowed.push(action);
    }
    (allowed, refused)
}

fn execute(nc: &mut nanacoin::NanaCoin<'_>, action: &Action, now: u64) -> Result<(), NanaError> {
    match action {
        Action::Take { quote, .. } => nc.take_quote(quote),
        // Its own quotes lapse after a day if the bot stops tending them.
        Action::Post { side, rate, coins } => {
            nc.post_quote(side, *rate, *coins, now + 86_400).map(|_| ())
        }
        Action::Cancel { quote } => nc.cancel_quote(quote),
        Action::Tickets { lotto, count, .. } => nc.buy_tickets(*lotto, *count),
        Action::Accept { loan, .. } => nc.accept_loan(*loan),
        Action::Lend { request, terms, .. } => nc.offer_loan(*request, terms),
        Action::Ask { terms, .. } => nc.request_loan(terms),
    }
}

fn strategy_choices() -> Vec<(&'static str, &'static str)> {
    STRATEGIES.to_vec()
}

impl Bot for Trader {
    fn info(&self) -> BotInfo {
        BotInfo {
            id: self.id,
            name: self.name,
            description: "Trades on NanaCoin with its own money, by a strategy. Posts its trades when it has a Mastodon key.",
            schedule: Schedule::Manual,
            // An hour-old view of the market is stale.
            grace_minutes: 30,
            default_instance: "https://mastomini.local",
            uses_llm: false,
            needs_mastodon: false,
        }
    }

    fn settings(&self) -> Vec<Setting> {
        let mut s = schedule_settings("every", "09:00", "us_eastern", "60");
        s.extend(nanacoin::nanacoin_settings());
        s.push(Setting::new(
            "strategy",
            "Strategy",
            Kind::Choice {
                options: strategy_choices(),
            },
            "band",
        ));
        s.push(
            Setting::new(
                "dry_run",
                "Dry run: only say what it would do",
                Kind::Toggle,
                "yes",
            )
            .help("Turn off to trade with real money."),
        );
        s.push(
            Setting::new(
                "post_trades",
                "Post its trades to Mastodon",
                Kind::Toggle,
                "yes",
            )
            .help("Only when a Mastodon server and key are set above."),
        );
        s.push(visibility_setting());
        s.push(
            Setting::new("allow_bots", "Trade with other bots", Kind::Toggle, "no").help(
                "Off: two bots never trade with each other, so they can't ping-pong the book.",
            ),
        );
        // Money limits: whole NC and US cents.
        let limits = "Money limits";
        s.push(number("reserve_nc", "Always keep (NC)", 1_000_000, "10").group(limits));
        s.push(number("reserve_cents", "Always keep (¢)", 10_000_000, "100").group(limits));
        s.push(number("max_run_nc", "Most NC per run", 1_000_000, "50").group(limits));
        s.push(number("max_day_nc", "Most NC per day", 1_000_000, "200").group(limits));
        s.push(number("max_day_cents", "Most ¢ per day", 10_000_000, "2000").group(limits));

        let when = |key: &'static str| [key];
        // 1. Forex band
        let band = when("band");
        s.push(
            number("band_buy", "Buy at or under (¢ per NC)", 10_000, "10")
                .shown_when("strategy", &band),
        );
        s.push(
            number("band_sell", "Sell at or over (¢ per NC)", 10_000, "14")
                .shown_when("strategy", &band),
        );
        s.push(number("band_size", "NC per trade", 100_000, "5").shown_when("strategy", &band));
        s.push(
            number("band_max_nc", "Hold at most (NC)", 1_000_000, "500")
                .shown_when("strategy", &band),
        );
        s.push(
            Setting::new(
                "band_mode",
                "How",
                Kind::Choice {
                    options: vec![
                        ("take", "Take quotes in the band"),
                        ("make", "Keep its own quotes on the book"),
                        ("both", "Both"),
                    ],
                },
                "take",
            )
            .shown_when("strategy", &band),
        );
        // 2. Lotto regular
        let lotto = when("lotto");
        s.push(number("lotto_spend", "NC per lotto", 100_000, "10").shown_when("strategy", &lotto));
        s.push(
            number(
                "lotto_max_price",
                "Skip tickets over (NC; 0: any)",
                100_000,
                "0",
            )
            .shown_when("strategy", &lotto),
        );
        s.push(
            Setting::new(
                "lotto_kinds",
                "Which lottos",
                Kind::Choice {
                    options: vec![
                        ("any", "Any"),
                        ("simple", "Simple"),
                        ("delayed", "Delayed"),
                        ("savings", "Savings (your money back, plus interest)"),
                    ],
                },
                "any",
            )
            .shown_when("strategy", &lotto),
        );
        // 3. Cheap borrower
        let borrow = when("borrow");
        s.push(
            number("borrow_max_apr", "Accept under (% a year)", 1_000, "5")
                .shown_when("strategy", &borrow),
        );
        s.push(
            number("borrow_max_total", "Owe at most (NC)", 1_000_000, "100")
                .shown_when("strategy", &borrow),
        );
        s.push(
            number(
                "borrow_ask",
                "Ask to borrow (NC; 0: don't ask)",
                1_000_000,
                "0",
            )
            .shown_when("strategy", &borrow)
            .help("Keeps one loan request open so people can offer the bot loans."),
        );
        // 4. Bot bank
        let lend = when("lend");
        s.push(
            number("lend_min_apr", "Lend at or over (% a year)", 1_000, "8")
                .shown_when("strategy", &lend),
        );
        s.push(
            number("lend_max_each", "Most per loan (NC)", 1_000_000, "50")
                .shown_when("strategy", &lend),
        );
        s.push(
            number("lend_max_total", "Most lent in all (NC)", 1_000_000, "200")
                .shown_when("strategy", &lend),
        );
        // 5. Steady buyer
        let dca = when("dca");
        s.push(
            Setting::new(
                "dca_side",
                "Each run",
                Kind::Choice {
                    options: vec![("buy", "Buy NC"), ("sell", "Sell NC")],
                },
                "buy",
            )
            .shown_when("strategy", &dca),
        );
        s.push(number("dca_size", "NC each run", 100_000, "2").shown_when("strategy", &dca));
        s.push(
            number("dca_limit", "Limit (¢ per NC)", 10_000, "12")
                .shown_when("strategy", &dca)
                .help("Buying: pay at most this. Selling: take at least this."),
        );
        // 6. Rebalancer
        let reb = when("rebalance");
        s.push(
            number("reb_target", "Keep in NC (% of value)", 100, "50").shown_when("strategy", &reb),
        );
        s.push(number("reb_band", "Trade when off by (%)", 100, "10").shown_when("strategy", &reb));
        s.push(number("reb_size", "Most NC per trade", 100_000, "20").shown_when("strategy", &reb));
        // 7. Mean reversion
        let rev = when("revert");
        s.push(
            number(
                "rev_window",
                "Average of the last (trades)",
                RATES_KEPT as i64,
                "10",
            )
            .shown_when("strategy", &rev),
        );
        s.push(
            number("rev_pct", "Trade when off the average by (%)", 100, "10")
                .shown_when("strategy", &rev),
        );
        s.push(number("rev_size", "Most NC per trade", 100_000, "10").shown_when("strategy", &rev));
        s
    }

    fn schedule(&self, settings: &Settings) -> Schedule {
        schedule_from(settings)
    }

    fn ready(&self, settings: &Settings) -> Result<(), String> {
        nanacoin::ready(settings)?;
        strategy::check(settings.get("strategy"), settings)
    }

    fn check(&self, run: &mut Run<'_>) -> Result<String, RunError> {
        let mut nc = run.nanacoin()?;
        let decimals = nc.status()?.decimals;
        let me = nc.me()?;
        drop(nc);
        let mut text = format!(
            "NanaCoin: {} has {} NC and {}¢{}",
            me.display_name,
            format_nc(me.balance, decimals),
            me.usd_cents,
            if me.is_bot() {
                ""
            } else {
                " (not a bot account: it can only dry-run)"
            }
        );
        if run.has_mastodon() {
            let acct = run.mastodon().verify_credentials()?;
            text.push_str(&format!("; Mastodon {acct}"));
        }
        Ok(text)
    }

    fn run(&self, run: &mut Run<'_>) -> Result<String, RunError> {
        let s = run.settings;
        let strategy = s.get("strategy");
        let dry = s.yes("dry_run");
        let allow_bots = s.yes("allow_bots");
        let now_ms = run.now_ms;
        let today = {
            let l = s.tz("zone").local(now_ms);
            format!("{}-{:02}-{:02}", l.year, l.month, l.day)
        };
        let mut state = run.state.clone();
        if state.get("day") != Some(&today) {
            state.insert("day".into(), today);
            state.insert("spent_nc".into(), "0".into());
            state.insert("spent_cents".into(), "0".into());
        }
        let spent = |k: &str| -> i64 { state.get(k).and_then(|v| v.parse().ok()).unwrap_or(0) };
        let (spent_nc, spent_cents) = (spent("spent_nc"), spent("spent_cents"));
        let mut rates: Vec<i64> = state
            .get("rates")
            .map(|v| v.split(',').filter_map(|r| r.parse().ok()).collect())
            .unwrap_or_default();

        // Read the market and act, all on NanaCoin; post afterwards.
        let mut nc = run.nanacoin()?;
        let decimals = nc.status()?.decimals;
        let me = nc.me()?;
        if !me.is_bot() && !dry {
            return Err(RunError::permanent(
                "This key belongs to a person's account. Traders trade only from a bot member (dry run still works).",
            ));
        }
        let mut market = Market {
            me,
            decimals,
            now: now_ms / 1000,
            ..Market::default()
        };
        if matches!(strategy, "band" | "dca" | "rebalance" | "revert") {
            market.quotes = nc.quotes()?.into_iter().filter(|q| q.open()).collect();
        }
        if strategy == "lotto" {
            market.lottos = nc.lottos()?;
        }
        if matches!(strategy, "borrow" | "lend") {
            market.loans = nc.loans()?;
        }
        if !allow_bots {
            market.bots = nc
                .users()?
                .into_iter()
                .filter(|u| u.is_bot())
                .map(|u| u.account)
                .collect();
        }
        if matches!(strategy, "rebalance" | "revert") {
            let cursor = state.get("fx_cursor").and_then(|v| v.parse().ok());
            let feed = nc.activity(cursor, 50)?;
            rates.extend(
                feed.events
                    .iter()
                    .filter(|e| e.kind == "quote_taken")
                    .filter_map(|e| e.rate),
            );
            let next = feed.events.last().map_or(feed.sequence, |e| e.seq);
            state.insert("fx_cursor".into(), next.to_string());
            if rates.len() > RATES_KEPT {
                rates.drain(..rates.len() - RATES_KEPT);
            }
            state.insert(
                "rates".into(),
                rates
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        market.rates = rates;

        let limits = Limits {
            reserve_nc: minor(s.number("reserve_nc"), decimals),
            reserve_cents: s.number("reserve_cents"),
            run_nc: minor(s.number("max_run_nc"), decimals),
            day_nc: minor(s.number("max_day_nc"), decimals) - spent_nc,
            day_cents: s.number("max_day_cents") - spent_cents,
            allow_bots,
        };
        let (actions, refused) = guard(strategy::decide(strategy, &market, s), &market, &limits);

        let mut done: Vec<&Action> = Vec::new();
        let mut failure: Option<NanaError> = None;
        let (mut out_nc, mut out_cents) = (0, 0);
        if !dry {
            for action in &actions {
                match execute(&mut nc, action, market.now) {
                    Ok(()) => {
                        let (n, c) = action.outflow(decimals);
                        out_nc += n;
                        out_cents += c;
                        done.push(action);
                    }
                    Err(e) => {
                        failure = Some(e);
                        break;
                    }
                }
            }
        }
        drop(nc);

        for why in &refused {
            run.log(format!("Left out: {why}"));
        }
        if dry {
            for action in &actions {
                run.log(format!("Would: {}", action.describe(decimals)));
            }
        }
        for action in &done {
            run.log(action.describe(decimals));
        }
        state.insert("spent_nc".into(), (spent_nc + out_nc).to_string());
        state.insert("spent_cents".into(), (spent_cents + out_cents).to_string());
        run.state = state;

        if !dry && s.yes("post_trades") && run.has_mastodon() {
            let visibility = Visibility::parse(s.get("visibility"));
            let slot = run.slot_ms;
            for action in done.iter().filter(|a| a.newsworthy()) {
                let status = NewStatus {
                    text: format!("🤖 {}.", action.describe(decimals)),
                    visibility,
                    spoiler_text: None,
                    in_reply_to_id: None,
                };
                // A trade is done either way: a failed post is only noted.
                if let Err(e) = run.post_keyed(&status, &format!("{slot}-{}", action.key())) {
                    run.log(format!("Couldn't post a trade: {}", e.message));
                    break;
                }
            }
        }

        if let Some(e) = failure {
            return Err(RunError {
                message: format!("Stopped after {} of {}: {e}", done.len(), actions.len()),
                retryable: e.retryable(),
            });
        }
        Ok(match (dry, actions.len(), refused.len()) {
            (true, n, _) => format!(
                "Dry run: would do {n} thing{}",
                if n == 1 { "" } else { "s" }
            ),
            (false, 0, 0) => "Nothing to do".into(),
            (false, 0, r) => format!("Nothing within its limits ({r} left out)"),
            (false, n, _) => format!("Did {n} thing{}", if n == 1 { "" } else { "s" }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastodon::testing::Scripted;
    use crate::mastodon::HttpRequest;
    use crate::nanacoin::Member;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    const NC: i64 = 10_000;

    fn trader() -> Trader {
        Trader {
            id: "trader_1",
            name: "Trader 1",
        }
    }

    fn settings(values: &[(&str, &str)]) -> Settings {
        let mut map: BTreeMap<String, String> = values
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        map.entry("nc_key".into()).or_insert_with(|| "nc_B".into());
        Settings::new(trader().settings(), map)
    }

    fn market(balance: i64, cents: i64) -> Market {
        Market {
            me: Member {
                account: "account-9".into(),
                balance: balance * NC,
                usd_cents: cents,
                kind: "bot".into(),
                ..Member::default()
            },
            decimals: 4,
            bots: vec!["account-7".into()],
            ..Market::default()
        }
    }

    fn take(quote: &str, maker: &str, side: &str, coins: i64, cents: i64) -> Action {
        Action::Take {
            quote: quote.into(),
            side: side.into(),
            rate: 10,
            coins: coins * NC,
            cents,
            maker: maker.into(),
            maker_name: "M".into(),
        }
    }

    fn limits() -> Limits {
        Limits {
            reserve_nc: 10 * NC,
            reserve_cents: 100,
            run_nc: 50 * NC,
            day_nc: 60 * NC,
            day_cents: 1_000,
            allow_bots: false,
        }
    }

    #[test]
    fn guard_rails() {
        let m = market(100, 1_000);
        let actions = vec![
            take("quote-1", "account-9", "ASK", 1, 10),  // itself
            take("quote-2", "account-7", "ASK", 1, 10),  // a bot
            take("quote-3", "account-2", "BID", 40, 0),  // sells 40 NC: fine
            take("quote-4", "account-2", "BID", 20, 0),  // 60 > 50 per run
            take("quote-5", "account-2", "ASK", 5, 950), // under the cash reserve
            take("quote-6", "account-2", "ASK", 5, 800), // fine
        ];
        let (allowed, refused) = guard(actions, &m, &limits());
        assert_eq!(
            allowed.iter().map(Action::key).collect::<Vec<_>>(),
            vec!["take-quote-3", "take-quote-6"]
        );
        assert_eq!(refused.len(), 4);
        assert!(refused[0].ends_with("that is the bot itself"));
        assert!(refused[1].ends_with("the other side is a bot"));
        assert!(refused[2].ends_with("over the most NC per run"));
        assert!(refused[3].ends_with("under the cash reserve"));
        let mut low = limits();
        low.reserve_nc = 70 * NC;
        let (allowed, refused) = guard(vec![take("quote-3", "account-2", "BID", 40, 0)], &m, &low);
        assert!(allowed.is_empty());
        assert!(refused[0].ends_with("under the NC reserve"));
    }

    /// A run's result, log, memory and every request it sent.
    type Outcome = (
        Result<String, RunError>,
        Vec<String>,
        BTreeMap<String, String>,
        Vec<HttpRequest>,
    );

    fn run_with(values: &[(&str, &str)], mastodon: bool, answers: Vec<(u16, Value)>) -> Outcome {
        let mut http = Scripted::default();
        for (status, body) in answers {
            http.answer(status, body);
        }
        let sent = http.sent.clone();
        let s = settings(values);
        let (instance, token) = if mastodon {
            ("https://mastomini.local", "K")
        } else {
            ("", "")
        };
        let at = 1_790_000_000_000;
        let mut run = Run::new("trader_1", at, at, &s, &mut http, instance, token, None);
        let result = trader().run(&mut run);
        let (log, state) = (run.log.clone(), run.state.clone());
        drop(run);
        let sent = sent.lock().unwrap().clone();
        (result, log, state, sent)
    }

    fn status() -> Value {
        json!({"decimals": 4, "journal_generation": 2})
    }

    fn me(kind: &str) -> Value {
        json!({"id": "user-9", "account": "account-9", "display_name": "Bot",
               "kind": kind, "balance": 100 * NC, "usd_cents": 5000})
    }

    fn book() -> Value {
        let quote = |id: &str, side: &str, rate: i64| {
            json!({"id": id, "maker": "account-2", "maker_name": "Robin", "side": side,
                   "cents_per_coin": rate, "coins": 5 * NC, "cents": 5 * rate,
                   "status": "OPEN", "expires_at": 0, "live": true})
        };
        json!({"quotes": [quote("quote-1", "ASK", 9), quote("quote-2", "BID", 15)]})
    }

    fn users() -> Value {
        json!({"users": [{"id": "user-2", "account": "account-2", "kind": "human"}]})
    }

    #[test]
    fn dry_run_moves_no_money() {
        let (r, log, _, sent) = run_with(
            &[],
            true,
            vec![
                (200, status()),
                (200, me("human")),
                (200, book()),
                (200, users()),
            ],
        );
        assert_eq!(r.unwrap(), "Dry run: would do 2 things");
        assert_eq!(
            log,
            vec![
                "Would: Bought 5 NC at 9¢ from Robin",
                "Would: Sold 5 NC at 15¢ to Robin"
            ]
        );
        assert!(
            sent.iter().all(|r| r.method == "GET"),
            "no mutations, no posts"
        );
    }

    #[test]
    fn live_trades_are_keyed_counted_and_posted() {
        let posted = json!({"id": "1", "url": "u"});
        let (r, _, state, sent) = run_with(
            &[("dry_run", "no")],
            true,
            vec![
                (200, status()),
                (200, me("bot")),
                (200, book()),
                (200, users()),
                (201, json!({})),
                (201, json!({})),
                (200, posted.clone()),
                (200, posted),
            ],
        );
        assert_eq!(r.unwrap(), "Did 2 things");
        let take = &sent[4];
        assert_eq!(
            take.url,
            "https://nanacoin.local/api/v1/quotes/quote-1/take"
        );
        assert!(take.headers.contains(&(
            "Idempotency-Key".into(),
            "g2:m0:mmb:trader_1:slot:1790000000000:take-quote-1".into()
        )));
        let post: Value = serde_json::from_slice(&sent[6].body).unwrap();
        assert_eq!(post["status"], "🤖 Bought 5 NC at 9¢ from Robin.");
        assert_eq!(state["spent_nc"], (5 * NC).to_string(), "sold 5 NC");
        assert_eq!(state["spent_cents"], "45", "paid 45¢");
    }

    #[test]
    fn without_mastodon_it_trades_quietly_and_stops_at_a_money_error() {
        let (r, log, state, sent) = run_with(
            &[("dry_run", "no")],
            false,
            vec![
                (200, status()),
                (200, me("bot")),
                (200, book()),
                (200, users()),
                (201, json!({})),
                (
                    409,
                    json!({"error": "insufficient_funds", "message": "Not enough NC"}),
                ),
            ],
        );
        let e = r.unwrap_err();
        assert!(!e.retryable);
        assert_eq!(
            e.message,
            "Stopped after 1 of 2: NanaCoin HTTP 409: Not enough NC"
        );
        assert_eq!(log, vec!["Bought 5 NC at 9¢ from Robin"]);
        assert_eq!(state["spent_cents"], "45");
        assert!(sent.iter().all(|r| !r.url.contains("mastomini")));
    }

    #[test]
    fn a_person_s_key_is_refused_for_real_trading() {
        let (r, _, _, _) = run_with(
            &[("dry_run", "no")],
            false,
            vec![(200, status()), (200, me("human"))],
        );
        assert!(r.unwrap_err().message.contains("bot member"));
    }

    #[test]
    fn settings_show_one_strategy_at_a_time_and_check_on_turning_on() {
        let specs = trader().settings();
        let band = specs.iter().find(|s| s.key == "band_buy").unwrap();
        assert_eq!(band.shown_when.as_ref().unwrap().values, vec!["band"]);
        assert!(trader().ready(&settings(&[])).is_ok());
        assert!(trader()
            .ready(&settings(&[("band_buy", "20"), ("band_sell", "10")]))
            .is_err());
        assert!(!trader().info().needs_mastodon);
        assert_eq!(trader().schedule(&settings(&[])).describe(), "Every hour");
    }
}
