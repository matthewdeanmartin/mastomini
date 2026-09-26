//! ESP32-S3 firmware for mastomini-bots: the bots, their scheduler and the
//! admin site (HTTPS on 443, HTTP on 80) at `mastomini-bots.local`.
//!
//! Wi-Fi from the build's credentials (build.rs), SNTP for the clock (bots
//! wait for it), mDNS for the name, settings in the `store` NVS partition.
//! The admin site's certificate comes from `certs/` (`make certs`), issued
//! by mastomini's household CA; the CA's private key is never in here.

#[cfg(not(target_os = "espidf"))]
compile_error!(
    "build firmware with --target xtensa-esp32s3-espidf --no-default-features --features esp32"
);

use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{cpu::Core, peripherals::Peripherals, task::thread::ThreadSpawnConfiguration},
    mdns::EspMdns,
    nvs::{EspDefaultNvsPartition, EspNvsPartition, NvsCustom},
    sntp::{EspSntp, SntpConf},
    wifi::{BlockingWifi, EspWifi},
};
use mastobots::{
    api::{Ctx, Platform, Tls},
    service::Service,
};
use std::{
    net::Ipv4Addr,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[path = "esp32/client.rs"]
mod client;
#[path = "esp32/net.rs"]
mod net;
#[path = "esp32/nvs_kv.rs"]
mod nvs_kv;
#[path = "esp32/server.rs"]
mod server;

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

static IP: OnceLock<Ipv4Addr> = OnceLock::new();

fn now_ms() -> Option<u64> {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    (ms > CLOCK_VALID_AFTER_MS).then_some(ms as u64)
}

fn uptime_ms() -> u64 {
    // SAFETY: reads the monotonic system timer.
    (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1000) as u64
}

// ESP-IDF constants in match patterns keep their C names.
#[allow(non_upper_case_globals)]
fn reset_reason() -> &'static str {
    use esp_idf_svc::sys::*;
    // SAFETY: takes no arguments.
    match unsafe { esp_reset_reason() } {
        esp_reset_reason_t_ESP_RST_POWERON => "power on",
        esp_reset_reason_t_ESP_RST_SW => "software restart",
        esp_reset_reason_t_ESP_RST_PANIC => "crash",
        esp_reset_reason_t_ESP_RST_INT_WDT
        | esp_reset_reason_t_ESP_RST_TASK_WDT
        | esp_reset_reason_t_ESP_RST_WDT => "watchdog",
        esp_reset_reason_t_ESP_RST_BROWNOUT => "brownout (power dipped)",
        _ => "other",
    }
}

