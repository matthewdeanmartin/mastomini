//! Build identity for every build; firmware settings for the board.
//!
//! Wi-Fi credentials come from `MASTOMINI_WIFI_SSID` / `MASTOMINI_WIFI_PASSWORD`
//! in the environment, or else from the first gitignored `.env` that defines
//! them: this crate, the repository root, then `../mastomini_rs/.env` (the
//! bots board lives on the same household network). Values are never
//! printed.

use std::path::{Path, PathBuf};

const CREDENTIAL_FILES: [&str; 3] = [".env", "../.env", "../mastomini_rs/.env"];

const SETTINGS: [(&str, &[&str]); 4] = [
    ("MASTOMINI_WIFI_SSID", &["MASTOMINI_WIFI_SSID", "WIFI_SSID"]),
    (
        "MASTOMINI_WIFI_PASSWORD",
        &["MASTOMINI_WIFI_PASSWORD", "WIFI_PASSWORD"],
    ),
    ("MASTOMINI_BOTS_HOSTNAME", &["MASTOMINI_BOTS_HOSTNAME"]),
    (
        "MASTOMINI_NTP_SERVER",
        &["MASTOMINI_NTP_SERVER", "NTP_SERVER"],
    ),
];

/// `KEY = "value"` / `KEY=value` lines: `#` comments, optional `export`,
/// single or double quotes.
fn parse(text: &str, want: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if line.starts_with('#') || key.trim() != want {
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

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-env-changed=MASTOMINI_TRACE_TIMING");
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));

    // `GET /api/v1/version`: which sources this binary came from.
    let commit = git(&root, &["rev-parse", "--short=12", "HEAD"]).unwrap_or_default();
    let dirty = git(
        &root,
        &["status", "--porcelain", "--untracked-files=no", "--", "."],
    )
    .map(|s| if s.is_empty() { "0" } else { "1" })
    .unwrap_or("");
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    println!("cargo:rustc-env=MASTOBOTS_COMMIT={commit}");
    println!("cargo:rustc-env=MASTOBOTS_DIRTY={dirty}");
    println!("cargo:rustc-env=MASTOBOTS_BUILT_MS={ms}");

    if std::env::var_os("CARGO_FEATURE_ESP32").is_none() {
        return;
    }
    #[cfg(feature = "esp32")]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("espidf") {
        embuild::espidf::sysenv::output();
    }

    // The firmware embeds the admin site's certificate (scripts/certs.sh).
    for name in [
        "server.crt",
        "server.key",
        "household-ca.crt",
        "household-ca.der",
        "certificate.json",
    ] {
        let path = root.join("certs").join(name);
        assert!(
            path.exists(),
            "{} is missing: run make certs (README.md)",
            path.display()
        );
        println!("cargo:rerun-if-changed={}", path.display());
    }
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
