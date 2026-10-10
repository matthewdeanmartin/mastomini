//! Moving a household to another board: an encrypted export of the whole
//! store, and a restore onto an unprovisioned board
//! (spec/board_migration_plan.md, path 2).
//!
//! The archive holds every store record as stored: accounts and password
//! verifiers, posts, sessions, sealed direct-message keys, everything. It
//! holds no hostname (URLs are rendered from the board's own name), so it
//! restores under the same name, where signed-in devices keep working, or a
//! new one, where they sign in again.
//!
//! Format (little endian), the header authenticated as associated data:
//!
//! ```text
//! magic "MMEXPORT" | version u16 | schema u16 | created_ms u64 | records u32
//! | kdf rounds u32 | salt [16] | nonce [12] | ChaCha20-Poly1305(payload)
//! payload: per record ns u8 | key length u8 | key | value length u32 | value
//! ```
//!
//! The key is PBKDF2-HMAC-SHA256 of the passphrase; the rounds are in the
//! header so they can rise later.
use super::*;
use crate::store::{Key, Ns};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use zeroize::Zeroize;

pub const MAGIC: &[u8; 8] = b"MMEXPORT";
pub const VERSION: u16 = 1;
/// An export holds the service lock while deriving the key: estimated at
/// ~6 s on the S3 (sign-in's 10,000 rounds take ~1.2 s there), less on the P4.
pub const KDF_ROUNDS: u32 = 50_000;
pub const PASSPHRASE_MIN: usize = 12;
pub const PASSPHRASE_MAX: usize = 256;
/// Largest archive accepted (the store holds far less in practice).
pub const ARCHIVE_MAX: usize = 4 * 1024 * 1024;
const HEADER_LEN: usize = 8 + 2 + 2 + 8 + 4 + 4 + 16 + 12;
const TAG_LEN: usize = 16;

/// What a restore brought back.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Restored {
    pub records: u32,
    pub accounts: usize,
    pub statuses: usize,
    pub schema: u16,
    pub created_ms: u64,
}

struct Header {
    schema: u16,
    created_ms: u64,
    records: u32,
    rounds: u32,
    salt: [u8; 16],
    nonce: [u8; 12],
}

impl Header {
    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&self.schema.to_le_bytes());
        out.extend_from_slice(&self.created_ms.to_le_bytes());
        out.extend_from_slice(&self.records.to_le_bytes());
        out.extend_from_slice(&self.rounds.to_le_bytes());
        out.extend_from_slice(&self.salt);
        out.extend_from_slice(&self.nonce);
        out
    }

    fn parse(archive: &[u8]) -> Result<Header> {
        let not_ours = || Error::Invalid("This file is not a mastomini export".into());
        if archive.len() < HEADER_LEN + TAG_LEN || &archive[..8] != MAGIC {
            return Err(not_ours());
        }
        let u16_at = |at: usize| u16::from_le_bytes([archive[at], archive[at + 1]]);
        let u32_at =
            |at: usize| u32::from_le_bytes(archive[at..at + 4].try_into().unwrap_or_default());
        if u16_at(8) != VERSION {
            return Err(Error::Invalid(format!(
                "This export is format {}; this board reads format {VERSION}. Update the board's firmware.",
                u16_at(8)
            )));
        }
        let header = Header {
            schema: u16_at(10),
            created_ms: u64::from_le_bytes(archive[12..20].try_into().unwrap_or_default()),
            records: u32_at(20),
            rounds: u32_at(24),
            salt: archive[28..44].try_into().map_err(|_| not_ours())?,
            nonce: archive[44..56].try_into().map_err(|_| not_ours())?,
        };
        if header.rounds == 0 || header.rounds > 10_000_000 {
            return Err(not_ours());
        }
        Ok(header)
    }
}

fn check_passphrase(passphrase: &str) -> Result<()> {
    let n = passphrase.chars().count();
    if n < PASSPHRASE_MIN || passphrase.len() > PASSPHRASE_MAX {
        return Err(Error::Invalid(format!(
            "The passphrase must be {PASSPHRASE_MIN} to {PASSPHRASE_MAX} characters"
        )));
    }
    Ok(())
}

fn cipher(passphrase: &str, salt: &[u8; 16], rounds: u32) -> ChaCha20Poly1305 {
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(passphrase.as_bytes(), salt, rounds, &mut key);
    let cipher = ChaCha20Poly1305::new(&key.into());
    key.zeroize();
    cipher
}

