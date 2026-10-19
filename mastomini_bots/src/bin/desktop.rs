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

use mastobots::api::{Ctx, Platform, Tls};
use mastobots::desktop_client::DesktopClient;
use mastobots::server::{self, Bots};
use mastobots::service::Service;
use mastobots::store::FileStore;
use miniframework::desktop::{serve_https, DesktopPlatform, Https};
use miniframework::Site;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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

fn load_tls(address: String) -> Result<(Https, Tls), String> {
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
        Https {
            address,
            cert_pem: read("server.crt")?,
            key_pem: read("server.key")?,
        },
        tls,
    ))
}

fn main() -> Result<(), BoxError> {
    miniframework::desktop::init_logging();
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
        match load_tls(format!("{bind}:{https_port}")) {
            Ok((config, tls)) => {
                ctx.tls = Some(tls);
                https = Some(config);
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
    {
        let shared = Arc::clone(&svc);
        let mut timer_client = DesktopClient::with_timeout(
            household_ca.as_deref(),
            &env("MASTOBOTS_RESOLVE", ""),
            std::time::Duration::from_secs(5),
        )?;
        std::thread::spawn(move || loop {
            mastobots::scheduler::tick(&shared, &mut timer_client, now_ms());
            mastobots::status::tick(&shared, &mut timer_client, now_ms());
            std::thread::sleep(std::time::Duration::from_secs(1));
        });
    }

    let site = Site::new(
        server::config("mastomini-bots.local", "desktop"),
        Bots::new(svc, ctx, now_ms, mono_ms),
        DesktopPlatform,
    );
    serve_https(&site, &format!("{bind}:{port}"), https.as_ref(), || {})?;
    Ok(())
}
