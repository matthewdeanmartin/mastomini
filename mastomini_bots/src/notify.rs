//! Getting something to a person, outside the bot's own timeline: ntfy
//! (phone push), email through Brevo's HTTPS API, and a Mastodon direct
//! message on the bot's own server. Any bot may add [`settings`] and call
//! [`deliver`]; the SRE bot is the first.
//!
//! - Every channel is an ordinary HTTPS request on the bot's existing
//!   client, one at a time. Plain SMTP (Gmail) needs a raw TLS socket the
//!   board's HTTP client does not offer; it is not here.
//! - [`Ledger`] keeps the nagging down: one notice per fingerprint until it
//!   gets worse, clears and comes back, or is older than the re-notify
//!   period; a daily cap; quiet hours that hold notices for later.
//! - Errors are kept, short, in a five-entry delivery log, and say what to
//!   fix ("401 from Brevo: check the API key").
use crate::bot::Run;
use crate::mastodon::{HttpRequest, NewStatus, Visibility};
use crate::settings::{Kind, Setting, Settings};
use crate::text::fit;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

pub const LEDGER: &str = "notified";
pub const LOG: &str = "notify_log";
const MINUTE: u64 = 60_000;
const ENTRIES: usize = 12;
const LOG_ENTRIES: usize = 5;
const BODY_MAX: usize = 3000;

pub struct Message {
    /// One line: the subject, the ntfy title.
    pub title: String,
    pub body: String,
    /// ntfy priority 1-5 (3 is ntfy's default).
    pub priority: u8,
}

/// The notification settings, in their own group.
pub fn settings() -> Vec<Setting> {
    const G: &str = "Notifications";
    vec![
        Setting::new("notify_ntfy_topic", "ntfy topic", Kind::Secret, "")
            .optional()
            .group(G)
            .help("A long random topic name is the password on ntfy.sh: anyone who knows it can read it. Subscribe to the same topic in the ntfy app."),
        Setting::new("notify_ntfy_server", "ntfy server", Kind::Text, "https://ntfy.sh")
            .group(G)
            .help("Leave as ntfy.sh unless you run your own."),
        Setting::new("notify_ntfy_token", "ntfy access token", Kind::Secret, "")
            .optional()
            .group(G)
            .help("Only for a reserved or protected topic: ntfy.sh → Account → Access tokens."),
        Setting::new("notify_email_to", "Email to", Kind::Text, "")
            .optional()
            .group(G),
        Setting::new("notify_email_from", "Email from", Kind::Text, "")
            .optional()
            .group(G)
            .help("A sender address verified in Brevo (Senders, domains & dedicated IPs → Senders)."),
        Setting::new("notify_brevo_key", "Brevo API key", Kind::Secret, "")
            .optional()
            .group(G)
            .help("Brevo → SMTP & API → API keys → Generate (starts xkeysib-). The SMTP key is a different thing and will not work here."),
        Setting::new("notify_dm_to", "Direct message to", Kind::Text, "")
            .optional()
            .group(G)
            .help("An account on this bot's Mastodon server, as @name. It gets a direct message from the bot."),
        Setting::new("notify_quiet_from", "Quiet from", Kind::Time, "")
            .optional()
            .group(G)
            .help("Notices found in the quiet hours wait for the first run after them. In the bot's time zone."),
        Setting::new("notify_quiet_to", "Quiet until", Kind::Time, "")
            .optional()
            .group(G),
        Setting::new("notify_renotify_days", "Remind again after (days)", Kind::Number { min: 0, max: 30 }, "3")
            .group(G)
            .help("A finding still there after this long is sent once more. 0: never."),
        Setting::new("notify_max_per_day", "Most notices per day", Kind::Number { min: 1, max: 24 }, "6")
            .group(G),
    ]
}

/// The channels with enough settings to send.
pub fn channels(settings: &Settings) -> Vec<&'static str> {
    let mut out = Vec::new();
    if settings.is_set("notify_ntfy_topic") {
        out.push("ntfy");
    }
    if settings.is_set("notify_brevo_key")
        && settings.is_set("notify_email_to")
        && settings.is_set("notify_email_from")
    {
        out.push("email");
    }
    if settings.is_set("notify_dm_to") {
        out.push("dm");
    }
    out
}

fn status_error(who: &str, status: u16, hint: &str) -> String {
    format!("{status} from {who}: {hint}")
}

