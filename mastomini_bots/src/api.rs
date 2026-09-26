//! The admin site's JSON API (`/api/v1/...`), the admin app (`/app/`) and
//! the household CA (`/ca`), over [`http`](crate::http) types so the desktop
//! build and the board share it.
//!
//! One admin, one password (set on the first visit). Everything but
//! `status`, `version`, `setup`, `login`, `/ca` and the app itself needs a
//! session. API keys for Mastodon go in, never come back out.

use crate::http::{Request, Response, BODY_LIMIT};
use crate::service::{ConfigUpdate, JobKind, OpenRouterUpdate, RunRecord, Service};
use crate::store::KvStore;
use crate::tz::iso;
use serde::Serialize;
use serde_json::{json, Value};

/// Facts only the platform knows, for `/api/v1/diag`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Platform {
    pub target: &'static str,
    pub uptime_ms: u64,
    pub heap_internal_free: Option<u64>,
    pub heap_internal_min_free: Option<u64>,
    pub psram_free: Option<u64>,
    pub wifi_rssi: Option<i32>,
    pub reset_reason: Option<&'static str>,
    pub ip: Option<String>,
}

/// The HTTPS side, when this build serves it.
#[derive(Debug, Clone)]
pub struct Tls {
    pub ca_der: Vec<u8>,
    pub ca_pem: String,
    /// `certificate.json` from scripts/certs.sh.
    pub info: Value,
}

pub struct Ctx {
    pub platform: fn() -> Platform,
    pub tls: Option<Tls>,
}

fn build() -> Value {
    json!({
        "name": "mastomini-bots",
        "version": env!("CARGO_PKG_VERSION"),
        "commit": Some(env!("MASTOBOTS_COMMIT")).filter(|c| !c.is_empty()),
        "dirty": env!("MASTOBOTS_DIRTY") == "1",
        "built_at": env!("MASTOBOTS_BUILT_MS").parse::<u64>().ok().map(iso),
    })
}

fn record(r: &Option<RunRecord>) -> Value {
    r.as_ref().map_or(Value::Null, |r| {
        json!({
            "slot_at": iso(r.slot_ms),
            "started_at": iso(r.started_ms),
            "finished_at": iso(r.finished_ms),
            "manual": r.manual,
            "ok": r.ok,
            "summary": r.summary,
        })
    })
}

fn bot_json<S: KvStore>(svc: &Service<S>, i: usize, now: Option<u64>) -> Value {
    let info = &svc.infos[i];
    let rec = &svc.records[i];
    let rt = &svc.runtime[i];
    json!({
        "id": info.id,
        "name": info.name,
        "description": info.description,
        "schedule": svc.schedule(i).describe(),
        "uses_llm": info.uses_llm,
        "settings": svc.settings(i).public(),
        "grace_minutes": info.grace_minutes,
        "enabled": rec.config.enabled,
        "instance": rec.config.instance,
        "key_set": !rec.config.token.is_empty(),
        "running": rt.running,
        "queued": rt.manual_requested || rt.check_requested,
        "next_run_at": now.and_then(|n| svc.next_run(i, n)).map(iso),
        "retrying": rt.retry_at.is_some(),
        "last_run": record(&rec.last_run),
        "last_check": record(&rec.last_check),
        "runs": rec.runs,
        "failures": rec.failures,
    })
}

/// The device's OpenRouter settings; the key only says whether it is set.
fn openrouter_json<S: KvStore>(svc: &Service<S>) -> Value {
    json!({
        "key_set": !svc.openrouter.key.is_empty(),
        "model": svc.openrouter.model,
        "default_model": crate::openrouter::DEFAULT_MODEL,
        "used_by": svc.infos.iter().filter(|i| i.uses_llm).map(|i| i.id).collect::<Vec<_>>(),
    })
}

fn find<S: KvStore>(svc: &Service<S>, id: &str) -> Result<usize, Response> {
    svc.index(id)
        .ok_or_else(|| Response::error(404, "No such bot"))
}

fn str_field<'v>(body: &'v Value, name: &str) -> &'v str {
    body[name].as_str().unwrap_or("")
}

