//! Encrypted direct messages: member keys, per-device seals and the
//! request-scoped session key (see [`crate::crypto`] and spec/05).
//!
//! Records (all new kinds, nothing else changes layout):
//! - `mm_key` `k` + slot: the member's public key and password-sealed secret
//! - `mm_key` `w` + token key suffix: the secret sealed for one device
//! - `mm_stat` `d` + status id: the envelope of a direct message, whose
//!   status record keeps empty text

use super::*;
use crate::auth;
use crate::crypto::{self, Envelope, SecretKey};
use serde::{Deserialize, Serialize};

/// A member's key pair, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserKeyRec {
    pub account_id: u64,
    pub public: [u8; 32],
    pub salt: [u8; 16],
    pub rounds: u32,
    pub sealed: crypto::Sealed,
}

/// An unlocked secret, redacted from debug output.
#[derive(Clone)]
pub struct Held(pub SecretKey);

impl fmt::Debug for Held {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Held(..)")
    }
}

/// Shown to a participant whose device holds no key (signed in before
/// encryption, or the key can't be opened).
pub const LOCKED_TEXT: &str =
    "🔒 Encrypted direct message. Sign in again on this device to read it.";

impl<S: Store> Service<S> {
    pub fn user_key(&self, slot: u8) -> Option<&UserKeyRec> {
        let id = self.state.account(slot)?.rec.id;
        self.state.user_keys[slot as usize]
            .as_ref()
            .filter(|k| k.account_id == id)
    }

    fn write_user_key(&mut self, slot: u8, secret: &SecretKey, password: &str) -> Result<()> {
        let account_id = self.state.account(slot).ok_or(Error::NotFound)?.rec.id;
        let public = x25519_public(secret);
        let salt = crypto::new_salt();
        let rounds = self.config.password_rounds;
        let rec = UserKeyRec {
            account_id,
            public,
            salt,
            rounds,
            sealed: crypto::seal_with_password(secret, password, &salt, rounds, account_id),
        };
        self.put(Ns::Key, &keys::user_key(slot), Kind::UserKey, &rec)?;
        self.state.user_keys[slot as usize] = Some(rec);
        Ok(())
    }

    /// A new key pair for a member whose password we have just been given.
    pub(crate) fn create_user_key(&mut self, slot: u8, password: &str) -> Result<SecretKey> {
        let (secret, _) = crypto::new_keypair();
        self.write_user_key(slot, &secret, password)?;
        Ok(secret)
    }

    /// Open the member's secret with their (already verified) password.
    /// Members from before encryption get their key pair here, at their
    /// next sign-in.
    pub fn unlock(&mut self, slot: u8, password: &str) -> Result<SecretKey> {
        let Some(rec) = self.user_key(slot) else {
            return self.create_user_key(slot, password);
        };
        crypto::open_with_password(&rec.sealed, password, &rec.salt, rec.rounds, rec.account_id)
            .ok_or(Error::Unauthorized)
    }

    /// Re-seal the member's secret under a new password (the old one opens
    /// it), so their direct messages survive a password change.
    pub(crate) fn reseal_user_key(&mut self, slot: u8, current: &str, new: &str) -> Result<()> {
        let secret = match self.user_key(slot) {
            Some(_) => self.unlock(slot, current)?,
            None => crypto::new_keypair().0,
        };
        self.write_user_key(slot, &secret, new)
    }

    pub(crate) fn seal_for_token(
        &mut self,
        slot: u8,
        token: &str,
        secret: &SecretKey,
    ) -> Result<()> {
        let account_id = self.state.account(slot).ok_or(Error::NotFound)?.rec.id;
        let sealed = crypto::seal_with_token(secret, token, account_id);
        let key = keys::token_seal(&auth::sha256(token));
        self.put(Ns::Key, &key, Kind::TokenKey, &sealed)?;
        self.state.token_seals.insert(key, sealed);
        Ok(())
    }

    /// Erase a user token and its device seal.
    pub(crate) fn forget_token(&mut self, hash: &[u8; 32]) -> Result<()> {
        self.erase(Ns::Tok, &keys::token(hash))?;
        self.state.tokens.retain(|t| t.rec.hash != *hash);
        let seal = keys::token_seal(hash);
        if self.state.token_seals.remove(&seal).is_some() {
            self.erase(Ns::Key, &seal)?;
        }
        Ok(())
    }

    /// Unlock the caller's secret for this request only.
    pub fn open_session(&mut self, slot: u8, token: &str) {
        self.close_session();
        let Some(account_id) = self.state.account(slot).map(|a| a.rec.id) else {
            return;
        };
        let secret = self
            .state
            .token_seals
            .get(&keys::token_seal(&auth::sha256(token)))
            .and_then(|sealed| crypto::open_with_token(sealed, token, account_id));
        self.state.session = secret.map(|s| (slot, Held(s)));
    }

    /// Forget the unlocked secret (zeroed on drop).
    pub fn close_session(&mut self) {
        self.state.session = None;
    }

    /// Encrypt a direct message for its author and everyone it mentions.
    pub(crate) fn seal_dm(
        &self,
        status_id: u64,
        participants: &[u8],
        text: &str,
        spoiler: &str,
    ) -> Result<Envelope> {
        let mut recipients = Vec::new();
        for slot in participants {
            let Some(key) = self.user_key(*slot) else {
                let name = self
                    .state
                    .account(*slot)
                    .map_or(String::new(), |a| a.rec.username.clone());
                return invalid(format!(
                    "@{name} needs to sign in again before they can receive direct messages"
                ));
            };
            recipients.push((key.account_id, key.public));
        }
        let plaintext = postcard::to_allocvec(&(text, spoiler))
            .map_err(|e| Error::Invalid(format!("Could not encode message: {e}")))?;
        Ok(crypto::encrypt(&plaintext, status_id, &recipients))
    }

    /// `(text, spoiler_text)` of a direct message, if `viewer` is the
    /// member whose key this request unlocked and a participant.
    pub fn dm_text(&self, viewer: Option<u8>, status_id: u64) -> Option<(String, String)> {
        let (slot, Held(secret)) = self.state.session.as_ref()?;
        if viewer != Some(*slot) {
            return None;
        }
        let account_id = self.state.account(*slot)?.rec.id;
        let envelope = self.state.dms.get(&status_id)?;
        let plain = crypto::decrypt(envelope, status_id, account_id, secret)?;
        postcard::from_bytes(&plain).ok()
    }

    /// Text and spoiler as `viewer` may read them: plain for ordinary
    /// posts, decrypted (or [`LOCKED_TEXT`]) for direct messages.
    pub fn readable_text(&self, viewer: Option<u8>, status: &Status) -> (String, String) {
        if status.rec.visibility != Visibility::Direct
            || !self.state.dms.contains_key(&status.rec.id)
        {
            return (status.rec.text.clone(), status.rec.spoiler_text.clone());
        }
        self.dm_text(viewer, status.rec.id)
            .unwrap_or_else(|| (LOCKED_TEXT.to_string(), String::new()))
    }
}

fn x25519_public(secret: &SecretKey) -> [u8; 32] {
    x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(**secret)).to_bytes()
}
