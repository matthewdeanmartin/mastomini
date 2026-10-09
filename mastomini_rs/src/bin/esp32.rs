//! ESP32-S3 firmware: the same API as the desktop build, persisted in NVS,
//! on miniframework's board runner.
//!
//! The framework brings up Wi-Fi (the saved network, else the setup network
//! for first boot: see `net.rs`), SNTP and mDNS, and serves HTTPS on 443 and
//! plain HTTP on 80, both serving everything ("Easy mode", spec/05), with
//! TLS handshakes on core 0 and the connection loop on core 1. This file
//! opens the store, starts mastomini's own light and incident sampler, and
//! runs the scheduler bridge. The certificate comes from `certs/` (`make
//! certs`, docs/security/https.md); the CA's private key never does.
//!
//! Startup steps (a failed startup explains itself on http://<board>:8080/
//! from step 5 on): 1-7 are miniframework's (system, NVS, Wi-Fi driver,
//! Wi-Fi join, network, app, listen); mastomini adds 8 store partition,
//! 9 store open.

#![deny(unsafe_code, unsafe_op_in_unsafe_fn)]

#[cfg(not(target_os = "espidf"))]
compile_error!(
    "build firmware with --target xtensa-esp32s3-espidf --no-default-features --features esp32"
);

use esp_idf_svc::hal::peripherals::Peripherals;
use mastomini::{
    api::{diag::Platform, Ctx},
    domain::{Config, Service},
    server::{self, Household},
    tls::Tls,
};
use miniframework::{
    esp::{self, BoardConfig},
    status::SIGNALS,
    Site,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[path = "esp32/net.rs"]
mod net;
#[path = "esp32/nvs_store.rs"]
#[allow(unsafe_code)] // Fallible NVS enumeration and statistics SDK boundary.
mod nvs_store;
#[path = "esp32/scheduler_client.rs"]
mod scheduler_client;
// Both boards must enforce the same constrained household CA, including the
// mbedTLS extension handling already used by the bots HTTP client.
#[path = "../../../mastomini_bots/src/bin/esp32/household_tls.rs"]
#[allow(unsafe_code)] // Constrained household CA callbacks into mbedTLS.
mod household_tls;
#[path = "../../../mastomini_bots/src/household_trust.rs"]
mod household_trust;

#[path = "esp32/status_led.rs"]
mod status_led;
use nvs_store::NvsStore;

const HOSTNAME: &str = match option_env!("MASTOMINI_HOSTNAME") {
    Some(name) => name,
    None => "mastomini",
};

/// `scripts/certs.sh` output. build.rs refuses to build without it, and
/// `scripts/certs-check.sh` has vetted it. The server key is necessarily in
/// the firmware; the CA's key is not.
const SERVER_CERT: &str = concat!(include_str!("../../certs/mastomini.crt"), "\0");
const SERVER_KEY: &str = concat!(include_str!("../../certs/mastomini.key"), "\0");
const CA_DER: &[u8] = include_bytes!("../../certs/household-ca.der");
const CERT_INFO: &str = include_str!("../../certs/certificate.json");

/// Before this instant the RTC has not been set by SNTP (2024-01-01).
const CLOCK_VALID_AFTER_MS: u128 = 1_704_067_200_000;

fn now_ms() -> Option<u64> {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    (ms > CLOCK_VALID_AFTER_MS).then_some(ms as u64)
}

/// Milliseconds since boot (monotonic), for the manually set clock.
fn uptime_ms() -> u64 {
    // The safe task timer service reads the same boot-relative clock.
    static TIMER: std::sync::OnceLock<esp_idf_svc::timer::EspTaskTimerService> =
        std::sync::OnceLock::new();
    TIMER
        .get_or_init(|| esp_idf_svc::timer::EspTaskTimerService::new().expect("task timer service"))
        .now()
        .as_millis() as u64
}

fn reset_reason() -> &'static str {
    use esp_idf_svc::hal::reset::ResetReason::*;
    match esp_idf_svc::hal::reset::ResetReason::get() {
        PowerOn => "power on",
        ExternalPin => "reset pin",
        Software => "software restart",
        Panic => "crash",
        Watchdog | InterruptWatchdog | TaskWatchdog => "watchdog",
        DeepSleep => "deep sleep",
        Brownout => "brownout (power dipped)",
        USBPeripheral => "USB",
        _ => "other",
    }
}

/// Board facts for `/diag`.
#[allow(unsafe_code)] // SDK heap/RSSI queries with local initialized outputs.
fn platform() -> Platform {
    use esp_idf_svc::sys::*;
    let internal = MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT;
    // SAFETY: heap and reset queries take no pointers; the AP record is a
    // local that the call fills in.
    unsafe {
        let mut ap = wifi_ap_record_t::default();
        let rssi = (esp_wifi_sta_get_ap_info(&mut ap) == ESP_OK).then_some(i32::from(ap.rssi));
        Platform {
            target: "esp32s3",
            uptime_ms: uptime_ms(),
            heap_internal_free: Some(heap_caps_get_free_size(internal) as u64),
            heap_internal_min_free: Some(heap_caps_get_minimum_free_size(internal) as u64),
            heap_internal_largest_block: Some(heap_caps_get_largest_free_block(internal) as u64),
            psram_free: Some(heap_caps_get_free_size(MALLOC_CAP_SPIRAM) as u64),
            psram_min_free: Some(heap_caps_get_minimum_free_size(MALLOC_CAP_SPIRAM) as u64),
            reset_reason: Some(reset_reason()),
            wifi_rssi: rssi,
        }
    }
}

