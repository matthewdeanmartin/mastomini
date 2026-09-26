//! Tokens the server issues itself, not through an OAuth app: members' API
//! keys (for bots and scripts) and browser sessions for the plain post and
//! profile pages (`api/pages.rs`).
//!
//! Each is an ordinary [`TokenRec`] with `app_id` [`LOCAL_APP`], plus a
//! [`LocalTokenRec`] beside it naming what it is. So the token layout is
//! unchanged, and everything that ends tokens (revoking, signing out
//! everywhere, a password change, deleting the account) ends these too.
//!
//! API keys are a separate pool: signing in on a ninth device evicts the
//! member's oldest *session*, never a key a bot depends on. Browser sessions
//! are devices like any other. Neither can read direct messages: those stay
//! sealed to devices that signed in with the password through an app. A key
//! can still send them (that needs only the recipients' public keys).

use super::*;
use crate::auth::{self, sha256};
use serde::{Deserialize, Serialize};

/// `TokenRec::app_id` of a local token. Real app ids come from the id
/// generator and are never 0.
pub const LOCAL_APP: u64 = 0;
pub const MAX_API_KEYS_PER_ACCOUNT: usize = 4;
pub const MAX_API_KEYS: usize = 16;
pub const MAX_API_KEY_NAME: usize = 40;
/// What the pages need: they never write.
pub const BROWSER_SCOPES: &str = "read";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalKind {
    ApiKey,
    Browser,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalTokenRec {
    /// The `TokenRec::id` this describes.
    pub token_id: u64,
    pub kind: LocalKind,
    /// The member's name for an API key ("Weather bot"); shown as the
    /// application on its posts.
    pub name: String,
}

impl State {
    pub fn local_token(&self, token_id: u64) -> Option<&LocalTokenRec> {
        self.local_tokens.get(&token_id)
    }

    pub(crate) fn is_api_key(&self, token_id: u64) -> bool {
        self.local_token(token_id)
            .is_some_and(|l| l.kind == LocalKind::ApiKey)
    }

    /// The name to show as a post's application: a registered app's, or an
    /// API key's. `None` once the key is revoked.
    pub fn application_name(&self, app_id: u64) -> Option<(&str, Option<&str>)> {
        if let Some(app) = self.apps.get(&app_id) {
            return Some((app.name.as_str(), app.website.as_deref()));
        }
        self.local_token(app_id)
            .filter(|l| l.kind == LocalKind::ApiKey)
            .map(|l| (l.name.as_str(), None))
    }
}

impl<S: Store> Service<S> {
    /// A new API key for the member: `(token id, the key)`. The key is shown
    /// once; only its hash is stored. `password` is checked again, so an app
    /// holding one of the member's tokens cannot mint itself a key.
    pub fn create_api_key(
        &mut self,
        slot: u8,
        password: &str,
        name: &str,
        scopes: &str,
        now_ms: u64,
    ) -> Result<(u64, String)> {
        let username = self
            .state
            .account(slot)
            .ok_or(Error::NotFound)?
            .rec
            .username
            .clone();
        if self.check_password(&username, password, now_ms)? != slot {
            return Err(Error::Unauthorized);
        }
        let name = name.trim();
        if name.is_empty() {
            return invalid("Validation failed: Give the key a name, such as the bot it is for");
        }
        let scopes = parse_scopes(scopes)?;
        let keys_of = |slot: Option<u8>| {
            self.state
                .tokens
                .iter()
                .filter(|t| slot.is_none_or(|s| t.rec.slot == s) && self.state.is_api_key(t.rec.id))
                .count()
        };
        if keys_of(Some(slot)) >= MAX_API_KEYS_PER_ACCOUNT {
            return Err(Error::TooMany(format!(
                "You have {MAX_API_KEYS_PER_ACCOUNT} API keys, the most allowed; revoke one first"
            )));
        }
        if keys_of(None) >= MAX_API_KEYS {
            return Err(Error::TooMany(format!(
                "The household has {MAX_API_KEYS} API keys, the most allowed"
            )));
        }
        let name: String = name.chars().take(MAX_API_KEY_NAME).collect();
        self.issue_local(slot, LocalKind::ApiKey, &name, scopes, now_ms)
    }

    /// The member's API keys, newest first.
    pub fn api_keys(&self, slot: u8) -> Vec<(&Token, &LocalTokenRec)> {
        let mut out: Vec<_> = self
            .state
            .tokens
            .iter()
            .filter(|t| t.rec.slot == slot)
            .filter_map(|t| {
                self.state
                    .local_token(t.rec.id)
                    .filter(|l| l.kind == LocalKind::ApiKey)
                    .map(|l| (t, l))
            })
            .collect();
        out.sort_by_key(|(t, _)| core::cmp::Reverse(t.rec.id));
        out
    }

    /// Sign in on the plain pages: a read-only token for the session cookie.
    pub fn browser_session(
        &mut self,
        username: &str,
        password: &str,
        now_ms: u64,
    ) -> Result<String> {
        let slot = self.check_password(username, password, now_ms)?;
        let scopes = parse_scopes(BROWSER_SCOPES)?;
        self.issue_local(slot, LocalKind::Browser, "Web browser", scopes, now_ms)
            .map(|(_, token)| token)
    }

    fn issue_local(
        &mut self,
        slot: u8,
        kind: LocalKind,
        name: &str,
        scopes: Vec<String>,
        now_ms: u64,
    ) -> Result<(u64, String)> {
        // A browser session is a device: it may evict the member's oldest
        // session. An API key is capped separately above and evicts nothing.
        let token =
            self.issue_token_with(slot, LOCAL_APP, scopes, now_ms, kind == LocalKind::Browser)?;
        let hash = sha256(&token);
        let token_id = self
            .state
            .tokens
            .iter()
            .find(|t| auth::ct_eq(&t.rec.hash, &hash))
            .map(|t| t.rec.id)
            .ok_or(Error::NotFound)?;
        let rec = LocalTokenRec {
            token_id,
            kind,
            name: name.to_string(),
        };
        // The token without this record is repaired away at boot: a power
        // cut between the two writes leaves nothing usable behind.
        if let Err(e) = self.put(
            Ns::Tok,
            &keys::local_token(token_id),
            Kind::LocalToken,
            &rec,
        ) {
            let _ = self.forget_token(&hash);
            return Err(e);
        }
        self.state.local_tokens.insert(token_id, rec);
        Ok((token_id, token))
    }
}
