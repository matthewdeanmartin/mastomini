//! Shared miniframework adapter. The app retains its API, auth, static site
//! and storage; the framework owns socket I/O and machine diagnostics.
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
        }
    }

    pub fn on_error(mut self, error: fn()) -> Self {
        self.error = error;
        self
    }
}

impl<S: KvStore + 'static> miniframework::Service for Bots<S> {
    fn handle(&self, req: &miniframework::Request<'_>, reply: &mut Reply<'_>) {
        let response = if req.length > BODY_LIMIT {
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
    config.app_paths = &["/"];
    config.body_limit = BODY_LIMIT;
    config.response_limit = 64 * 1024;
    config.influx_tags = vec![("target", target.into())];
    config
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
