//! Desktop build: the same bots, scheduler and admin site as the board,
//! for developing bots. Settings in a JSON file.
//!
//! Environment:
//! - `MASTOBOTS_BIND` (default `127.0.0.1`), `MASTOBOTS_PORT` (`8090`)
//! - `MASTOBOTS_HTTPS_PORT`: also serve HTTPS (needs `make certs`)
//! - `MASTOBOTS_STORE` (default `mastomini-bots.json`)
//! - `MASTOBOTS_CA` (default `certs/household-ca.crt`, if present): trusted
//!   for the bots' own requests, so `https://mastomini.local` verifies
//! - `MASTOBOTS_RESOLVE`: `mastomini.local=192.168.1.161`, skip name lookups

use mastobots::api::{self, Ctx, Platform, Tls};
use mastobots::desktop_client::DesktopClient;
use mastobots::http::{Request, BODY_LIMIT};
use mastobots::service::Service;
use mastobots::store::FileStore;
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tiny_http::{Header, Response, Server, SslConfig};

type Shared = Arc<Mutex<Service<FileStore>>>;
type BoxError = Box<dyn std::error::Error + Send + Sync>;

static STARTED: OnceLock<Instant> = OnceLock::new();

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

fn mono_ms() -> u64 {
    STARTED.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn platform() -> Platform {
    Platform {
        target: "desktop",
        uptime_ms: mono_ms(),
        ..Platform::default()
    }
}

fn load_tls() -> Result<(SslConfig, Tls), String> {
    let read = |name: &str| {
        std::fs::read(format!("certs/{name}")).map_err(|e| format!("certs/{name}: {e}"))
    };
    let info = serde_json::from_slice(&read("certificate.json")?).map_err(|e| e.to_string())?;
    let tls = Tls {
        ca_der: read("household-ca.der")?,
        ca_pem: String::from_utf8(read("household-ca.crt")?).map_err(|e| e.to_string())?,
        info,
    };
    Ok((
        SslConfig {
            certificate: read("server.crt")?,
            private_key: read("server.key")?,
        },
        tls,
    ))
}

fn main() -> Result<(), BoxError> {
    mono_ms();
    let bind = env("MASTOBOTS_BIND", "127.0.0.1");
    let port = env("MASTOBOTS_PORT", "8090");
    let https_port = env("MASTOBOTS_HTTPS_PORT", "");
    let path = env("MASTOBOTS_STORE", "mastomini-bots.json");
    let ca_path = env("MASTOBOTS_CA", "certs/household-ca.crt");
    let household_ca = std::fs::read(&ca_path).ok();

    let mut ctx = Ctx {
        platform,
        tls: None,
    };
    let mut https = None;
    if !https_port.is_empty() {
        match load_tls() {
            Ok((config, tls)) => {
                ctx.tls = Some(tls);
                https = Some(Server::https(format!("{bind}:{https_port}"), config)?);
            }
            Err(e) => println!("HTTPS off: {e}. Run make certs to turn it on."),
        }
    }
    let store = FileStore::open(&path)?;
    let svc = Service::open(store, mastobots::bots::all())?;
    if svc.verifier.is_none() {
        println!("First visit sets the admin password: http://{bind}:{port}/app/");
    }
    let svc: Shared = Arc::new(Mutex::new(svc));
    let ctx = Arc::new(ctx);
    let client = DesktopClient::new(household_ca.as_deref(), &env("MASTOBOTS_RESOLVE", ""))?;
    println!(
        "mastomini-bots {} on http://{bind}:{port} (store {path}; household CA {})",
        env!("CARGO_PKG_VERSION"),
        if household_ca.is_some() {
            ca_path.as_str()
        } else {
            "not found"
        }
    );
    {
        let svc = Arc::clone(&svc);
        std::thread::spawn(move || mastobots::runner::run_forever(svc, Box::new(client), now_ms));
    }
    if let Some(server) = https {
        println!("HTTPS on https://localhost:{https_port}");
        let (svc, ctx) = (Arc::clone(&svc), Arc::clone(&ctx));
        std::thread::spawn(move || serve(&server, true, &svc, &ctx));
    }
    let server = Server::http(format!("{bind}:{port}"))?;
    serve(&server, false, &svc, &ctx);
    Ok(())
}

fn serve(server: &Server, secure: bool, svc: &Shared, ctx: &Ctx) {
    for mut request in server.incoming_requests() {
        let mut req = Request::new(request.method().as_str(), request.url());
        req.secure = secure;
        req.headers = request
            .headers()
            .iter()
            .map(|h| (h.field.as_str().to_string(), h.value.as_str().to_string()))
            .collect();
        let mut body = Vec::new();
        if request
            .as_reader()
            .take(BODY_LIMIT as u64 + 1)
            .read_to_end(&mut body)
            .is_err()
        {
            continue;
        }
        req.body = body;
        let reply = {
            let mut svc = svc.lock().unwrap_or_else(|e| e.into_inner());
            api::handle(&mut svc, ctx, &req, now_ms(), mono_ms())
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
        let _ = request.respond(response);
    }
}