fn ntfy(run: &mut Run<'_>, m: &Message) -> Result<(), String> {
    let s = run.settings;
    let server = s.get("notify_ntfy_server").trim_end_matches('/');
    let server = if server.is_empty() {
        "https://ntfy.sh"
    } else {
        server
    };
    let mut headers = vec![
        ("Title".to_string(), fit(&m.title, 200)),
        ("Priority".to_string(), m.priority.clamp(1, 5).to_string()),
        ("Tags".to_string(), "warning".to_string()),
    ];
    if s.is_set("notify_ntfy_token") {
        headers.push((
            "Authorization".into(),
            format!("Bearer {}", s.get("notify_ntfy_token")),
        ));
    }
    let req = HttpRequest {
        method: "POST",
        url: format!("{server}/{}", s.get("notify_ntfy_topic").trim()),
        headers,
        body: fit(&m.body, BODY_MAX).into_bytes(),
    };
    let r = run.http.send_bounded(&req, 4096)?;
    match r.status {
        200..=299 => Ok(()),
        401 | 403 => Err(status_error(
            "ntfy",
            r.status,
            "the topic is protected; check the access token",
        )),
        429 => Err(status_error(
            "ntfy",
            r.status,
            "rate limited; fewer notices per day",
        )),
        n => Err(status_error("ntfy", n, "check the server and topic")),
    }
}

fn email(run: &mut Run<'_>, m: &Message) -> Result<(), String> {
    let s = run.settings;
    let body = json!({
        "sender": {"name": "Household SRE", "email": s.get("notify_email_from").trim()},
        "to": [{"email": s.get("notify_email_to").trim()}],
        "subject": fit(&m.title, 200),
        "textContent": fit(&m.body, BODY_MAX),
    });
    let req = HttpRequest {
        method: "POST",
        url: "https://api.brevo.com/v3/smtp/email".into(),
        headers: vec![
            ("api-key".into(), s.get("notify_brevo_key").to_string()),
            ("Content-Type".into(), "application/json".into()),
            ("Accept".into(), "application/json".into()),
        ],
        body: body.to_string().into_bytes(),
    };
    let r = run.http.send_bounded(&req, 4096)?;
    match r.status {
        200..=299 => Ok(()),
        401 => Err(status_error(
            "Brevo",
            401,
            "check the API key (not the SMTP key)",
        )),
        400 => {
            let why: String = serde_json::from_slice::<serde_json::Value>(&r.body)
                .ok()
                .and_then(|v| v["message"].as_str().map(String::from))
                .unwrap_or_default();
            Err(status_error(
                "Brevo",
                400,
                &fit(&format!("{why} (is the sender verified?)"), 120),
            ))
        }
        n => Err(status_error("Brevo", n, "try again later")),
    }
}

fn dm(run: &mut Run<'_>, m: &Message, key: &str) -> Result<(), String> {
    let to = run.settings.get("notify_dm_to").trim();
    let to = format!("@{}", to.trim_start_matches('@'));
    let limit = run.mastodon().max_characters().unwrap_or(500);
    let text = fit(&format!("{to} {}\n\n{}", m.title, m.body), limit);
    run.post_keyed(
        &NewStatus {
            text,
            visibility: Visibility::Direct,
            spoiler_text: None,
            in_reply_to_id: None,
        },
        key,
    )
    .map(|_| ())
    .map_err(|e| e.message)
}

/// Sends `m` to one channel. `key` makes the DM idempotent on a retry.
pub fn send(run: &mut Run<'_>, channel: &str, m: &Message, key: &str) -> Result<(), String> {
    match channel {
        "ntfy" => ntfy(run, m),
        "email" => email(run, m),
        "dm" => dm(run, m, &format!("notify:{key}")),
        other => Err(format!("unknown channel {other}")),
    }
}

/// Sends to every configured channel; logs each attempt. Ok when at least
/// one channel took it (the rest are in the log).
pub fn deliver(run: &mut Run<'_>, m: &Message, key: &str) -> Result<Vec<String>, String> {
    let channels = channels(run.settings);
    if channels.is_empty() {
        return Err("no notification channel is set up".into());
    }
    let mut results = Vec::new();
    let mut any = false;
    for channel in channels {
        let result = send(run, channel, m, key);
        any |= result.is_ok();
        let line = match &result {
            Ok(()) => format!("{channel} ok"),
            Err(e) => format!("{channel}: {}", fit(e, 100)),
        };
        log(&mut run.state, run.now_ms, &line);
        results.push(line);
    }
    if any {
        Ok(results)
    } else {
        Err(results.join("; "))
    }
}

fn log(state: &mut BTreeMap<String, String>, now_ms: u64, line: &str) {
    let mut lines: Vec<String> = state
        .get(LOG)
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    lines.push(format!("{} {line}", crate::tz::iso(now_ms)));
    let skip = lines.len().saturating_sub(LOG_ENTRIES);
    lines.drain(..skip);
    state.insert(
        LOG.into(),
        serde_json::to_string(&lines).unwrap_or_default(),
    );
}

