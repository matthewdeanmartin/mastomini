//! Desktop development server: the same API as the board, over plain HTTP,
//! persisting to a file store.
//!
//! Environment:
//! - `MASTOMINI_BIND` (default `127.0.0.1`) and `MASTOMINI_PORT` (`8080`)
//! - `MASTOMINI_STORE` (default `mastomini.store`)
//! - `MASTOMINI_BASE_URL` (default `http://<bind>:<port>`), used in URLs
//!   and `@user@host` mentions
//! - `MASTOMINI_PASSWORD_ROUNDS` (default: the firmware value)

use mastomini::api::{self, Ctx};
use mastomini::domain::{Config, Service};
use mastomini::http::{Request, UPLOAD_BODY_LIMIT};
use mastomini::store::file::FileStore;
use std::io::Read;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tiny_http::{Header, Response, Server};

fn env(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn now_ms() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let bind = env("MASTOMINI_BIND", "127.0.0.1");
    let port = env("MASTOMINI_PORT", "8080");
    let path = env("MASTOMINI_STORE", "mastomini.store");
    let base_url = env("MASTOMINI_BASE_URL", &format!("http://{bind}:{port}"));
    let ctx = Ctx::new(&base_url);
    let mut config = Config {
        host: ctx.host().to_string(),
        ..Config::default()
    };
    if let Ok(rounds) = env("MASTOMINI_PASSWORD_ROUNDS", "").parse() {
        config.password_rounds = rounds;
    }

    let store = FileStore::open(&path).map_err(|e| format!("store {path}: {e}"))?;
    let mut svc = Service::open(store, config).map_err(|e| format!("store {path}: {e}"))?;
    let server = Server::http(format!("{bind}:{port}"))?;
    println!(
        "mastomini {} on {base_url} (store: {path})",
        env!("CARGO_PKG_VERSION")
    );
    if !svc.state.provisioned {
        println!(
            "Not set up yet: POST {base_url}/api/mastomini/v1/provision with username and password"
        );
    }

    for mut request in server.incoming_requests() {
        let started = Instant::now();
        let mut req = Request::new(request.method().as_str(), request.url());
        req.headers = request
            .headers()
            .iter()
            .map(|h| (h.field.as_str().to_string(), h.value.as_str().to_string()))
            .collect();
        let mut body = Vec::new();
        // Read one byte past the largest allowed body so the API can refuse it.
        let read = request
            .as_reader()
            .take(UPLOAD_BODY_LIMIT as u64 + 1)
            .read_to_end(&mut body);
        if let Err(e) = read {
            eprintln!("{} {}: read failed: {e}", req.method, req.path);
            continue;
        }
        req.body = body;

        let reply = api::handle(&mut svc, &ctx, &req, now_ms());
        println!(
            "{} {} -> {} ({} ms)",
            req.method,
            req.path,
            reply.status,
            started.elapsed().as_millis()
        );
        let headers: Vec<Header> = reply
            .headers
            .iter()
            .filter_map(|(k, v)| Header::from_bytes(k.as_bytes(), v.as_bytes()).ok())
            .collect();
        let len = reply.body.len();
        let response = Response::new(
            reply.status.into(),
            headers,
            &reply.body[..],
            Some(len),
            None,
        );
        if let Err(e) = request.respond(response) {
            eprintln!("{} {}: client went away: {e}", req.method, req.path);
        }
        if let Some(reason) = svc.latched() {
            eprintln!("storage failure, serving read-only until restart: {reason}");
        }
    }
    Ok(())
}
