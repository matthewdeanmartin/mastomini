//! The one admin: a password set on first visit, and sessions (bearer
//! tokens for the admin app). Sessions live in RAM: after a restart the admin
//! signs in again, and nothing secret about them is ever written to flash.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PASSWORD_MIN: usize = 8;
pub const ROUNDS: u32 = 10_000;
const SESSION_MS: u64 = 30 * 24 * 60 * 60 * 1000;
const MAX_SESSIONS: usize = 8;
const LOCKOUT_FAILURES: u32 = 5;
const LOCKOUT_MS: u64 = 60_000;

/// Stored under the `admin` key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verifier {
    salt: String,
    hash: String,
    rounds: u32,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    getrandom::getrandom(&mut bytes).expect("random bytes");
    hex(&bytes)
}

fn derive(password: &str, salt: &str, rounds: u32) -> String {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), salt.as_bytes(), rounds, &mut out);
    hex(&out)
}

/// Constant-time comparison of equal-length strings.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

impl Verifier {
    pub fn new(password: &str) -> Result<Verifier, String> {
        if password.chars().count() < PASSWORD_MIN {
            return Err(format!("Use at least {PASSWORD_MIN} characters"));
        }
        let salt = random_hex(16);
        Ok(Verifier {
            hash: derive(password, &salt, ROUNDS),
            salt,
            rounds: ROUNDS,
        })
    }

    pub fn check(&self, password: &str) -> bool {
        same(&derive(password, &self.salt, self.rounds), &self.hash)
    }
}

#[derive(Debug, Default)]
pub struct Sessions {
    /// (sha256 of token, expires at)
    live: Vec<([u8; 32], u64)>,
    failures: u32,
    locked_until: u64,
}

impl Sessions {
    pub fn locked(&self, now_ms: u64) -> bool {
        now_ms < self.locked_until
    }

    pub fn failed(&mut self, now_ms: u64) {
        self.failures += 1;
        if self.failures >= LOCKOUT_FAILURES {
            self.failures = 0;
            self.locked_until = now_ms + LOCKOUT_MS;
        }
    }

    /// A new session token; the oldest goes when there are too many.
    pub fn start(&mut self, now_ms: u64) -> String {
        self.failures = 0;
        self.live.retain(|(_, until)| *until > now_ms);
        if self.live.len() >= MAX_SESSIONS {
            self.live.remove(0);
        }
        let token = random_hex(32);
        self.live
            .push((Sha256::digest(token.as_bytes()).into(), now_ms + SESSION_MS));
        token
    }

    pub fn valid(&self, token: &str, now_ms: u64) -> bool {
        let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        self.live
            .iter()
            .any(|(h, until)| *h == hash && *until > now_ms)
    }

    pub fn end(&mut self, token: &str) {
        let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        self.live.retain(|(h, _)| *h != hash);
    }

    pub fn end_all(&mut self) {
        self.live.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_and_sessions() {
        assert!(Verifier::new("short").is_err());
        let v = Verifier::new("correct horse").unwrap();
        assert!(v.check("correct horse"));
        assert!(!v.check("correct horsf"));
        let mut s = Sessions::default();
        let t = s.start(0);
        assert!(s.valid(&t, 1));
        assert!(!s.valid(&t, SESSION_MS + 1));
        assert!(!s.valid("nope", 1));
        s.end(&t);
        assert!(!s.valid(&t, 1));
        for _ in 0..LOCKOUT_FAILURES {
            s.failed(10);
        }
        assert!(s.locked(11) && !s.locked(10 + LOCKOUT_MS));
    }
}