/// Handle one request. `now` is wall-clock ms once SNTP has set it;
/// `mono_ms` is time since boot (sessions and lockouts use it, so signing
/// in works before the clock is set).
pub fn handle<S: KvStore>(
    svc: &mut Service<S>,
    ctx: &Ctx,
    req: &Request,
    now: Option<u64>,
    mono_ms: u64,
) -> Response {
    if req.body.len() > BODY_LIMIT {
        return Response::error(413, "Request body is too large");
    }
    let seg = req.segments();
    let method = req.method.as_str();
    match (method, seg.as_slice()) {
        ("GET", []) => return Response::redirect("/app/"),
        ("GET" | "HEAD", ["app", ..]) => return crate::web::serve(req),
        ("GET", ["favicon.ico"]) => return Response::new(204, "image/x-icon", Vec::new()),
        ("GET", ["ca" | "ca.pem"]) => {
            return match &ctx.tls {
                Some(tls) if seg[0] == "ca" => {
                    Response::new(200, "application/x-x509-ca-cert", tls.ca_der.clone())
                        .with_header(
                            "Content-Disposition",
                            "attachment; filename=\"household-ca.der\"",
                        )
                }
                Some(tls) => Response::new(200, "application/x-pem-file", tls.ca_pem.clone())
                    .with_header(
                        "Content-Disposition",
                        "attachment; filename=\"household-ca.pem\"",
                    ),
                None => Response::error(404, "This build serves plain HTTP only"),
            }
        }
        (_, ["api", "v1", ..]) => {}
        _ => return Response::error(404, "Not found"),
    }
    let rest = &seg[2..];
    let signed_in = req.bearer().is_some_and(|t| svc.sessions.valid(t, mono_ms));
    // Public.
    match (method, rest) {
        ("GET", ["version"]) => return Response::ok(build()),
        ("GET", ["status"]) => {
            return Response::ok(json!({
                "setup_needed": svc.verifier.is_none(),
                "signed_in": signed_in,
                "secure": req.secure,
                "https": ctx.tls.as_ref().map(|t| &t.info),
                "clock": now.map(iso),
                "build": build(),
            }))
        }
        ("POST", ["setup"]) => {
            if svc.verifier.is_some() {
                return Response::error(409, "The admin password is already set; sign in");
            }
            let body = match req.json() {
                Ok(b) => b,
                Err(r) => return r,
            };
            return match svc.set_password(str_field(&body, "password")) {
                Ok(()) => {
                    svc.activity.add(now, None, "info", "Admin password set");
                    Response::ok(json!({ "token": svc.sessions.start(mono_ms) }))
                }
                Err(e) => Response::error(422, &e),
            };
        }
        ("POST", ["login"]) => {
            let body = match req.json() {
                Ok(b) => b,
                Err(r) => return r,
            };
            if svc.sessions.locked(mono_ms) {
                return Response::error(429, "Too many wrong passwords; wait a minute")
                    .with_header("Retry-After", "60");
            }
            let ok = svc
                .verifier
                .as_ref()
                .is_some_and(|v| v.check(str_field(&body, "password")));
            if !ok {
                svc.sessions.failed(mono_ms);
                return Response::error(401, "Wrong password");
            }
            return Response::ok(json!({ "token": svc.sessions.start(mono_ms) }));
        }
        _ => {}
    }
    if !signed_in {
        return Response::error(401, "Sign in first").with_header("WWW-Authenticate", "Bearer");
    }
    let body = match req.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let result: Result<Response, Response> = (|| match (method, rest) {
        ("POST", ["logout"]) => {
            if let Some(t) = req.bearer() {
                svc.sessions.end(t);
            }
            Ok(Response::ok(json!({})))
        }
        ("POST", ["password"]) => {
            let current_ok = svc
                .verifier
                .as_ref()
                .is_some_and(|v| v.check(str_field(&body, "current")));
            if !current_ok {
                return Err(Response::error(403, "The current password is wrong"));
            }
            svc.set_password(str_field(&body, "new"))
                .map_err(|e| Response::error(422, &e))?;
            svc.activity.add(
                now,
                None,
                "info",
                "Admin password changed; every session signed out",
            );
            Ok(Response::ok(json!({})))
        }
        ("GET", ["bots"]) => Ok(Response::ok(Value::Array(
            (0..svc.infos.len())
                .map(|i| bot_json(svc, i, now))
                .collect(),
        ))),
        ("GET", ["bots", id]) => Ok(Response::ok(bot_json(svc, find(svc, id)?, now))),
        ("PUT" | "PATCH", ["bots", id]) => {
            let i = find(svc, id)?;
            let update: ConfigUpdate = serde_json::from_value(body.clone())
                .map_err(|e| Response::error(400, &format!("Bad settings: {e}")))?;
            svc.configure(i, update, now)
                .map_err(|e| Response::error(422, &e))?;
            Ok(Response::ok(bot_json(svc, i, now)))
        }
        ("POST", ["bots", id, action @ ("run" | "check")]) => {
            let i = find(svc, id)?;
            let kind = if *action == "run" {
                JobKind::Manual
            } else {
                JobKind::Check
            };
            if now.is_none() {
                return Err(Response::error(
                    503,
                    "The clock is not set yet; try again shortly",
                ));
            }
            svc.request(i, kind, now)
                .map_err(|e| Response::error(422, &e))?;
            Ok(Response::json(202, &bot_json(svc, i, now)))
        }
        ("GET", ["activity"]) => {
            let since = req
                .query_param("since")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let limit = req
                .query_param("limit")
                .and_then(|s| s.parse().ok())
                .unwrap_or(50usize)
                .min(200);
            Ok(Response::ok(json!(svc.activity.recent(since, limit))))
        }
        ("GET", ["integrations", "openrouter"]) => Ok(Response::ok(openrouter_json(svc))),
        ("PUT" | "PATCH", ["integrations", "openrouter"]) => {
            let update: OpenRouterUpdate = serde_json::from_value(body.clone())
                .map_err(|e| Response::error(400, &format!("Bad settings: {e}")))?;
            svc.set_openrouter(update)
                .map_err(|e| Response::error(422, &e))?;
            Ok(Response::ok(openrouter_json(svc)))
        }
        ("GET", ["diag"]) => Ok(Response::ok(json!({
            "platform": (ctx.platform)(),
            "clock": now.map(iso),
            "build": build(),
        }))),
        _ => Err(Response::error(404, "Not found")),
    })();
    result.unwrap_or_else(|r| r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemStore;

    const NOW: u64 = 1_790_000_000_000;

    fn server() -> (Service<MemStore>, Ctx) {
        let svc = Service::open(MemStore::default(), crate::bots::all()).unwrap();
        let ctx = Ctx {
            platform: || Platform {
                target: "test",
                ..Default::default()
            },
            tls: None,
        };
        (svc, ctx)
    }

    fn call(s: &mut (Service<MemStore>, Ctx), req: Request) -> Response {
        handle(&mut s.0, &s.1, &req, Some(NOW), 1000)
    }

    fn authed(method: &str, url: &str, token: &str) -> Request {
        Request::new(method, url).with_header("Authorization", &format!("Bearer {token}"))
    }

    #[test]
    fn first_visit_sets_the_password_then_sign_in() {
        let mut s = server();
        let status = call(&mut s, Request::new("GET", "/api/v1/status")).json_body();
        assert_eq!(status["setup_needed"], true);
        let r = call(
            &mut s,
            Request::new("POST", "/api/v1/setup").with_json(&json!({"password": "short"})),
        );
        assert_eq!(r.status, 422);
        let r = call(
            &mut s,
            Request::new("POST", "/api/v1/setup").with_json(&json!({"password": "long enough"})),
        );
        let token = r.json_body()["token"].as_str().unwrap().to_string();
        assert_eq!(
            call(&mut s, authed("GET", "/api/v1/bots", &token)).status,
            200
        );
        // Setup happens once.
        let r = call(
            &mut s,
            Request::new("POST", "/api/v1/setup").with_json(&json!({"password": "hijacker!"})),
        );
        assert_eq!(r.status, 409);
        assert_eq!(
            call(&mut s, Request::new("GET", "/api/v1/bots")).status,
            401
        );
        let r = call(
            &mut s,
            Request::new("POST", "/api/v1/login").with_json(&json!({"password": "wrong one"})),
        );
        assert_eq!(r.status, 401);
        let r = call(
            &mut s,
            Request::new("POST", "/api/v1/login").with_json(&json!({"password": "long enough"})),
        );
        assert_eq!(r.status, 200);
        call(&mut s, authed("POST", "/api/v1/logout", &token));
        assert_eq!(
            call(&mut s, authed("GET", "/api/v1/bots", &token)).status,
            401
        );
    }

    #[test]
    fn configure_a_bot_without_ever_seeing_its_key_again() {
        let mut s = server();
        let r = call(
            &mut s,
            Request::new("POST", "/api/v1/setup").with_json(&json!({"password": "long enough"})),
        );
        let token = r.json_body()["token"].as_str().unwrap().to_string();
        let bots = call(&mut s, authed("GET", "/api/v1/bots", &token)).json_body();
        assert_eq!(bots[0]["id"], "good_morning");
        assert_eq!(bots[0]["schedule"], "Every day at 07:30 US Eastern");
        assert_eq!(bots[0]["instance"], "https://mastomini.local");
        let r = call(
            &mut s,
            authed("PUT", "/api/v1/bots/good_morning", &token)
                .with_json(&json!({"token": "SECRET-KEY", "enabled": true})),
        );
        assert_eq!(r.status, 200, "{:?}", r.json_body());
        let text = String::from_utf8_lossy(&r.body).to_string();
        assert!(!text.contains("SECRET-KEY"));
        assert_eq!(r.json_body()["key_set"], true);
        assert!(r.json_body()["next_run_at"].is_string());
        let r = call(
            &mut s,
            authed("PUT", "/api/v1/bots/good_morning", &token)
                .with_json(&json!({"settings": {"time": "06:45", "zone": "us_pacific"}})),
        );
        assert_eq!(r.status, 200, "{:?}", r.json_body());
        assert_eq!(r.json_body()["schedule"], "Every day at 06:45 US Pacific");
        let settings = r.json_body()["settings"].clone();
        let time = settings
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["key"] == "time")
            .unwrap();
        assert_eq!(time["value"], "06:45");
        assert_eq!(time["kind"], "time");
        let r = call(
            &mut s,
            authed("PUT", "/api/v1/bots/good_morning", &token)
                .with_json(&json!({"settings": {"time": "7 o'clock"}})),
        );
        assert_eq!(r.status, 422);
        let r = call(
            &mut s,
            authed("POST", "/api/v1/bots/good_morning/run", &token),
        );
        assert_eq!(r.status, 202);
        assert_eq!(r.json_body()["queued"], true);
        let activity = call(&mut s, authed("GET", "/api/v1/activity?limit=2", &token)).json_body();
        assert_eq!(activity[0]["text"], "Run requested");
        assert_eq!(
            call(&mut s, authed("GET", "/api/v1/bots/nope", &token)).status,
            404
        );
        let r = call(
            &mut s,
            authed("PUT", "/api/v1/bots/good_morning", &token)
                .with_json(&json!({"instance": "ftp://x"})),
        );
        assert_eq!(r.status, 422);
    }

    #[test]
    fn the_openrouter_key_goes_in_and_never_out() {
        let mut s = server();
        let r = call(
            &mut s,
            Request::new("POST", "/api/v1/setup").with_json(&json!({"password": "long enough"})),
        );
        let token = r.json_body()["token"].as_str().unwrap().to_string();
        let path = "/api/v1/integrations/openrouter";
        assert_eq!(call(&mut s, Request::new("GET", path)).status, 401);
        let v = call(&mut s, authed("GET", path, &token)).json_body();
        assert_eq!(v["key_set"], false);
        assert_eq!(v["default_model"], "google/gemma-4-31b-it");
        assert_eq!(v["used_by"], json!(["llm_reply", "llm_post"]));
        let r = call(
            &mut s,
            authed("PUT", path, &token)
                .with_json(&json!({"key": "sk-or-secret", "model": "google/gemma-4-31b-it"})),
        );
        assert_eq!(r.status, 200);
        assert!(!String::from_utf8_lossy(&r.body).contains("sk-or-secret"));
        assert_eq!(r.json_body()["key_set"], true);
        assert_eq!(s.0.openrouter.key, "sk-or-secret");
        let r = call(
            &mut s,
            authed("PUT", path, &token).with_json(&json!({"model": "has space"})),
        );
        assert_eq!(r.status, 422);
        // The LLM bot's settings form: templates with their variables.
        let bots = call(&mut s, authed("GET", "/api/v1/bots", &token)).json_body();
        let llm = bots
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["id"] == "llm_reply")
            .unwrap();
        assert_eq!(llm["uses_llm"], true);
        assert_eq!(llm["schedule"], "Every 5 minutes");
        let prompt = llm["settings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["key"] == "reply_prompt")
            .unwrap();
        assert_eq!(prompt["kind"], "long_text");
        assert!(prompt["placeholders"]
            .as_array()
            .unwrap()
            .contains(&json!("conversation")));
    }

    #[test]
    fn public_pages() {
        let mut s = server();
        assert_eq!(
            call(&mut s, Request::new("GET", "/")).header("Location"),
            Some("/app/")
        );
        assert_eq!(call(&mut s, Request::new("GET", "/ca")).status, 404);
        let v = call(&mut s, Request::new("GET", "/api/v1/version")).json_body();
        assert_eq!(v["name"], "mastomini-bots");
        assert_eq!(call(&mut s, Request::new("GET", "/etc/passwd")).status, 404);
    }
}