fn board_config() -> BoardConfig {
    // Built-in credentials are optional (developers' .env): a board joins
    // the network saved on it, else opens the setup network.
    let mut config = BoardConfig::s3(
        option_env!("MASTOMINI_WIFI_SSID").unwrap_or(""),
        option_env!("MASTOMINI_WIFI_PASSWORD").unwrap_or(""),
        HOSTNAME,
    );
    config.instance = "mastomini household Mastodon server";
    config.cert_pem = SERVER_CERT.as_bytes();
    config.key_pem = SERVER_KEY.as_bytes();
    config.saved_wifi = Some(net::NAMESPACE);
    config.setup_network = Some(net::AP_SSID);
    config.force_setup = option_env!("MASTOMINI_FORCE_SETUP") == Some("1");
    config.mdns_https_path = "/api/v1/instance";
    config.ntp_server = option_env!("MASTOMINI_NTP_SERVER");
    // Was mastomini's own server: 512 KiB of unsent responses at most.
    config.limits.response_budget = 512 * 1024;
    config
}

fn main() {
    if let Err(error) = run() {
        status_led::fatal();
        // Blinks the step, logs every 5 s, explains itself on port 8080.
        esp::fail(&error.to_string());
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Also starts the framework's incident history (/.well-known/incidents).
    esp::init();
    let peripherals = Peripherals::take()?;
    status_led::start(peripherals.pins);
    log::info!("mastomini {} starting", env!("CARGO_PKG_VERSION"));
    log::info!("Reset reason: {}", reset_reason());
    let board = esp::start(board_config(), peripherals.modem)?;

    SIGNALS.step(8, "store partition");
    let partition = board.partition(nvs_store::PARTITION.to_str()?)?;
    SIGNALS.step(9, "store open");
    let store = NvsStore::open(partition).map_err(|e| format!("store: {e}"))?;

    let base_url = option_env!("MASTOMINI_BASE_URL")
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://{HOSTNAME}.local"));
    let https_url = if base_url.starts_with("https://") {
        base_url.clone()
    } else {
        format!("https://{HOSTNAME}.local")
    };
    let ctx = Arc::new(Ctx {
        platform,
        uptime_ms,
        tls: Some(Tls::new(CA_DER.to_vec(), CERT_INFO, &https_url)?),
        ..Ctx::new(&base_url)
    });
    let config = Config {
        host: ctx.host().to_string(),
        ..Config::default()
    };
    let service = Service::open(store, config).map_err(|e| format!("store: {e}"))?;
    log::info!(
        "store: provisioned={} accounts={} statuses={} repairs={}",
        service.state.provisioned,
        service.state.active_accounts().count(),
        service.state.statuses.len(),
        service.state.repairs
    );
    status_led::phrase(&service.led_phrase);
    let shared = Arc::new(Mutex::new(service));

    let setup = board
        .wifi_setup()
        .map(|wifi| Arc::new(net::BoardSetup::new(wifi)));
    let mut household = Household::new(Arc::clone(&shared), Arc::clone(&ctx), now_ms)
        .on_storage_failed(|| {
            mastomini::incidents::record(mastomini::incidents::Kind::StorageFailed, 0)
        });
    if let Some(setup) = &setup {
        household = household.with_setup(Box::new(SharedSetup(Arc::clone(setup))));
    }
    let site = Site::new(
        server::config(&ctx, "esp32s3"),
        household,
        board.platform(peripherals.temp_sensor),
    );

    let health = SIGNALS.health(false);
    status_led::wifi(health.wifi);
    status_led::setup(SIGNALS.in_setup());
    // Registration success does not prove a client's multicast path works.
    status_led::ready(health.mdns);
    match esp::station_ip() {
        Some(ip) => log::info!("Ready at {base_url} (https://{ip}/ and http://{ip}/)"),
        None if SIGNALS.in_setup() => log::info!(
            "Setup network {} is up: join it and open http://{}/",
            net::AP_SSID,
            mastomini::captive::AP_IP
        ),
        None => log::info!("Ready at {base_url} (Wi-Fi still joining)"),
    }

    // Store writes from the scheduler bridge happen here, on the calling
    // (main) task, whose stack is internal RAM; serving runs on core 1.
    let mut last = Instant::now();
    board.serve(site, move |_| {
        if last.elapsed() < Duration::from_secs(5) {
            return;
        }
        last = Instant::now();
        let provisioned = {
            let service = shared.lock().unwrap_or_else(|e| e.into_inner());
            status_led::phrase(&service.led_phrase);
            service.state.provisioned
        };
        status_led::wifi(SIGNALS.health(false).wifi);
        status_led::setup(SIGNALS.in_setup());
        if let Some(setup) = &setup {
            setup.maybe_close(provisioned);
        }
        mastomini::scheduler_bridge::transfer(&shared, scheduler_client::send);
    })
}

/// The setup pages, shared between the site and the main loop.
struct SharedSetup(Arc<net::BoardSetup>);

impl server::Setup for SharedSetup {
    fn active(&self) -> bool {
        self.0.active()
    }
    fn joined(&self) -> bool {
        self.0.joined()
    }
    fn page(&self, req: &mastomini::http::Request, provisioned: bool) -> mastomini::http::Response {
        self.0.page(req, provisioned)
    }
}
