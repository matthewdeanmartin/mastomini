//! Mastodon-style snowflake IDs and their compact store-key form.
//!
//! An id is `(unix_ms << 16) | seq`. No counter is persisted: at boot the service
//! feeds every stored id to [`IdGen::observe`], and new ids are
//! `max(now_id, last + 1)`, so a clock that steps backwards still yields
//! increasing ids (spec/02-storage.md "IDs").

/// Crockford base32 alphabet. Lexicographic order of encoded strings matches
/// numeric order because every id is encoded to the same width.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Width of an encoded u64: ceil(64 / 5).
pub const B32_LEN: usize = 13;

#[derive(Debug, Default, Clone)]
pub struct IdGen {
    last: u64,
}

impl IdGen {
    /// Record an id found in storage so later ids sort after it.
    pub fn observe(&mut self, id: u64) {
        self.last = self.last.max(id);
    }

    pub fn last(&self) -> u64 {
        self.last
    }

    /// Next id for wall-clock time `now_ms`.
    pub fn next(&mut self, now_ms: u64) -> u64 {
        let candidate = now_ms << 16;
        let id = candidate.max(self.last + 1);
        self.last = id;
        id
    }
}

/// Milliseconds encoded in an id.
pub fn millis(id: u64) -> u64 {
    id >> 16
}

/// Encode as 13 characters of Crockford base32.
pub fn b32(id: u64) -> [u8; B32_LEN] {
    let mut out = [b'0'; B32_LEN];
    let mut value = id;
    for slot in out.iter_mut().rev() {
        *slot = ALPHABET[(value & 31) as usize];
        value >>= 5;
    }
    out
}

pub fn b32_str(id: u64) -> String {
    // The alphabet is ASCII, so this cannot fail.
    String::from_utf8(b32(id).to_vec()).unwrap_or_default()
}

pub fn from_b32(text: &str) -> Option<u64> {
    if text.len() != B32_LEN {
        return None;
    }
    let mut value: u64 = 0;
    for byte in text.bytes() {
        let digit = ALPHABET.iter().position(|&c| c == byte)? as u64;
        value = value.checked_mul(32)?.checked_add(digit)?;
    }
    Some(value)
}

/// Parse an API id (decimal string). Junk and out-of-range values are `None`.
pub fn parse(text: &str) -> Option<u64> {
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b32_round_trips_and_preserves_order() {
        let ids = [0u64, 1, 31, 32, 1_700_000_000_000 << 16, u64::MAX];
        for pair in ids.windows(2) {
            assert!(b32(pair[0]) < b32(pair[1]));
        }
        for id in ids {
            assert_eq!(from_b32(&b32_str(id)), Some(id));
        }
    }

    #[test]
    fn ids_stay_monotonic_when_clock_steps_back() {
        let mut gen = IdGen::default();
        let a = gen.next(10_000);
        let b = gen.next(5_000);
        let c = gen.next(5_000);
        assert!(a < b && b < c);
    }

    #[test]
    fn observed_ids_are_respected() {
        let mut gen = IdGen::default();
        gen.observe(99 << 16);
        assert!(gen.next(1) > 99 << 16);
    }
}
