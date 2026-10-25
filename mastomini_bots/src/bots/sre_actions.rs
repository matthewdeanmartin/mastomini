//! One durable pending action; all network work runs on the existing worker.
//! A persisted `executing` record is never retried after a crash or lost reply.
use crate::http::{Request, Response};
use crate::mastodon::{normalize_instance, HttpClient, HttpRequest};
use crate::service::Service;
use crate::store::KvStore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const STORE: &str = "sre.action";
const DAY: u64 = 86_400_000;

struct ApprovalClient<'a>(&'a mut dyn HttpClient);
impl HttpClient for ApprovalClient<'_> {
    fn send(&mut self, req: &HttpRequest) -> Result<crate::mastodon::HttpResponse, String> {
        self.0.send_bounded(req, 16 * 1024)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Pending {
    pub id: String,
    pub board: String,
    pub action: String,
    pub stage: String,
    pub not_before_ms: u64,
    pub expires_ms: u64,
    pub code: String,
    pub summary: String,
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub approved_by: String,
    #[serde(default)]
    pub approval_account: String,
    #[serde(default)]
    pub approval_instance: String,
    #[serde(default)]
    pub reply_to: String,
}

pub fn load<S: KvStore>(store: &S) -> Result<Option<Box<Pending>>, String> {
    let Some(text) = store.get(STORE)? else {
        return Ok(None);
    };
    if text.len() > 2048 {
        return Err("Oversized SRE action record".into());
    }
    let mut item: Pending = serde_json::from_str(&text).map_err(|_| "Invalid SRE action record")?;
    if item.stage == "executing" || item.stage == "checking" {
        item.stage = "uncertain".into();
        item.code.clear();
        item.summary =
            "Interrupted by reboot; inspect the board before proposing another action".into();
    }
    Ok(Some(Box::new(item)))
}

fn save<S: KvStore>(svc: &mut Service<S>, item: Pending) -> Result<(), String> {
    let text = serde_json::to_string(&item).map_err(|_| "Could not encode action")?;
    if text.len() > 2048 {
        return Err("SRE action record too large".into());
    }
    svc.store.put(STORE, &text)?;
    svc.sre_action = Some(Box::new(item));
    Ok(())
}
fn random() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| "Random source unavailable")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// UTC quiet window, at most one day ahead. The browser shows the exact time.
fn window(now: u64, start: &str) -> Result<(u64, u64), String> {
    let (h, m) = start.split_once(':').ok_or("Invalid UTC quiet window")?;
    let h: u64 = h.parse().map_err(|_| "Invalid hour")?;
    let m: u64 = m.parse().map_err(|_| "Invalid minute")?;
    if h > 23 || m > 59 {
        return Err("Invalid UTC quiet window".into());
    }
    let mut begin = now / DAY * DAY + (h * 60 + m) * 60_000;
    if now >= begin + 30 * 60_000 {
        begin += DAY;
    }
    Ok((begin.max(now), begin + 30 * 60_000))
}

