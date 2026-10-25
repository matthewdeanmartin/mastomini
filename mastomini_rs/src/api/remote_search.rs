//! Asking other servers, for searches and for signing in to the search
//! server. Handlers never wait on the network: the server adapter asks
//! [`pending`] what to send, sends it without the service lock, hands the
//! answer to [`finish`], and repeats (at most [`MAX_STEPS`] times) before
//! the request is handled from what is now known.
//!
//! Signing in is OAuth with PKCE against the search server, read-only
//! ([`SEARCH_SCOPES`]): the board registers itself as an app there, an
//! admin's browser logs in on the server's own page, and the server sends
//! the browser back to [`CALLBACK`] with a code the board exchanges for a
//! token. The token is never shown; everyone's searches use it.

use super::remote::html_to_text;
use super::{fail, Call, Ctx, Reply};
use crate::domain::remote::{
    normalize_acct, valid_host, Answer, IncomingPost, RemoteProfile, SEARCH_SCOPES,
};
use crate::domain::Service;
use crate::http::{encode, Request, Response};
use crate::store::Store;
use serde_json::{json, Value};

/// Answers read from another server, at most.
/// A signed-in search answers with whole posts: about 5 KiB each.
pub const LOOKUP_LIMIT: usize = 256 * 1024;
/// Fetches one request may make: register, exchange, verify.
pub const MAX_STEPS: usize = 3;
pub const CALLBACK: &str = "/api/mastomini/v1/remote/oauth/callback";
/// Per kind. Fewer is faster: the board waits for the answer.
const SEARCH_RESULTS: usize = 5;
const TAG_POSTS: usize = 20;

/// One request to another server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outbound {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    /// `/api/v1/accounts/lookup` on the account's own server.
    Account {
        acct: String,
    },
    /// A search, or a hashtag's timeline, on the search server.
    Search {
        key: String,
        server: String,
    },
    Register {
        server: String,
    },
    Token {
        state: String,
    },
    Verify {
        state: String,
        server: String,
    },
}

impl Outbound {
    fn get(url: String, token: Option<&str>, kind: Kind) -> Outbound {
        let mut headers = vec![("Accept".to_string(), "application/json".to_string())];
        if let Some(token) = token {
            headers.push(("Authorization".into(), format!("Bearer {token}")));
        }
        Outbound {
            method: "GET",
            url,
            headers,
            body: Vec::new(),
            kind,
        }
    }

    fn form(url: String, fields: &[(&str, &str)], kind: Kind) -> Outbound {
        let body = fields
            .iter()
            .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        Outbound {
            method: "POST",
            url,
            headers: vec![
                ("Accept".into(), "application/json".into()),
                (
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                ),
            ],
            body: body.into_bytes(),
            kind,
        }
    }
}

pub fn search_key(needle: &str) -> String {
    format!("q:{needle}")
}

pub fn tag_key(tag: &str) -> String {
    format!("tag:{tag}")
}

/// `#tag` as a hashtag name, if it is one.
pub fn hashtag(needle: &str) -> Option<&str> {
    let tag = needle.strip_prefix('#')?;
    (!tag.is_empty() && tag.len() <= 100 && tag.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .then_some(tag)
}

fn redirect_uri(ctx: &Ctx) -> String {
    format!("{}{CALLBACK}", ctx.base_url)
}

/// What to send before this request can be answered, if anything.
pub fn pending<S: Store>(
    svc: &mut Service<S>,
    ctx: &Ctx,
    req: &Request,
    now: u64,
) -> Option<Outbound> {
    match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/api/mastomini/v1/admin/search_login") => register(svc, ctx, req, now),
        ("GET", CALLBACK) => sign_in_step(svc, ctx, req, now),
        ("GET", _) => search(svc, req, now),
        _ => None,
    }
}

