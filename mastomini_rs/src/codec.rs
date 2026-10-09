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
    Code = 19,
    Revision = 20,
    Poll = 21,
    FollowRequest = 22,
    List = 23,
    Filter = 24,
    Social = 25,
    AccountNote = 26,
    Announcement = 27,
    LocalToken = 28,
    Scheduler = 29,
    Scheduled = 30,
    LedPhrase = 31,
    /// Password verifier and matching sealed DM key share one commit point.
    AccountWithKey = 32,
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
        [VERSION, k, payload @ ..] if *k == kind as u8 => {
            let (value, rest) = postcard::take_from_bytes(payload)
                .map_err(|e| StoreError::Corrupt(format!("decode {kind:?}: {e}")))?;
            if !rest.is_empty() {
                return Err(StoreError::Corrupt(format!("trailing bytes in {kind:?}")));
            }
            Ok(value)
        }
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
    fn records_require_exact_consumption_and_complete_payloads() {
        let bytes = encode(Kind::Follow, &(7u64, true)).unwrap();
        for end in 0..bytes.len() {
            assert!(decode::<(u64, bool)>(Kind::Follow, &bytes[..end]).is_err());
        }
        for suffix in [vec![0], vec![255], bytes.clone()] {
            let mut extra = bytes.clone();
            extra.extend(suffix);
            assert!(decode::<(u64, bool)>(Kind::Follow, &extra).is_err());
        }
    }

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
