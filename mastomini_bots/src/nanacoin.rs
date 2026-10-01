//! The part of the NanaCoin API bots use, over any [`HttpClient`].
//!
//! Written against the contract in `sprint/nanabots-2-nanacoin.md`: the
//! activity feed, `kind` on members and `apr_bps` on loans are new there;
//! everything else is NanaCoin's existing `/api/v1`.
//!
//! Amounts are NC *minor units* (`decimals` places, 4 by default); forex
//! rates are US cents per whole NC. Every request that moves money carries
//! an `Idempotency-Key` made from the bot, its slot and what it is doing,
//! so a retried run never moves money twice.

use crate::mastodon::{HttpClient, HttpRequest};
use crate::settings::{Kind, Setting, Settings};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

/// The settings every NanaCoin bot shares.
pub fn nanacoin_settings() -> Vec<Setting> {
    vec![
        Setting::new(
            "nc_server",
            "NanaCoin server",
            Kind::Text,
            "https://nanacoin.local",
        ),
        Setting::new("nc_key", "NanaCoin API key", Kind::Secret, "").help(
            "An nc_… key. Nana makes a bot's key on its member page; a news bot needs only a read key.",
        ),
    ]
}

/// Settings refused when turning a NanaCoin bot on.
pub fn ready(settings: &Settings) -> Result<(), String> {
    if !settings.is_set("nc_key") {
        return Err("Add a NanaCoin API key before turning the bot on".into());
    }
    crate::mastodon::normalize_instance(settings.get("nc_server")).map(|_| ())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NanaError {
    /// `None`: no HTTP answer (network, TLS, timeout).
    pub status: Option<u16>,
    /// NanaCoin's `error` code, e.g. `insufficient_funds`.
    pub code: String,
    pub message: String,
}

impl NanaError {
    /// No answer, rate limited, or a server that may recover. 507 (a full
    /// table) and 409 (funds, conflicts) wait for a person, not a retry.
    pub fn retryable(&self) -> bool {
        match self.status {
            None => true,
            Some(s) => s == 429 || (s >= 500 && s != 507),
        }
    }
}

impl std::fmt::Display for NanaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(s) => write!(f, "NanaCoin HTTP {s}: {}", self.message),
            None => write!(f, "NanaCoin: {}", self.message),
        }
    }
}