/// A search that names another server's account, or that the search
/// server should answer. Only for a signed-in member, at most
/// [`crate::domain::remote::SEARCHES_PER_HOUR`] an hour, never when the
/// answer is already known.
fn search<S: Store>(svc: &mut Service<S>, req: &Request, now: u64) -> Option<Outbound> {
    if !svc.remote.settings.enabled {
        return None;
    }
    let params = crate::http::Params::parse(req).ok()?;
    let resolve = params.flag("resolve");
    let (q, by_name, posts) = match req.path.as_str() {
        // A submitted search: apps rarely send `resolve` for a name
        // (Mastodon searches the accounts it knows without it), so names go
        // to the search server either way.
        "/api/v2/search" => {
            let kind = params.get("type").unwrap_or("");
            if params.u64("offset").unwrap_or(0) > 0 || kind == "hashtags" {
                return None;
            }
            (params.get("q")?, kind != "statuses", kind != "accounts")
        }
        // Mention autocomplete asks on every keystroke: only when the app
        // explicitly asks to resolve.
        "/api/v1/accounts/search" => (params.get("q")?, resolve, false),
        "/api/v1/accounts/lookup" => (params.get("acct")?, false, false),
        _ => return None,
    };
    let needle = q.trim().to_lowercase();
    if needle.chars().count() < 2 || needle.len() > 100 {
        return None;
    }
    let slot = svc.principal(req.bearer()?, now)?.slot?;
    let server = svc.remote.settings.search_server.clone();
    let token = svc.search_token().map(str::to_string);
    let out = if let Some(tag) = hashtag(&needle) {
        // Posts with a hashtag: the tag's public timeline. Some servers
        // want an account for it; the sign-in provides one.
        let key = tag_key(tag);
        if !posts || server.is_empty() || svc.remote_search_cached(&key, now).is_some() {
            return None;
        }
        Outbound::get(
            format!(
                "https://{server}/api/v1/timelines/tag/{}?limit={TAG_POSTS}",
                encode(tag)
            ),
            token.as_deref(),
            Kind::Search { key, server },
        )
    } else if needle.trim_start_matches('@').contains('@') {
        let acct = normalize_acct(&needle).ok()?;
        if acct.split_once('@').map(|(_, h)| h) == Some(svc.config.host.as_str())
            || svc.remote_ref_by_acct(&acct).is_some()
            || svc.remote_search_cached(&acct, now).is_some()
        {
            return None;
        }
        let (user, host) = acct.split_once('@').expect("normalized");
        Outbound::get(
            format!(
                "https://{host}/api/v1/accounts/lookup?acct={}",
                encode(user)
            ),
            None,
            Kind::Account { acct },
        )
    } else {
        let needle = needle.trim_start_matches('@');
        let key = search_key(needle);
        // Without a sign-in a server only searches account names.
        if server.is_empty()
            || !(by_name || (posts && token.is_some()))
            || svc.remote_search_cached(&key, now).is_some()
        {
            return None;
        }
        let url = match &token {
            // Signed in: accounts and posts in one request. (Handles go to
            // their own server above; resolving a plain word only slows it.)
            Some(_) => format!(
                "https://{server}/api/v2/search?limit={SEARCH_RESULTS}&q={}",
                encode(needle)
            ),
            None => format!(
                "https://{server}/api/v2/search?type=accounts&limit={SEARCH_RESULTS}&q={}",
                encode(needle)
            ),
        };
        Outbound::get(url, token.as_deref(), Kind::Search { key, server })
    };
    svc.remote_search_allowed(slot, now).then_some(out)
}

/// An admin starting a sign-in: register this board as an app on the
/// server first, once.
fn register<S: Store>(
    svc: &mut Service<S>,
    ctx: &Ctx,
    req: &Request,
    now: u64,
) -> Option<Outbound> {
    let principal = svc.principal(req.bearer()?, now)?;
    let slot = principal.slot?;
    if !principal.allows("admin:write") || svc.require_admin(slot).is_err() {
        return None;
    }
    let params = crate::http::Params::parse(req).ok()?;
    let server = params.get("server")?.trim().to_ascii_lowercase();
    if !valid_host(&server)
        || svc.search_app(&server).is_some()
        || svc.registration_failed(&server).is_some()
    {
        return None;
    }
    // The callback is on this board's configured address, as at sign-in.
    let callback = redirect_uri(ctx);
    Some(Outbound::form(
        format!("https://{server}/api/v1/apps"),
        &[
            ("client_name", "mastomini household search"),
            ("redirect_uris", &callback),
            ("scopes", SEARCH_SCOPES),
        ],
        Kind::Register { server },
    ))
}

