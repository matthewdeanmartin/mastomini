//! Desktop development server: the same API as the board, over HTTP and
//! (once `make certs` has run) HTTPS, persisting to a file store. It runs
//! miniframework's connection loop, as the board does (HTTP/2 too: by ALPN
//! over HTTPS, and prior-knowledge h2c over HTTP).
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
//! - `MASTOMINI_TRACE_TIMING`: log every request's timing (else only slow ones)

use mastomini::api::Ctx;
use mastomini::domain::{Config, Service};
use mastomini::server::{self, Household};
use mastomini::store::file::FileStore;
use mastomini::tls::{self, Tls};
use miniframework::desktop::{serve_https, DesktopPlatform, Https};
use miniframework::Site;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

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
fn load_tls(dir: &Path, https_url: &str, address: String) -> Result<(Https, Tls), String> {
    let read = |name: &str| {
        std::fs::read(dir.join(name)).map_err(|e| format!("{}: {e}", dir.join(name).display()))
    };
    let info = String::from_utf8(read(tls::INFO_FILE)?).map_err(|e| e.to_string())?;
    let public = Tls::new(read(tls::CA_DER_FILE)?, &info, https_url)?;
    let https = Https {
        address,
        cert_pem: read(tls::CERT_FILE)?,
        key_pem: read(tls::KEY_FILE)?,
    };
    Ok((https, public))
}

fn main() -> Result<(), BoxError> {
    miniframework::desktop::init_logging();
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
        match load_tls(Path::new(&dir), &url, format!("{bind}:{https_port}")) {
            Ok((config, public)) => {
                ctx.tls = Some(public);
                println!("HTTPS on {url} (devices trust it via {url}/trust)");
                https = Some(config);
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
    println!(
        "mastomini {} on {base_url} (store: {path})",
        env!("CARGO_PKG_VERSION")
    );
    if !svc.state.provisioned {
        println!(
            "Not set up yet: POST {base_url}/api/mastomini/v1/provision with username and password"
        );
    }
    let shared = Arc::new(Mutex::new(svc));
    let ctx = Arc::new(ctx);
    {
        let shared = Arc::clone(&shared);
        let ca = std::fs::read(env("MASTOMINI_SCHEDULER_CA", "certs/household-ca.crt"))
            .unwrap_or_default();
        let agent = mastomini::scheduler_bridge::desktop_agent(&ca)?;
        std::thread::spawn(move || loop {
            mastomini::scheduler_bridge::transfer(&shared, |job| {
                mastomini::scheduler_bridge::desktop_send(&agent, job)
            });
            std::thread::sleep(std::time::Duration::from_secs(5));
        });
    }
    let site_config = server::config(&ctx, "desktop");
    let household = Household::new(shared, Arc::clone(&ctx), now_ms).on_storage_failed(|| {
        eprintln!("storage failure, serving read-only until restart");
    });
    let site = Site::new(site_config, household, DesktopPlatform);
    serve_https(&site, &format!("{bind}:{port}"), https.as_ref(), || {})?;
    Ok(())
}
