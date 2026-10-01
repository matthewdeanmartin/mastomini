//! Trading strategies: pure functions from a market snapshot and the bot's
//! settings to the actions it would like to take. They never call the
//! network and never check money limits: [`super::guard`] does that for
//! every strategy alike, including the language-model one on the roadmap,
//! which will propose the same [`Action`]s.

use crate::nanacoin::{format_nc, minor, Loan, LoanTerms, Lotto, Member, Quote};
use crate::settings::Settings;

/// What the strategy sees.
#[derive(Debug, Clone, Default)]
pub struct Market {
    pub me: Member,
    pub decimals: u8,
    /// Unix seconds.
    pub now: u64,
    /// Open quotes, the bot's own included.
    pub quotes: Vec<Quote>,
    pub lottos: Vec<Lotto>,
    pub loans: Vec<Loan>,
    /// Accounts of bot members (`account-N`).
    pub bots: Vec<String>,
    /// Recent forex trade rates, oldest first.
    pub rates: Vec<i64>,
}

impl Market {
    fn whole(&self, setting: i64) -> i64 {
        minor(setting, self.decimals)
    }

    fn mine(&self, q: &Quote) -> bool {
        q.maker == self.me.account
    }

    /// The bot's own open quotes on one side.
    fn own(&self, side: &'static str) -> impl Iterator<Item = &Quote> + '_ {
        self.quotes
            .iter()
            .filter(move |q| self.mine(q) && q.side == side && q.open())
    }

    /// The cheapest open ASK (someone selling NC) not the bot's own.
    fn best_ask(&self, most_coins: i64) -> Option<&Quote> {
        self.quotes
            .iter()
            .filter(|q| q.side == "ASK" && q.open() && !self.mine(q) && q.coins <= most_coins)
            .min_by_key(|q| (q.cents_per_coin, q.id.clone()))
    }

    /// The richest open BID (someone buying NC) not the bot's own.
    fn best_bid(&self, most_coins: i64) -> Option<&Quote> {
        self.quotes
            .iter()
            .filter(|q| q.side == "BID" && q.open() && !self.mine(q) && q.coins <= most_coins)
            .max_by_key(|q| (q.cents_per_coin, std::cmp::Reverse(q.id.clone())))
    }

    /// The last trade rate, else the middle of the book.
    pub fn price(&self) -> Option<i64> {
        if let Some(r) = self.rates.last() {
            return Some(*r);
        }
        let ask = self.best_ask(i64::MAX).map(|q| q.cents_per_coin);
        let bid = self.best_bid(i64::MAX).map(|q| q.cents_per_coin);
        match (ask, bid) {
            (Some(a), Some(b)) => Some((a + b) / 2),
            (a, b) => a.or(b),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Take a whole quote. An ASK quote: the bot buys NC; a BID: it sells.
    Take {
        quote: String,
        side: String,
        rate: i64,
        coins: i64,
        cents: i64,
        maker: String,
        maker_name: String,
    },
    /// Put a quote on the book: `BID` to buy NC, `ASK` to sell.
    Post {
        side: &'static str,
        rate: i64,
        coins: i64,
    },
    Cancel {
        quote: String,
    },
    Tickets {
        lotto: u64,
        title: String,
        count: u32,
        cost: i64,
        house: String,
    },
    /// Accept a loan offered to the bot.
    Accept {
        loan: u64,
        amount: i64,
        apr_bps: u32,
        lender: String,
        lender_name: String,
    },
    /// Offer a loan in answer to someone's request.
    Lend {
        request: u64,
        terms: LoanTerms,
        apr_bps: u32,
        borrower_name: String,
    },
    /// Ask to borrow.
    Ask {
        terms: LoanTerms,
        apr_bps: u32,
    },
}

impl Action {
    /// The other party's account, for the no-bots and not-myself rules.
    pub fn counterparty(&self) -> Option<&str> {
        match self {
            Action::Take { maker, .. } => Some(maker),
            Action::Tickets { house, .. } => Some(house),
            Action::Accept { lender, .. } => Some(lender),
            Action::Lend { terms, .. } => Some(&terms.borrower),
            _ => None,
        }
    }

    /// NC and US cents that could leave the bot's accounts.
    pub fn outflow(&self, decimals: u8) -> (i64, i64) {
        match self {
            Action::Take { side, coins, .. } if side == "BID" => (*coins, 0),
            Action::Take { cents, .. } => (0, *cents),
            Action::Post {
                side: "ASK", coins, ..
            } => (*coins, 0),
            Action::Post { rate, coins, .. } => (0, cents_for(*coins, *rate, decimals)),
            Action::Tickets { cost, .. } => (*cost, 0),
            Action::Lend { terms, .. } => (terms.amount, 0),
            _ => (0, 0),
        }
    }

    /// Stable within a slot: part of the idempotency key.
    pub fn key(&self) -> String {
        match self {
            Action::Take { quote, .. } => format!("take-{quote}"),
            Action::Post { side, rate, .. } => format!("{}-{rate}", side.to_ascii_lowercase()),
            Action::Cancel { quote } => format!("cancel-{quote}"),
            Action::Tickets { lotto, .. } => format!("lotto-{lotto}"),
            Action::Accept { loan, .. } => format!("accept-{loan}"),
            Action::Lend { request, .. } => format!("lend-{request}"),
            Action::Ask { .. } => "ask-loan".into(),
        }
    }

    /// "Bought 10 NC at 11¢ from Robin"
    pub fn describe(&self, decimals: u8) -> String {
        let nc = |v: i64| format_nc(v, decimals);
        let pct = |bps: u32| format_nc(i64::from(bps), 2);
        match self {
            Action::Take {
                side,
                rate,
                coins,
                maker_name,
                ..
            } if side == "ASK" => format!("Bought {} NC at {rate}¢ from {maker_name}", nc(*coins)),
            Action::Take {
                rate,
                coins,
                maker_name,
                ..
            } => format!("Sold {} NC at {rate}¢ to {maker_name}", nc(*coins)),
            Action::Post { side, rate, coins } => format!(
                "Offered to {} {} NC at {rate}¢",
                if *side == "BID" { "buy" } else { "sell" },
                nc(*coins)
            ),
            Action::Cancel { quote } => format!("Withdrew {quote}"),
            Action::Tickets {
                title, count, cost, ..
            } => format!(
                "Bought {count} ticket{} in “{title}” for {} NC",
                if *count == 1 { "" } else { "s" },
                nc(*cost)
            ),
            Action::Accept {
                amount,
                apr_bps,
                lender_name,
                ..
            } => format!(
                "Borrowed {} NC from {lender_name} at {}%/yr",
                nc(*amount),
                pct(*apr_bps)
            ),
            Action::Lend {
                terms,
                apr_bps,
                borrower_name,
                ..
            } => format!(
                "Offered {borrower_name} a loan of {} NC at {}%/yr",
                nc(terms.amount),
                pct(*apr_bps)
            ),
            Action::Ask { terms, apr_bps } => format!(
                "Asked to borrow {} NC at up to {}%/yr",
                nc(terms.amount),
                pct(*apr_bps)
            ),
        }
    }

    /// Worth a Mastodon post: money moved or a public offer was made.
    pub fn newsworthy(&self) -> bool {
        !matches!(self, Action::Cancel { .. })
    }
}

/// US cents for `coins` minor units at `rate` cents per whole NC.
pub fn cents_for(coins: i64, rate: i64, decimals: u8) -> i64 {
    (i128::from(coins) * i128::from(rate) / 10i128.pow(u32::from(decimals))) as i64
}

fn take(q: &Quote) -> Action {
    Action::Take {
        quote: q.id.clone(),
        side: q.side.clone(),
        rate: q.cents_per_coin,
        coins: q.coins,
        cents: q.cents,
        maker: q.maker.clone(),
        maker_name: q.maker_name.clone(),
    }
}

/// The strategies, as (setting value, label) for the Strategy choice.
pub const STRATEGIES: [(&str, &str); 7] = [
    ("band", "Forex band: buy at X, sell at Y"),
    ("lotto", "Lotto regular: buy tickets in every lotto"),
    ("borrow", "Cheap borrower: accept loans under a rate"),
    ("lend", "Bot bank: lend to requests over a rate"),
    ("dca", "Steady buyer: a little every run"),
    ("rebalance", "Rebalancer: keep a NC/¢ split"),
    ("revert", "Mean reversion: buy dips, sell rises"),
];

/// What `strategy` would like to do now.
pub fn decide(strategy: &str, m: &Market, s: &Settings) -> Vec<Action> {
    match strategy {
        "band" => band(m, s),
        "lotto" => lotto(m, s),
        "borrow" => borrow(m, s),
        "lend" => lend(m, s),
        "dca" => dca(m, s),
        "rebalance" => rebalance(m, s),
        "revert" => revert(m, s),
        _ => Vec::new(),
    }
}

/// Settings that can't work together, for turning the bot on.
pub fn check(strategy: &str, s: &Settings) -> Result<(), String> {
    match strategy {
        "band" if s.number("band_buy") >= s.number("band_sell") => {
            Err("Forex band: the buy rate must be below the sell rate".into())
        }
        _ => Ok(()),
    }
}

/// Strategy 1: Buy at ≤ X, sell at ≥ Y; take quotes in the band, keep own quotes
/// on the book, or both.
fn band(m: &Market, s: &Settings) -> Vec<Action> {
    let (buy, sell) = (s.number("band_buy"), s.number("band_sell"));
    let size = m.whole(s.number("band_size"));
    let most = m.whole(s.number("band_max_nc"));
    let mode = s.get("band_mode");
    let mut out = Vec::new();
    if mode != "make" {
        if let Some(q) = m.best_ask(size).filter(|q| q.cents_per_coin <= buy) {
            if m.me.balance + q.coins <= most {
                out.push(take(q));
            }
        }
        if let Some(q) = m.best_bid(size).filter(|q| q.cents_per_coin >= sell) {
            out.push(take(q));
        }
    }
    if mode != "take" {
        for q in m
            .own("BID")
            .filter(|q| q.cents_per_coin != buy || q.coins != size)
        {
            out.push(Action::Cancel {
                quote: q.id.clone(),
            });
        }
        for q in m
            .own("ASK")
            .filter(|q| q.cents_per_coin != sell || q.coins != size)
        {
            out.push(Action::Cancel {
                quote: q.id.clone(),
            });
        }
        let has = |side: &'static str, rate: i64| {
            m.own(side)
                .any(|q| q.cents_per_coin == rate && q.coins == size)
        };
        if !has("BID", buy) && m.me.balance + size <= most {
            out.push(Action::Post {
                side: "BID",
                rate: buy,
                coins: size,
            });
        }
        if !has("ASK", sell) {
            out.push(Action::Post {
                side: "ASK",
                rate: sell,
                coins: size,
            });
        }
    }
    out
}

/// Strategy 2: A fixed spend in every open lotto, once.
fn lotto(m: &Market, s: &Settings) -> Vec<Action> {
    let spend = m.whole(s.number("lotto_spend"));
    let most_price = m.whole(s.number("lotto_max_price"));
    let kinds = s.get("lotto_kinds");
    m.lottos
        .iter()
        .filter(|l| l.status == "OPEN" && l.terms.closes_at > m.now && l.my_tickets == 0)
        .filter(|l| l.house != m.me.account && l.terms.ticket_price > 0)
        .filter(|l| kinds == "any" || l.terms.kind.eq_ignore_ascii_case(kinds))
        .filter(|l| most_price == 0 || l.terms.ticket_price <= most_price)
        .filter_map(|l| {
            let count = u32::try_from(spend / l.terms.ticket_price).ok()?;
            (count > 0).then(|| Action::Tickets {
                lotto: l.id,
                title: l.terms.title.clone(),
                count,
                cost: l.terms.ticket_price * i64::from(count),
                house: l.house.clone(),
            })
        })
        .collect()
}

/// Principal the bot owes, counting accepted loans not yet funded.
fn owed(m: &Market) -> i64 {
    m.loans
        .iter()
        .filter(|l| l.borrower == m.me.account)
        .map(|l| match l.status.as_str() {
            "ACTIVE" => l.principal,
            "ARMED" => l.amount,
            _ => 0,
        })
        .sum()
}

/// Strategy 3: Accept loans offered to the bot under a yearly rate, up to a total;
/// optionally keep a request open to invite offers.
fn borrow(m: &Market, s: &Settings) -> Vec<Action> {
    let max_bps = (s.number("borrow_max_apr") * 100) as u32;
    let cap = m.whole(s.number("borrow_max_total"));
    let mut room = cap - owed(m);
    let mut out = Vec::new();
    let mut offers: Vec<&Loan> = m
        .loans
        .iter()
        .filter(|l| l.status == "OFFERED" && l.borrower == m.me.account && !l.credit)
        .filter(|l| l.apr_bps() < max_bps)
        .collect();
    offers.sort_by_key(|l| (l.apr_bps(), l.id));
    for l in offers {
        if l.amount <= room {
            room -= l.amount;
            out.push(Action::Accept {
                loan: l.id,
                amount: l.amount,
                apr_bps: l.apr_bps(),
                lender: l.lender.clone(),
                lender_name: l.lender_name.clone(),
            });
        }
    }
    let ask = m.whole(s.number("borrow_ask"));
    let asking = m
        .loans
        .iter()
        .any(|l| l.status == "REQUESTED" && l.borrower == m.me.account);
    if ask > 0 && ask <= room && !asking && out.is_empty() && max_bps > 0 {
        out.push(Action::Ask {
            terms: LoanTerms {
                borrower: m.me.account.clone(),
                amount: ask,
                // A yearly rate, paid back over six months.
                rate_bps: max_bps.saturating_sub(1).max(1),
                rate_days: 365,
                payment_days: 30,
                installment: (ask / 6).max(1),
                memo: "Bot loan request".into(),
            },
            apr_bps: max_bps.saturating_sub(1).max(1),
        });
    }
    out
}

/// Strategy 4: Answer loan requests at or over a yearly rate, up to a total lent
/// and one loan per borrower.
fn lend(m: &Market, s: &Settings) -> Vec<Action> {
    let min_bps = (s.number("lend_min_apr") * 100) as u32;
    let each = m.whole(s.number("lend_max_each"));
    let lent: i64 = m
        .loans
        .iter()
        .filter(|l| l.lender == m.me.account)
        .filter(|l| matches!(l.status.as_str(), "ACTIVE" | "ARMED" | "OFFERED"))
        .map(|l| {
            if l.status == "ACTIVE" {
                l.principal
            } else {
                l.amount
            }
        })
        .sum();
    let mut room = m.whole(s.number("lend_max_total")) - lent;
    let busy = |borrower: &str| {
        m.loans.iter().any(|l| {
            l.lender == m.me.account
                && l.borrower == borrower
                && matches!(l.status.as_str(), "ACTIVE" | "ARMED" | "OFFERED")
        })
    };
    let mut out = Vec::new();
    let mut requests: Vec<&Loan> = m
        .loans
        .iter()
        .filter(|l| l.status == "REQUESTED" && l.borrower != m.me.account && !l.credit)
        .filter(|l| l.apr_bps() >= min_bps && l.amount <= each && !busy(&l.borrower))
        .collect();
    requests.sort_by_key(|l| (std::cmp::Reverse(l.apr_bps()), l.id));
    let mut seen: Vec<&str> = Vec::new();
    for l in requests {
        if l.amount > room || seen.contains(&l.borrower.as_str()) {
            continue;
        }
        room -= l.amount;
        seen.push(&l.borrower);
        out.push(Action::Lend {
            request: l.id,
            apr_bps: l.apr_bps(),
            borrower_name: l.borrower_name.clone(),
            terms: LoanTerms {
                borrower: l.borrower.clone(),
                amount: l.amount,
                rate_bps: l.rate_bps,
                rate_days: l.rate_days,
                payment_days: l.payment_days,
                installment: l.installment,
                memo: "From a bot bank".into(),
            },
        });
    }
    out
}

/// Strategy 5: Buy (or sell) a little every run at a limit: take the best quote,
/// else keep one of its own on the book.
fn dca(m: &Market, s: &Settings) -> Vec<Action> {
    let size = m.whole(s.number("dca_size"));
    let limit = s.number("dca_limit");
    let buying = s.get("dca_side") != "sell";
    let quote = if buying {
        m.best_ask(size).filter(|q| q.cents_per_coin <= limit)
    } else {
        m.best_bid(size).filter(|q| q.cents_per_coin >= limit)
    };
    if let Some(q) = quote {
        return vec![take(q)];
    }
    let side = if buying { "BID" } else { "ASK" };
    if m.own(side).next().is_some() {
        Vec::new()
    } else {
        vec![Action::Post {
            side,
            rate: limit,
            coins: size,
        }]
    }
}

/// Strategy 6: Keep the NC share of the bot's value near a target, trading only
/// when it drifts past the band.
fn rebalance(m: &Market, s: &Settings) -> Vec<Action> {
    let Some(price) = m.price().filter(|p| *p > 0) else {
        return Vec::new();
    };
    let size = m.whole(s.number("reb_size"));
    let coins_cents = cents_for(m.me.balance.max(0), price, m.decimals);
    let total = coins_cents + m.me.usd_cents.max(0);
    if total <= 0 {
        return Vec::new();
    }
    let share = coins_cents * 100 / total;
    let (target, band) = (s.number("reb_target"), s.number("reb_band"));
    let quote = if share > target + band {
        m.best_bid(size)
    } else if share < target - band {
        m.best_ask(size)
    } else {
        None
    };
    quote.map(take).into_iter().collect()
}

/// Strategy 7: Buy when the best ask is a percentage under the recent average trade
/// rate; sell when the best bid is that much over it.
fn revert(m: &Market, s: &Settings) -> Vec<Action> {
    let window = s.number("rev_window").max(3) as usize;
    if m.rates.len() < 3 {
        return Vec::new();
    }
    let recent = &m.rates[m.rates.len().saturating_sub(window)..];
    let avg = recent.iter().sum::<i64>() / recent.len() as i64;
    let pct = s.number("rev_pct");
    let size = m.whole(s.number("rev_size"));
    let mut out = Vec::new();
    if let Some(q) = m
        .best_ask(size)
        .filter(|q| q.cents_per_coin * 100 <= avg * (100 - pct))
    {
        out.push(take(q));
    }
    if let Some(q) = m
        .best_bid(size)
        .filter(|q| q.cents_per_coin * 100 >= avg * (100 + pct))
    {
        out.push(take(q));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::Bot;
    use crate::bots::trader::Trader;
    use std::collections::BTreeMap;

    const NC: i64 = 10_000;

    fn settings(values: &[(&str, &str)]) -> Settings {
        let trader = Trader {
            id: "trader_1",
            name: "Trader 1",
        };
        Settings::new(
            trader.settings(),
            values
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    fn quote(id: &str, maker: &str, side: &str, rate: i64, coins: i64) -> Quote {
        Quote {
            id: id.into(),
            maker: maker.into(),
            maker_name: maker.replace("account-", "M"),
            side: side.into(),
            cents_per_coin: rate,
            coins: coins * NC,
            cents: coins * rate,
            status: "OPEN".into(),
            expires_at: 0,
            live: true,
        }
    }

    fn market(quotes: Vec<Quote>) -> Market {
        Market {
            me: Member {
                id: "user-9".into(),
                account: "account-9".into(),
                display_name: "Bot".into(),
                kind: "bot".into(),
                balance: 100 * NC,
                usd_cents: 5_000,
            },
            decimals: 4,
            now: 1_000,
            quotes,
            ..Market::default()
        }
    }

    fn keys(actions: &[Action]) -> Vec<String> {
        actions.iter().map(Action::key).collect()
    }

    #[test]
    fn band_takes_inside_the_band_and_keeps_its_own_quotes() {
        let s = settings(&[
            ("band_buy", "10"),
            ("band_sell", "14"),
            ("band_size", "5"),
            ("band_max_nc", "1000"),
            ("band_mode", "both"),
        ]);
        let m = market(vec![
            quote("quote-1", "account-2", "ASK", 9, 5),
            quote("quote-2", "account-2", "ASK", 8, 50), // too big to take
            quote("quote-3", "account-3", "BID", 15, 2),
            quote("quote-4", "account-3", "BID", 13, 2), // below the band
            quote("quote-5", "account-9", "ASK", 9, 5),  // its own, at an old rate
        ]);
        assert_eq!(
            keys(&decide("band", &m, &s)),
            vec![
                "take-quote-1",
                "take-quote-3",
                "cancel-quote-5",
                "bid-10",
                "ask-14"
            ]
        );
        // Take only: no quotes of its own.
        let s = settings(&[
            ("band_buy", "10"),
            ("band_sell", "14"),
            ("band_mode", "take"),
        ]);
        assert!(decide("band", &m, &s)
            .iter()
            .all(|a| matches!(a, Action::Take { .. })));
        assert!(check(
            "band",
            &settings(&[("band_buy", "14"), ("band_sell", "14")])
        )
        .is_err());
    }

    #[test]
    fn lotto_buys_once_per_open_lotto() {
        let s = settings(&[("lotto_spend", "10"), ("lotto_kinds", "any")]);
        let lotto = |id: u64, price: i64, mine: u32, status: &str| Lotto {
            id,
            terms: crate::nanacoin::LottoTerms {
                kind: "SAVINGS".into(),
                title: format!("Pot {id}"),
                ticket_price: price * NC,
                closes_at: 2_000,
                rate_bps: 0,
            },
            house: "account-1".into(),
            my_tickets: mine,
            status: status.into(),
            ..Lotto::default()
        };
        let mut m = market(vec![]);
        m.lottos = vec![
            lotto(1, 2, 0, "OPEN"),
            lotto(2, 2, 3, "OPEN"),
            lotto(3, 2, 0, "WAITING"),
            lotto(4, 20, 0, "OPEN"), // one ticket costs more than the spend
        ];
        let actions = decide("lotto", &m, &s);
        assert_eq!(keys(&actions), vec!["lotto-1"]);
        assert_eq!(
            actions[0].describe(4),
            "Bought 5 tickets in “Pot 1” for 10 NC"
        );
        let simple = settings(&[("lotto_spend", "10"), ("lotto_kinds", "simple")]);
        assert!(decide("lotto", &m, &simple).is_empty());
    }

    fn loan(id: u64, status: &str, lender: &str, borrower: &str, amount: i64, apr: u32) -> Loan {
        serde_json::from_value(serde_json::json!({
            "id": id, "status": status, "lender": lender, "lender_name": "L",
            "borrower": borrower, "borrower_name": "B", "amount": amount * NC,
            "rate_bps": apr, "rate_days": 365, "payment_days": 30, "installment": NC,
            "apr_bps": apr
        }))
        .unwrap()
    }

    #[test]
    fn borrower_takes_only_cheap_loans_up_to_its_total() {
        let s = settings(&[
            ("borrow_max_apr", "5"),
            ("borrow_max_total", "100"),
            ("borrow_ask", "0"),
        ]);
        let mut m = market(vec![]);
        m.loans = vec![
            loan(1, "OFFERED", "account-2", "account-9", 60, 400),
            loan(2, "OFFERED", "account-3", "account-9", 60, 300), // cheaper: first
            loan(3, "OFFERED", "account-4", "account-9", 10, 500), // not under 5%
            loan(4, "OFFERED", "account-4", "account-5", 10, 100), // someone else's
        ];
        assert_eq!(keys(&decide("borrow", &m, &s)), vec!["accept-2"]);
        let asking = settings(&[
            ("borrow_max_apr", "5"),
            ("borrow_max_total", "100"),
            ("borrow_ask", "20"),
        ]);
        m.loans.clear();
        let actions = decide("borrow", &m, &asking);
        assert_eq!(
            actions[0].describe(4),
            "Asked to borrow 20 NC at up to 4.99%/yr"
        );
    }

    #[test]
    fn bot_bank_lends_at_good_rates_once_per_borrower() {
        let s = settings(&[
            ("lend_min_apr", "8"),
            ("lend_max_each", "50"),
            ("lend_max_total", "80"),
        ]);
        let mut m = market(vec![]);
        m.loans = vec![
            loan(1, "REQUESTED", "", "account-2", 40, 900),
            loan(2, "REQUESTED", "", "account-2", 10, 1_000), // same borrower: the better one
            loan(3, "REQUESTED", "", "account-3", 60, 2_000), // over the most each
            loan(4, "REQUESTED", "", "account-4", 45, 800),   // over the remaining total
            loan(5, "REQUESTED", "", "account-5", 20, 700),   // under the rate
            loan(6, "REQUESTED", "", "account-6", 30, 850),
        ];
        // Best rate first: 2 (10 NC), then 6 (30 NC); 4 no longer fits in 80.
        assert_eq!(keys(&decide("lend", &m, &s)), vec!["lend-2", "lend-6"]);
    }

    #[test]
    fn steady_buyer_takes_or_waits_on_the_book() {
        let s = settings(&[("dca_side", "buy"), ("dca_size", "5"), ("dca_limit", "12")]);
        let m = market(vec![quote("quote-1", "account-2", "ASK", 13, 5)]);
        assert_eq!(keys(&decide("dca", &m, &s)), vec!["bid-12"]);
        let m = market(vec![
            quote("quote-1", "account-2", "ASK", 11, 5),
            quote("quote-2", "account-2", "ASK", 10, 5),
        ]);
        assert_eq!(keys(&decide("dca", &m, &s)), vec!["take-quote-2"]);
        let m = market(vec![quote("quote-3", "account-9", "BID", 12, 5)]);
        assert!(
            decide("dca", &m, &s).is_empty(),
            "its own bid is already waiting"
        );
    }

    #[test]
    fn rebalancer_and_mean_reversion() {
        // 100 NC at 50¢ is $50 of coins against $50 cash: balanced at 50%.
        let s = settings(&[("reb_target", "50"), ("reb_band", "10"), ("reb_size", "20")]);
        let mut m = market(vec![
            quote("quote-1", "account-2", "ASK", 52, 10),
            quote("quote-2", "account-3", "BID", 48, 10),
        ]);
        m.rates = vec![50];
        assert!(decide("rebalance", &m, &s).is_empty());
        m.rates = vec![100]; // coins now worth twice the cash: sell some
        assert_eq!(keys(&decide("rebalance", &m, &s)), vec!["take-quote-2"]);

        let s = settings(&[("rev_window", "5"), ("rev_pct", "10"), ("rev_size", "20")]);
        m.rates = vec![60, 60, 60];
        assert_eq!(keys(&decide("revert", &m, &s)), vec!["take-quote-1"]);
        m.rates = vec![40, 40, 40];
        assert_eq!(keys(&decide("revert", &m, &s)), vec!["take-quote-2"]);
        m.rates = vec![50, 50];
        assert!(
            decide("revert", &m, &s).is_empty(),
            "too few trades to judge"
        );
    }
}
