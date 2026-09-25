//! ESP32-S3 firmware: the same API as the desktop build, persisted in NVS.
//!
//! Wi-Fi (with a setup network for first boot, see `net.rs`), SNTP, mDNS,
//! and the API on HTTPS port 443 and plain HTTP port 80, both serving
//! everything ("Easy mode", spec/05). The certificate comes from `certs/`
//! (`make certs`, docs/security/https.md); the CA's private key never does.

#[cfg(not(target_os = "espidf"))]
compile_error!(
    "build firmware with --target xtensa-esp32s3-espidf --no-default-features --features esp32"
);

use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{cpu::Core, peripherals::Peripherals, task::thread::ThreadSpawnConfiguration},
    http::{
        server::{Configuration as HttpConfiguration, EspHttpServer},
        Method,
    },
    io::{Read, Write},
    mdns::EspMdns,
    nvs::{EspDefaultNvsPartition, EspNvs, EspNvsPartition, NvsCustom},
    sntp::{EspSntp, SntpConf},
    sys::EspError,
    tls::X509,
    wifi::{BlockingWifi, EspWifi},
};
use mastomini::{
    api::{self, diag::Platform, Ctx},
    domain::{Config, Service},
    http::{Request, Response, FORWARDED_HEADERS, UPLOAD_BODY_LIMIT},
    tls::Tls,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[path = "esp32/net.rs"]
mod net;
#[path = "esp32/nvs_store.rs"]
mod nvs_store;
use net::Net;
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

type Shared = Arc<Mutex<Service<NvsStore>>>;

/// Before this instant the RTC has not been set by SNTP (2024-01-01).
const CLOCK_VALID_AFTER_MS: u128 = 1_704_067_200_000;

/// Request headers the API reads. esp-idf's server cannot enumerate headers.

fn now_ms() -> Option<u64> {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    (ms > CLOCK_VALID_AFTER_MS).then_some(ms as u64)
}

/// Milliseconds since boot (monotonic), for the manually set clock.
fn uptime_ms() -> u64 {
    // SAFETY: reads the monotonic system timer.
    (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1000) as u64
}

fn reset_reason(reason: esp_idf_svc::sys::esp_reset_reason_t) -> &'static str {
    use esp_idf_svc::sys::*;
    match reason {
        esp_reset_reason_t_ESP_RST_POWERON => "power on",
        esp_reset_reason_t_ESP_RST_EXT => "reset pin",
        esp_reset_reason_t_ESP_RST_SW => "software restart",
        esp_reset_reason_t_ESP_RST_PANIC => "crash",
        esp_reset_reason_t_ESP_RST_INT_WDT
        | esp_reset_reason_t_ESP_RST_TASK_WDT
        | esp_reset_reason_t_ESP_RST_WDT => "watchdog",
        esp_reset_reason_t_ESP_RST_DEEPSLEEP => "deep sleep",
        esp_reset_reason_t_ESP_RST_BROWNOUT => "brownout (power dipped)",
        esp_reset_reason_t_ESP_RST_USB => "USB",
        _ => "other",
    }
}