impl<S: Store> Service<S> {
    /// The owner's encrypted copy of the whole household. One call reads the
    /// whole store under the service lock, so it is a single point in time.
    pub fn export_archive(
        &mut self,
        actor: u8,
        password: &str,
        passphrase: &str,
        now_ms: u64,
    ) -> Result<Vec<u8>> {
        let account = self.state.account(actor).ok_or(Error::Unauthorized)?;
        if account.rec.role != Role::Owner {
            return Err(Error::Forbidden(
                "Only the household's owner can export it".into(),
            ));
        }
        let username = account.rec.username.clone();
        if self.check_password(&username, password, now_ms)? != actor {
            return Err(Error::Unauthorized);
        }
        check_passphrase(passphrase)?;

        let mut payload = Vec::new();
        let mut records = 0u32;
        for ns in Ns::ALL {
            self.store
                .for_each(ns, &mut |key, value| {
                    let key = key.as_str().as_bytes();
                    payload.push(ns as u8);
                    payload.push(key.len() as u8);
                    payload.extend_from_slice(key);
                    payload.extend_from_slice(&(value.len() as u32).to_le_bytes());
                    payload.extend_from_slice(value);
                    records += 1;
                    Ok(())
                })
                .map_err(|e| Error::Unavailable(format!("Reading the store failed: {e}")))?;
        }
        if payload.len() + HEADER_LEN + TAG_LEN > ARCHIVE_MAX {
            return Err(Error::Invalid(
                "The household is too large to export".into(),
            ));
        }
        let mut salt = [0u8; 16];
        let mut nonce = [0u8; 12];
        getrandom::getrandom(&mut salt)
            .and_then(|()| getrandom::getrandom(&mut nonce))
            .map_err(|_| Error::Unavailable("No random numbers for the export".into()))?;
        // Test servers lower every KDF cost (Config::password_rounds); the
        // rounds travel in the header, so imports need no such setting.
        let rounds = if self.config.password_rounds < crate::auth::PASSWORD_ROUNDS {
            self.config.password_rounds.max(1)
        } else {
            KDF_ROUNDS
        };
        let header = Header {
            schema: SCHEMA,
            created_ms: now_ms,
            records,
            rounds,
            salt,
            nonce,
        };
        let aad = header.bytes();
        let sealed = cipher(passphrase, &salt, rounds)
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &payload,
                    aad: &aad,
                },
            )
            .map_err(|_| Error::Unavailable("Encrypting the export failed".into()))?;
        payload.zeroize();
        let mut archive = aad;
        archive.extend_from_slice(&sealed);
        Ok(archive)
    }

    /// Restore an export onto this board, which must not be provisioned yet.
    /// The store is cleared, the records written as they were, and the state
    /// rebuilt as at boot. Any failure leaves the board empty again.
    pub fn import_archive(&mut self, archive: &[u8], passphrase: &str) -> Result<Restored> {
        if self.state.provisioned {
            return Err(Error::Forbidden(
                "This board already has a household; restore only onto a new or reset board".into(),
            ));
        }
        if archive.len() > ARCHIVE_MAX {
            return Err(Error::Invalid("The file is too large".into()));
        }
        let header = Header::parse(archive)?;
        if header.schema > SCHEMA {
            return Err(Error::Invalid(format!(
                "This export comes from newer firmware (store schema {}); this board has {SCHEMA}. \
                 Update this board first.",
                header.schema
            )));
        }
        let aad = &archive[..HEADER_LEN];
        let mut payload = cipher(passphrase, &header.salt, header.rounds)
            .decrypt(
                Nonce::from_slice(&header.nonce),
                Payload {
                    msg: &archive[HEADER_LEN..],
                    aad,
                },
            )
            .map_err(|_| Error::Invalid("Wrong passphrase, or the file is damaged".into()))?;
        let records = parse_records(&payload, header.records);
        payload.zeroize();
        let records = records?;

        self.clear_store()?;
        let restored = self.write_records(&records).and_then(|()| {
            self.reload().map_err(|e| {
                Error::Invalid(format!("The restored household failed its checks: {e}"))
            })
        });
        if let Err(e) = restored {
            // Never leave a half-restored household: back to an empty board.
            let _ = self
                .clear_store()
                .and_then(|()| self.reload().map_err(|e| Error::Unavailable(e.to_string())));
            return Err(e);
        }
        if !self.state.provisioned {
            let _ = self
                .clear_store()
                .and_then(|()| self.reload().map_err(|e| Error::Unavailable(e.to_string())));
            return Err(Error::Invalid("The export holds no household".into()));
        }
        Ok(Restored {
            records: header.records,
            accounts: self.state.active_accounts().count(),
            statuses: self.state.statuses.len(),
            schema: header.schema,
            created_ms: header.created_ms,
        })
    }

    fn clear_store(&mut self) -> Result<()> {
        let unavailable = |e: crate::store::StoreError| Error::Unavailable(format!("Store: {e}"));
        for ns in Ns::ALL {
            let mut keys = Vec::new();
            self.store
                .for_each(ns, &mut |key, _| {
                    keys.push(*key);
                    Ok(())
                })
                .map_err(unavailable)?;
            for key in keys {
                self.store.erase(ns, &key).map_err(unavailable)?;
            }
        }
        Ok(())
    }

    fn write_records(&mut self, records: &[(Ns, Key, Vec<u8>)]) -> Result<()> {
        for (ns, key, value) in records {
            self.store
                .set(*ns, key, value)
                .map_err(|e| Error::Unavailable(format!("Writing the store failed: {e}")))?;
        }
        Ok(())
    }
}

fn parse_records(payload: &[u8], expected: u32) -> Result<Vec<(Ns, Key, Vec<u8>)>> {
    let damaged = || Error::Invalid("The export is damaged".into());
    let mut out = Vec::with_capacity(expected.min(65_536) as usize);
    let mut at = 0;
    while at < payload.len() {
        let ns = Ns::from_u8(payload[at]).ok_or_else(damaged)?;
        let key_len = *payload.get(at + 1).ok_or_else(damaged)? as usize;
        let key_end = at + 2 + key_len;
        let key = std::str::from_utf8(payload.get(at + 2..key_end).ok_or_else(damaged)?)
            .map_err(|_| damaged())?;
        let key = Key::new(key).map_err(|_| damaged())?;
        let len_bytes = payload.get(key_end..key_end + 4).ok_or_else(damaged)?;
        let value_len = u32::from_le_bytes(len_bytes.try_into().map_err(|_| damaged())?) as usize;
        let value_end = key_end + 4 + value_len;
        let value = payload.get(key_end + 4..value_end).ok_or_else(damaged)?;
        out.push((ns, key, value.to_vec()));
        at = value_end;
    }
    if out.len() != expected as usize {
        return Err(damaged());
    }
    Ok(out)
}
