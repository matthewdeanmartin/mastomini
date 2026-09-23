//! Store value encoding: `[version][kind][postcard payload]`.
//!
//! The kind byte catches a value written under the wrong key prefix. An
//! unknown version fails closed so an older firmware never misreads data
//! written by a newer one.

use crate::store::StoreError;
use serde::{de::DeserializeOwned, Serialize};

pub const VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Schema = 1,
    Server = 2,
    Account = 3,
    App = 4,
    Token = 5,
    Status = 6,
    Boost = 7,
    Reaction = 8,
    Follow = 9,
    Block = 10,
    Mute = 11,
    AccountMod = 12,
    Report = 13,
    Collection = 14,
    Terms = 15,
    UserKey = 16,
    TokenKey = 17,
    Dm = 18,
}

pub fn encode<T: Serialize>(kind: Kind, value: &T) -> Result<Vec<u8>, StoreError> {
    let mut out = vec![VERSION, kind as u8];
    let payload = postcard::to_allocvec(value)
        .map_err(|e| StoreError::Corrupt(format!("encode {kind:?}: {e}")))?;
    out.extend_from_slice(&payload);
    Ok(out)
}

pub fn decode<T: DeserializeOwned>(kind: Kind, bytes: &[u8]) -> Result<T, StoreError> {
    match bytes {
        [VERSION, k, payload @ ..] if *k == kind as u8 => postcard::from_bytes(payload)
            .map_err(|e| StoreError::Corrupt(format!("decode {kind:?}: {e}"))),
        [VERSION, k, ..] => Err(StoreError::Corrupt(format!(
            "expected {kind:?}, found kind {k}"
        ))),
        [v, ..] => Err(StoreError::Corrupt(format!(
            "unsupported record version {v} (firmware understands {VERSION})"
        ))),
        [] => Err(StoreError::Corrupt("empty record".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_kind_check() {
        let bytes = encode(Kind::Follow, &(7u64, true)).unwrap();
        assert_eq!(
            decode::<(u64, bool)>(Kind::Follow, &bytes).unwrap(),
            (7, true)
        );
        assert!(decode::<(u64, bool)>(Kind::Boost, &bytes).is_err());
        let mut newer = bytes.clone();
        newer[0] = VERSION + 1;
        assert!(decode::<(u64, bool)>(Kind::Follow, &newer).is_err());
    }
}
