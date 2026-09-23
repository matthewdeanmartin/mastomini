//! Password verifiers, bearer tokens and PKCE (spec/05-auth-network-tls.md).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// PBKDF2 rounds for new verifiers. The stored round count lets this rise
/// later without invalidating existing passwords. Target on the board is
/// ~250 ms per check (to be measured in sprint 6).
pub const PASSWORD_ROUNDS: u32 = 10_000;
pub const PASSWORD_MIN: usize = 4;
pub const PASSWORD_MAX: usize = 128;

/// Failed logins before a username is locked, and for how long.
pub const LOCKOUT_FAILURES: u32 = 5;
pub const LOCKOUT_MS: u64 = 5 * 60 * 1000;

/// Authorization codes are single use and short lived.
pub const CODE_TTL_MS: u64 = 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verifier {
    pub salt: [u8; 16],
    pub rounds: u32,
    pub hash: [u8; 32],
}

impl Verifier {
    pub fn new(password: &str, rounds: u32) -> Verifier {
        let mut salt = [0; 16];
        fill_random(&mut salt);
        let hash = derive(password, &salt, rounds);
        Verifier { salt, rounds, hash }
    }

    pub fn verify(&self, password: &str) -> bool {
        ct_eq(&derive(password, &self.salt, self.rounds), &self.hash)
    }
}

fn derive(password: &str, salt: &[u8], rounds: u32) -> [u8; 32] {
    let mut out = [0; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, rounds.max(1), &mut out);
    out
}

/// Constant-time comparison of equal-length secrets.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn fill_random(buf: &mut [u8]) {
    // getrandom only fails when the OS has no entropy source; there is no
    // safe way to continue issuing credentials in that case.
    getrandom::getrandom(buf).expect("OS random number generator unavailable");
}

/// A fresh 256-bit secret, base64url encoded (43 characters).
pub fn random_secret() -> String {
    let mut bytes = [0; 32];
    fill_random(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn sha256(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

/// RFC 7636 S256: `BASE64URL(SHA256(verifier)) == challenge`.
pub fn pkce_matches(verifier: &str, challenge: &str) -> bool {
    let computed = URL_SAFE_NO_PAD.encode(sha256(verifier));
    ct_eq(computed.as_bytes(), challenge.as_bytes())
}

pub fn password_ok(password: &str) -> Result<(), &'static str> {
    if password.len() < PASSWORD_MIN {
        return Err("Password is too short (minimum is 4 characters)");
    }
    if password.len() > PASSWORD_MAX {
        return Err("Password is too long (maximum is 128 bytes)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_accepts_only_the_password() {
        let v = Verifier::new("hunter2", 10);
        assert!(v.verify("hunter2"));
        assert!(!v.verify("hunter3"));
    }

    #[test]
    fn pkce_rfc7636_example() {
        assert!(pkce_matches(
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        ));
        assert!(!pkce_matches(
            "wrong",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        ));
    }

    #[test]
    fn secrets_are_unique_and_url_safe() {
        let a = random_secret();
        assert_eq!(a.len(), 43);
        assert_ne!(a, random_secret());
        assert!(a
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    }
}
