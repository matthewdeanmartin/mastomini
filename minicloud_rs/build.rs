use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=src/plugins");
    println!("cargo:rerun-if-env-changed=MINICLOUD_PLUGINS");
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let selected = env::var("MINICLOUD_PLUGINS").ok();
    let mut paths: Vec<_> = fs::read_dir(root.join("src/plugins"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|s| s == "rs"))
        .collect();
    paths.sort();
    let mut modules = String::new();
    let mut entries = String::new();
    for path in paths {
        let name = path.file_stem().unwrap().to_str().unwrap();
        assert!(name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
        let source = fs::read_to_string(&path).unwrap();
        if source
            .lines()
            .filter_map(|l| l.strip_prefix("// minicloud-feature: "))
            .any(|f| {
                env::var_os(format!(
                    "CARGO_FEATURE_{}",
                    f.to_uppercase().replace('-', "_")
                ))
                .is_none()
            })
        {
            continue;
        }
        if selected
            .as_ref()
            .is_some_and(|s| !s.split(',').any(|n| n == name))
        {
            continue;
        }
        modules += &format!("#[path = {:?}] mod plugin_{name};\n", path);
        entries += &format!("Box::new(plugin_{name}::Plugin),");
    }
    fs::write(
        out.join("plugins.rs"),
        format!(
            "{modules}\npub fn plugins() -> Vec<Box<dyn crate::Plugin>> {{ vec![{entries}] }}\n"
        ),
    )
    .unwrap();

    // Angular production assets are gzip-compressed at build time, kept in flash.
    println!("cargo:rerun-if-changed=web");
    let mut assets = String::from("pub static ASSETS: &[(&str, &[u8])] = &[\n");
    if let Ok(files) = fs::read_dir(root.join("web")) {
        for file in files {
            let path = file.unwrap().path();
            if path.extension().is_some_and(|s| s == "gz") {
                let filename = path
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .trim_end_matches(".gz");
                assets += &format!("({filename:?}, include_bytes!({path:?})),\n");
            }
        }
    }
    assets += "];\n";
    fs::write(out.join("assets.rs"), assets).unwrap();
    for key in [
        "MINICLOUD_WIFI_SSID",
        "MINICLOUD_WIFI_PASSWORD",
        "MINICLOUD_ADMIN_TOKEN",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
        if let Ok(value) = env::var(key) {
            println!("cargo:rustc-env={key}={value}");
        }
    }
    #[cfg(feature = "esp32")]
    embuild::espidf::sysenv::output();
}
