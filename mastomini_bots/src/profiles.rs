//! Public bot instructions and independently persisted profile publication.
use crate::{
    bot::{BotInfo, RunError},
    http::{Request, Response},
    mastodon::{HttpClient, Mastodon},
    service::Service,
    store::KvStore,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

pub const DEFAULT_ORIGIN: &str = "https://mastomini-bots.local";
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Publication {
    pub signature: String,
    pub account_url: String,
}
#[derive(Clone, Default)]
pub struct Runtime {
    pub requested: bool,
    pub attempted: String,
    pub retry_at: u64,
    pub error: Option<String>,
}
pub fn instructions(id: &str) -> &'static str {
    match id {
        "good_morning" => "Follow for a scheduled morning greeting. The owner chooses its time, time zone and message. Replies do not control this bot.",
        "fishtank" => "Follow the daily life of a simulated aquarium. Fish and events develop from a fixed seed and start date. This is a watch-only simulation; replies and votes do not change it.",
        "mars_rover" => "Explore Mars together! Everyone controls one shared rover, revealing terrain and collecting samples from new sites. Reply to a post in the current mission thread, mentioning this bot, with scan to look around (8 battery), move north to drive (10, or 15 into a crater), or sample to collect material (15). Map and status are free; charge restores battery. Commands run at the next poll. Reply help to get started, or help moves, help energy, or help samples for details. Coordinates such as D4 mean column D, row 4 on an 8 by 8 map. A sol is one mission day: each daily update recharges the rover and attempts an automatic move. Posts labeled Game save (ignore) contain the bot's memory; you never need to read or copy their code.",
        "shoebox" => "A new civilization begins with three inhabitants. Vote in the latest poll to choose its next action. The most votes wins; ties and days without votes are resolved randomly. Polls last 23 hours. Gemma narrates births, inventions, politics, disasters and questionable leaders using a rule book and condensed history. Win by reaching 12 inhabitants, reliable food, safe shelter, three useful inventions and an elected government. After victory or extinction, a new game begins the following day. Replies are conversation, not game commands.",
        "lost_things" => "A mysterious lost item appears each day. Reply directly to its post with a fictional claim that answers the item's challenge. Gemma may accept or reject it. Each item can be awarded once; only the latest seven items accept claims. Bot replies and direct messages are ignored. This is fictional lost property: do not send personal information or real belongings.",
        "llm_reply" => "Mention this account to start a conversation. It checks for eligible mentions on its configured schedule and writes replies with a language model. It skips mentions from before its first check. The owner sets its character, reply rules and whether it also posts a daily thought. Its replies are generated fiction and conversation, and can be mistaken.",
        "llm_post" => "Follow for scheduled language-model posts. The owner chooses the character, prompt and posting schedule. This posting bot does not process replies as commands. Generated text can be mistaken.",
        "nana_news" => "Follow for a scheduled NanaCoin market bulletin. It reports market activity from NanaCoin. Replies do not place orders or control the news bot.",
        id if id.starts_with("trader_") => "This automated NanaCoin trader follows the owner's selected strategy, reserves and spending limits. Dry-run mode logs proposed actions without trading. With posting enabled, it shares trading updates here. Replies do not place trades or change its strategy; the owner manages it on the bot board.",
        _ => "Follow this bot's posts. Its owner controls its settings and schedule on the bot board.",
    }
}
fn bio(info: &BotInfo) -> String {
    // Mastomini's bio budget is 160 characters / 640 bytes.
    crate::text::fit(info.description, 160)
}
fn signature(info: &BotInfo, origin: &str, instance: &str, token: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!(
            "profile-v1\0{}\0{}\0{}\0{}\0{}",
            info.id,
            bio(info),
            origin,
            instance,
            token
        ))
    )
}
pub fn validate_origin(value: &str) -> Result<String, String> {
    let origin = crate::mastodon::normalize_instance(value)?;
    // Mastomini stores profile field values in 100 bytes, including the path.
    if origin.len() + "/bots/".len() + 13 > 100 {
        return Err("Board address is too long for a profile link".into());
    }
    Ok(origin)
}
pub fn load<S: KvStore>(store: &S, id: &str) -> Result<Publication, String> {
    store
        .get(&format!("p.{id}"))?
        .map(|v| serde_json::from_str(&v).map_err(|e| e.to_string()))
        .unwrap_or_else(|| Ok(Publication::default()))
}
fn publish(
    http: &mut dyn HttpClient,
    info: &BotInfo,
    origin: &str,
    instance: &str,
    token: &str,
) -> Result<String, RunError> {
    let mut client = Mastodon::new(http, instance, token);
    let me = client.call("GET", "/api/v1/accounts/verify_credentials", None, None)?;
    let username = me["username"]
        .as_str()
        .filter(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'))
        .ok_or_else(|| RunError::permanent("Account has no valid local username"))?;
    let account_url = format!("{instance}/@{username}");
    let url = format!("{origin}/bots/{}", info.id);
    let mut fields = vec![json!({"name":"Instructions", "value":url})];
    // Source fields are raw editable text; public fields may contain HTML.
    let existing = me["source"]["fields"]
        .as_array()
        .ok_or_else(|| RunError::permanent("Account response has no editable profile fields"))?;
    for field in existing {
        if field["name"] == "Instructions" {
            continue;
        }
        if fields.len() == 4 {
            break;
        }
        fields.push(json!({"name":field["name"].as_str().unwrap_or(""), "value":field["value"].as_str().unwrap_or("")}));
    }
    client.call(
        "PATCH",
        "/api/v1/accounts/update_credentials",
        Some(&json!({"bot":true,"note":bio(info),"fields_attributes":fields})),
        None,
    )?;
    Ok(account_url)
}
/// One profile per pass, with no network calls while holding the service lock.
/// Profiles have their own NVS record so game memory retains its full budget.
pub fn tick<S: KvStore>(
    shared: &Arc<Mutex<Service<S>>>,
    http: &mut dyn HttpClient,
    now: Option<u64>,
) {
    let Some(now) = now else {
        return;
    };
    let job = {
        let svc = shared.lock().unwrap_or_else(|e| e.into_inner());
        (0..svc.infos.len()).find_map(|i| {
            let config = &svc.records[i].config;
            if config.instance.is_empty() || config.token.is_empty() || svc.runtime[i].running {
                return None;
            }
            let sig = signature(
                &svc.infos[i],
                &svc.public_origin,
                &config.instance,
                &config.token,
            );
            let rt = &svc.profile_runtime[i];
            if !rt.requested
                && (svc.profiles[i].signature == sig || rt.attempted == sig && now < rt.retry_at)
            {
                return None;
            }
            Some((
                i,
                svc.infos[i].clone(),
                svc.public_origin.clone(),
                config.instance.clone(),
                config.token.clone(),
                sig,
            ))
        })
    };
    let Some((i, info, origin, instance, token, sig)) = job else {
        return;
    };
    let result = publish(http, &info, &origin, &instance, &token);
    let mut svc = shared.lock().unwrap_or_else(|e| e.into_inner());
    // A concurrent settings change must not mark the new account as published.
    if signature(
        &svc.infos[i],
        &svc.public_origin,
        &svc.records[i].config.instance,
        &svc.records[i].config.token,
    ) != sig
    {
        return;
    }
    svc.profile_runtime[i].requested = false;
    svc.profile_runtime[i].attempted = sig.clone();
    let result = result.and_then(|account_url| {
        let publication = Publication {
            signature: sig,
            account_url,
        };
        let data =
            serde_json::to_string(&publication).map_err(|e| RunError::permanent(e.to_string()))?;
        svc.store
            .put(&format!("p.{}", info.id), &data)
            .map_err(|e| RunError {
                message: e,
                retryable: true,
            })?;
        svc.profiles[i] = publication;
        Ok(())
    });
    match result {
        Ok(()) => {
            svc.profile_runtime[i].error = None;
            svc.activity.add(
                Some(now),
                Some(info.id),
                "ok",
                "Published bio and Instructions profile link",
            );
        }
        Err(e) => {
            svc.profile_runtime[i].retry_at = if e.retryable {
                now.saturating_add(30 * 60_000)
            } else {
                u64::MAX
            };
            svc.profile_runtime[i].error = Some(e.message.clone());
            svc.activity.add(
                Some(now),
                Some(info.id),
                "error",
                format!("Profile publication: {}", e.message),
            );
        }
    }
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
pub fn page<S: KvStore>(svc: &Service<S>, req: &Request, id: Option<&str>) -> Response {
    if !matches!(req.method.as_str(), "GET" | "HEAD") {
        return Response::error(405, "Use GET or HEAD").with_header("Allow", "GET, HEAD");
    }
    let (title, body) = if let Some(id) = id {
        let Some(i) = svc.index(id) else {
            return Response::error(404, "No such bot");
        };
        let info = &svc.infos[i];
        let mut body = format!(
            "<h1>{}</h1><p>{}</p><h2>How to participate</h2><p>{}</p><p>Schedule: {}.</p>",
            escape(info.name),
            escape(info.description),
            escape(instructions(id)),
            escape(&svc.schedule(i).describe())
        );
        if !svc.profiles[i].account_url.is_empty() {
            body.push_str(&format!(
                "<p><a rel=\"me\" href=\"{}\">Follow this bot</a></p>",
                escape(&svc.profiles[i].account_url)
            ));
        }
        body.push_str("<p><a href=\"/bots\">All bots</a></p>");
        (info.name.to_string(), body)
    } else {
        let items = svc
            .infos
            .iter()
            .map(|i| {
                format!(
                    "<li><a href=\"/bots/{}\">{}</a> — {}</li>",
                    i.id,
                    escape(i.name),
                    escape(i.description)
                )
            })
            .collect::<String>();
        ("Meet the bots".into(), format!("<h1>Meet the bots</h1><p>What each bot does and how to take part.</p><ul>{items}</ul>"))
    };
    let html = format!("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>body{{font:18px/1.6 system-ui;margin:3rem auto;padding:0 1rem;max-width:44rem;color:#243040;background:#faf9f6}}a{{color:#245f91}}li{{margin:1rem 0}}h1,h2{{line-height:1.2}}</style><main>{body}</main></html>", escape(&title));
    let mut response = Response::new(200, "text/html; charset=utf-8", html)
        .with_header("Cache-Control", "no-cache")
        .with_header("X-Content-Type-Options", "nosniff");
    if req.method == "HEAD" {
        response.body.clear();
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mastodon::testing::Scripted, store::MemStore};
    fn shared() -> Arc<Mutex<Service<MemStore>>> {
        let mut svc = Service::open(MemStore::default(), crate::bots::all()).unwrap();
        svc.records[0].config.token = "private-key".into();
        Arc::new(Mutex::new(svc))
    }
    fn response(http: &Scripted) {
        http.answer(200, json!({"username":"morning", "source":{"fields":[{"name":"Other","value":"https://example.org"},{"name":"Instructions","value":"https://old.example"}]}}));
        http.answer(200, json!({"id":"1"}));
    }
    #[test]
    fn publishes_once_preserves_fields_and_survives_restart() {
        let svc = shared();
        let mut http = Scripted::default();
        response(&http);
        tick(&svc, &mut http, Some(1000));
        tick(&svc, &mut http, Some(2000));
        let sent = http.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].method, "PATCH");
        let body: serde_json::Value = serde_json::from_slice(&sent[1].body).unwrap();
        assert_eq!(body["fields_attributes"][0]["name"], "Instructions");
        assert_eq!(
            body["fields_attributes"][0]["value"],
            "https://mastomini-bots.local/bots/good_morning"
        );
        assert_eq!(body["fields_attributes"][1]["name"], "Other");
        assert_eq!(body["fields_attributes"].as_array().unwrap().len(), 2);
        assert!(body["note"].as_str().unwrap().chars().count() <= 160);
        assert_eq!(body["bot"], true);
        drop(sent);
        let mut s = svc.lock().unwrap();
        let record = serde_json::to_string(&s.records[0]).unwrap();
        s.store.put("b.good_morning", &record).unwrap();
        let restarted = Arc::new(Mutex::new(
            Service::open(s.store.clone(), crate::bots::all()).unwrap(),
        ));
        drop(s);
        tick(&restarted, &mut http, Some(3000));
        assert_eq!(http.sent.lock().unwrap().len(), 2);
        let s = restarted.lock().unwrap();
        let page = page(
            &s,
            &Request::new("GET", "/bots/good_morning"),
            Some("good_morning"),
        );
        let html = String::from_utf8(page.body).unwrap();
        assert!(html.contains("rel=\"me\" href=\"https://mastomini.local/@morning\""));
        assert!(!html.contains("private-key"));
    }
    #[test]
    fn errors_retry_without_spamming_and_manual_request_retries_permanent_errors() {
        let svc = shared();
        let mut http = Scripted::default();
        http.answer(403, json!({"error":"missing write:accounts"}));
        tick(&svc, &mut http, Some(1000));
        tick(&svc, &mut http, Some(10000000));
        assert_eq!(http.sent.lock().unwrap().len(), 1);
        assert!(svc.lock().unwrap().profile_runtime[0].error.is_some());
        svc.lock().unwrap().profile_runtime[0].requested = true;
        response(&http);
        tick(&svc, &mut http, Some(10000001));
        assert!(svc.lock().unwrap().profile_runtime[0].error.is_none());
        svc.lock()
            .unwrap()
            .set_public_origin("https://new.local")
            .unwrap();
        http.fail("offline");
        tick(&svc, &mut http, Some(10000002));
        tick(&svc, &mut http, Some(10000003));
        assert_eq!(http.sent.lock().unwrap().len(), 4);
        response(&http);
        tick(&svc, &mut http, Some(12000003));
        let sent = http.sent.lock().unwrap();
        let body: serde_json::Value = serde_json::from_slice(&sent[5].body).unwrap();
        assert_eq!(
            body["fields_attributes"][0]["value"],
            "https://new.local/bots/good_morning"
        );
    }
    #[test]
    fn pages_are_public_and_do_not_expose_settings() {
        let svc = shared();
        let s = svc.lock().unwrap();
        for info in &s.infos {
            let r = page(&s, &Request::new("GET", "/"), Some(info.id));
            assert_eq!(r.status, 200);
            let html = String::from_utf8(r.body).unwrap();
            assert!(html.contains("How to participate"));
            assert!(!html.contains("private-key"));
        }
        assert_eq!(
            page(&s, &Request::new("GET", "/"), Some("missing")).status,
            404
        );
        assert!(page(&s, &Request::new("HEAD", "/"), Some("shoebox"))
            .body
            .is_empty());
        assert_eq!(page(&s, &Request::new("POST", "/"), None).status, 405);
        assert!(validate_origin("javascript:alert(1)").is_err());
        assert!(validate_origin("https://example.org/path").is_err());
    }
}