/// Called only after the admin API's session check. No network under its lock.
pub fn api<S: KvStore>(
    svc: &mut Service<S>,
    req: &Request,
    body: &Value,
    now: Option<u64>,
) -> Response {
    let result = (|| -> Result<Value, String> {
        let now = now.ok_or("Clock is not set")?;
        if req.method == "GET" {
            return Ok(json!({"pending": svc.sre_action}));
        }
        let i = svc.index("sre").ok_or("SRE bot unavailable")?;
        if req.path.ends_with("/propose") {
            if svc.sre_action.as_ref().is_some_and(|p| {
                matches!(
                    p.stage.as_str(),
                    "new" | "checking" | "pending" | "approved" | "executing"
                ) && (p.expires_ms > now || matches!(p.stage.as_str(), "checking" | "executing"))
            }) {
                return Err("An action is already pending".into());
            }
            let settings = svc.settings(i);
            let board = normalize_instance(settings.get("ops_board"))?;
            if !board.starts_with("https://")
                || !settings
                    .get("targets")
                    .lines()
                    .any(|t| t.trim().trim_end_matches('/') == board)
            {
                return Err("Choose an HTTPS action board from the SRE targets".into());
            }
            if settings.get("ops_safe_key").is_empty()
                || settings.get("ops_approval_key").is_empty()
            {
                return Err("Provision the separate safe and approval keys first".into());
            }
            let (not_before_ms, mut expires_ms) = window(now, settings.get("ops_quiet_utc"))?;
            if let Some(deadline) = body["expires_ms"].as_u64() {
                expires_ms = expires_ms.min(deadline);
                if expires_ms <= not_before_ms {
                    return Err("Forecast leaves no safe quiet window".into());
                }
            }
            let approval_account = settings.get("ops_admin_id").to_string();
            let approval_instance = svc.records[i].config.instance.clone();
            let reply_to = svc.records[i]
                .state
                .get("ops_digest_id")
                .cloned()
                .unwrap_or_default();
            save(
                svc,
                Pending {
                    id: random()?,
                    board,
                    action: "restart-planned".into(),
                    stage: "new".into(),
                    not_before_ms,
                    expires_ms,
                    code: random()?,
                    summary: "Waiting for catalog and dry-run checks".into(),
                    evidence: body["evidence"]
                        .as_str()
                        .unwrap_or("Requested from the admin site")
                        .chars()
                        .take(256)
                        .collect(),
                    approved_by: String::new(),
                    approval_account,
                    approval_instance,
                    reply_to,
                },
            )?;
        } else {
            let mut item = svc
                .sre_action
                .as_deref()
                .cloned()
                .ok_or("No pending action")?;
            if body["id"].as_str() != Some(&item.id) || now >= item.expires_ms {
                return Err("Action changed or expired; refresh".into());
            }
            if req.path.ends_with("/approve") {
                if item.stage != "pending" || body["code"].as_str() != Some(&item.code) {
                    return Err("A current reviewed action and matching code are required".into());
                }
                item.stage = "approved".into();
                item.code.clear();
                item.approved_by = "admin-site".into();
            } else if req.path.ends_with("/cancel") {
                if matches!(item.stage.as_str(), "executing" | "checking") {
                    return Err("Action already in progress".into());
                }
                item.stage = "cancelled".into();
                item.code.clear();
            } else {
                return Err("Unknown action request".into());
            }
            save(svc, item)?;
        }
        Ok(json!({"pending": svc.sre_action}))
    })();
    match result {
        Ok(v) => Response::ok(v),
        Err(e) => Response::error(409, &e),
    }
    .with_header("Cache-Control", "no-store")
}

fn call(
    http: &mut dyn HttpClient,
    board: &str,
    method: &'static str,
    path: &str,
    key: &str,
    id: &str,
    grant: &str,
) -> Result<Value, String> {
    let mut headers = vec![
        ("Accept".into(), "application/json".into()),
        ("Authorization".into(), format!("Bearer {key}")),
        ("Idempotency-Key".into(), id.into()),
    ];
    if !grant.is_empty() {
        headers.push(("X-Ops-Approval".into(), grant.into()));
    }
    let response = http
        .send_bounded(
            &HttpRequest {
                method,
                url: format!("{board}{path}"),
                headers,
                body: Vec::new(),
            },
            8192,
        )
        .map_err(|_| "No answer; inspect the board before retrying")?;
    if !(200..300).contains(&response.status) || response.body.len() > 8192 {
        return Err(format!(
            "Board refused or response exceeded limit (HTTP {})",
            response.status
        ));
    }
    serde_json::from_slice(&response.body).map_err(|_| "Invalid action response".into())
}