/// The quiet hours hold `now_ms`, in the bot's time zone.
pub fn quiet(settings: &Settings, now_ms: u64) -> bool {
    if !settings.is_set("notify_quiet_from") || !settings.is_set("notify_quiet_to") {
        return false;
    }
    let local = settings.tz("zone").local(now_ms);
    let at = local.hour * 60 + local.minute;
    let (fh, fm) = settings.time("notify_quiet_from");
    let (th, tm) = settings.time("notify_quiet_to");
    let (from, to) = (fh * 60 + fm, th * 60 + tm);
    if from <= to {
        (from..to).contains(&at)
    } else {
        at >= from || at < to
    }
}

/// What was sent: fingerprint → (minute sent, level).
#[derive(Default, Serialize, Deserialize)]
pub struct Ledger {
    sent: BTreeMap<String, (u32, u8)>,
    /// Day number and how many notices went out on it.
    day: (u32, u8),
}

impl Ledger {
    pub fn read(state: &BTreeMap<String, String>) -> Ledger {
        state
            .get(LEDGER)
            .filter(|s| s.len() <= 1024)
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    /// Of `current` (fingerprint, level), the ones to send now. Anything
    /// not current is forgotten, so a recurrence is news again.
    pub fn due(
        &mut self,
        current: &[(String, u8)],
        now_ms: u64,
        renotify_days: u32,
    ) -> Vec<String> {
        let now = (now_ms / MINUTE) as u32;
        self.sent
            .retain(|fp, _| current.iter().any(|(c, _)| c == fp));
        current
            .iter()
            .filter(|(fp, level)| match self.sent.get(fp) {
                None => true,
                Some((_, was)) if level > was => true,
                Some((at, _)) => {
                    renotify_days > 0 && now.saturating_sub(*at) >= renotify_days * 24 * 60
                }
            })
            .map(|(fp, _)| fp.clone())
            .collect()
    }

    /// Room left today under the cap.
    pub fn room(&self, now_ms: u64, cap: u8) -> bool {
        let today = (now_ms / (24 * 60 * MINUTE)) as u32;
        self.day.0 != today || self.day.1 < cap
    }

    pub fn mark(&mut self, sent: &[(String, u8)], now_ms: u64) {
        let now = (now_ms / MINUTE) as u32;
        let today = (now_ms / (24 * 60 * MINUTE)) as u32;
        if self.day.0 != today {
            self.day = (today, 0);
        }
        self.day.1 = self.day.1.saturating_add(1);
        for (fp, level) in sent {
            self.sent.insert(fp.clone(), (now, *level));
        }
        while self.sent.len() > ENTRIES {
            let oldest = self
                .sent
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => self.sent.remove(&k),
                None => break,
            };
        }
    }

