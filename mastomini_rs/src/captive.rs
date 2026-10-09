//! Captive-portal routing for the board's setup network (spec/06): which
//! requests go to the setup pages. The DNS answer that points every name at
//! the board is miniframework's (`miniframework::captive`).

pub use miniframework::captive::AP_IP;

/// Should this request be sent to the setup pages (captive portal)?
pub fn captive(path: &str, joined: bool) -> Option<&'static str> {
    const OURS: [&str; 7] = [
        "/setup",
        "/api/",
        "/oauth/",
        "/avatars/",
        "/headers/",
        "/.well-known/",
        "/nodeinfo",
    ];
    if path == "/" {
        return (!joined).then_some("/setup/wifi");
    }
    (!OURS.iter().any(|p| path.starts_with(p))).then_some("/setup/wifi")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captive_redirects_foreign_paths_only() {
        assert_eq!(captive("/generate_204", false), Some("/setup/wifi"));
        assert_eq!(captive("/hotspot-detect.html", true), Some("/setup/wifi"));
        assert_eq!(captive("/", false), Some("/setup/wifi"));
        assert_eq!(
            captive("/", true),
            None,
            "after joining, / is the household setup"
        );
        assert_eq!(captive("/setup", false), None);
        assert_eq!(captive("/api/v1/instance", false), None);
    }
}
