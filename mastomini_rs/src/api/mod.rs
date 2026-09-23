//! Mastodon REST API and the household admin API, over [`http`](crate::http)
//! types so the same code serves the desktop build and the board.

mod about;
mod accounts;
mod collections;
pub mod entities;
mod household;
pub mod html;
mod instance;
mod misc;
mod moderation;
mod oauth;
mod setup;
mod statuses;
pub mod time;
mod timelines;

use crate::domain::query::PageQuery;
use crate::domain::{Error, Principal, Service};
use crate::http::{self, Params, Request, Response};
use crate::store::Store;

/// Where the server is reachable, for URLs in responses.
#[derive(Debug, Clone)]
pub struct Ctx {
    /// `https://mastomini.local` or `http://127.0.0.1:8080`, no trailing slash.
    pub base_url: String,
}

impl Ctx {
    pub fn new(base_url: &str) -> Ctx {
        Ctx {
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Host (and port), as used in `acct` URIs and `@user@host` mentions.
    pub fn host(&self) -> &str {
        self.base_url
            .split_once("://")
            .map_or(self.base_url.as_str(), |(_, h)| h)
    }
}

/// One request being handled.
pub(crate) struct Call<'a, S: Store> {
    pub svc: &'a mut Service<S>,
    pub ctx: &'a Ctx,
    pub req: &'a Request,
    pub params: Params,
    pub now: u64,
    pub principal: Option<Principal>,
}

pub(crate) type Reply = Result<Response, Response>;

pub(crate) fn fail(e: Error) -> Response {
    match e {
        Error::NotFound => Response::error(404, "Record not found"),
        Error::Unauthorized => Response::error(401, "The access token is invalid")
            .with_header("WWW-Authenticate", "Bearer error=\"invalid_token\""),
        Error::Forbidden(m) => Response::error(403, &m),
        Error::Invalid(m) => Response::error(422, &m),
        Error::TooMany(m) => Response::error(429, &m).with_header("Retry-After", "300"),
        Error::Unavailable(m) => Response::error(503, &m).with_header("Retry-After", "60"),
    }
}

impl<S: Store> Call<'_, S> {
    /// The signed-in user's slot, with a scope check: reads need `read`,
    /// everything else `write`.
    pub fn user(&self) -> Result<u8, Response> {
        let needed = if self.req.method == "GET" {
            "read"
        } else {
            "write"
        };
        self.user_scoped(needed)
    }

    pub fn user_scoped(&self, needed: &str) -> Result<u8, Response> {
        let principal = self
            .principal
            .as_ref()
            .ok_or_else(|| fail(Error::Unauthorized))?;
        let slot = principal
            .slot
            .ok_or_else(|| Response::error(422, "This method requires an authenticated user"))?;
        if principal.disabled {
            return Err(Response::error(403, "Your login is currently disabled"));
        }
        if !principal.allows(needed) && !(needed == "read" && principal.allows("profile")) {
            return Err(Response::error(
                403,
                "This action is outside the authorized scopes",
            ));
        }
        Ok(slot)
    }

    pub fn id(&self, text: &str) -> Result<u64, Response> {
        crate::ids::parse(text).ok_or_else(|| fail(Error::NotFound))
    }

    pub fn page_query(&self) -> PageQuery {
        let p = &self.params;
        PageQuery {
            max_id: p.u64("max_id"),
            since_id: p.u64("since_id"),
            min_id: p.u64("min_id"),
            limit: p.u64("limit").unwrap_or(0) as usize,
        }
    }

    /// Mastodon's `Link` header for a page with the given first/last ids.
    pub fn link(&self, ids: &[u64], limit: usize) -> Option<String> {
        let (first, last) = (ids.first()?, ids.last()?);
        let base = format!("{}{}", self.ctx.base_url, self.req.path);
        let keep: Vec<String> = self
            .req
            .query
            .split('&')
            .filter(|kv| {
                let k = kv.split('=').next().unwrap_or("");
                !kv.is_empty() && !matches!(k, "max_id" | "min_id" | "since_id" | "limit")
            })
            .map(str::to_string)
            .collect();
        let extra = if keep.is_empty() {
            String::new()
        } else {
            format!("&{}", keep.join("&"))
        };
        let mut links = Vec::new();
        if ids.len() >= limit {
            links.push(format!(
                "<{base}?max_id={last}&limit={limit}{extra}>; rel=\"next\""
            ));
        }
        links.push(format!(
            "<{base}?min_id={first}&limit={limit}{extra}>; rel=\"prev\""
        ));
        Some(links.join(", "))
    }

    pub fn paged(&self, body: serde_json::Value, ids: &[u64], limit: usize) -> Response {
        let response = Response::ok(body);
        match self.link(ids, limit) {
            Some(link) => response.with_header("Link", &link),
            None => response,
        }
    }
}

