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

/// What goes into the build fingerprint, relative to the repository root.
/// `scripts/firmware-version.py` has the same list and rules; a client test
/// checks that both agree. Test-only files are left out so that editing a
/// test does not make the board look out of date.
const FINGERPRINT_INPUTS: [&str; 20] = [
    "mastomini_rs/Cargo.toml",
    "mastomini_rs/Cargo.lock",
    "mastomini_rs/build.rs",
    "mastomini_rs/sdkconfig.defaults",
    "mastomini_rs/partitions.csv",
    "mastomini_rs/components_esp32s3.lock",
    "mastomini_rs/src",
    "mastomini_rs/assets",
    "mastomini_rs/scripts/bundle-web.mjs",
    "mastomini_rs/certs/mastomini.crt",
    "mastomini_rs/certs/household-ca.der",
    "mastomini_rs/certs/certificate.json",
    "mastomini_ui/src",
    "mastomini_ui/public",
    "mastomini_ui/package.json",
    "mastomini_ui/package-lock.json",
    "mastomini_ui/angular.json",
    "mastomini_ui/tsconfig.json",
    "mastomini_ui/tsconfig.app.json",
    "mastomini_ui/proxy.conf.mjs",
];

fn test_only(name: &str) -> bool {
    matches!(name, "tests" | "tests.rs" | "testkit.rs") || name.ends_with(".spec.ts")
}

/// Files under `path` (itself, if a file), as repository-relative paths
/// with `/` separators.
fn collect(repo: &Path, path: &Path, out: &mut Vec<String>) {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if test_only(name) {
        return;
    }
    if path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                collect(repo, &entry.path(), out);
            }
        }
    } else if path.is_file() {
        let relative = path.strip_prefix(repo).unwrap_or(path);
        let parts: Vec<String> = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        out.push(parts.join("/"));
    }
}

/// SHA-256 over every input's path, length and bytes, in sorted path order;
/// a missing input counts as its path alone. First 12 hex digits.
fn fingerprint(repo: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut files = Vec::new();
    let mut missing = Vec::new();
    for input in FINGERPRINT_INPUTS {
        let path = repo.join(input);
        if path.exists() {
            collect(repo, &path, &mut files);
        } else {
            missing.push(input.to_string());
        }
    }
    files.sort();
    let mut hash = Sha256::new();
    for file in &files {
        let bytes = std::fs::read(repo.join(file)).unwrap_or_default();
        hash.update(file.as_bytes());
        hash.update(b"\0");
        hash.update(bytes.len().to_string().as_bytes());
        hash.update(b"\0");
        hash.update(&bytes);
    }
    missing.sort();
    for input in &missing {
        hash.update(b"missing\0");
        hash.update(input.as_bytes());
        hash.update(b"\0");
    }
    hash.finalize()
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `MASTOMINI_BUILD_*` for `/api/mastomini/v1/version`: which sources this
/// binary was built from, so anyone can tell whether a board needs flashing.
fn build_identity(root: &Path) {
    let repo = root.parent().unwrap_or(root);
    for input in FINGERPRINT_INPUTS {
        println!("cargo:rerun-if-changed={}", repo.join(input).display());
    }
    // A commit moves HEAD (or the branch it points to) without touching any
    // input: refresh the commit and dirty flag. Not .git/index: `git status`
    // rewrites it, which would rebuild the crate for nothing.
    println!(
        "cargo:rerun-if-changed={}",
        repo.join(".git/HEAD").display()
    );
    if let Some(branch) = git(repo, &["symbolic-ref", "-q", "HEAD"]) {
        println!(
            "cargo:rerun-if-changed={}",
            repo.join(".git").join(branch).display()
        );
    }
    println!(
        "cargo:rustc-env=MASTOMINI_BUILD_FINGERPRINT={}",
        fingerprint(repo)
    );
    let commit = git(repo, &["rev-parse", "--short=12", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=MASTOMINI_BUILD_COMMIT={commit}");
    let dirty = git(repo, &["status", "--porcelain", "--untracked-files=no"])
        .map(|s| if s.is_empty() { "0" } else { "1" })
        .unwrap_or("");
    println!("cargo:rustc-env=MASTOMINI_BUILD_DIRTY={dirty}");
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    println!("cargo:rustc-env=MASTOMINI_BUILD_MS={ms}");
    let profile = std::env::var("PROFILE").unwrap_or_default();
    println!("cargo:rustc-env=MASTOMINI_BUILD_PROFILE={profile}");
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Development only: boot into the setup network even with saved Wi-Fi.
    println!("cargo:rerun-if-env-changed=MASTOMINI_FORCE_SETUP");
    println!("cargo:rerun-if-env-changed=MASTOMINI_TRACE_TIMING");
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    build_identity(&root);
    if std::env::var_os("CARGO_FEATURE_ESP32").is_none() {
        return;
    }
    #[cfg(feature = "esp32")]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("espidf") {
        embuild::espidf::sysenv::output();
    }

    // The firmware embeds the HTTPS certificate (src/bin/esp32.rs).
    for name in [
        "mastomini.crt",
        "mastomini.key",
        "household-ca.der",
        "certificate.json",
    ] {
        let path = root.join("certs").join(name);
        assert!(
            path.exists(),
            "{} is missing: run make certs (docs/security/https.md)",
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
