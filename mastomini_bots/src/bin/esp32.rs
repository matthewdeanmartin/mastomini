//! ESP32-S3 firmware for mastomini-bots: the bots, their scheduler and the
//! admin site (HTTPS on 443, HTTP on 80) at `mastomini-bots.local`.
//!
//! Wi-Fi from the build's credentials (build.rs), SNTP for the clock (bots
//! wait for it), mDNS for the name, settings in the `store` NVS partition.
//! The admin site's certificate comes from `certs/` (`make certs`), issued
//! by mastomini's household CA; the CA's private key is never in here.

#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(not(target_os = "espidf"))]
compile_error!(
    "build firmware with --target xtensa-esp32s3-espidf --no-default-features --features esp32"
);

use esp_idf_svc::hal::{
    cpu::Core, peripherals::Peripherals, task::thread::ThreadSpawnConfiguration,
};
use mastobots::server::{self, Bots};
use mastobots::{
    api::{Ctx, Platform, Tls},
    service::Service,
};
use miniframework::{
    esp::{self, BoardConfig},
    Site,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[allow(unsafe_code)] // Reviewed ESP-IDF outbound HTTP client boundary.
#[path = "esp32/client.rs"]
mod client;
#[allow(unsafe_code)] // mbedTLS trust-anchor and verification callback boundary.
#[path = "esp32/household_tls.rs"]
mod household_tls;
use mastobots::household_trust;
#[path = "esp32/nvs_kv.rs"]
mod nvs_kv;
#[path = "esp32/status_led.rs"]
mod status_led;

const HOSTNAME: &str = match option_env!("MASTOMINI_BOTS_HOSTNAME") {
    Some(name) => name,
    None => "mastomini-bots",
};

const SERVER_CERT: &str = concat!(include_str!("../../certs/server.crt"), "\0");
const SERVER_KEY: &str = concat!(include_str!("../../certs/server.key"), "\0");
/// The household CA: served at `/ca`, and trusted for the bots' own requests
/// to household servers such as mastomini.local.
const CA_PEM: &str = include_str!("../../certs/household-ca.crt");
const CA_DER: &[u8] = include_bytes!("../../certs/household-ca.der");
const CERT_INFO: &str = include_str!("../../certs/certificate.json");

type Shared = Arc<Mutex<Service<nvs_kv::NvsKv>>>;

/// Before this instant SNTP has not set the clock (2024-01-01).
const CLOCK_VALID_AFTER_MS: u128 = 1_704_067_200_000;

fn now_ms() -> Option<u64> {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    (ms > CLOCK_VALID_AFTER_MS).then_some(ms as u64)
}

fn uptime_ms() -> u64 {
    static TIMER: std::sync::OnceLock<esp_idf_svc::timer::EspTaskTimerService> =
        std::sync::OnceLock::new();
    TIMER
        .get_or_init(|| esp_idf_svc::timer::EspTaskTimerService::new().expect("task timer service"))
        .now()
        .as_millis() as u64
}

fn reset_reason() -> &'static str {
    esp::reset_reason()
}

