//! Durable key/value store with NVS semantics (spec/02-storage.md).
//!
//! Every `set`/`erase` is atomic per key and durable before it returns. There
//! are no multi-key transactions: the domain orders its writes so that each
//! user action has a single commit point and boot repairs the rest.

#[cfg(feature = "desktop")]
pub mod file;
pub mod mem;

use core::fmt;

/// NVS limits key names to 15 characters.
pub const KEY_MAX: usize = 15;

/// Size of one NVS entry. Used to estimate flash usage the way NVS accounts it.
pub const ENTRY_SIZE: usize = 32;

/// Namespaces act as tables and are enumerated independently at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Ns {
    Cfg = 0,
    Acct = 1,
    App = 2,
    Tok = 3,
    Stat = 4,
    Hist = 5,
    Rx = 6,
    Rel = 7,
    /// Account moderation state and reports.
    Mod = 8,
    Coll = 9,
    /// Member key pairs and per-device seals for direct messages.
    Key = 10,
}

impl Ns {
    pub const ALL: [Ns; 11] = [
        Ns::Cfg,
        Ns::Acct,
        Ns::App,
        Ns::Tok,
        Ns::Stat,
        Ns::Hist,
        Ns::Rx,
        Ns::Rel,
        Ns::Mod,
        Ns::Coll,
        Ns::Key,
    ];

    /// NVS namespace name (at most 15 characters).
    pub fn name(self) -> &'static str {
        match self {
            Ns::Cfg => "mm_cfg",
            Ns::Acct => "mm_acct",
            Ns::App => "mm_app",
            Ns::Tok => "mm_tok",
            Ns::Stat => "mm_stat",
            Ns::Hist => "mm_hist",
            Ns::Rx => "mm_rx",
            Ns::Rel => "mm_rel",
            Ns::Mod => "mm_mod",
            Ns::Coll => "mm_coll",
            Ns::Key => "mm_key",
        }
    }

    pub fn from_u8(value: u8) -> Option<Ns> {
        Ns::ALL.get(value as usize).copied()
    }
}

/// A short printable-ASCII key, stored inline.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    len: u8,
    buf: [u8; KEY_MAX],
}

impl Key {
    pub fn new(text: &str) -> Result<Key, StoreError> {
        let bytes = text.as_bytes();
        if bytes.is_empty() || bytes.len() > KEY_MAX || !bytes.iter().all(|b| b.is_ascii_graphic())
        {
            return Err(StoreError::BadKey);
        }
        let mut buf = [0; KEY_MAX];
        buf[..bytes.len()].copy_from_slice(bytes);
        Ok(Key {
            len: bytes.len() as u8,
            buf,
        })
    }

    /// Build a key from a one-letter prefix and ASCII parts.
    pub fn from_parts(parts: &[&[u8]]) -> Result<Key, StoreError> {
        let mut buf = [0; KEY_MAX];
        let mut len = 0;
        for part in parts {
            let end = len + part.len();
            if end > KEY_MAX {
                return Err(StoreError::BadKey);
            }
            buf[len..end].copy_from_slice(part);
            len = end;
        }
        let key = Key {
            len: len as u8,
            buf,
        };
        Key::new(key.as_str())
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len as usize]).unwrap_or("")
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({})", self.as_str())
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    BadKey,
    ValueTooLarge,
    /// The partition has no room left even after eviction.
    Full,
    /// I/O failure. The service latches unavailable until restart.
    Io(String),
    /// Stored data failed validation.
    Corrupt(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::BadKey => f.write_str("invalid store key"),
            StoreError::ValueTooLarge => f.write_str("value too large"),
            StoreError::Full => f.write_str("store full"),
            StoreError::Io(e) => write!(f, "store I/O: {e}"),
            StoreError::Corrupt(e) => write!(f, "store corrupt: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// Usage in NVS entries, the unit the low watermark is measured in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub used_entries: usize,
    pub total_entries: usize,
}

impl StoreStats {
    pub fn used_fraction(&self) -> f32 {
        if self.total_entries == 0 {
            return 1.0;
        }
        self.used_entries as f32 / self.total_entries as f32
    }
}

/// Values larger than this are rejected. NVS blobs can be bigger, but no
/// mastomini record needs more.
pub const VALUE_MAX: usize = 8 * 1024;

/// Entries NVS uses for one key/value: a header entry, a blob index entry and
/// the data rounded up to whole entries.
pub fn entries_for(value_len: usize) -> usize {
    2 + value_len.div_ceil(ENTRY_SIZE)
}

/// Entries in an 8 MiB NVS partition: 2,048 pages of 126 entries, minus the
/// one page NVS keeps free for garbage collection.
pub const NVS_8MIB_ENTRIES: usize = 2047 * 126;

pub type Visitor<'a> = dyn FnMut(&Key, &[u8]) -> Result<(), StoreError> + 'a;

pub trait Store {
    /// Copy of the value, or `None` if absent.
    fn get(&mut self, ns: Ns, key: &Key) -> Result<Option<Vec<u8>>, StoreError>;
    /// Atomically replace the value. Durable when it returns.
    fn set(&mut self, ns: Ns, key: &Key, value: &[u8]) -> Result<(), StoreError>;
    /// Remove the key. Removing an absent key is not an error.
    fn erase(&mut self, ns: Ns, key: &Key) -> Result<(), StoreError>;
    /// Visit every key in a namespace in key order.
    fn for_each(&mut self, ns: Ns, visit: &mut Visitor<'_>) -> Result<(), StoreError>;
    fn stats(&mut self) -> Result<StoreStats, StoreError>;
}

impl<S: Store + ?Sized> Store for Box<S> {
    fn get(&mut self, ns: Ns, key: &Key) -> Result<Option<Vec<u8>>, StoreError> {
        (**self).get(ns, key)
    }
    fn set(&mut self, ns: Ns, key: &Key, value: &[u8]) -> Result<(), StoreError> {
        (**self).set(ns, key, value)
    }
    fn erase(&mut self, ns: Ns, key: &Key) -> Result<(), StoreError> {
        (**self).erase(ns, key)
    }
    fn for_each(&mut self, ns: Ns, visit: &mut Visitor<'_>) -> Result<(), StoreError> {
        (**self).for_each(ns, visit)
    }
    fn stats(&mut self) -> Result<StoreStats, StoreError> {
        (**self).stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_bounded_printable_ascii() {
        assert!(Key::new("s0123456789ABC").is_ok());
        assert_eq!(Key::new("0123456789abcdef"), Err(StoreError::BadKey));
        assert_eq!(Key::new(""), Err(StoreError::BadKey));
        assert_eq!(Key::new("a b"), Err(StoreError::BadKey));
        let key = Key::from_parts(&[b"f", b"0123456789ABC", b"a"]).unwrap();
        assert_eq!(key.as_str(), "f0123456789ABCa");
        assert!(Key::from_parts(&[b"ff", b"0123456789ABC", b"a"]).is_err());
    }

    #[test]
    fn entry_estimate_matches_nvs_layout() {
        assert_eq!(entries_for(0), 2);
        assert_eq!(entries_for(1), 3);
        assert_eq!(entries_for(32), 3);
        assert_eq!(entries_for(33), 4);
    }
}