/// Board facts for `/diag`.
fn platform() -> Platform {
    use esp_idf_svc::sys::*;
    let internal = MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT;
    // SAFETY: heap and reset queries take no pointers; the AP record is a
    // local that the call fills in.
    unsafe {
        let mut ap: wifi_ap_record_t = core::mem::zeroed();
        let rssi = (esp_wifi_sta_get_ap_info(&mut ap) == ESP_OK).then_some(i32::from(ap.rssi));
        Platform {
            target: "esp32s3",
            uptime_ms: uptime_ms(),
            heap_internal_free: Some(heap_caps_get_free_size(internal) as u64),
            heap_internal_min_free: Some(heap_caps_get_minimum_free_size(internal) as u64),
            heap_internal_largest_block: Some(heap_caps_get_largest_free_block(internal) as u64),
            psram_free: Some(heap_caps_get_free_size(MALLOC_CAP_SPIRAM) as u64),
            psram_min_free: Some(heap_caps_get_minimum_free_size(MALLOC_CAP_SPIRAM) as u64),
            reset_reason: Some(reset_reason(esp_reset_reason())),
            wifi_rssi: rssi,
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    log::info!("mastomini {} starting", env!("CARGO_PKG_VERSION"));

    let peripherals = Peripherals::take()?;
    let event_loop = EspSystemEventLoop::take()?;
    // Never auto-erase NVS on a version/full error: it may contain data.
    let system_nvs = EspDefaultNvsPartition::take_with(false)?;
    // The custom-partition convenience constructor can erase on some init
    // errors, so initialize first and propagate every error (as nanacoin).
    // SAFETY: static NUL-terminated partition name.
    esp_idf_svc::sys::esp!(unsafe {
        esp_idf_svc::sys::nvs_flash_init_partition(nvs_store::PARTITION.as_ptr())
    })?;
    let partition = EspNvsPartition::<NvsCustom>::take("store")?;
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
    let shared = Arc::new(Mutex::new(service));

    let wifi_nvs = EspNvs::new(system_nvs.clone(), net::NAMESPACE, true)?;
    let wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, event_loop.clone(), Some(system_nvs))?,
        event_loop,
    )?;
    let mut net = Net::new(wifi, wifi_nvs);
    let force_setup = option_env!("MASTOMINI_FORCE_SETUP") == Some("1");
    let joined = match net.saved() {
        _ if force_setup => Err("setup forced by this build (MASTOMINI_FORCE_SETUP)".to_string()),
        Some((ssid, password, saved)) => net
            .join(&ssid, &password, saved)
            .map_err(|e| format!("Could not join {ssid} ({e}). Choose the network again.")),
        None => Err("No Wi-Fi network is saved yet.".to_string()),
    };
    let ip = match joined {
        Ok(ip) => {
            log::info!("Wi-Fi up: {ip}");
            Some(ip)
        }
        Err(reason) => {
            log::warn!("{reason}");
            net.start_setup(Some(reason))?;
            ThreadSpawnConfiguration {
                name: Some(c"mm-dns"),
                stack_size: 6 * 1024,
                pin_to_core: Some(Core::Core0),
                ..Default::default()
            }
            .set()?;
            std::thread::Builder::new().spawn(net::dns_responder)?;
            ThreadSpawnConfiguration::default().set()?;
            None
        }
    };
    let net = Arc::new(Mutex::new(net));

    let mut time_config = SntpConf::default();
    if let Some(server) = option_env!("MASTOMINI_NTP_SERVER") {
        time_config.servers.fill(server);
    }
    let _sntp = EspSntp::new(&time_config)?;

    // Two listeners, one service (spec/05 "Easy mode"). Sockets: each httpd
    // takes its open sockets plus a listening and a control socket, and both
    // together must fit CONFIG_LWIP_MAX_SOCKETS (16) with the captive DNS
    // socket: (5 + 2) + (4 + 2) + 1 = 14. Measured (spec/04 "Streaming"):
    // stalls come from the listen backlog at 8+ simultaneous connects, not
    // from the socket count, and least-recently-used purging keeps clients
    // moving. TLS sessions' buffers live in PSRAM (sdkconfig.defaults).
    let mut https = EspHttpServer::new(&HttpConfiguration {
        https_port: 443,
        // Distinct from the HTTP server's default 32768.
        ctrl_port: 32769,
        core: Some(Core::Core1),
        // serde_json rendering, plus the TLS handshake's frames.
        stack_size: 32 * 1024,
        max_open_sockets: 5,
        max_sessions: 5,
        lru_purge_enable: true,
        max_uri_handlers: 8,
        uri_match_wildcard: true,
        session_timeout: Duration::from_secs(10),
        server_certificate: Some(X509::pem_until_nul(SERVER_CERT.as_bytes())),
        private_key: Some(X509::pem_until_nul(SERVER_KEY.as_bytes())),
        ..Default::default()
    })?;
    register(&mut https, true, &shared, &ctx, &net)?;
    let mut http = EspHttpServer::new(&HttpConfiguration {
        http_port: 80,
        core: Some(Core::Core1),
        stack_size: 32 * 1024,
        max_open_sockets: 4,
        max_sessions: 4,
        // Evict the least recently used connection instead of refusing.
        lru_purge_enable: true,
        max_uri_handlers: 8,
        uri_match_wildcard: true,
        session_timeout: Duration::from_secs(10),
        ..Default::default()
    })?;
    register(&mut http, false, &shared, &ctx, &net)?;

    let mut mdns = EspMdns::take()?;
    mdns.set_hostname(HOSTNAME)?;
    mdns.set_instance_name("mastomini household Mastodon server")?;
    mdns.add_service(
        Some("mastomini"),
        "_https",
        "_tcp",
        443,
        &[("path", "/api/v1/instance")],
    )?;
    // Plain HTTP: where a new device finds /trust.
    mdns.add_service(
        Some("mastomini setup"),
        "_http",
        "_tcp",
        80,
        &[("path", "/trust")],
    )?;
    match ip {
        Some(ip) => log::info!("Ready at {base_url} (https://{ip}/ and http://{ip}/)"),
        None => log::info!(
            "Setup network {} is up: join it and open http://{}/",
            net::AP_SSID,
            net::AP_IP
        ),
    }

    let mut ticks: u32 = 0;
    loop {
        std::thread::sleep(Duration::from_secs(5));
        ticks = ticks.wrapping_add(1);
        let provisioned = shared.lock().unwrap().state.provisioned;
        {
            let mut net = net.lock().unwrap();
            net.maybe_close_setup(provisioned);
            if ticks % 6 == 0 {
                net.keep_connected();
            }
        }
        if ticks % 12 != 0 {
            continue;
        }
        let caps = esp_idf_svc::sys::MALLOC_CAP_INTERNAL | esp_idf_svc::sys::MALLOC_CAP_8BIT;
        // SAFETY: heap queries take no pointers.
        unsafe {
            log::info!(
                "heap internal free={} largest={} min={} psram free={} clock={}",
                esp_idf_svc::sys::heap_caps_get_free_size(caps),
                esp_idf_svc::sys::heap_caps_get_largest_free_block(caps),
                esp_idf_svc::sys::heap_caps_get_minimum_free_size(caps),
                esp_idf_svc::sys::heap_caps_get_free_size(esp_idf_svc::sys::MALLOC_CAP_SPIRAM),
                if now_ms().is_some() { "valid" } else { "unset" }
            );
        }
    }
}