impl From<NanaError> for crate::bot::RunError {
    fn from(e: NanaError) -> Self {
        crate::bot::RunError {
            retryable: e.retryable(),
            message: e.to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Status {
    #[serde(default = "four")]
    pub decimals: u8,
    #[serde(default)]
    pub journal_generation: u64,
    #[serde(default)]
    pub currency: String,
}

fn four() -> u8 {
    4
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Member {
    pub id: String,
    pub account: String,
    #[serde(default)]
    pub display_name: String,
    /// `human` or `bot`.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub balance: i64,
    #[serde(default)]
    pub usd_cents: i64,
}

impl Member {
    pub fn is_bot(&self) -> bool {
        self.kind == "bot"
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Activity {
    #[serde(default)]
    pub incarnation: u64,
    #[serde(default)]
    pub sequence: u64,
    #[serde(default = "four")]
    pub decimals: u8,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub events: Vec<Event>,
}

/// One feed row. Fields beyond the first five depend on `kind`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Event {
    pub seq: u64,
    pub at: u64,
    pub kind: String,
    pub actor: String,
    pub actor_name: String,
    pub actor_bot: bool,
    pub subject: String,
    pub title: String,
    pub side: String,
    pub amount: Option<i64>,
    pub rate: Option<i64>,
    pub coins: Option<i64>,
    pub apr_bps: Option<u32>,
    pub closes_at: Option<u64>,
    pub lotto_kind: String,
    pub editions: Option<u32>,
    pub other: String,
    pub other_name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Quote {
    pub id: String,
    pub maker: String,
    pub maker_name: String,
    /// `BID`: the maker buys NC. `ASK`: the maker sells NC.
    pub side: String,
    pub cents_per_coin: i64,
    /// Minor units.
    pub coins: i64,
    pub cents: i64,
    pub status: String,
    pub expires_at: u64,
    pub live: bool,
}

impl Quote {
    pub fn open(&self) -> bool {
        self.live && self.status == "OPEN"
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct LottoTerms {
    /// `SIMPLE`, `DELAYED` or `SAVINGS`.
    pub kind: String,
    pub title: String,
    pub ticket_price: i64,
    pub closes_at: u64,
    pub rate_bps: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Lotto {
    pub id: u64,
    pub terms: LottoTerms,
    pub house: String,
    pub pool: i64,
    pub tickets: u64,
    pub my_tickets: u32,
    /// `OPEN`, `WAITING`, `PAYING` or `SETTLED`.
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Loan {
    pub id: u64,
    pub lender: String,
    pub lender_name: String,
    pub borrower: String,
    pub borrower_name: String,
    pub amount: i64,
    pub rate_bps: u32,
    pub rate_days: u16,
    pub payment_days: u16,
    pub installment: i64,
    pub credit: bool,
    /// `OFFERED`, `ARMED`, `ACTIVE`, `PAID`, `DECLINED`, `CANCELLED`, `REQUESTED`.
    pub status: String,
    pub principal: i64,
    apr_bps: Option<u32>,
}

impl Loan {
    /// The yearly rate in hundredths of a percent: the server's figure
    /// (sprint 2), else the same arithmetic.
    pub fn apr_bps(&self) -> u32 {
        self.apr_bps.unwrap_or_else(|| {
            (u64::from(self.rate_bps) * 365 / u64::from(self.rate_days.max(1))) as u32
        })
    }
}

/// Terms for asking for, or answering a request for, a loan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoanTerms {
    pub borrower: String,
    pub amount: i64,
    pub rate_bps: u32,
    pub rate_days: u16,
    pub payment_days: u16,
    pub installment: i64,
    pub memo: String,
}

impl LoanTerms {
    fn json(&self) -> Value {
        json!({
            "borrower": self.borrower,
            "amount": self.amount,
            "rate_bps": self.rate_bps,
            "rate_days": self.rate_days,
            "payment_days": self.payment_days,
            "installment": self.installment,
            "credit": false,
            "memo": self.memo,
        })
    }
}

/// `5` / `1.25` from minor units: exact, without trailing zeros.
pub fn format_nc(minor: i64, decimals: u8) -> String {
    let scale = 10i128.pow(u32::from(decimals));
    let value = i128::from(minor);
    let sign = if value < 0 { "-" } else { "" };
    let (whole, frac) = (value.abs() / scale, value.abs() % scale);
    if frac == 0 {
        return format!("{sign}{whole}");
    }
    let frac = format!("{frac:0width$}", width = usize::from(decimals));
    format!("{sign}{whole}.{}", frac.trim_end_matches('0'))
}

/// Whole NC to minor units.
pub fn minor(whole: i64, decimals: u8) -> i64 {
    whole.saturating_mul(10i64.saturating_pow(u32::from(decimals)))
}

pub struct NanaCoin<'a> {
    http: &'a mut dyn HttpClient,
    server: String,
    key: String,
    /// `mmb:<bot>:<slot|manual>:<ms>`, the stable part of every key.
    key_prefix: String,
    status: Option<Status>,
}

impl<'a> NanaCoin<'a> {
    pub fn new(
        http: &'a mut dyn HttpClient,
        server: &str,
        key: &str,
        key_prefix: String,
    ) -> NanaCoin<'a> {
        NanaCoin {
            http,
            server: server.trim_end_matches('/').to_string(),
            key: key.to_string(),
            key_prefix,
            status: None,
        }
    }

    fn call(
        &mut self,
        method: &'static str,
        path: &str,
        body: Option<&Value>,
        idempotency_key: Option<&str>,
    ) -> Result<Value, NanaError> {
        let mut headers = vec![
            ("Authorization".to_string(), format!("Bearer {}", self.key)),
            ("Accept".to_string(), "application/json".to_string()),
            (
                "User-Agent".to_string(),
                concat!("mastomini-bots/", env!("CARGO_PKG_VERSION")).to_string(),
            ),
        ];
        if body.is_some() {
            headers.push(("Content-Type".into(), "application/json".into()));
        }
        if let Some(key) = idempotency_key {
            headers.push(("Idempotency-Key".into(), key.into()));
        }
        let req = HttpRequest {
            method,
            url: format!("{}/api/v1{path}", self.server),
            headers,
            body: body
                .map(|b| serde_json::to_vec(b).unwrap_or_default())
                .unwrap_or_default(),
        };
        let res = self.http.send(&req).map_err(|message| NanaError {
            status: None,
            code: String::new(),
            message,
        })?;
        let value: Value = serde_json::from_slice(&res.body).unwrap_or(Value::Null);
        if (200..300).contains(&res.status) {
            return Ok(value);
        }
        let text = |k: &str| value[k].as_str().unwrap_or("").to_string();
        let message = match (text("message"), text("error")) {
            (m, _) if !m.is_empty() => m,
            (_, e) if !e.is_empty() => e,
            _ => String::from_utf8_lossy(&res.body)
                .chars()
                .take(200)
                .collect(),
        };
        Err(NanaError {
            status: Some(res.status),
            code: text("error"),
            message,
        })
    }

    fn get<T: DeserializeOwned>(
        &mut self,
        path: &str,
        field: Option<&str>,
    ) -> Result<T, NanaError> {
        let mut v = self.call("GET", path, None, None)?;
        if let Some(field) = field {
            v = v[field].take();
        }
        serde_json::from_value(v).map_err(|e| NanaError {
            status: Some(200),
            code: "unexpected_answer".into(),
            message: format!("unexpected answer from {path}: {e}"),
        })
    }

    /// Public status: the economy's decimals and journal generation.
    pub fn status(&mut self) -> Result<Status, NanaError> {
        if let Some(s) = &self.status {
            return Ok(s.clone());
        }
        let status: Status = self.get("/status", None)?;
        self.status = Some(status.clone());
        Ok(status)
    }

    /// `g<generation>:mmb:<bot>:<slot>:<what>`: the same for every attempt
    /// at this slot's `what`, different for every slot and generation.
    pub fn key(&mut self, what: &str) -> Result<String, NanaError> {
        let generation = self.status()?.journal_generation;
        let mut key = format!("g{generation}:{}:{what}", self.key_prefix);
        key.truncate(80);
        Ok(key)
    }

    fn post(&mut self, path: &str, body: &Value, what: &str) -> Result<Value, NanaError> {
        let key = self.key(what)?;
        self.call("POST", path, Some(body), Some(&key))
    }

    pub fn me(&mut self) -> Result<Member, NanaError> {
        self.get("/me", None)
    }

    pub fn users(&mut self) -> Result<Vec<Member>, NanaError> {
        self.get("/users", Some("users"))
    }

    /// Events after `after`, oldest first.
    pub fn activity(&mut self, after: Option<u64>, limit: usize) -> Result<Activity, NanaError> {
        let mut path = format!("/activity?limit={}", limit.clamp(1, 50));
        if let Some(after) = after {
            path.push_str(&format!("&after={after}"));
        }
        self.get(&path, None)
    }

    pub fn quotes(&mut self) -> Result<Vec<Quote>, NanaError> {
        self.get("/quotes", Some("quotes"))
    }

    /// `side`: `BID` to buy NC, `ASK` to sell. `expires_at` in unix seconds.
    pub fn post_quote(
        &mut self,
        side: &str,
        cents_per_coin: i64,
        coins: i64,
        expires_at: u64,
    ) -> Result<Quote, NanaError> {
        let body = json!({
            "side": side,
            "cents_per_coin": cents_per_coin,
            "coins": coins,
            "expires_at": expires_at,
        });
        let v = self.post(
            "/quotes",
            &body,
            &format!("{}-{cents_per_coin}", side.to_ascii_lowercase()),
        )?;
        Ok(serde_json::from_value(v).unwrap_or_default())
    }

    pub fn take_quote(&mut self, id: &str) -> Result<(), NanaError> {
        self.post(
            &format!("/quotes/{id}/take"),
            &json!({}),
            &format!("take-{id}"),
        )
        .map(|_| ())
    }

    pub fn cancel_quote(&mut self, id: &str) -> Result<(), NanaError> {
        self.post(
            &format!("/quotes/{id}/cancel"),
            &json!({}),
            &format!("cancel-{id}"),
        )
        .map(|_| ())
    }

    pub fn lottos(&mut self) -> Result<Vec<Lotto>, NanaError> {
        self.get("/lottos", Some("lottos"))
    }

    pub fn buy_tickets(&mut self, lotto: u64, count: u32) -> Result<(), NanaError> {
        self.post(
            &format!("/lottos/{lotto}/tickets"),
            &json!({ "count": count }),
            &format!("lotto-{lotto}"),
        )
        .map(|_| ())
    }

    /// Loans this member may see: their own, and every open request.
    pub fn loans(&mut self) -> Result<Vec<Loan>, NanaError> {
        self.get("/loans", Some("loans"))
    }

    pub fn accept_loan(&mut self, loan: u64) -> Result<(), NanaError> {
        self.post(
            &format!("/loans/{loan}/accept"),
            &json!({}),
            &format!("accept-{loan}"),
        )
        .map(|_| ())
    }

    /// Answer someone's loan request with an offer on these terms.
    pub fn offer_loan(&mut self, request: u64, terms: &LoanTerms) -> Result<(), NanaError> {
        self.post(
            &format!("/loans/{request}/offer"),
            &terms.json(),
            &format!("lend-{request}"),
        )
        .map(|_| ())
    }

    /// Ask to borrow (`terms.borrower` is this member's own account).
    pub fn request_loan(&mut self, terms: &LoanTerms) -> Result<(), NanaError> {
        self.post("/loans/request", &terms.json(), "ask-loan")
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastodon::testing::Scripted;

    fn client(http: &mut Scripted) -> NanaCoin<'_> {
        NanaCoin::new(
            http,
            "https://nanacoin.local/",
            "nc_K",
            "mmb:trader_1:slot:1000".into(),
        )
    }

    #[test]
    fn amounts_are_exact_minor_units() {
        assert_eq!(format_nc(50_000, 4), "5");
        assert_eq!(format_nc(12_345, 4), "1.2345");
        assert_eq!(format_nc(12_500, 4), "1.25");
        assert_eq!(format_nc(-5, 2), "-0.05");
        assert_eq!(format_nc(7, 0), "7");
        assert_eq!(minor(5, 4), 50_000);
    }

    #[test]
    fn money_requests_carry_a_stable_generation_key() {
        let mut http = Scripted::default();
        http.answer(200, json!({"decimals": 4, "journal_generation": 7}));
        http.answer(201, json!({"quote": {}}));
        http.answer(201, json!({}));
        let sent = http.sent.clone();
        let mut nc = client(&mut http);
        nc.take_quote("quote-3").unwrap();
        nc.buy_tickets(41, 2).unwrap();
        let sent = sent.lock().unwrap();
        assert_eq!(sent[0].url, "https://nanacoin.local/api/v1/status");
        assert_eq!(
            sent[1].url,
            "https://nanacoin.local/api/v1/quotes/quote-3/take"
        );
        let key = |i: usize| {
            sent[i]
                .headers
                .iter()
                .find(|(k, _)| k == "Idempotency-Key")
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(key(1), "g7:mmb:trader_1:slot:1000:take-quote-3");
        assert_eq!(key(2), "g7:mmb:trader_1:slot:1000:lotto-41");
        assert_eq!(
            serde_json::from_slice::<Value>(&sent[2].body).unwrap(),
            json!({"count": 2})
        );
        assert!(sent[1]
            .headers
            .contains(&("Authorization".into(), "Bearer nc_K".into())));
    }

    #[test]
    fn errors_say_whether_a_retry_could_help() {
        let mut http = Scripted::default();
        http.answer(
            409,
            json!({"error": "insufficient_funds", "message": "Not enough NC"}),
        );
        http.answer(
            507,
            json!({"error": "member_quote_limit", "message": "Too many quotes"}),
        );
        http.answer(
            503,
            json!({"error": "unavailable", "message": "Clock not set"}),
        );
        http.fail("connection refused");
        let mut nc = client(&mut http);
        let e = nc.me().unwrap_err();
        assert_eq!(e.code, "insufficient_funds");
        assert_eq!(e.to_string(), "NanaCoin HTTP 409: Not enough NC");
        assert!(!e.retryable());
        assert!(!nc.me().unwrap_err().retryable());
        assert!(nc.me().unwrap_err().retryable());
        assert!(nc.me().unwrap_err().retryable());
    }

    #[test]
    fn reads_the_feed_and_loans() {
        let mut http = Scripted::default();
        http.answer(
            200,
            json!({"incarnation": 1, "sequence": 9, "decimals": 4, "truncated": false,
                   "events": [{"seq": 9, "at": 5, "kind": "listing_opened", "actor": "user-2",
                               "actor_name": "Robin", "title": "Bike", "side": "SELL",
                               "amount": 50000, "unknown_future_field": 1}]}),
        );
        http.answer(
            200,
            json!({"loans": [{"id": 4, "amount": 10, "rate_bps": 10, "rate_days": 30,
                              "status": "REQUESTED"},
                             {"id": 5, "rate_bps": 1, "rate_days": 1, "apr_bps": 400}]}),
        );
        let sent = http.sent.clone();
        let mut nc = client(&mut http);
        let feed = nc.activity(Some(8), 100).unwrap();
        assert_eq!(feed.events[0].title, "Bike");
        assert_eq!(feed.events[0].amount, Some(50_000));
        let loans = nc.loans().unwrap();
        assert_eq!(loans[0].apr_bps(), 121);
        assert_eq!(loans[1].apr_bps(), 400);
        assert_eq!(
            sent.lock().unwrap()[0].url,
            "https://nanacoin.local/api/v1/activity?limit=50&after=8"
        );
    }
}
