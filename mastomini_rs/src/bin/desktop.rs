//! Desktop development server: the same API as the board, over HTTP and
//! (once `make certs` has run) HTTPS, persisting to a file store.
//!
//! Environment:
//! - `MASTOMINI_BIND` (default `127.0.0.1`) and `MASTOMINI_PORT` (`8080`)
//! - `MASTOMINI_HTTPS_PORT`: also serve HTTPS on this port (`make run` sets
//!   8443). Unset: HTTP only, so test servers never compete for a port.
//! - `MASTOMINI_TLS_DIR` (default `certs`): `scripts/certs.sh` output
//! - `MASTOMINI_HTTPS_URL` (default `https://localhost:<https port>`): where
//!   `/trust` sends people
//! - `MASTOMINI_STORE` (default `mastomini.store`)
//! - `MASTOMINI_BASE_URL` (default `http://<bind>:<port>`), used in URLs
//!   and `@user@host` mentions
//! - `MASTOMINI_PASSWORD_ROUNDS` (default: the firmware value)

use mastomini::api::{self, Ctx};
use mastomini::domain::{Config, Service};
use mastomini::http::{Request, UPLOAD_BODY_LIMIT};
use mastomini::store::file::FileStore;
use mastomini::tls::{self, Tls};
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tiny_http::{Header, Response, Server, SslConfig};

type Shared = Arc<Mutex<Service<FileStore>>>;
type BoxError = Box<dyn std::error::Error + Send + Sync>;

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

/// The server certificate and key, and what `/ca` and `/trust` show.
fn load_tls(dir: &Path, https_url: &str) -> Result<(SslConfig, Tls), String> {
    let read = |name: &str| {
        std::fs::read(dir.join(name)).map_err(|e| format!("{}: {e}", dir.join(name).display()))
    };
    let info = String::from_utf8(read(tls::INFO_FILE)?).map_err(|e| e.to_string())?;
    let public = Tls::new(read(tls::CA_DER_FILE)?, &info, https_url)?;
    let config = SslConfig {
        certificate: read(tls::CERT_FILE)?,
        private_key: read(tls::KEY_FILE)?,
    };
    Ok((config, public))
}

fn main() -> Result<(), BoxError> {
    let bind = env("MASTOMINI_BIND", "127.0.0.1");
    let port = env("MASTOMINI_PORT", "8080");
    let https_port = env("MASTOMINI_HTTPS_PORT", "");
    let path = env("MASTOMINI_STORE", "mastomini.store");
    let base_url = env("MASTOMINI_BASE_URL", &format!("http://{bind}:{port}"));
    let mut ctx = Ctx::new(&base_url);
    let mut https = None;
    if !https_port.is_empty() {
        let dir = env("MASTOMINI_TLS_DIR", "certs");
        let url = env(
            "MASTOMINI_HTTPS_URL",
            &format!("https://localhost:{https_port}"),
        );
        match load_tls(Path::new(&dir), &url) {
            Ok((config, public)) => {
                ctx.tls = Some(public);
                https = Some((Server::https(format!("{bind}:{https_port}"), config)?, url));
            }
            Err(e) => println!("HTTPS off: {e}. Run make certs to turn it on."),
        }
    }
    let mut config = Config {
        host: ctx.host().to_string(),
        ..Config::default()
    };
    if let Ok(rounds) = env("MASTOMINI_PASSWORD_ROUNDS", "").parse() {
        config.password_rounds = rounds;
    }

    let store = FileStore::open(&path).map_err(|e| format!("store {path}: {e}"))?;
    let svc = Service::open(store, config).map_err(|e| format!("store {path}: {e}"))?;
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
    let svc: Shared = Arc::new(Mutex::new(svc));
    let ctx = Arc::new(ctx);
    if let Some((server, url)) = https {
        println!("HTTPS on {url} (devices trust it via {url}/trust)");
        let (svc, ctx) = (Arc::clone(&svc), Arc::clone(&ctx));
        std::thread::spawn(move || serve(&server, true, &svc, &ctx));
    }
    serve(&server, false, &svc, &ctx);
    Ok(())
}

/// One listener's request loop. Both listeners share the service, one
/// request at a time, as on the board.
fn serve(server: &Server, secure: bool, svc: &Shared, ctx: &Ctx) {
    for mut request in server.incoming_requests() {
        let started = Instant::now();
        let mut req = Request::new(request.method().as_str(), request.url());
        req.secure = secure;
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

        let body_ms = started.elapsed().as_secs_f64() * 1000.0;
        let lock_started = Instant::now();
        let lock_ms;
        let reply = {
            let mut svc = svc.lock().unwrap_or_else(|e| e.into_inner());
            lock_ms = lock_started.elapsed().as_secs_f64() * 1000.0;
            let reply = api::handle(&mut svc, ctx, &req, now_ms());
            if let Some(reason) = svc.latched() {
                eprintln!("storage failure, serving read-only until restart: {reason}");
            }
            reply.with_timing(&format!("body;dur={body_ms:.3}, lock;dur={lock_ms:.3}"))
        };
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
        let send_started = Instant::now();
        let result = request.respond(response);
        if started.elapsed().as_millis() >= 250
            || std::env::var_os("MASTOMINI_TRACE_TIMING").is_some()
        {
            println!("timing method={} route={} status={} bytes={} body_ms={body_ms:.3} lock_ms={lock_ms:.3} send_ms={:.3} accepted_total_ms={:.3}",
                req.method, mastomini::http::route_label(&req.path), reply.status, len,
                send_started.elapsed().as_secs_f64()*1000.0,started.elapsed().as_secs_f64()*1000.0);
        }
        if let Err(e) = result {
            eprintln!("{} {}: client went away: {e}", req.method, req.path);
        }
    }
}
