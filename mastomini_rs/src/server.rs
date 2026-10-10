//! mastomini as a miniframework app. The framework owns the connection loop,
//! HTTP, HTTPS and HTTP/2, `/metrics`, `/api/v1/sys` and `/api/v1/log`; this
//! adapter hands every other request to mastomini's own router
//! ([`api::handle`]) under the service lock and sends its response as is.
//!
//! The same adapter runs on the desktop and on the board. The lock covers
//! the API work, never socket I/O: the framework writes the reply after
//! `handle` returns.
use crate::api::{self, Ctx};
use crate::domain::Service;
use crate::http::{self, Request, Response};
use crate::store::Store;
use miniframework::{Config, Cors, Reply};
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub type Shared<S> = Arc<Mutex<Service<S>>>;

/// The board's setup network, as the setup pages see it (the board
/// implements it on `miniframework::esp::WifiSetup`; the desktop has none).
pub trait Setup: Send + Sync {
    /// The setup network is open.
    fn active(&self) -> bool;
    /// The board has joined a home network from the setup network.
    fn joined(&self) -> bool;
    /// `GET`/`POST /setup/wifi`.
    fn page(&self, req: &Request, provisioned: bool) -> Response;
}

pub struct Household<S: Store> {
    pub shared: Shared<S>,
    pub ctx: Arc<Ctx>,
    /// Wall-clock milliseconds, `None` while the clock is not set.
    now_ms: fn() -> Option<u64>,
    setup: Option<Box<dyn Setup>>,
    /// Called once when a request leaves the store failed (read-only).
    storage_failed: fn(),
}

impl<S: Store> Household<S> {
    pub fn new(shared: Shared<S>, ctx: Arc<Ctx>, now_ms: fn() -> Option<u64>) -> Self {
        Self {
            shared,
            ctx,
            now_ms,
            setup: None,
            storage_failed: || {},
        }
    }

    pub fn with_setup(mut self, setup: Box<dyn Setup>) -> Self {
        self.setup = Some(setup);
        self
    }

    pub fn on_storage_failed(mut self, report: fn()) -> Self {
        self.storage_failed = report;
        self
    }
}

/// Log every request's timing, not just slow ones: built in on the board
/// (`MASTOMINI_TRACE_TIMING=1` at build time), at run time on the desktop.
fn trace_timing() -> bool {
    option_env!("MASTOMINI_TRACE_TIMING") == Some("1")
        || std::env::var_os("MASTOMINI_TRACE_TIMING").is_some()
}

/// The largest request body: profile uploads (`update_credentials`, with
/// avatar and header images) stream up to this; everything else is
/// refused by the framework above [`http::BODY_LIMIT`].
const UPLOAD_ROUTE: &str = "/api/v1/accounts/update_credentials";
const IMPORT_ROUTE: &str = "/api/mastomini/v1/import";

/// Routes whose body streams in above [`http::BODY_LIMIT`], and their limit:
/// profile uploads, and restoring a household export onto a new board.
fn streamed_limit(method: &str, path: &str) -> Option<usize> {
    if !matches!(method, "PATCH" | "POST" | "PUT") {
        return None;
    }
    match path {
        UPLOAD_ROUTE => Some(http::UPLOAD_BODY_LIMIT),
        IMPORT_ROUTE => Some(crate::domain::transfer::ARCHIVE_MAX),
        _ => None,
    }
}

