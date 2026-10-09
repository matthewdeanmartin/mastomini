//! Minicloud on a PC: miniframework's connection loop (the one the C6
//! runs) for HTTP, plus the MQTT broker and worker on their own thread.
use minicloud::{
    broker::Broker,
    http::{self, Minicloud},
    Cloud,
};
use miniframework::desktop::{serve, DesktopPlatform};
use miniframework::Site;
use std::{
    env,
    fs::OpenOptions,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env::var("MINICLOUD_DATA").unwrap_or_else(|_| "data".into()));
    std::fs::create_dir_all(&root)?;
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("writer.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)?;
    let token =
        env::var("MINICLOUD_ADMIN_TOKEN").unwrap_or_else(|_| "local-prototype-token".into());
    let bind = env::var("MINICLOUD_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    if bind != "127.0.0.1"
        && bind != "localhost"
        && (token == "local-prototype-token" || token.len() < 7)
    {
        return Err("LAN binding requires your own MINICLOUD_ADMIN_TOKEN (7+ characters)".into());
    }
    let http_port = env::var("MINICLOUD_PORT").unwrap_or_else(|_| "8090".into());
    let mqtt_port = env::var("MINICLOUD_MQTT_PORT").unwrap_or_else(|_| "1883".into());
    let shared = Arc::new(Mutex::new(Cloud::open(&root, token)?));
    let address = format!("{bind}:{http_port}");
    let mut broker = Broker::bind(&format!("{bind}:{mqtt_port}"))?;
    let workers = shared.clone();
    thread::Builder::new()
        .name("mqtt-worker".into())
        .stack_size(64 * 1024)
        .spawn(move || loop {
            broker.step(&workers);
            if let Err(error) = workers.lock().unwrap().work_once() {
                eprintln!("worker: {error}");
            }
            thread::sleep(Duration::from_millis(20));
        })?;
    println!("Minicloud HTTP http://{bind}:{http_port}/  MQTT {bind}:{mqtt_port}");
    println!(
        "Public screen: /  Management: /admin  Data: {}",
        root.display()
    );
    miniframework::desktop::init_logging();
    let mut config = http::config(&address, "desktop");
    // A PC has memory for large JSON listings.
    config.response_limit = 1024 * 1024;
    let site = Site::new(config, Minicloud { shared }, DesktopPlatform);
    serve(&site, &address, || {})?;
    drop(lock);
    Ok(())
}