fn platform() -> Platform {
    use esp_idf_svc::sys::*;
    let internal = MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT;
    // SAFETY: heap queries take no pointers; the AP record is a local the
    // call fills in.
    unsafe {
        let mut ap: wifi_ap_record_t = core::mem::zeroed();
        let rssi = (esp_wifi_sta_get_ap_info(&mut ap) == ESP_OK).then_some(i32::from(ap.rssi));
        Platform {
            target: "esp32s3",
            uptime_ms: uptime_ms(),
            heap_internal_free: Some(heap_caps_get_free_size(internal) as u64),
            heap_internal_min_free: Some(heap_caps_get_minimum_free_size(internal) as u64),
            psram_free: Some(heap_caps_get_free_size(MALLOC_CAP_SPIRAM) as u64),
            wifi_rssi: rssi,
            reset_reason: Some(reset_reason()),
            ip: IP.get().map(|ip| ip.to_string()),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    log::info!("mastomini-bots {} starting", env!("CARGO_PKG_VERSION"));

    let peripherals = Peripherals::take()?;
    let event_loop = EspSystemEventLoop::take()?;
    // Never auto-erase NVS: the store partition holds the bots' settings.
    let system_nvs = EspDefaultNvsPartition::take_with(false)?;
    // SAFETY: static NUL-terminated partition name.
    esp_idf_svc::sys::esp!(unsafe {
        esp_idf_svc::sys::nvs_flash_init_partition(nvs_kv::PARTITION.as_ptr())
    })?;
    let partition = EspNvsPartition::<NvsCustom>::take("store")?;
    let store = nvs_kv::NvsKv::open(partition)?;
    let svc = Service::open(store, mastobots::bots::all())?;
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

    let wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, event_loop.clone(), Some(system_nvs))?,
        event_loop,
    )?;
    let mut net = net::Net::new(wifi);
    let (ssid, password) = match (
        option_env!("MASTOMINI_WIFI_SSID"),
        option_env!("MASTOMINI_WIFI_PASSWORD"),
    ) {
        (Some(s), p) => (s, p.unwrap_or("")),
        _ => {
            log::error!(
                "No Wi-Fi in this build: set MASTOMINI_WIFI_SSID/PASSWORD in .env and rebuild"
            );
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
    };
    let ip = net.join(ssid, password);
    let _ = IP.set(ip);
    log::info!("Wi-Fi up: {ip}");
    // Answer promptly: modem sleep delays an idle board by 100-300 ms. The
    // board is mains powered.
    // SAFETY: Wi-Fi is started.
    unsafe { esp_idf_svc::sys::esp_wifi_set_ps(esp_idf_svc::sys::wifi_ps_type_t_WIFI_PS_NONE) };

    let mut time_config = SntpConf::default();
    if let Some(server) = option_env!("MASTOMINI_NTP_SERVER") {
        time_config.servers.fill(server);
    }
    let _sntp = EspSntp::new(&time_config)?;

    let mut mdns = EspMdns::take()?;
    mdns.set_hostname(HOSTNAME)?;
    mdns.set_instance_name("mastomini-bots")?;
    mdns.add_service(
        Some("mastomini-bots"),
        "_https",
        "_tcp",
        443,
        &[("path", "/app/")],
    )?;
    mdns.add_service(
        Some("mastomini-bots (plain)"),
        "_http",
        "_tcp",
        80,
        &[("path", "/app/")],
    )?;

    let ctx = Arc::new(Ctx {
        platform,
        tls: Some(Tls {
            ca_der: CA_DER.to_vec(),
            ca_pem: CA_PEM.to_string(),
            info: serde_json::from_str(CERT_INFO)?,
        }),
    });
    // Sockets: (4 + 2) + (3 + 2) = 11 of CONFIG_LWIP_MAX_SOCKETS (16),
    // leaving room for the bots' own connections. One admin, a few tabs.
    server::init_tls(SERVER_CERT, SERVER_KEY)?;
    for (secure, port, ctrl_port, sockets) in [(true, 443, 32769, 4), (false, 80, 32768, 3)] {
        server::start(
            &server::Settings {
                port,
                ctrl_port,
                sockets,
                backlog: 5,
            },
            server::Listener {
                secure,
                shared: Arc::clone(&shared),
                ctx: Arc::clone(&ctx),
            },
        )?;
    }

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
        std::thread::Builder::new()
            .stack_size(16 * 1024)
            .spawn(move || {
                mastobots::runner::run_forever(
                    shared,
                    Box::new(client::EspClient::new(CA_PEM)),
                    now_ms,
                )
            })?;
    }
    ThreadSpawnConfiguration::default().set()?;
    log::info!("Ready at https://{HOSTNAME}.local/app/ (https://{ip}/app/)");

    let mut ticks: u32 = 0;
    loop {
        std::thread::sleep(Duration::from_secs(30));
        ticks = ticks.wrapping_add(1);
        net.keep_connected();
        if ticks % 10 == 0 {
            let p = platform();
            log::info!(
                "heap internal free={:?} min={:?} psram free={:?} clock={}",
                p.heap_internal_free,
                p.heap_internal_min_free,
                p.psram_free,
                if now_ms().is_some() { "valid" } else { "unset" }
            );
        }
    }
}