impl<S: Store + Send + 'static> Household<S> {
    fn request(req: &miniframework::Request<'_>) -> Result<Request, u16> {
        let limit = streamed_limit(req.method, req.path).unwrap_or(http::BODY_LIMIT);
        if req.length > limit {
            return Err(413);
        }
        let mut body = Vec::new();
        if req.length > req.body.len() {
            // A streamed upload: read it whole (it is at most
            // its route's limit; the parsers need all of it).
            body.try_reserve_exact(req.length).map_err(|_| 400u16)?;
            req.body_reader()
                .take(req.length as u64)
                .read_to_end(&mut body)
                .map_err(|_| 400u16)?;
            if body.len() != req.length {
                return Err(400);
            }
        } else {
            body.extend_from_slice(req.body);
        }
        Ok(Request {
            method: req.method.to_string(),
            path: req.path.to_string(),
            query: req.query.to_string(),
            headers: req.headers().to_vec(),
            body,
            secure: req.secure,
        })
    }

    fn respond(&self, request: &Request) -> (Response, f64) {
        if let Some(setup) = &self.setup {
            if request.path == "/setup/wifi" {
                let provisioned = self.lock().state.provisioned;
                return (setup.page(request, provisioned), 0.0);
            }
            if setup.active() {
                if let Some(to) = crate::captive::captive(&request.path, setup.joined()) {
                    let to = format!("http://{}{to}", crate::captive::AP_IP);
                    return (Response::redirect(&to), 0.0);
                }
            }
        }
        let waiting = Instant::now();
        let mut service = self.lock();
        let lock_ms = waiting.elapsed().as_secs_f64() * 1000.0;
        let failed_before = service.latched().is_some();
        let response = api::handle(&mut service, &self.ctx, request, (self.now_ms)());
        if !failed_before && service.latched().is_some() {
            (self.storage_failed)();
        }
        (response, lock_ms)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Service<S>> {
        // A poisoned lock means a handler panicked mid-request; with
        // panic=abort on the board this only happens on the desktop.
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl<S: Store + Send + 'static> miniframework::Service for Household<S> {
    fn handle(&self, req: &miniframework::Request<'_>, reply: &mut Reply<'_>) {
        let started = Instant::now();
        let request = match Self::request(req) {
            Ok(request) => request,
            Err(status) => {
                reply.owned(
                    status,
                    "application/json",
                    br#"{"error":"The request body could not be read"}"#.to_vec(),
                );
                return;
            }
        };
        let body_ms = started.elapsed().as_secs_f64() * 1000.0;
        let (response, lock_ms) = self.respond(&request);
        let total = started.elapsed();
        if total.as_millis() >= 250 || trace_timing() {
            log::info!(
                "timing method={} route={} status={} bytes={} body_ms={body_ms:.3} lock_ms={lock_ms:.3} handler_ms={:.3}",
                request.method,
                http::route_label(&request.path),
                response.status,
                response.body.len(),
                total.as_secs_f64() * 1000.0
            );
        }
        let response =
            response.with_timing(&format!("body;dur={body_ms:.3}, lock;dur={lock_ms:.3}"));
        let content_type = response
            .header("Content-Type")
            .unwrap_or("application/octet-stream")
            .to_string();
        let Response {
            status,
            headers,
            body,
        } = response;
        reply.owned(status, content_type, body);
        for (name, value) in headers {
            if !name.eq_ignore_ascii_case("Content-Type") {
                reply.header(name, value);
            }
        }
    }

    fn streamed_body(&self, method: &str, path: &str) -> Option<usize> {
        streamed_limit(method, path)
    }

    fn cors(&self, path: &str) -> Cors {
        if api::cors(path) {
            Cors::Public
        } else {
            Cors::None
        }
    }

    fn metrics(&self) -> Vec<(&'static str, f64)> {
        let stats = self.lock().store().stats().ok();
        stats.map_or_else(Vec::new, |s| {
            vec![
                ("store_entries_used", s.used_entries as f64),
                ("store_entries_total", s.total_entries as f64),
            ]
        })
    }
}

/// The site around [`Household`]: mastomini answers every path itself
/// (its API, OAuth pages, profiles, the household app at `/app/`, `/ca`
/// and `/trust`); the framework adds `/metrics`, `/api/v1/sys` and
/// `/api/v1/log`.
pub fn config(ctx: &Ctx, target: &str) -> Config {
    let mut config = Config::new("mastomini", ctx.host());
    config.version = env!("CARGO_PKG_VERSION");
    config.build = env!("MASTOMINI_BUILD_FINGERPRINT");
    config.app_paths = &["/"];
    config.body_limit = http::BODY_LIMIT;
    // Only the framework's own replies use this buffer; mastomini's
    // responses are handed over whole (`Reply::owned`).
    config.response_limit = 64 * 1024;
    config.cors_methods = "GET, POST, PUT, PATCH, DELETE, OPTIONS";
    config.cors_headers =
        "Authorization, Content-Type, Idempotency-Key, If-None-Match, Cache-Control";
    config.expose_headers = "Link, X-RateLimit-Reset, Server-Timing, ETag";
    config.cors_max_age = 86400;
    config.influx_tags = vec![("target", target.to_string())];
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::testkit::{Server, T0};
    use crate::store::mem::{Failure, FaultStore};
    use miniframework::{desktop::DesktopPlatform, http as transport, site::Scratch, Site};
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    fn render(mut response: transport::Response) -> String {
        let mut bytes = Vec::new();
        while !response.next().is_empty() {
            let len = response.next().len();
            bytes.extend_from_slice(response.next());
            response.advance(len);
        }
        String::from_utf8(bytes).unwrap()
    }

    fn upload(token: &str, length: usize) -> transport::Request {
        transport::Request {
            method: "PATCH".into(),
            uri: UPLOAD_ROUTE.into(),
            headers: vec![
                ("Authorization".into(), format!("Bearer {token}")),
                ("Content-Type".into(), "application/json".into()),
            ],
            body: Vec::new(),
            consumed: 0,
            close: false,
            streamed: Some(length),
        }
    }

    #[test]
    fn short_or_failed_streams_never_mutate_the_profile() {
        let mut server = Server::provisioned();
        let token = server.login("alice", "alicepw", "read write");
        let shared = Arc::new(Mutex::new(server.svc));
        let ctx = Arc::new(server.ctx);
        let site = Site::new(
            config(&ctx, "test"),
            Household::new(shared.clone(), ctx, || Some(T0)),
            DesktopPlatform,
        );
        let mut payload = br#"{"display_name":"changed"}"#.to_vec();
        payload.resize(5000, b' ');
        let mut scratch = Scratch::new(65536);
        let response = site.respond_with(
            &upload(&token, payload.len() + 1),
            false,
            &mut scratch,
            Some(&mut payload.as_slice()),
        );
        assert!(render(response).starts_with("HTTP/1.1 400"));
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .state
                .account(0)
                .unwrap()
                .rec
                .display_name,
            ""
        );
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::ConnectionReset.into())
            }
        }
        let response = site.respond_with(
            &upload(&token, 5000),
            false,
            &mut scratch,
            Some(&mut Broken),
        );
        assert!(render(response).starts_with("HTTP/1.1 400"));
        // A successful stream forwards authentication and consumes exactly its declared bytes.
        let mut input = payload.clone();
        input.extend_from_slice(b"next request");
        let mut reader = std::io::Cursor::new(input);
        let response = site.respond_with(
            &upload(&token, payload.len()),
            false,
            &mut scratch,
            Some(&mut reader),
        );
        let wire = render(response);
        assert!(wire.starts_with("HTTP/1.1 200"), "{wire}");
        assert!(wire.contains("Access-Control-Allow-Origin: *"));
        assert!(wire.contains("Server-Timing:"));
        assert_eq!(reader.position(), payload.len() as u64);
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .state
                .account(0)
                .unwrap()
                .rec
                .display_name,
            "changed"
        );
        let response = site.respond_with(
            &upload(&token, http::UPLOAD_BODY_LIMIT + 1),
            false,
            &mut scratch,
            Some(&mut Broken),
        );
        assert!(render(response).starts_with("HTTP/1.1 413"));
    }

    #[test]
    fn storage_failure_is_reported_once_through_the_framework_adapter() {
        static REPORTS: AtomicUsize = AtomicUsize::new(0);
        let mut server = Server::provisioned();
        let token = server.login("alice", "alicepw", "read write");
        let domain_config = server.svc.config.clone();
        let mut service =
            Service::open(FaultStore::new(server.svc.into_store()), domain_config).unwrap();
        service.store().fail_after(Failure::Write, 0);
        let shared = Arc::new(Mutex::new(service));
        let ctx = Arc::new(server.ctx);
        let household = Household::new(shared, ctx.clone(), || Some(T0)).on_storage_failed(|| {
            REPORTS.fetch_add(1, SeqCst);
        });
        let site = Site::new(config(&ctx, "test"), household, DesktopPlatform);
        let mut request = upload(&token, 0);
        request.streamed = None;
        request.body = br#"{"display_name":"changed"}"#.to_vec();
        let mut scratch = Scratch::new(65536);
        for _ in 0..2 {
            let wire = render(site.respond(&request, false, &mut scratch));
            assert!(wire.starts_with("HTTP/1.1 503"), "{wire}");
        }
        assert_eq!(REPORTS.load(SeqCst), 1);
    }
}
