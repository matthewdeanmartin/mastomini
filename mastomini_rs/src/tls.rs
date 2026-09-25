//! What the server can say about its HTTPS setup: the household CA it serves
//! at `/ca` and the certificate it presents (docs/security/https.md).
//!
//! The binaries do the TLS itself (tiny_http + rustls on the desktop,
//! esp-tls on the board) from the files `scripts/certs.sh` writes to
//! `certs/`. This is only the public side of that material, so no private
//! key ever reaches the API.

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Files in `certs/` (see `scripts/certs.sh`).
pub const CERT_FILE: &str = "mastomini.crt";
pub const KEY_FILE: &str = "mastomini.key";
pub const CA_DER_FILE: &str = "household-ca.der";
pub const INFO_FILE: &str = "certificate.json";

/// DER of the name-constraints extension's OID (2.5.29.30).
const NAME_CONSTRAINTS_OID: [u8; 5] = [0x06, 0x03, 0x55, 0x1d, 0x1e];

#[derive(Debug, Clone)]
pub struct Tls {
    /// The household CA's public certificate, DER.
    pub ca_der: Vec<u8>,
    /// `AB:CD:…`, as `openssl x509 -fingerprint -sha256` and devices show it.
    pub ca_fingerprint: String,
    pub ca_name: String,
    /// What the server certificate is valid for (DNS names and IPs).
    pub names: Vec<String>,
    /// When the server certificate expires (ISO 8601).
    pub not_after: String,
    /// The HTTPS address to send people to, e.g. `https://mastomini.local`.
    pub https_url: String,
}

#[derive(Deserialize)]
struct Info {
    names: Vec<String>,
    not_after: String,
    ca_name: String,
}

impl Tls {
    /// From `household-ca.der` and `certificate.json`.
    pub fn new(ca_der: Vec<u8>, info_json: &str, https_url: &str) -> Result<Tls, String> {
        let info: Info = serde_json::from_str(info_json)
            .map_err(|e| format!("{INFO_FILE} is not valid ({e}); run make certs"))?;
        if ca_der.first() != Some(&0x30) {
            return Err(format!(
                "{CA_DER_FILE} is not a DER certificate; run make certs"
            ));
        }
        let ca_fingerprint = Sha256::digest(&ca_der)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        Ok(Tls {
            ca_der,
            ca_fingerprint,
            ca_name: info.ca_name,
            names: info.names,
            not_after: info.not_after,
            https_url: https_url.trim_end_matches('/').to_string(),
        })
    }

    /// The CA in PEM form, for Linux and command-line tools.
    pub fn ca_pem(&self) -> String {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&self.ca_der);
        let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
        for line in b64.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(line).unwrap_or(""));
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");
        pem
    }

    /// Does the CA limit itself to household names and private addresses?
    /// (`scripts/certs.sh` adds name constraints; a CA brought from
    /// elsewhere may not have them.) A byte search for the extension's OID:
    /// it cannot appear by accident in a certificate this small.
    pub fn name_constrained(&self) -> bool {
        self.ca_der
            .windows(NAME_CONSTRAINTS_OID.len())
            .any(|w| w == NAME_CONSTRAINTS_OID)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fake CA: DER-looking bytes that contain the name-constraints OID.
    pub fn sample() -> Tls {
        let mut der = vec![0x30, 0x82, 0x01, 0x00];
        der.extend_from_slice(&NAME_CONSTRAINTS_OID);
        der.extend_from_slice(&[0xAA; 60]);
        Tls::new(
            der,
            r#"{"names":["mastomini.local","localhost","127.0.0.1"],"not_after":"2028-12-23T00:05:49Z","ca_name":"mastomini Household CA 2026-09-25"}"#,
            "https://mastomini.local/",
        )
        .unwrap()
    }

    #[test]
    fn fingerprint_pem_and_constraints() {
        let tls = sample();
        assert_eq!(tls.https_url, "https://mastomini.local");
        assert_eq!(tls.ca_fingerprint.len(), 32 * 3 - 1);
        assert!(tls.ca_fingerprint.split(':').all(|h| h.len() == 2));
        let pem = tls.ca_pem();
        assert!(pem.starts_with("-----BEGIN CERTIFICATE-----\n"));
        assert!(pem.lines().all(|l| l.len() <= 64));
        assert!(tls.name_constrained());
        let plain = Tls::new(
            vec![0x30, 1, 2],
            r#"{"names":[],"not_after":"","ca_name":""}"#,
            "",
        )
        .unwrap();
        assert!(!plain.name_constrained());
    }

    #[test]
    fn refuses_what_is_not_certificate_material() {
        assert!(Tls::new(b"-----BEGIN".to_vec(), "{}", "").is_err());
        assert!(Tls::new(vec![0x30], "not json", "").is_err());
    }
}