/// The server sent the admin's browser back: exchange the code, then ask
/// who signed in.
fn sign_in_step<S: Store>(
    svc: &mut Service<S>,
    ctx: &Ctx,
    req: &Request,
    now: u64,
) -> Option<Outbound> {
    let params = crate::http::Params::parse(req).ok()?;
    let state = params.get("state")?.to_string();
    let flow = svc.login_flow(&state, now)?;
    if flow.outcome.is_some() {
        return None;
    }
    let server = flow.server.clone();
    if let Some(token) = &flow.token {
        return Some(Outbound::get(
            format!("https://{server}/api/v1/accounts/verify_credentials"),
            Some(token),
            Kind::Verify { state, server },
        ));
    }
    let code = params.get("code")?;
    let redirect = redirect_uri(ctx);
    Some(Outbound::form(
        format!("https://{server}/oauth/token"),
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &redirect),
            ("client_id", &flow.client_id),
            ("client_secret", &flow.client_secret),
            ("code_verifier", &flow.verifier),
            ("scope", SEARCH_SCOPES),
        ],
        Kind::Token { state },
    ))
}

fn json_of(answer: &Result<(u16, Vec<u8>), String>) -> Option<Value> {
    match answer {
        Ok((200, body)) => serde_json::from_slice(body).ok(),
        _ => None,
    }
}

fn status_of(answer: &Result<(u16, Vec<u8>), String>) -> String {
    match answer {
        Ok((200, body)) => format!("its answer was not understood ({} bytes)", body.len()),
        Ok((status, _)) => format!("it answered {status}"),
        Err(_) => "it did not answer".into(),
    }
}

/// Keep what the other server answered (or that it did not).
pub fn finish<S: Store>(
    svc: &mut Service<S>,
    out: Outbound,
    answer: Result<(u16, Vec<u8>), String>,
    now: u64,
) {
    match out.kind {
        Kind::Account { acct } => {
            let found = match (&answer, json_of(&answer)) {
                (_, Some(v)) => Ok(Answer {
                    profiles: vec![profile(acct.clone(), &v)],
                    posts: Vec::new(),
                }),
                (Ok((404 | 410, _)), _) => Ok(Answer::default()),
                // Not every server answers Mastodon's lookup; the feed may
                // still work. The nightly fetch finds out.
                _ => Ok(Answer {
                    profiles: vec![RemoteProfile {
                        acct: acct.clone(),
                        display_name: String::new(),
                        note: String::new(),
                    }],
                    posts: Vec::new(),
                }),
            };
            svc.remote_search_store(acct, found, now);
        }
        Kind::Search { key, server } => {
            let found = match json_of(&answer) {
                Some(v) => Ok(parse_search(&server, &v)),
                None => {
                    log::info!("remote search: {}", status_of(&answer));
                    Err(format!("The search server: {}", status_of(&answer)))
                }
            };
            svc.remote_search_store(key, found, now);
        }
        Kind::Register { server } => {
            let app = json_of(&answer)
                .and_then(|v| {
                    Some((
                        v["client_id"].as_str()?.to_string(),
                        v["client_secret"].as_str()?.to_string(),
                    ))
                })
                .ok_or_else(|| status_of(&answer));
            svc.set_registration(server, app);
        }
        Kind::Token { state } => {
            let token = json_of(&answer)
                .and_then(|v| v["access_token"].as_str().map(str::to_string))
                .ok_or_else(|| format!("the code was refused ({})", status_of(&answer)));
            svc.login_token(&state, token);
        }
        Kind::Verify { state, server } => {
            let acct = json_of(&answer)
                .and_then(|v| v["acct"].as_str().map(str::to_string))
                .map(|a| {
                    if a.contains('@') {
                        a
                    } else {
                        format!("{a}@{server}")
                    }
                })
                .ok_or_else(|| {
                    format!(
                        "the server would not say who signed in ({})",
                        status_of(&answer)
                    )
                });
            svc.login_done(&state, acct, now);
        }
    }
}

