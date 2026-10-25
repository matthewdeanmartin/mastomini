//! Findings whose rule says `issue` become issues in the household tracker
//! (`microcontroller/miniframework/apps/issue_tracker`, on `ops.local`).
//!
//! - One issue per board and problem family: every crash of a board is the
//!   same issue (`gamematcher.local|crash`), counted and reopened by the
//!   tracker's dedup, not a new issue per crash.
//! - Labels say where it came from: `sre`, `board:<name>`, `runbook:<family>`.
//! - Issues this bot filed that have not been reported for
//!   `tracker_close_days` days are closed as `stale` (needs a `triage`
//!   token). A recurrence reopens them.
//! - The tracker being down never fails the digest: up to [`QUEUE`]
//!   filings wait in state for the next run.
use crate::bot::Run;
use crate::mastodon::HttpRequest;
use crate::settings::{Kind, Setting, Settings};
use crate::text::fit;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const QUEUE_KEY: &str = "tracker_queue";
const QUEUE: usize = 3;
/// Filings and closings per run, so one bad morning cannot flood it.
const PER_RUN: usize = 4;
const DAY_MS: u64 = 86_400_000;

pub fn settings() -> Vec<Setting> {
    const G: &str = "Issue tracker";
    vec![
        Setting::new("tracker_url", "Tracker", Kind::Text, "")
            .optional()
            .group(G)
            .help("The issue tracker's base URL. Empty: https://ops.local, when it is in Boards."),
        Setting::new("tracker_token", "Tracker token", Kind::Secret, "")
            .optional()
            .group(G)
            .help("On the tracker: POST /api/v1/tokens as admin, name sre-bot, scope triage (report files only; triage also closes stale issues). Shown once."),
        Setting::new("tracker_close_days", "Close after (days unseen)", Kind::Number { min: 0, max: 365 }, "30")
            .group(G)
            .help("An issue this bot filed and has not reported for this long is closed as stale; it reopens if it happens again. 0: never close."),
    ]
}

/// The issue family of a rule: repeats and first times share an issue.
pub fn family(rule: &str) -> &str {
    match rule {
        "reset.fault" | "reset.fault.repeat" => "crash",
        "reset.hung" | "reset.hung.repeat" => "hang",
        r => r,
    }
}

/// One issue to file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Filing {
    pub fingerprint: String,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

impl Filing {
    pub fn new(host: &str, rule: &str, text: &str, todo: &str) -> Filing {
        let family = family(rule);
        let board = host.trim_end_matches(".local");
        Filing {
            fingerprint: format!("{host}|{family}"),
            title: fit(&format!("{host}: {text}"), 120),
            body: format!(
                "{host}: {text}\n\nWhat to do: {todo}\n\nFiled by the SRE bot (rule `{rule}`). It counts repeats here and closes this when it stops happening."
            ),
            labels: vec![
                "sre".into(),
                format!("board:{board}"),
                format!("runbook:{family}"),
            ],
        }
    }
}

/// The tracker: the setting, or else the household's `ops.local` when it is
/// one of the boards. Nothing is filed without a token.
fn base(settings: &Settings) -> Option<String> {
    if !settings.is_set("tracker_token") {
        return None;
    }
    match settings.get("tracker_url").trim().trim_end_matches('/') {
        "" => super::sre::targets(settings)
            .into_iter()
            .find(|t| super::sre::host(t) == "ops.local"),
        url => Some(url.to_string()),
    }
}

pub fn configured(settings: &Settings) -> bool {
    base(settings).is_some()
}

fn call(
    run: &mut Run<'_>,
    method: &'static str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, (bool, String)> {
    let base = base(run.settings).ok_or((false, "no tracker".to_string()))?;
    let req = HttpRequest {
        method,
        url: format!("{base}{path}"),
        headers: vec![
            (
                "Authorization".into(),
                format!("Bearer {}", run.settings.get("tracker_token")),
            ),
            ("Content-Type".into(), "application/json".into()),
            ("Accept".into(), "application/json".into()),
        ],
        body: body.map(|b| b.to_string().into_bytes()).unwrap_or_default(),
    };
    // Retryable: no answer, or the tracker's own trouble.
    let r = run
        .http
        .send_bounded(&req, 64 * 1024)
        .map_err(|e| (true, e))?;
    let v: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
    match r.status {
        200..=299 => Ok(v),
        401 => Err((false, "401: the tracker token is wrong or revoked".into())),
        403 => Err((
            false,
            "403: the tracker token needs scope triage to close issues".into(),
        )),
        n => Err((
            n >= 500 || n == 429,
            format!(
                "{n}: {}",
                v["message"]
                    .as_str()
                    .or(v["error"].as_str())
                    .unwrap_or("refused")
            ),
        )),
    }
}