// Heap/RSSI statistics have no safe SDK wrapper available to this callback.
#[allow(unsafe_code)]
fn platform() -> Platform {
    use esp_idf_svc::sys::*;
    let internal = MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT;
    // SAFETY: heap queries take no pointers; the AP record is a local the
    // call fills in.
    let mut ap = wifi_ap_record_t::default();
    let rssi =
        (unsafe { esp_wifi_sta_get_ap_info(&mut ap) } == ESP_OK).then_some(i32::from(ap.rssi));
    Platform {
        target: "esp32s3",
        uptime_ms: uptime_ms(),
        // SAFETY: capability masks are SDK constants; no pointer arguments.
        heap_internal_free: Some(unsafe { heap_caps_get_free_size(internal) } as u64),
        heap_internal_min_free: Some(unsafe { heap_caps_get_minimum_free_size(internal) } as u64),
        psram_free: Some(unsafe { heap_caps_get_free_size(MALLOC_CAP_SPIRAM) } as u64),
        wifi_rssi: rssi,
        reset_reason: Some(reset_reason()),
        ip: esp::station_ip(),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    esp::init();
    if let Err(e) = run() {
        log::error!("Startup failed: {e}");
        status_led::fatal();
        esp::fail(&e.to_string());
    }
    Ok(())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    log::info!(
        "mastomini-bots {} starting; reset {}",
        env!("CARGO_PKG_VERSION"),
        reset_reason()
    );

    let peripherals = Peripherals::take()?;
    status_led::start(peripherals.pins);
    let mut config = BoardConfig::s3(
        option_env!("MASTOMINI_WIFI_SSID").unwrap_or(""),
        option_env!("MASTOMINI_WIFI_PASSWORD").unwrap_or(""),
        HOSTNAME,
    );
    config.instance = "mastomini-bots";
    config.cert_pem = SERVER_CERT.as_bytes();
    config.key_pem = SERVER_KEY.as_bytes();
    config.mdns_https_path = "/app/";
    config.ntp_server = option_env!("MASTOMINI_NTP_SERVER");
    if config.ssid.is_empty() {
        log::error!("No Wi-Fi in this build: set MASTOMINI_WIFI_SSID/PASSWORD in .env and rebuild");
        return Err("No Wi-Fi in this build".into());
    }
    // Preserve the old 4 HTTPS / 3 HTTP limit; leave sockets for bot clients.
    config.limits.tls_clients = 4;
    config.limits.http_clients = 3;
    config.limits.response_budget = 512 * 1024;
    let board = esp::start(config, peripherals.modem)?;
    let partition = board.partition(nvs_kv::PARTITION.to_str()?)?;
    let mut store = nvs_kv::NvsKv::open(partition)?;
    if mastobots::recovery::reset_admin_once(
        &mut store,
        option_env!("MASTOMINI_BOTS_RESET_ADMIN_ONCE"),
    )? {
        log::warn!("One-time admin recovery applied; choose a new password at /app/");
    }
    let svc = Service::open(store, mastobots::bots::all())?;
    status_led::phrase(&svc.led_phrase);
    log::info!(
        "{} bots; admin password {}",
        svc.infos.len(),
        if svc.verifier.is_some() {
            "set"
        } else {
            "not set yet (first visit sets it)"
        }
    );
    let shared: Shared = Arc::new(Mutex::new(svc));

    let ctx = Arc::new(Ctx {
        platform,
        tls: Some(Tls {
            ca_der: CA_DER.to_vec(),
            ca_pem: CA_PEM.to_string(),
            info: serde_json::from_str(CERT_INFO)?,
        }),
    });
    let site = Site::new(
        server::config(&format!("{HOSTNAME}.local"), "esp32s3"),
        Bots::new(Arc::clone(&shared), ctx, now_ms, uptime_ms).on_error(status_led::error),
        board.platform(peripherals.temp_sensor),
    );

    // The scheduler: its own task, so a slow server never holds up the admin
    // site. TLS client handshakes need a deep stack.
    ThreadSpawnConfiguration {
        name: Some(c"mm-bots"),
        stack_size: 16 * 1024,
        pin_to_core: Some(Core::Core0),
        ..Default::default()
    }
    .set()?;
    {
        let shared = Arc::clone(&shared);
        let resolver = board.resolver();
        std::thread::Builder::new()
            .stack_size(16 * 1024)
            .spawn(move || {
                let mut client = client::EspClient::new(resolver);
                loop {
                    status_led::scheduler_busy(true);
                    mastobots::runner::tick(&shared, &mut client, now_ms);
                    status_led::scheduler_busy(false);
                    std::thread::sleep(Duration::from_secs(1));
                }
            })?;
    }
    // Independent timer worker: an LLM request must not hold up scheduled posts.
    {
        let shared = Arc::clone(&shared);
        let resolver = board.resolver();
        std::thread::Builder::new()
            .stack_size(16 * 1024)
            .spawn(move || {
                let mut client = client::EspClient::scheduler(resolver);
                loop {
                    mastobots::scheduler::tick(&shared, &mut client, now_ms());
                    mastobots::status::tick(&shared, &mut client, now_ms());
                    std::thread::sleep(Duration::from_secs(1));
                }
            })?;
    }

    ThreadSpawnConfiguration::default().set()?;
    let mut announced = false;
    let mut maintenance = std::time::Instant::now();
    board.serve(site, |_| {
        if !announced && miniframework::status::SIGNALS.health(false).ready {
            if let Some(ip) = esp::station_ip() {
                log::info!("Ready at https://{HOSTNAME}.local/app/ (https://{ip}/app/)");
                announced = true;
            }
        }
        if maintenance.elapsed() < Duration::from_secs(1) {
            return;
        }
        maintenance = std::time::Instant::now();
        if let Ok(svc) = shared.try_lock() {
            status_led::phrase(&svc.led_phrase);
        }
    })
}