fn profile(acct: String, v: &Value) -> RemoteProfile {
    RemoteProfile {
        acct,
        display_name: v["display_name"].as_str().unwrap_or("").to_string(),
        note: html_to_text(v["note"].as_str().unwrap_or("")),
    }
}

/// `name@server` for an account a server showed, its bare names being its own.
fn acct_of(server: &str, account: &Value) -> Option<String> {
    let acct = account["acct"].as_str()?;
    let acct = if acct.contains('@') {
        acct.to_string()
    } else {
        format!("{acct}@{server}")
    };
    normalize_acct(&acct).ok()
}

/// A search answer (`{accounts, statuses}`) or a timeline (`[status]`).
fn parse_search(server: &str, v: &Value) -> Answer {
    let profiles = v["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .take(SEARCH_RESULTS)
        .filter_map(|a| Some(profile(acct_of(server, a)?, a)))
        .collect();
    let statuses = v
        .as_array()
        .or_else(|| v["statuses"].as_array())
        .into_iter()
        .flatten()
        .take(TAG_POSTS);
    let posts = statuses
        .filter_map(|s| {
            // A boost shows the boosted post.
            let s = if s["reblog"].is_object() {
                &s["reblog"]
            } else {
                s
            };
            if !matches!(s["visibility"].as_str(), Some("public" | "unlisted")) {
                return None;
            }
            let author = profile(acct_of(server, &s["account"])?, &s["account"]);
            let url = s["url"].as_str().or_else(|| s["uri"].as_str())?.to_string();
            Some((
                author,
                IncomingPost {
                    url,
                    text: html_to_text(s["content"].as_str().unwrap_or("")),
                    cw: s["spoiler_text"].as_str().unwrap_or("").to_string(),
                    sensitive: s["sensitive"].as_bool().unwrap_or(false),
                    published_ms: s["created_at"]
                        .as_str()
                        .and_then(super::time::parse_iso)
                        .unwrap_or(0),
                    images: s["media_attachments"]
                        .as_array()
                        .map_or(0, |m| m.len() as u16),
                },
            ))
        })
        .collect();
    Answer { profiles, posts }
}

// Routes.

/// `POST /api/mastomini/v1/admin/search_login` `{server}`: where to send
/// the admin's browser.
fn start<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let actor = c.user_scoped("admin:write")?;
    let server = c
        .params
        .get("server")
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let redirect = redirect_uri(c.ctx);
    let url = c
        .svc
        .start_search_login(actor, &server, &redirect, c.now)
        .map_err(fail)?;
    Ok(Response::ok(json!({ "authorize_url": url })))
}

/// The search server sends the admin's browser here; it goes on to the
/// admin page with the outcome.
fn callback<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let state = c.params.get("state").unwrap_or("");
    let page = "/app/#/admin/remote";
    let outcome = if let Some(denied) = c.params.get("error") {
        if c.svc.login_flow(state, c.now).is_some() {
            c.svc.remote.login = None;
        }
        Err(format!("the server said {denied}"))
    } else {
        c.svc
            .take_login(state)
            .unwrap_or_else(|| Err("this sign-in expired or was already used".into()))
    };
    Ok(Response::redirect(&match outcome {
        Ok(_) => format!("{page}?search=connected"),
        Err(why) => format!("{page}?search=failed&reason={}", encode(&why)),
    }))
}

fn sign_out<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let actor = c.user_scoped("admin:write")?;
    c.svc.search_sign_out(actor, c.now).map_err(fail)?;
    Ok(Response::ok(json!({})))
}

/// Routes under `/api/mastomini/v1/`; `seg` starts after `v1`.
pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, seg) {
        ("POST", ["admin", "search_login"]) => start(c),
        ("DELETE", ["admin", "search_login"]) => sign_out(c),
        ("GET", ["remote", "oauth", "callback"]) => callback(c),
        _ => return None,
    })
}