    pub fn write(&self, state: &mut BTreeMap<String, String>) {
        state.insert(
            LEDGER.into(),
            serde_json::to_string(self).unwrap_or_default(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastodon::testing::Scripted;

    const DAY: u64 = 86_400_000;
    const NOW: u64 = 1_790_000_000_000;

    fn settings(values: &[(&str, &str)]) -> Settings {
        let mut specs = settings_specs();
        specs.extend(crate::settings::schedule_settings(
            "daily",
            "07:30",
            "us_eastern",
            "1440",
        ));
        Settings::new(
            specs,
            values
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    fn settings_specs() -> Vec<Setting> {
        super::settings()
    }

    fn message() -> Message {
        Message {
            title: "[SRE] gamematcher.local crashed".into(),
            body: "gamematcher.local: crashed 2 h ago.".into(),
            priority: 4,
        }
    }

    #[test]
    fn each_channel_sends_the_exact_request() {
        let http = Scripted::default();
        http.answer(200, json!({"id": "n1"}));
        http.answer(201, json!({"messageId": "<1@brevo>"}));
        http.answer(
            200,
            json!({"configuration": {"statuses": {"max_characters": 500}}}),
        );
        http.answer(200, json!({"id": "9", "url": "https://mastomini.local/9"}));
        let sent = http.sent.clone();
        let s = settings(&[
            ("notify_ntfy_topic", "house-sre-x7q2"),
            ("notify_ntfy_token", "tk_1"),
            ("notify_brevo_key", "xkeysib-abc"),
            ("notify_email_to", "me@example.com"),
            ("notify_email_from", "sre@example.com"),
            ("notify_dm_to", "@matt"),
        ]);
        let mut http = http;
        let mut run = Run::new(
            "sre",
            NOW,
            NOW,
            &s,
            &mut http,
            "https://mastomini.local",
            "K",
            None,
        );
        let results = deliver(&mut run, &message(), "slot1").unwrap();
        assert_eq!(results, ["ntfy ok", "email ok", "dm ok"]);
        let sent = sent.lock().unwrap();
        assert_eq!(sent[0].url, "https://ntfy.sh/house-sre-x7q2");
        assert!(sent[0].headers.contains(&("Priority".into(), "4".into())));
        assert!(sent[0]
            .headers
            .contains(&("Authorization".into(), "Bearer tk_1".into())));
        assert_eq!(sent[0].body, b"gamematcher.local: crashed 2 h ago.");
        assert_eq!(sent[1].url, "https://api.brevo.com/v3/smtp/email");
        assert!(sent[1]
            .headers
            .contains(&("api-key".into(), "xkeysib-abc".into())));
        let mail: serde_json::Value = serde_json::from_slice(&sent[1].body).unwrap();
        assert_eq!(mail["to"][0]["email"], "me@example.com");
        assert_eq!(mail["subject"], "[SRE] gamematcher.local crashed");
        let dm: serde_json::Value = serde_json::from_slice(&sent[3].body).unwrap();
        assert_eq!(dm["visibility"], "direct");
        assert!(dm["status"].as_str().unwrap().starts_with("@matt [SRE]"));
        assert!(run.state[LOG].contains("email ok"));
    }

    #[test]
    fn errors_say_what_to_fix_and_one_working_channel_is_enough() {
        let http = Scripted::default();
        http.answer(200, json!({}));
        http.answer(
            401,
            json!({"code": "unauthorized", "message": "Key not found"}),
        );
        let s = settings(&[
            ("notify_ntfy_topic", "t"),
            ("notify_brevo_key", "smtp-key-by-mistake"),
            ("notify_email_to", "me@example.com"),
            ("notify_email_from", "sre@example.com"),
        ]);
        let mut http = http;
        let mut run = Run::new(
            "sre",
            NOW,
            NOW,
            &s,
            &mut http,
            "https://mastomini.local",
            "K",
            None,
        );
        let results = deliver(&mut run, &message(), "k").unwrap();
        assert_eq!(
            results[1],
            "email: 401 from Brevo: check the API key (not the SMTP key)"
        );
        let s = settings(&[]);
        let mut run = Run::new(
            "sre",
            NOW,
            NOW,
            &s,
            &mut http,
            "https://mastomini.local",
            "K",
            None,
        );
        assert_eq!(
            deliver(&mut run, &message(), "k").unwrap_err(),
            "no notification channel is set up"
        );
    }

    #[test]
    fn the_ledger_sends_once_until_worse_cleared_or_stale() {
        let mut l = Ledger::default();
        let crash = vec![("gm|reset.fault|1".to_string(), 2)];
        assert_eq!(l.due(&crash, NOW, 3).len(), 1);
        l.mark(&crash, NOW);
        assert!(l.due(&crash, NOW + DAY, 3).is_empty(), "already sent");
        let worse = vec![("gm|reset.fault|1".to_string(), 3)];
        assert_eq!(l.due(&worse, NOW + DAY, 3).len(), 1, "escalated");
        assert_eq!(l.due(&crash, NOW + 3 * DAY, 3).len(), 1, "reminder");
        assert!(l.due(&crash, NOW + 3 * DAY, 0).is_empty(), "reminders off");
        assert!(l.due(&[], NOW, 3).is_empty());
        assert_eq!(
            l.due(&crash, NOW, 3).len(),
            1,
            "cleared, then back: news again"
        );
    }

    #[test]
    fn the_daily_cap_and_quiet_hours() {
        let mut l = Ledger::default();
        for _ in 0..2 {
            assert!(l.room(NOW, 2));
            l.mark(&[], NOW);
        }
        assert!(!l.room(NOW, 2));
        assert!(l.room(NOW + DAY, 2));
        // 1_790_000_000_000 is 2026-09-21 14:13 UTC, 10:13 in New York.
        let s = settings(&[("notify_quiet_from", "10:00"), ("notify_quiet_to", "11:00")]);
        assert!(quiet(&s, NOW));
        let s = settings(&[("notify_quiet_from", "22:00"), ("notify_quiet_to", "07:00")]);
        assert!(!quiet(&s, NOW));
        assert!(quiet(&s, NOW + 14 * 3_600_000));
        assert!(!quiet(&settings(&[]), NOW));
    }
}