/// Route every method on one listener to the API. `secure`: this is the
/// HTTPS listener.
fn register(
    server: &mut EspHttpServer<'static>,
    secure: bool,
    shared: &Shared,
    ctx: &Arc<Ctx>,
    net: &Arc<Mutex<Net>>,
) -> Result<(), EspError> {
    for (method, name) in [
        (Method::Get, "GET"),
        (Method::Head, "HEAD"),
        (Method::Post, "POST"),
        (Method::Put, "PUT"),
        (Method::Patch, "PATCH"),
        (Method::Delete, "DELETE"),
        (Method::Options, "OPTIONS"),
    ] {
        let shared = Arc::clone(shared);
        let ctx = Arc::clone(ctx);
        let net = Arc::clone(net);
        server.fn_handler::<esp_idf_svc::io::EspIOError, _>("/*", method, move |mut req| {
            let started = Instant::now();
            let mut body_ms = 0.0;
            let mut lock_ms = 0.0;
            let head = name == "HEAD";
            let mut request = Request::new(if head { "GET" } else { name }, req.uri());
            request.secure = secure;
            for header in FORWARDED_HEADERS {
                if let Some(value) = req.header(header) {
                    request
                        .headers
                        .push((header.to_string(), value.to_string()));
                }
            }
            let length = req
                .header("Content-Length")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            let reply = if length > UPLOAD_BODY_LIMIT {
                Response::error(413, "Request body is too large")
            } else {
                let mut body = vec![0; length];
                if req.read_exact(&mut body).is_err() {
                    Response::error(400, "Request body ended early")
                } else {
                    request.body = body;
                    body_ms = started.elapsed().as_secs_f64()*1000.0;
                    // Never hold the network and service locks together.
                    let (setup, joined) = {
                        let net = net.lock().unwrap();
                        (net.setup_mode, net.sta_ip.is_some())
                    };
                    if request.path == "/setup/wifi" {
                        let provisioned = shared.lock().unwrap().state.provisioned;
                        net.lock().unwrap().page(&request, provisioned)
                    } else if let Some(to) =
                        setup.then(|| net::captive(&request.path, joined)).flatten()
                    {
                        Response::redirect(&format!("http://{}{to}", net::AP_IP))
                    } else {
                        let waiting = Instant::now();
                        let mut service = shared.lock().unwrap();
                        lock_ms = waiting.elapsed().as_secs_f64()*1000.0;
                        api::handle(&mut service, &ctx, &request, now_ms())
                    }
                }
            };
            let reply = reply.with_timing(&format!("body;dur={body_ms:.3}, lock;dur={lock_ms:.3}"));
            let mut headers: Vec<(&str, &str)> = reply
                .headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            // Plain HTTP: free the socket promptly, the board has few. HTTPS:
            // keep it, because a new TLS handshake costs about a second of
            // CPU and a request on an open connection about 0.1 s (measured
            // on the board); least-recently-used purging reclaims idle ones.
            if !secure {
                headers.push(("Connection", "close"));
            }
            let sending = Instant::now();
            let mut response = req.into_response(reply.status, None, &headers)?;
            if !head {
                response.write_all(&reply.body)?;
            }
            if started.elapsed().as_millis() >= 250 || option_env!("MASTOMINI_TRACE_TIMING")==Some("1") {
                log::info!("timing method={} route={} status={} bytes={} body_ms={body_ms:.3} lock_ms={lock_ms:.3} send_ms={:.3} accepted_total_ms={:.3}",
                    name,mastomini::http::route_label(&request.path),reply.status,reply.body.len(),sending.elapsed().as_secs_f64()*1000.0,started.elapsed().as_secs_f64()*1000.0);
            }
            Ok(())
        })?;
    }
    Ok(())
}
