//! C6 adapter: internal SPIFFS, HTTP, small MQTT broker and strip LCD renderer.
//! Never automatically formats flash, never requires an SD card.
#[cfg(not(target_os = "espidf"))]
compile_error!("use make firmware to build for riscv32imac-esp-espidf");
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{gpio::PinDriver, peripherals::Peripherals},
    mdns::EspMdns,
    nvs::EspDefaultNvsPartition,
    sys,
    wifi::{AuthMethod, ClientConfiguration, Configuration, EspWifi},
};
use minicloud::{broker::Broker, http, Cloud, TextSize};
use std::{
    ffi::CString,
    net::TcpListener,
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn main() {
    if let Err(error) = run() {
        loop {
            eprintln!("Minicloud startup failed: {error}");
            thread::sleep(Duration::from_secs(5));
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    let token = option_env!("MINICLOUD_ADMIN_TOKEN")
        .filter(|s| s.len() >= 24)
        .ok_or("set a 24+ character MINICLOUD_ADMIN_TOKEN before building firmware")?;
    let ssid = option_env!("MINICLOUD_WIFI_SSID").ok_or("MINICLOUD_WIFI_SSID missing")?;
    let password =
        option_env!("MINICLOUD_WIFI_PASSWORD").ok_or("MINICLOUD_WIFI_PASSWORD missing")?;
    let peripherals = Peripherals::take()?;
    let boot = PinDriver::input(peripherals.pins.gpio9, esp_idf_svc::hal::gpio::Pull::Up)?;
    eprintln!("Minicloud startup: LCD");
    // SAFETY: display exclusively owns documented GPIO/SPI peripherals. The
    // board application never assigns these pins to another Rust peripheral.
    sys::esp!(unsafe { sys::display_init() })?;
    // SAFETY: immutable driver metadata; the C string has static lifetime.
    unsafe {
        eprintln!(
            "Minicloud LCD driver: {} {}x{}",
            std::ffi::CStr::from_ptr(sys::minicloud_display_driver()).to_string_lossy(),
            sys::minicloud_display_width(),
            sys::minicloud_display_height()
        );
    }
    eprintln!("Minicloud startup: SPIFFS");
    // SAFETY: static C strings, one application writer, mounted for process
    // lifetime. No SD use or automatic formatting. Use the C API because the
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
    eprintln!("Minicloud startup: Wi-Fi and network stack");
    let mut wifi = EspWifi::new(
        peripherals.modem,
        EspSystemEventLoop::take()?,
        Some(EspDefaultNvsPartition::take()?),
    )?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid.try_into().map_err(|_| "SSID too long")?,
        password: password.try_into().map_err(|_| "Wi-Fi password too long")?,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    }))?;
    wifi.start()?;
    eprintln!("Minicloud startup: HTTP, MQTT, SNTP and mDNS");
    // EspWifi initializes esp-netif/lwIP. Bind sockets after that initialization.
    let listener = TcpListener::bind("0.0.0.0:80")?;
    let http_shared = shared.clone();
    thread::Builder::new()
        .name("minicloud-http".into())
        .stack_size(32768)
        .spawn(move || {
            if let Err(error) = http::serve(listener, http_shared) {
                eprintln!("HTTP stopped: {error}");
            }
        })?;
    let mut broker = Broker::bind("0.0.0.0:1883")?;
    let _sntp = esp_idf_svc::sntp::EspSntp::new_default()?;
    let mut _mdns = EspMdns::take()?;
    _mdns.set_hostname("minicloud")?;
    _mdns.set_instance_name("Minicloud household cloud")?;
    _mdns.add_service(Some("Minicloud"), "_http", "_tcp", 80, &[("path", "/")])?;
    _mdns.add_service(Some("Minicloud MQTT"), "_mqtt", "_tcp", 1883, &[])?;
    eprintln!("Minicloud services ready: http://minicloud.local/ MQTT :1883; joining Wi-Fi");

    let mut joining = false;
    let mut attempt = Instant::now() - Duration::from_secs(5);
    let mut render = String::new();
    let mut pressed = false;
    let mut debounce = Instant::now();
    let mut reported_ip = String::new();
    loop {
        let connected = wifi.is_connected().unwrap_or(false);
        let ip = wifi
            .sta_netif()
            .get_ip_info()
            .ok()
            .map(|i| i.ip.to_string())
            .unwrap_or_default();
        if connected && ip != "0.0.0.0" {
            joining = false;
            if ip != reported_ip {
                eprintln!("Minicloud network ready: http://{ip}/ http://minicloud.local/");
                reported_ip = ip.clone();
            }
        } else if joining && attempt.elapsed() > Duration::from_secs(60) {
            let _ = wifi.disconnect();
            joining = false;
            attempt = Instant::now();
        } else if !joining && attempt.elapsed() >= Duration::from_secs(5) {
            if wifi.connect().is_ok() {
                joining = true;
            }
            attempt = Instant::now();
        }
        broker.step(&shared);
        let mut cloud = shared.lock().unwrap();
        if let Err(error) = cloud.work_once() {
            eprintln!("worker: {error}");
        }
        let low = boot.is_low();
        if low && !pressed && debounce.elapsed() >= Duration::from_millis(150) {
            cloud.advance();
            debounce = Instant::now();
        }
        pressed = low;
        let notice = cloud.current();
        let signature = format!(
            "{}:{}:{}:{}",
            cloud.state.revision,
            notice.as_ref().map(|n| n.identity()).unwrap_or_default(),
            ip,
            cloud.screen_page()
        );
        if signature != render {
            render = signature;
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
                            b.bucket == i.bucket
                                && b.key == i.key
                                && b.mime == "application/x-rgb565"
                        })
                    })
                    .map(|b| {
                        cloud
                            .store
                            .blob_path(&b.hash)
                            .to_string_lossy()
                            .into_owned()
                    })
                    .unwrap_or_default();
                {
                    let pages = minicloud::layout::pages(&n.text, &n.size);
                    let page = cloud.screen_page().min(pages.len() - 1);
                    let recipient = if pages.len() > 1 {
                        format!("{} ({}/{})", n.recipient, page + 1, pages.len())
                    } else {
                        n.recipient
                    };
                    (n.source, recipient, pages[page].clone(), scale, image)
                }
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
                eprintln!("LCD error {result}");
            }
        } else {
            drop(cloud);
        }
        thread::sleep(Duration::from_millis(20));
    }
}
