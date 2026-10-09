//! C6 adapter: internal SPIFFS, the strip LCD renderer and a small MQTT
//! broker, with HTTP served by miniframework. Never automatically formats
//! flash, never requires an SD card.
//!
//! Tasks: miniframework's connection loop has its own task; this main task
//! runs the device loop (MQTT broker, worker, BOOT button, LCD) every 20 ms.
//! Wi-Fi joins in the background, so the screen and button work without a
//! network. A failed startup is explained at http://<board>:8080/ once
//! Wi-Fi is up.
#[cfg(not(target_os = "espidf"))]
compile_error!("use make firmware to build for riscv32imac-esp-espidf");
use esp_idf_svc::{
    hal::{gpio::PinDriver, peripherals::Peripherals},
    sys,
};
use minicloud::{
    broker::Broker,
    http::{self, Minicloud},
    Cloud, TextSize,
};
use miniframework::{
    esp::{self, BoardConfig},
    status::SIGNALS,
    Site,
};
use std::{
    ffi::CString,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn main() {
    if let Err(error) = run() {
        esp::fail(&error.to_string());
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    esp::init();
    let token = option_env!("MINICLOUD_ADMIN_TOKEN")
        .filter(|s| s.len() >= 7)
        .ok_or("set a 7+ character MINICLOUD_ADMIN_TOKEN before building firmware")?;
    let ssid = option_env!("MINICLOUD_WIFI_SSID").ok_or("MINICLOUD_WIFI_SSID missing")?;
    let password =
        option_env!("MINICLOUD_WIFI_PASSWORD").ok_or("MINICLOUD_WIFI_PASSWORD missing")?;
    let peripherals = Peripherals::take()?;
    let boot = PinDriver::input(peripherals.pins.gpio9, esp_idf_svc::hal::gpio::Pull::Up)?;
    SIGNALS.step(8, "SD card and LCD");
    // SAFETY: this adapter exclusively owns GPIO4/5/6/7/14 and SPI2.
    // Put the SD card into SPI mode before the LCD sends traffic on the bus.
    sys::esp!(unsafe { sys::minicloud_bus_init() })?;
    let card_result = unsafe { sys::minicloud_card_mount() };
    if card_result != 0 {
        log::warn!("TF card unavailable ({card_result}); onboard storage remains default");
    }
    // SAFETY: display exclusively owns documented GPIO/SPI peripherals. The
    // board application never assigns these pins to another Rust peripheral.
    sys::esp!(unsafe { sys::display_init() })?;
    // SAFETY: immutable driver metadata; the C string has static lifetime.
    unsafe {
        log::info!(
            "Minicloud LCD driver: {} {}x{}",
            std::ffi::CStr::from_ptr(sys::minicloud_display_driver()).to_string_lossy(),
            sys::minicloud_display_width(),
            sys::minicloud_display_height()
        );
    }
    SIGNALS.step(9, "SPIFFS and cloud state");
    // SAFETY: static C strings, one application writer, mounted for process
    // lifetime. No automatic formatting. Use the C API because the
    // svc 0.52.1 MountedSpiffs destructor unregisters a path instead of a label
    // and panics during startup-error cleanup, hiding the original error.
    sys::esp!(unsafe {
        sys::esp_vfs_spiffs_register(&sys::esp_vfs_spiffs_conf_t {
            base_path: c"/storage".as_ptr(),
            partition_label: c"storage".as_ptr(),
            max_files: 6,
            format_if_mount_failed: false,
        })
    })?;
    let shared = Arc::new(Mutex::new(Cloud::open(
        Path::new("/storage"),
        token.into(),
    )?));

    let mut config = BoardConfig::c6(ssid, password, "minicloud");
    config.instance = "Minicloud household cloud";
    // HTTPS (and HTTP/2) builds: `make firmware TRANSPORT=https|http2`,
    // after `make certs`. No PSRAM: one TLS session (a handshake peaks at
    // ~44 KB; two sessions exhausted the heap). HTTP/2 needs only one.
    #[cfg(feature = "https")]
    {
        config.cert_pem = concat!(include_str!("../../certs/minicloud.crt"), "\0").as_bytes();
        config.key_pem = concat!(include_str!("../../certs/minicloud.key"), "\0").as_bytes();
        config.limits.tls_clients = 1;
        config.limits.handshakes = 1;
    }
    // The screen and button must work while Wi-Fi is down or still joining.
    config.wait_for_wifi = false;
    let mut board = esp::start(config, peripherals.modem)?;
    if let Some(mdns) = board.mdns() {
        if let Err(e) = mdns.add_service(Some("Minicloud MQTT"), "_mqtt", "_tcp", 1883, &[]) {
            log::warn!("mDNS MQTT service: {e}");
        }
    }
    SIGNALS.step(10, "MQTT broker");
    // The network stack exists now (esp::start), so sockets can bind.
    let mut broker = Broker::bind("0.0.0.0:1883")?;

    let mut site_config = http::config("minicloud.local", "esp32c6");
    // No PSRAM: listings are bounded (48 blobs, 24 notices, 12 jobs).
    site_config.response_limit = 32 * 1024;
    #[cfg(feature = "https")]
    {
        // /trust and /ca, so devices can install the household CA.
        site_config.ca_der = Some(include_bytes!("../../certs/household-ca.der"));
    }
    let site = Site::new(
        site_config,
        Minicloud {
            shared: shared.clone(),
        },
        board.platform(peripherals.temp_sensor),
    );
    log::info!("Minicloud services ready: http://minicloud.local/ MQTT :1883; joining Wi-Fi");

    let mut render = String::new();
    let mut pressed = false;
    let mut debounce = Instant::now();
    let mut reported_ip = String::new();
    let mut last = Instant::now();
    board.serve(site, move |_| {
        if last.elapsed() < Duration::from_millis(20) {
            return;
        }
        last = Instant::now();
        let ip = esp::station_ip().unwrap_or_default();
        if !ip.is_empty() && ip != reported_ip {
            log::info!("Minicloud network ready: http://{ip}/ http://minicloud.local/");
            reported_ip = ip.clone();
        }
        if let Err(error) = device_turn(
            &shared,
            &mut broker,
            boot.is_low(),
            &mut pressed,
            &mut debounce,
            &mut render,
            ip,
        ) {
            log::warn!("device loop: {error}");
        }
    })
}

/// One turn of the device loop: MQTT, one queued job, the BOOT button, and
/// a redraw when what the screen should show has changed.
fn device_turn(
    shared: &Arc<Mutex<Cloud>>,
    broker: &mut Broker,
    low: bool,
    pressed: &mut bool,
    debounce: &mut Instant,
    render: &mut String,
    ip: String,
) -> Result<(), Box<dyn std::error::Error>> {
    broker.step(shared);
    let mut cloud = shared.lock().unwrap();
    if let Err(error) = cloud.work_once() {
        log::warn!("worker: {error}");
    }
    if low && !*pressed && debounce.elapsed() >= Duration::from_millis(150) {
        cloud.advance();
        *debounce = Instant::now();
    }
    *pressed = low;
    let notice = cloud.current();
    let signature = format!(
        "{}:{}:{}:{}",
        cloud.state.revision,
        notice.as_ref().map(|n| n.identity()).unwrap_or_default(),
        ip,
        cloud.screen_page()
    );
    if signature == *render {
        return Ok(());
    }
    *render = signature;
    let (source, recipient, text, scale, image) = if let Some(n) = notice {
        let scale = match n.size {
            TextSize::Small => 1,
            TextSize::Medium => 2,
            TextSize::Large => 3,
        };
        let image = n
            .image
            .as_ref()
            .and_then(|i| {
                cloud.state.blobs.iter().find(|b| {
                    b.bucket == i.bucket && b.key == i.key && b.mime == "application/x-rgb565"
                })
            })
            .and_then(|b| {
                cloud
                    .store
                    .path_for(b)
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        let pages = minicloud::layout::pages(&n.text, &n.size);
        let page = cloud.screen_page().min(pages.len() - 1);
        let recipient = if pages.len() > 1 {
            format!("{} ({}/{})", n.recipient, page + 1, pages.len())
        } else {
            n.recipient
        };
        (n.source, recipient, pages[page].clone(), scale, image)
    } else {
        (
            "MINICLOUD".into(),
            ip,
            "All caught up.".into(),
            2,
            String::new(),
        )
    };
    drop(cloud);
    let source = CString::new(source.replace('\0', ""))?;
    let recipient = CString::new(recipient.replace('\0', ""))?;
    let text = CString::new(text.replace('\0', ""))?;
    let image = CString::new(image)?;
    // SAFETY: NUL-terminated strings live until this synchronous call
    // returns. Only the main loop accesses the display DMA strip.
    let result = unsafe {
        sys::minicloud_display_frame(
            source.as_ptr(),
            recipient.as_ptr(),
            text.as_ptr(),
            scale,
            image.as_ptr(),
        )
    };
    if result != sys::ESP_OK {
        log::warn!("LCD error {result}");
    }
    Ok(())
}