fn cors(path: &str) -> bool {
    path.starts_with("/api/")
        || path.starts_with("/oauth/token")
        || path.starts_with("/oauth/revoke")
        || path.starts_with("/.well-known/")
        || path.starts_with("/nodeinfo/")
}

/// Handle one request. `now_ms` is `None` while the clock is not yet valid:
/// reads still work, writes return 503 (spec/05 "Time").
pub fn handle<S: Store>(
    svc: &mut Service<S>,
    ctx: &Ctx,
    req: &Request,
    now_ms: Option<u64>,
) -> Response {
    let mut response = if req.method == "OPTIONS" && cors(&req.path) {
        Response::new(200, "text/plain", Vec::new())
            .with_header(
                "Access-Control-Allow-Methods",
                "GET, POST, PUT, PATCH, DELETE, OPTIONS",
            )
            .with_header(
                "Access-Control-Allow-Headers",
                "Authorization, Content-Type, Idempotency-Key",
            )
            .with_header("Access-Control-Max-Age", "86400")
    } else {
        let response = dispatch(svc, ctx, req, now_ms);
        // The caller's direct-message key never outlives the request.
        svc.close_session();
        response
    };
    if cors(&req.path) {
        response = response
            .with_header("Access-Control-Allow-Origin", "*")
            .with_header("Access-Control-Expose-Headers", "Link, X-RateLimit-Reset");
    }
    response
}

fn dispatch<S: Store>(
    svc: &mut Service<S>,
    ctx: &Ctx,
    req: &Request,
    now_ms: Option<u64>,
) -> Response {
    let reading = matches!(req.method.as_str(), "GET" | "HEAD");
    let now = match now_ms {
        Some(now) => now,
        None if reading => svc.last_id() >> 16,
        None if req.path.starts_with("/setup") => return setup::clock_not_set(),
        None => {
            return fail(Error::Unavailable(
                "The server clock is not set yet; try again shortly".into(),
            ))
        }
    };
    let limit = if req.path == "/api/v1/accounts/update_credentials" {
        http::UPLOAD_BODY_LIMIT
    } else {
        http::BODY_LIMIT
    };
    if req.body.len() > limit {
        return Response::error(413, "Request body is too large");
    }
    svc.tick(now);
    let params = match Params::parse(req) {
        Ok(p) => p,
        Err(e) => return Response::error(400, &e),
    };
    let principal = req.bearer().and_then(|t| svc.principal(t, now));
    if let (Some(slot), Some(token)) = (principal.as_ref().and_then(|p| p.slot), req.bearer()) {
        svc.open_session(slot, token);
    }
    let mut call = Call {
        svc,
        ctx,
        req,
        params,
        now,
        principal,
    };
    let segments = req.segments();
    let result = route(&mut call, &segments);
    match result {
        Some(Ok(r)) | Some(Err(r)) => r,
        None => Response::error(404, "Record not found"),
    }
}

fn route<S: Store>(c: &mut Call<'_, S>, seg: &[&str]) -> Option<Reply> {
    let method = c.req.method.as_str();
    Some(match (method, seg) {
        (
            _,
            [".well-known", ..]
            | ["nodeinfo", ..]
            | ["api", "v1", "instance", ..]
            | ["api", "v2", "instance"],
        )
        | (_, ["api", "v1", "custom_emojis"]) => return instance::route(c, method, seg),
        (_, ["oauth", ..]) | (_, ["api", "v1", "apps", ..]) => return oauth::route(c, method, seg),
        (_, [] | ["setup", ..]) => return setup::route(c, method, seg),
        (_, ["about" | "terms-of-service" | "terms" | "privacy-policy"]) => {
            return about::route(c, method, seg)
        }
        (_, ["api", "v1", "accounts", _, "collections" | "in_collections"])
        | (_, ["api", "v1", "collections", ..]) => return collections::route(c, method, seg),
        (_, ["api", "v1", "accounts", ..]) => return accounts::route(c, method, seg),
        (_, ["api", "v1" | "v2", "admin", ..])
        | (_, ["api", "v1", "blocks" | "mutes" | "reports"]) => {
            return moderation::route(c, method, seg)
        }
        (_, ["api", "v1", "statuses", ..]) => return statuses::route(c, method, seg),
        (_, ["api", "mastomini", "v1", ..]) => return household::route(c, method, &seg[3..]),
        (
            "GET",
            ["avatars", "original", "missing.png"] | ["headers", "original", "missing.png"],
        ) => Ok(Response::new(200, "image/png", MISSING_PNG)
            .with_header("Cache-Control", "public, max-age=604800")),
        _ => return timelines::route(c, method, seg).or_else(|| misc::route(c, method, seg)),
    })
}

/// The default avatar/header: a 48×48 grey PNG.
static MISSING_PNG: &[u8] = include_bytes!("../../assets/missing.png");

#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;