/// What filing did, per fingerprint: `#12 https://ops.local/issues/12`
/// (with "(reopened)" or "(3 reports)" after the number), or why not.
pub type Outcome = Vec<(String, Result<String, String>)>;

/// Files `filings` (and any queued from earlier runs). Never fails.
pub fn file(run: &mut Run<'_>, filings: &[Filing]) -> Outcome {
    if !configured(run.settings) {
        return Vec::new();
    }
    let mut queue: Vec<Filing> = run
        .state
        .get(QUEUE_KEY)
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    for f in filings {
        queue.retain(|q| q.fingerprint != f.fingerprint);
    }
    let mut todo: Vec<Filing> = queue.into_iter().chain(filings.iter().cloned()).collect();
    let mut outcome = Vec::new();
    let mut waiting = Vec::new();
    let later = todo.split_off(todo.len().min(PER_RUN));
    waiting.extend(later);
    for f in todo {
        let body = json!({"title": f.title, "body": f.body, "labels": f.labels,
            "priority": "p2", "fingerprint": f.fingerprint});
        match call(run, "POST", "/api/v1/issues", Some(body)) {
            Ok(v) => {
                let id = v["id"].as_u64().unwrap_or(0);
                let what = if v["reopened"] == true {
                    " (reopened)".to_string()
                } else if v["deduped"] == true {
                    format!(" ({} reports)", v["count"])
                } else {
                    String::new()
                };
                // The tracker's own page for it.
                let page = base(run.settings)
                    .map(|b| format!(" {b}/issues/{id}"))
                    .unwrap_or_default();
                let note = format!("#{id}{what}{page}");
                outcome.push((f.fingerprint.clone(), Ok(note)));
            }
            Err((retry, e)) => {
                run.log(format!("Issue not filed ({}): {e}", f.fingerprint));
                outcome.push((f.fingerprint.clone(), Err(e)));
                if retry {
                    waiting.push(f);
                }
            }
        }
    }
    for f in &mut waiting {
        f.body = fit(&f.body, 600);
    }
    waiting.truncate(QUEUE);
    if waiting.is_empty() {
        run.state.remove(QUEUE_KEY);
    } else {
        run.state.insert(
            QUEUE_KEY.into(),
            serde_json::to_string(&waiting).unwrap_or_default(),
        );
    }
    outcome
}