fn perform(
    http: &mut dyn HttpClient,
    item: &Pending,
    safe: &str,
    admin: &str,
    checking: bool,
    now: u64,
) -> Result<(), String> {
    let began = std::time::Instant::now();
    let in_window = || now.saturating_add(began.elapsed().as_millis() as u64) < item.expires_ms;
    let catalog = call(
        http,
        &item.board,
        "GET",
        "/.well-known/ops/actions",
        safe,
        &item.id,
        "",
    )?;
    let installed = catalog["schema"] == "miniframework.actions/1"
        && catalog["actions"].as_array().is_some_and(|a| {
            a.iter().any(|a| {
                a["id"] == "restart-planned"
                    && a["risk"] == "disruptive"
                    && a["needs_approval"] == true
                    && a["params"] == "EmptyParams"
            })
        });
    if !installed {
        return Err("Board has no supported planned-restart action".into());
    }
    call(
        http,
        &item.board,
        "POST",
        "/.well-known/ops/actions/restart-planned?dry_run=1",
        safe,
        &item.id,
        "",
    )?;
    if checking {
        return Ok(());
    }
    if now < item.not_before_ms || !in_window() {
        return Err("Approval window expired".into());
    }
    // Fresh health before issuing the one-use grant. A network failure stops
    // here; a shared outage is separately suppressed at worker selection.
    let health = call(
        http,
        &item.board,
        "GET",
        "/.well-known/ops/health",
        safe,
        &item.id,
        "",
    )?;
    if health["schema"] != "miniframework.health/1"
        || !matches!(health["status"].as_str(), Some("ok" | "warn" | "fail"))
    {
        return Err("Fresh board health is unavailable".into());
    }
    if !in_window() {
        return Err("Approval window expired".into());
    }
    let approval = call(
        http,
        &item.board,
        "POST",
        "/.well-known/ops/approvals/restart-planned",
        admin,
        &item.id,
        "",
    )?;
    let grant = approval["token"]
        .as_str()
        .filter(|t| t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("Invalid approval grant")?;
    if !in_window() {
        return Err("Approval window expired".into());
    }
    let outcome = call(
        http,
        &item.board,
        "POST",
        "/.well-known/ops/actions/restart-planned",
        safe,
        &item.id,
        grant,
    )?;
    if outcome["outcome"] != "accepted" {
        return Err("Unrecognized outcome; inspect the board".into());
    }
    Ok(())
}

/// A trend may prepare a review, never approve one. Keep completed records for
/// review instead of generating an endless sequence of proposals from one sample.
pub fn suggest<S: KvStore>(shared: &Arc<Mutex<Service<S>>>, now: Option<u64>) {
    let Some(now) = now else {
        return;
    };
    let mut svc = shared.lock().unwrap();
    if svc.sre_action.is_some() {
        return;
    }
    let Some(i) = svc.index("sre") else {
        return;
    };
    let settings = svc.settings(i);
    if !svc.records[i].config.enabled || !settings.yes("ops_enabled") {
        return;
    }
    let Some(raw) = svc.records[i]
        .state
        .get("heap_trends")
        .filter(|s| s.len() <= 4096)
    else {
        return;
    };
    let Ok(trends) = serde_json::from_str::<Vec<super::sre_trends::Trend>>(raw) else {
        return;
    };
    let Ok(board) = normalize_instance(settings.get("ops_board")) else {
        return;
    };
    let host = board.strip_prefix("https://").unwrap_or("");
    let Ok((begin, _)) = window(now, settings.get("ops_quiet_utc")) else {
        return;
    };
    let Some(trend) = trends.iter().find(|t| {
        t.host == host
            && t.observed_ms <= now
            && now - t.observed_ms <= 15 * 60_000
            && begin.saturating_add(15 * 60_000) < t.estimated_floor_ms
    }) else {
        return;
    };
    let req = Request::new("POST", "/api/v1/bots/sre/actions/propose");
    let _ = api(
        &mut svc,
        &req,
        &json!({"expires_ms": trend.estimated_floor_ms.saturating_sub(5 * 60_000),
            "evidence": format!("Heap falling {:.1} KiB/h; estimated warning threshold at {}. This is an estimate, not a confirmed future outage.", -trend.bytes_per_hour / 1024.0, crate::tz::iso(trend.estimated_floor_ms))}),
        Some(now),
    );
}

fn approves(item: &Pending, status: &crate::mastodon::Status) -> bool {
    if item.approval_account.is_empty()
        || item.reply_to.is_empty()
        || status.account.id != item.approval_account
        || status.in_reply_to_id.as_deref() != Some(&item.reply_to)
    {
        return false;
    }
    let words: Vec<_> = status
        .text
        .split_whitespace()
        .filter(|s| !s.starts_with('@'))
        .collect();
    words == ["approve", item.code.as_str()] && !item.code.is_empty()
}

/// Authenticated account ids come from the configured instance's notification
/// API, never from post text or model output. Codes are action-specific and are
/// consumed durably before the executor can see an approved item.
pub fn mentions<S: KvStore>(
    shared: &Arc<Mutex<Service<S>>>,
    http: &mut dyn HttpClient,
    now: Option<u64>,
) {
    let Some(now) = now else {
        return;
    };
    let (item, token) = {
        let mut svc = shared.lock().unwrap();
        if now < svc.sre_mention_at {
            return;
        }
        svc.sre_mention_at = now.saturating_add(60_000);
        let Some(i) = svc.index("sre") else {
            return;
        };
        let Some(item) = svc.sre_action.as_deref() else {
            return;
        };
        if item.stage != "pending"
            || item.expires_ms <= now
            || item.approval_account.is_empty()
            || item.reply_to.is_empty()
            || !svc.records[i].config.enabled
            || !svc.settings(i).yes("ops_enabled")
            || svc.settings(i).get("ops_admin_id") != item.approval_account
            || svc.records[i].config.instance != item.approval_instance
        {
            return;
        }
        (item.clone(), svc.records[i].config.token.clone())
    };
    let Ok(mentions) =
        crate::mastodon::Mastodon::new(&mut ApprovalClient(http), &item.approval_instance, &token)
            .mentions(None, 20)
    else {
        return;
    };
    if !mentions.iter().any(|m| approves(&item, &m.status)) {
        return;
    }
    let mut svc = shared.lock().unwrap();
    if !svc
        .sre_action
        .as_ref()
        .is_some_and(|p| p.id == item.id && p.stage == "pending" && p.code == item.code)
    {
        return;
    }
    let mut next = item;
    next.stage = "approved".into();
    next.code.clear();
    next.approved_by = "admin-mention".into();
    let _ = save(&mut svc, next);
}

/// At most one action per pass, persisted before sending anything. The bot must
/// be enabled to execute; proposing/checking remains an explicit admin action.
pub fn tick<S: KvStore>(
    shared: &Arc<Mutex<Service<S>>>,
    http: &mut dyn HttpClient,
    now: Option<u64>,
) -> bool {
    let Some(now) = now else {
        return false;
    };
    let (item, safe, admin, checking, targets) = {
        let mut svc = shared.lock().unwrap();
        let Some(i) = svc.index("sre") else {
            return false;
        };
        let Some(mut item) = svc.sre_action.as_deref().cloned() else {
            return false;
        };
        let checking = item.stage == "new";
        if !checking && (item.stage != "approved" || now < item.not_before_ms) {
            return false;
        }
        if now >= item.expires_ms {
            item.stage = "expired".into();
            item.code.clear();
            let _ = save(&mut svc, item);
            return false;
        }
        let settings = svc.settings(i);
        if !checking && (!svc.records[i].config.enabled || !settings.yes("ops_enabled")) {
            return false;
        }
        if normalize_instance(settings.get("ops_board"))
            .ok()
            .as_deref()
            != Some(&item.board)
        {
            return false;
        }
        if !checking
            && svc.records[i]
                .state
                .get("ops_observations")
                .is_some_and(|s| s.contains("Shared outage suspected:"))
        {
            item.stage = "cancelled".into();
            item.summary = "Shared outage finding; restart suppressed. Check the network and propose again after recovery".into();
            item.code.clear();
            let _ = save(&mut svc, item);
            return false;
        }
        item.stage = if checking { "checking" } else { "executing" }.into();
        if save(&mut svc, item.clone()).is_err() {
            return false;
        }
        let targets: Vec<String> = settings
            .get("targets")
            .lines()
            .filter_map(|s| normalize_instance(s.trim()).ok())
            .take(12)
            .collect();
        (
            item,
            settings.get("ops_safe_key").to_string(),
            settings.get("ops_approval_key").to_string(),
            checking,
            targets,
        )
    };
    let mut down = 0;
    let began = std::time::Instant::now();
    if !checking {
        for base in &targets {
            // Never send the board token to another target.
            let request = HttpRequest {
                method: "GET",
                url: format!("{base}/.well-known/ops/health"),
                headers: vec![("Accept".into(), "application/json".into())],
                body: Vec::new(),
            };
            if !http
                .send_bounded(&request, 8192)
                .is_ok_and(|r| r.status == 200)
            {
                down += 1;
            }
            if down >= 2 {
                break;
            }
        }
    }
    let result = if down >= 2 {
        Err(
            "Several boards are unreachable now; shared outage suspected, restart suppressed"
                .into(),
        )
    } else {
        perform(
            http,
            &item,
            &safe,
            &admin,
            checking,
            now.saturating_add(began.elapsed().as_millis() as u64),
        )
    };
    let mut next = item;
    match result {
        Ok(()) => {
            next.stage = if checking { "pending" } else { "accepted" }.into();
            next.summary = if checking { "Dry run passed. Restart loses volatile state and interrupts service. Approve only for the displayed UTC window." } else { "Board accepted the planned restart; verify its health and reset reason after it returns" }.into();
        }
        Err(e) => {
            next.stage = if checking { "failed" } else { "uncertain" }.into();
            next.summary = e;
            next.code.clear();
        }
    }
    let mut svc = shared.lock().unwrap();
    // If persistence fails, the already durable executing/checking marker
    // remains; neither a later tick nor a reboot can repeat the network action.
    let _ = save(&mut svc, next);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastodon::testing::Scripted;
    const NOW: u64 = 2 * 3_600_000;
    fn prepared() -> Arc<Mutex<Service<crate::store::MemStore>>> {
        let mut svc = Service::open(
            crate::store::MemStore::default(),
            vec![Box::new(super::super::sre::Sre)],
        )
        .unwrap();
        svc.records[0].config.enabled = true;
        for (k, v) in [
            ("targets", "https://board.invalid"),
            ("ops_board", "https://board.invalid"),
            ("ops_safe_key", "SAFE"),
            ("ops_approval_key", "ADMIN"),
            ("ops_enabled", "yes"),
            ("ops_admin_id", "42"),
        ] {
            svc.records[0].settings.insert(k.into(), v.into());
        }
        svc.records[0]
            .state
            .insert("ops_digest_id".into(), "digest-1".into());
        let req = Request::new("POST", "/api/v1/bots/sre/actions/propose");
        assert_eq!(api(&mut svc, &req, &json!({}), Some(NOW)).status, 200);
        Arc::new(Mutex::new(svc))
    }
    fn dry_run(http: &mut Scripted) {
        http.answer(200, json!({"schema":"miniframework.actions/1","actions":[{"id":"restart-planned","risk":"disruptive","needs_approval":true,"params":"EmptyParams"}]}));
        http.answer(200, json!({"outcome":"dry-run; nothing executed"}));
    }
    #[test]
    fn approval_is_durable_single_use_and_lost_execution_response_is_not_retried() {
        let shared = prepared();
        let mut http = Scripted::default();
        dry_run(&mut http);
        assert!(tick(&shared, &mut http, Some(NOW)));
        assert!(!tick(&shared, &mut http, Some(NOW)));
        {
            let mut svc = shared.lock().unwrap();
            let p = svc.sre_action.as_deref().unwrap().clone();
            let req = Request::new("POST", "/api/v1/bots/sre/actions/approve");
            assert_eq!(
                api(
                    &mut svc,
                    &req,
                    &json!({"id":"wrong","code":p.code}),
                    Some(NOW)
                )
                .status,
                409
            );
            assert_eq!(
                api(&mut svc, &req, &json!({"id":p.id,"code":p.code}), Some(NOW)).status,
                200
            );
            assert_eq!(
                api(&mut svc, &req, &json!({"id":p.id,"code":p.code}), Some(NOW)).status,
                409
            );
            let stored = load(&svc.store).unwrap().unwrap();
            assert_eq!(stored.stage, "approved");
            assert!(stored.code.is_empty());
        }
        http.answer(200, json!({})); // shared-outage preflight
        dry_run(&mut http);
        http.answer(
            200,
            json!({"schema":"miniframework.health/1", "status":"warn"}),
        );
        http.answer(200, json!({"token":"0123456789abcdef0123456789abcdef"}));
        http.fail("response lost after restart request");
        assert!(tick(&shared, &mut http, Some(NOW)));
        assert_eq!(
            shared.lock().unwrap().sre_action.as_ref().unwrap().stage,
            "uncertain"
        );
        let count = http.sent.lock().unwrap().len();
        assert!(!tick(&shared, &mut http, Some(NOW)));
        assert_eq!(http.sent.lock().unwrap().len(), count);
    }
    #[test]
    fn mention_authority_is_exact_account_thread_and_code_not_display_text() {
        let shared = prepared();
        let p = shared
            .lock()
            .unwrap()
            .sre_action
            .as_deref()
            .unwrap()
            .clone();
        let mut status = crate::mastodon::Status {
            id: "reply".into(),
            account: crate::mastodon::Account {
                id: "42".into(),
                acct: "admin".into(),
                display_name: "admin".into(),
                note: String::new(),
                bot: false,
            },
            text: format!("@sre approve {}", p.code),
            spoiler_text: String::new(),
            visibility: "private".into(),
            in_reply_to_id: Some("digest-1".into()),
            mentions: vec!["sre".into()],
        };
        assert!(approves(&p, &status));
        status.account.id = "43".into();
        assert!(!approves(&p, &status));
        status.account.id = "42".into();
        status.in_reply_to_id = Some("other".into());
        assert!(!approves(&p, &status));
        status.in_reply_to_id = Some("digest-1".into());
        status.text = "@sre approve another-code".into();
        assert!(!approves(&p, &status));
    }
    #[test]
    fn quiet_window_and_reboot_fail_closed() {
        assert_eq!(
            window(2 * 3_600_000, "02:00").unwrap(),
            (2 * 3_600_000, 2 * 3_600_000 + 30 * 60_000)
        );
        assert!(window(3 * 3_600_000, "02:00").unwrap().0 > DAY);
        assert!(window(0, "24:00").is_err());
        let item = Pending {
            id: "i".into(),
            board: "b".into(),
            action: "restart-planned".into(),
            stage: "executing".into(),
            not_before_ms: 0,
            expires_ms: DAY,
            code: "c".into(),
            summary: String::new(),
            evidence: String::new(),
            approved_by: String::new(),
            approval_account: String::new(),
            approval_instance: String::new(),
            reply_to: String::new(),
        };
        let mut store = crate::store::MemStore::default();
        store
            .put(STORE, &serde_json::to_string(&item).unwrap())
            .unwrap();
        assert_eq!(load(&store).unwrap().unwrap().stage, "uncertain");
    }
    #[test]
    fn a_dry_run_never_uses_approval_key_or_executes() {
        let mut http = Scripted::default();
        http.answer(200, json!({"schema":"miniframework.actions/1","actions":[{"id":"restart-planned","risk":"disruptive","needs_approval":true,"params":"EmptyParams"}]}));
        http.answer(200, json!({"outcome":"dry-run; nothing executed"}));
        let sent = http.sent.clone();
        let p = Pending {
            id: "00000000000000000000000000000001".into(),
            board: "https://test.invalid".into(),
            action: "restart-planned".into(),
            stage: "checking".into(),
            not_before_ms: 0,
            expires_ms: DAY,
            code: String::new(),
            summary: String::new(),
            evidence: String::new(),
            approved_by: String::new(),
            approval_account: String::new(),
            approval_instance: String::new(),
            reply_to: String::new(),
        };
        perform(&mut http, &p, "SAFE", "SECRET-ADMIN", true, 0).unwrap();
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert!(sent[1].url.ends_with("?dry_run=1"));
        assert!(sent
            .iter()
            .all(|r| r.headers.iter().all(|(_, v)| !v.contains("SECRET-ADMIN"))));
    }
}
