//! Shared miniframework adapter. The app retains its API, auth, static site
//! and storage; the framework owns socket I/O and machine diagnostics.
//!
//! Network updates (`GET`/`POST /api/v1/ota`) are answered here, before the
//! app's body limit: an update streams through without being buffered. A
//! bundle installs only if the household's update key signed it, and only
//! for an admin session or the build machine's deploy key.
use crate::{
    api::{self, Ctx},
    http::{Request, Response, BODY_LIMIT},
    service::Service,
    store::KvStore,
};
use miniframework::{Config, Reply};
use std::sync::{Arc, Mutex};

pub struct Bots<S: KvStore> {
    pub shared: Arc<Mutex<Service<S>>>,
    pub ctx: Arc<Ctx>,
    now_ms: fn() -> Option<u64>,
    mono_ms: fn() -> u64,
    error: fn(),
    updates: Option<Updates>,
}

/// The board's network updates and who may start one.
pub struct Updates {
    pub updater: miniframework::ota::Updater,
    /// The build machine's key for `make ota` (Bearer); an admin session
    /// works too.
    pub deploy_key: Option<&'static str>,
}

/// An update: a 4 MiB slot plus the bundle header.
const OTA_BODY: usize = 4 * 1024 * 1024 + 8192;

/// Constant-time comparison (no early exit on the first difference).
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl<S: KvStore> Bots<S> {
    pub fn new(
        shared: Arc<Mutex<Service<S>>>,
        ctx: Arc<Ctx>,
        now_ms: fn() -> Option<u64>,
        mono_ms: fn() -> u64,
    ) -> Self {
        Self {
            shared,
            ctx,
            now_ms,
            mono_ms,
            error: || {},
            updates: None,
        }
    }

    pub fn with_updates(mut self, updates: Updates) -> Self {
        self.updates = Some(updates);
        self
    }

    /// `GET`/`POST /api/v1/ota`; `None` for every other request.
    fn ota(&self, req: &miniframework::Request<'_>) -> Option<Response> {
        if req.path != "/api/v1/ota" {
            return None;
        }
        let Some(u) = &self.updates else {
            return Some(Response::error(404, "This build has no network updates"));
        };
        match req.method {
            "GET" => Some(Response::new(
                200,
                "application/json",
                u.updater.status().to_json().into_bytes(),
            )),
            "POST" => {
                let bearer = req
                    .header("Authorization")
                    .strip_prefix("Bearer ")
                    .unwrap_or("")
                    .trim();
                let allowed = !bearer.is_empty()
                    && (u
                        .deploy_key
                        .is_some_and(|k| same(k.as_bytes(), bearer.as_bytes()))
                        || self
                            .shared
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .sessions
                            .valid(bearer, (self.mono_ms)()));
                if !allowed {
                    return Some(Response::error(
                        401,
                        "Sign in as the admin to install an update",
                    ));
                }
                // The lock is released: the bots keep running while it streams.
                Some(match u.updater.install(&mut req.body_reader()) {
                    Ok(m) => {
                        (u.updater.installed)();
                        Response::new(
                            200,
                            "application/json",
                            format!("{{\"version\":{:?},\"restarting\":true}}", m.version)
                                .into_bytes(),
                        )
                    }
                    Err(e) => Response::error(422, &e.to_string()),
                })
            }
            _ => Some(Response::error(405, "GET or POST")),
        }
    }

    pub fn on_error(mut self, error: fn()) -> Self {
        self.error = error;
        self
    }
}

impl<S: KvStore + 'static> miniframework::Service for Bots<S> {
    fn streamed_body(&self, method: &str, path: &str) -> Option<usize> {
        (method == "POST" && path == "/api/v1/ota" && self.updates.is_some()).then_some(OTA_BODY)
    }

    fn handle(&self, req: &miniframework::Request<'_>, reply: &mut Reply<'_>) {
        let response = if let Some(r) = self.ota(req) {
            r
        } else if req.length > BODY_LIMIT {
            Response::error(413, "Request body is too large")
        } else if req.length != req.body.len() {
            Response::error(400, "Request body ended early")
        } else {
            let request = Request {
                method: req.method.into(),
                path: req.path.into(),
                query: req.query.into(),
                headers: req.headers().to_vec(),
                body: req.body.to_vec(),
                secure: req.secure,
            };
            let mut service = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            api::handle(
                &mut service,
                &self.ctx,
                &request,
                (self.now_ms)(),
                (self.mono_ms)(),
            )
        };
        if response.status >= 500 {
            (self.error)();
        }
        let content_type = response
            .header("Content-Type")
            .unwrap_or("application/octet-stream")
            .to_string();
        reply.owned(response.status, content_type, response.body);
        for (name, value) in response.headers {
            if !name.eq_ignore_ascii_case("Content-Type") {
                reply.header(name, value);
            }
        }
    }
}

pub fn config(host: &str, target: &str) -> Config {
    let mut config = Config::new("mastomini-bots", host);
    config.version = env!("CARGO_PKG_VERSION");
    config.build = env!("MASTOBOTS_BUILT_MS");
    // Oct 19: 14,183 B current free, ~5.5 KiB low-water, 6,912 B
    // largest block, no failed allocations. Provisional tight-RAM floors.
    config.ops.heap_warn = 8 * 1024;
    config.ops.heap_fail = 4 * 1024;
    config.ops.largest_warn = 4 * 1024;
    config.app_paths = &["/"];
    config.body_limit = BODY_LIMIT;
    config.response_limit = 64 * 1024;
    config.influx_tags = vec![("target", target.into())];
    config
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
