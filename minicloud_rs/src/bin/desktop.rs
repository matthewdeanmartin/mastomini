use minicloud::{broker::Broker, http, Cloud};
use std::{
    env,
    fs::OpenOptions,
    net::TcpListener,
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
        && (token == "local-prototype-token" || token.len() < 24)
    {
        return Err("LAN binding requires your own MINICLOUD_ADMIN_TOKEN (24+ characters)".into());
    }
    let http_port = env::var("MINICLOUD_PORT").unwrap_or_else(|_| "8090".into());
    let mqtt_port = env::var("MINICLOUD_MQTT_PORT").unwrap_or_else(|_| "1883".into());
    let shared = Arc::new(Mutex::new(Cloud::open(&root, token)?));
    let listener = TcpListener::bind(format!("{bind}:{http_port}"))?;
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
    http::serve(listener, shared)?;
    drop(lock);
    Ok(())
}