/// Closes this bot's open issues not reported for `tracker_close_days`,
/// except those `current` (still firing). Returns the closed ids.
pub fn close_stale(run: &mut Run<'_>, current: &[String]) -> Vec<u64> {
    let days = run.settings.number("tracker_close_days").max(0) as u64;
    if !configured(run.settings) || days == 0 {
        return Vec::new();
    }
    let Ok(list) = call(
        run,
        "GET",
        "/api/v1/issues?state=open&label=sre&limit=200",
        None,
    ) else {
        return Vec::new();
    };
    let me = list["issues"].as_array().cloned().unwrap_or_default();
    let stale: Vec<(u64, String)> = me
        .iter()
        .filter(|i| {
            let fp = i["fingerprint"].as_str().unwrap_or("");
            !fp.is_empty()
                && !current.iter().any(|c| c == fp)
                && run
                    .now_ms
                    .saturating_sub(i["last_seen"].as_u64().unwrap_or(u64::MAX))
                    >= days * DAY_MS
        })
        .filter_map(|i| Some((i["id"].as_u64()?, i["fingerprint"].as_str()?.to_string())))
        .take(PER_RUN)
        .collect();
    let mut closed = Vec::new();
    for (id, fp) in stale {
        let patch = json!({"state": "closed", "resolution": "stale",
            "comment": format!("Not reported for {days} days, so the SRE bot closed it. It reopens if {fp} happens again.")});
        match call(run, "PATCH", &format!("/api/v1/issues/{id}"), Some(patch)) {
            Ok(_) => closed.push(id),
            Err((_, e)) => {
                run.log(format!("Could not close issue #{id}: {e}"));
                break;
            }
        }
    }
    closed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastodon::testing::Scripted;

    const NOW: u64 = 1_790_000_000_000;

    fn settings(values: &[(&str, &str)]) -> Settings {
        Settings::new(
            settings_specs(),
            values
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    fn settings_specs() -> Vec<Setting> {
        super::settings()
    }

    fn crash() -> Filing {
        Filing::new(
            "gamematcher.local",
            "reset.fault.repeat",
            "crashed 1 h ago: 2 faults in 7 days",
            "A firmware bug.",
        )
    }

    #[test]
    fn repeats_and_first_times_share_one_issue_per_board() {
        let a = crash();
        let b = Filing::new("gamematcher.local", "reset.fault", "crashed", "");
        assert_eq!(a.fingerprint, "gamematcher.local|crash");
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.labels, ["sre", "board:gamematcher", "runbook:crash"]);
        assert_eq!(family("heap.allocations"), "heap.allocations");
    }

    #[test]
    fn files_queues_when_down_and_retries_next_run() {
        let http = Scripted::default();
        http.fail("connection refused");
        http.answer(
            201,
            json!({"id": 7, "deduped": false, "reopened": false, "count": 1, "state": "open"}),
        );
        http.answer(
            200,
            json!({"id": 7, "deduped": true, "reopened": true, "count": 3, "state": "open"}),
        );
        let sent = http.sent.clone();
        let s = settings(&[
            ("tracker_url", "https://ops.local/"),
            ("tracker_token", "it_x"),
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
        let out = file(&mut run, &[crash()]);
        assert!(out[0].1.is_err());
        assert!(run.state.contains_key(QUEUE_KEY), "kept for the next run");
        let out = file(&mut run, &[]);
        assert_eq!(
            out,
            [(
                "gamematcher.local|crash".to_string(),
                Ok("#7 https://ops.local/issues/7".to_string())
            )]
        );
        assert!(!run.state.contains_key(QUEUE_KEY));
        let out = file(&mut run, &[crash()]);
        assert_eq!(
            out[0].1,
            Ok("#7 (reopened) https://ops.local/issues/7".to_string())
        );
        let sent = sent.lock().unwrap();
        assert_eq!(sent[1].url, "https://ops.local/api/v1/issues");
        assert!(sent[1]
            .headers
            .contains(&("Authorization".into(), "Bearer it_x".into())));
        let body: Value = serde_json::from_slice(&sent[1].body).unwrap();
        assert_eq!(body["fingerprint"], "gamematcher.local|crash");
    }

    #[test]
    fn a_rejected_filing_is_not_queued_and_nothing_happens_unconfigured() {
        let http = Scripted::default();
        http.answer(401, json!({"error": "unauthorized"}));
        let s = settings(&[
            ("tracker_url", "https://ops.local"),
            ("tracker_token", "wrong"),
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
        let out = file(&mut run, &[crash()]);
        assert_eq!(
            out[0].1,
            Err("401: the tracker token is wrong or revoked".into())
        );
        assert!(!run.state.contains_key(QUEUE_KEY));
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
        assert!(file(&mut run, &[crash()]).is_empty());
        assert!(close_stale(&mut run, &[]).is_empty());
    }

    #[test]
    fn ops_local_in_boards_is_the_tracker_by_default() {
        let with_board = Settings::new(
            {
                let mut s = settings_specs();
                s.push(Setting::new(
                    "targets",
                    "Boards",
                    Kind::Text,
                    "https://nanacoin.local\nhttps://ops.local/",
                ));
                s
            },
            [("tracker_token".to_string(), "it_x".to_string())].into(),
        );
        assert_eq!(base(&with_board).as_deref(), Some("https://ops.local"));
        assert_eq!(
            base(&settings(&[("tracker_token", "it_x")])),
            None,
            "no ops.local, no URL"
        );
        assert_eq!(
            base(&settings(&[("tracker_url", "https://ops.local")])),
            None,
            "no token"
        );
    }

    #[test]
    fn closes_only_quiet_issues_that_are_not_firing() {
        let http = Scripted::default();
        http.answer(
            200,
            json!({"total": 3, "issues": [
                {"id": 1, "fingerprint": "a.local|crash", "last_seen": NOW - 40 * DAY_MS},
                {"id": 2, "fingerprint": "b.local|crash", "last_seen": NOW - 2 * DAY_MS},
                {"id": 3, "fingerprint": "c.local|heap.low", "last_seen": NOW - 40 * DAY_MS},
            ]}),
        );
        http.answer(200, json!({"id": 1, "state": "closed"}));
        let sent = http.sent.clone();
        let s = settings(&[
            ("tracker_url", "https://ops.local"),
            ("tracker_token", "it_x"),
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
        assert_eq!(close_stale(&mut run, &["c.local|heap.low".into()]), [1]);
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(
            (sent[1].method, sent[1].url.as_str()),
            ("PATCH", "https://ops.local/api/v1/issues/1")
        );
        let body: Value = serde_json::from_slice(&sent[1].body).unwrap();
        assert_eq!(
            (body["state"].as_str(), body["resolution"].as_str()),
            (Some("closed"), Some("stale"))
        );
    }
}
