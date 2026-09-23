//! Firmware build settings. Desktop builds need none of this.
//!
//! Wi-Fi credentials come from `MASTOMINI_WIFI_SSID` / `MASTOMINI_WIFI_PASSWORD`
//! in the environment, or else from the first gitignored `.env` that defines
//! them (this crate, then the repository root). `WIFI_SSID` / `WIFI_PASSWORD`
//! are accepted as aliases. Values are never printed: build logs are shared
//! more freely than the files they came from.

use std::path::{Path, PathBuf};

const CREDENTIAL_FILES: [&str; 2] = [".env", "../.env"];

const SETTINGS: [(&str, &[&str]); 5] = [
    ("MASTOMINI_WIFI_SSID", &["MASTOMINI_WIFI_SSID", "WIFI_SSID"]),
    (
        "MASTOMINI_WIFI_PASSWORD",
        &["MASTOMINI_WIFI_PASSWORD", "WIFI_PASSWORD"],
    ),
    ("MASTOMINI_HOSTNAME", &["MASTOMINI_HOSTNAME"]),
    ("MASTOMINI_BASE_URL", &["MASTOMINI_BASE_URL"]),
    (
        "MASTOMINI_NTP_SERVER",
        &["MASTOMINI_NTP_SERVER", "NTP_SERVER"],
    ),
];

/// Reads `KEY = "value"` / `KEY=value` lines: `#` comments, optional
/// `export`, single or double quotes.
fn parse(text: &str, want: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != want {
            continue;
        }
        let value = value.trim();
        let quoted = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')));
        let value = match quoted {
            Some(v) => v.to_string(),
            None => value.split('#').next().unwrap_or("").trim().to_string(),
        };
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Development only: boot into the setup network even with saved Wi-Fi.
    println!("cargo:rerun-if-env-changed=MASTOMINI_FORCE_SETUP");
    if std::env::var_os("CARGO_FEATURE_ESP32").is_none() {
        return;
    }
    #[cfg(feature = "esp32")]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("espidf") {
        embuild::espidf::sysenv::output();
    }

    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    let sources: Vec<(PathBuf, String)> = CREDENTIAL_FILES
        .iter()
        .map(|relative| root.join(relative))
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            println!("cargo:rerun-if-changed={}", path.display());
            Some((path, text))
        })
        .collect();

    for (key, aliases) in SETTINGS {
        println!("cargo:rerun-if-env-changed={key}");
        if std::env::var_os(key).is_some() {
            continue;
        }
        let found = sources.iter().find_map(|(path, text)| {
            aliases
                .iter()
                .find_map(|name| parse(text, name))
                .map(|value| (path.as_path(), value))
        });
        if let Some((path, value)) = found {
            let shown: &Path = path.strip_prefix(&root).unwrap_or(path);
            println!("cargo:warning={key} taken from {}", shown.display());
            println!("cargo:rustc-env={key}={value}");
        }
    }
}
