//! OAuth apps, authorization codes and tokens (spec/05 "OAuth").
//!
//! Apps and user tokens are persisted because clients keep them forever.
//! Authorization codes and app-only (`client_credentials`) tokens are
//! ephemeral and live only in RAM.

use super::*;
use crate::auth::{self, sha256};

pub const KNOWN_SCOPES: [&str; 5] = ["read", "write", "follow", "push", "profile"];
const MAX_APP_NAME: usize = 60;
const MAX_WEBSITE: usize = 120;
const MAX_REDIRECT_URIS: usize = 4;
const MAX_REDIRECT_URI: usize = 256;
pub const OOB: &str = "urn:ietf:wg:oauth:2.0:oob";

/// Who is calling: a user token or an app-only token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub slot: Option<u8>,
    pub app_id: u64,
    pub scopes: Vec<String>,
}

impl Principal {
    /// True if the token grants `needed` (e.g. `read`, `write:statuses`).
    /// A parent scope covers its children; `read`/`write` also cover `follow`.
    pub fn allows(&self, needed: &str) -> bool {
        let parent = needed.split(':').next().unwrap_or(needed);
        self.scopes.iter().any(|s| {
            s == needed || s == parent || (parent == "follow" && (s == "read" || s == "write"))
        })
    }
}

/// What `/oauth/authorize` needs to issue a code.
#[derive(Debug, Clone)]
pub struct AuthCodeGrant {
    pub client_id: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub code_challenge: Option<String>,
}

fn valid_scope(scope: &str) -> bool {
    let parent = scope.split(':').next().unwrap_or(scope);
    matches!(parent, "read" | "write") || KNOWN_SCOPES.contains(&scope)
}

/// Parse a space-separated scope string. Empty means `read`, as in Mastodon.
pub fn parse_scopes(text: &str) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for scope in text.split([' ', '+']).filter(|s| !s.is_empty()) {
        if !valid_scope(scope) {
            return invalid(format!("Unknown scope: {scope}"));
        }
        if !out.iter().any(|s| s == scope) {
            out.push(scope.to_string());
        }
    }
    if out.is_empty() {
        out.push("read".into());
    }
    Ok(out)
}

/// Is every requested scope covered by the app's registered scopes?
fn scopes_within(requested: &[String], granted: &[String]) -> bool {
    let principal = Principal {
        slot: None,
        app_id: 0,
        scopes: granted.to_vec(),
    };
    requested.iter().all(|s| principal.allows(s))
}

fn truncate(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

impl<S: Store> Service<S> {
    /// `POST /api/v1/apps`. Returns the app and its plaintext secret, which
    /// is shown once and stored only as a hash.
    pub fn register_app(
        &mut self,
        name: &str,
        website: Option<&str>,
        redirect_uris: &[String],
        scopes: &str,
        now_ms: u64,
    ) -> Result<(AppRec, String)> {
        if name.trim().is_empty() {
            return invalid("Validation failed: Application name can't be blank");
        }
        if redirect_uris.is_empty() {
            return invalid("Validation failed: Redirect URI can't be blank");
        }
        if redirect_uris.len() > MAX_REDIRECT_URIS
            || redirect_uris
                .iter()
                .any(|u| u.len() > MAX_REDIRECT_URI || !u.contains(':'))
        {
            return invalid("Validation failed: Redirect URI must be an absolute URI");
        }
        let scopes = parse_scopes(scopes)?;
        self.govern(None, now_ms)?;
        if self.state.apps.len() >= MAX_APPS {
            let unused = self
                .state
                .apps
                .keys()
                .copied()
                .find(|id| !self.state.tokens.iter().any(|t| t.rec.app_id == *id));
            match unused {
                Some(id) => {
                    self.erase(Ns::App, &keys::app(id))?;
                    self.state.apps.remove(&id);
                }
                None => {
                    return Err(Error::TooMany(
                        "Too many registered apps; revoke a device in the household app".into(),
                    ))
                }
            }
        }
        let secret = auth::random_secret();
        let rec = AppRec {
            id: self.next_id(now_ms),
            client_id: auth::random_secret(),
            secret_hash: sha256(&secret),
            name: truncate(name.trim(), MAX_APP_NAME),
            website: website
                .filter(|w| !w.is_empty())
                .map(|w| truncate(w, MAX_WEBSITE)),
            redirect_uris: redirect_uris.to_vec(),
            scopes,
        };
        self.put(Ns::App, &keys::app(rec.id), Kind::App, &rec)?;
        self.state.apps.insert(rec.id, rec.clone());
        Ok((rec, secret))
    }

    pub fn app_by_client_id(&self, client_id: &str) -> Option<&AppRec> {
        self.state
            .apps
            .values()
            .find(|a| auth::ct_eq(a.client_id.as_bytes(), client_id.as_bytes()))
    }

    /// Validate an authorization request before showing the sign-in page.
    pub fn check_authorize_request(&self, grant: &AuthCodeGrant) -> Result<&AppRec> {
        let app = self.app_by_client_id(&grant.client_id).ok_or_else(|| {
            Error::Invalid("Client authentication failed due to unknown client".into())
        })?;
        if !app.redirect_uris.contains(&grant.redirect_uri) {
            return invalid("The redirect uri included is not valid.");
        }
        if !scopes_within(&grant.scopes, &app.scopes) {
            return invalid("The requested scope is invalid, unknown, or malformed.");
        }
        Ok(app)
    }

    /// Password accepted on the authorize page: issue a one-use code (RAM).
    pub fn authorize(
        &mut self,
        grant: &AuthCodeGrant,
        username: &str,
        password: &str,
        now_ms: u64,
    ) -> Result<String> {
        let app_id = self.check_authorize_request(grant)?.id;
        let slot = self.check_password(username, password, now_ms)?;
        self.state.codes.retain(|c| c.expires_ms > now_ms);
        if self.state.codes.len() >= MAX_CODES {
            return Err(Error::TooMany(
                "Too many sign-ins in progress; try again".into(),
            ));
        }
        let code = auth::random_secret();
        self.state.codes.push(AuthCode {
            code_hash: sha256(&code),
            app_id,
            slot,
            redirect_uri: grant.redirect_uri.clone(),
            scopes: grant.scopes.clone(),
            challenge: grant.code_challenge.clone(),
            expires_ms: now_ms + auth::CODE_TTL_MS,
        });
        Ok(code)
    }

    fn check_client(&self, client_id: &str, client_secret: Option<&str>) -> Result<u64> {
        let app = self
            .app_by_client_id(client_id)
            .ok_or(Error::Unauthorized)?;
        if let Some(secret) = client_secret {
            if !auth::ct_eq(&sha256(secret), &app.secret_hash) {
                return Err(Error::Unauthorized);
            }
        }
        Ok(app.id)
    }

    /// `grant_type=authorization_code`. Returns the new bearer token.
    pub fn exchange_code(
        &mut self,
        client_id: &str,
        client_secret: Option<&str>,
        code: &str,
        redirect_uri: &str,
        code_verifier: Option<&str>,
        now_ms: u64,
    ) -> Result<(String, Vec<String>)> {
        let app_id = self.check_client(client_id, client_secret)?;
        let hash = sha256(code);
        let index = self
            .state
            .codes
            .iter()
            .position(|c| auth::ct_eq(&c.code_hash, &hash))
            .ok_or_else(|| Error::Invalid("invalid_grant".into()))?;
        // Single use, even if the rest of the checks fail.
        let pending = self.state.codes.swap_remove(index);
        let proof_ok = match (&pending.challenge, code_verifier) {
            (Some(challenge), Some(verifier)) => auth::pkce_matches(verifier, challenge),
            (Some(_), None) => false,
            // Without PKCE the client must prove itself with its secret.
            (None, _) => client_secret.is_some(),
        };
        if pending.expires_ms <= now_ms
            || pending.app_id != app_id
            || pending.redirect_uri != redirect_uri
            || !proof_ok
        {
            return invalid("invalid_grant");
        }
        let token = self.issue_token(pending.slot, app_id, pending.scopes.clone(), now_ms)?;
        Ok((token, pending.scopes))
    }

    /// Persist a new user token, evicting the oldest ones past the caps.
    pub fn issue_token(
        &mut self,
        slot: u8,
        app_id: u64,
        scopes: Vec<String>,
        now_ms: u64,
    ) -> Result<String> {
        let epoch = self
            .state
            .account(slot)
            .ok_or(Error::NotFound)?
            .rec
            .token_epoch;
        self.govern(Some(slot), now_ms)?;
        loop {
            let mine = self
                .state
                .tokens
                .iter()
                .filter(|t| t.rec.slot == slot)
                .count();
            let victim = if mine >= MAX_TOKENS_PER_ACCOUNT {
                self.state
                    .tokens
                    .iter()
                    .filter(|t| t.rec.slot == slot)
                    .min_by_key(|t| t.rec.id)
            } else if self.state.tokens.len() >= MAX_TOKENS {
                self.state.tokens.iter().min_by_key(|t| t.rec.id)
            } else {
                None
            };
            let Some(hash) = victim.map(|t| t.rec.hash) else {
                break;
            };
            self.erase(Ns::Tok, &keys::token(&hash))?;
            self.state.tokens.retain(|t| t.rec.hash != hash);
        }
        let token = auth::random_secret();
        let rec = TokenRec {
            id: self.next_id(now_ms),
            hash: sha256(&token),
            slot,
            app_id,
            scopes,
            epoch,
        };
        self.put(Ns::Tok, &keys::token(&rec.hash), Kind::Token, &rec)?;
        self.state.tokens.push(Token {
            rec,
            last_used_ms: now_ms,
        });
        Ok(token)
    }

    /// `grant_type=client_credentials`: an app-only token, kept in RAM.
    pub fn app_token(
        &mut self,
        client_id: &str,
        client_secret: &str,
        scopes: &str,
    ) -> Result<(String, Vec<String>)> {
        let app_id = self.check_client(client_id, Some(client_secret))?;
        let scopes = parse_scopes(scopes)?;
        if self.state.app_tokens.len() >= MAX_APP_TOKENS {
            self.state.app_tokens.pop_front();
        }
        let token = auth::random_secret();
        self.state.app_tokens.push_back(AppToken {
            hash: sha256(&token),
            app_id,
            scopes: scopes.clone(),
        });
        Ok((token, scopes))
    }

    /// `POST /oauth/revoke`. Unknown tokens are not an error (RFC 7009).
    pub fn revoke(&mut self, token: &str) -> Result<()> {
        let hash = sha256(token);
        self.state.app_tokens.retain(|t| t.hash != hash);
        if self.state.tokens.iter().any(|t| t.rec.hash == hash) {
            self.erase(Ns::Tok, &keys::token(&hash))?;
            self.state.tokens.retain(|t| t.rec.hash != hash);
        }
        Ok(())
    }

    /// Resolve a bearer token. Disabled accounts and old epochs are rejected.
    pub fn principal(&mut self, token: &str, now_ms: u64) -> Option<Principal> {
        let hash = sha256(token);
        if let Some(t) = self
            .state
            .app_tokens
            .iter()
            .find(|t| auth::ct_eq(&t.hash, &hash))
        {
            return Some(Principal {
                slot: None,
                app_id: t.app_id,
                scopes: t.scopes.clone(),
            });
        }
        let state = &mut self.state;
        let token = state
            .tokens
            .iter_mut()
            .find(|t| auth::ct_eq(&t.rec.hash, &hash))?;
        let account = state.accounts[token.rec.slot as usize].as_ref()?;
        if account.rec.deleted || account.rec.disabled || account.rec.token_epoch != token.rec.epoch
        {
            return None;
        }
        token.last_used_ms = now_ms;
        Some(Principal {
            slot: Some(token.rec.slot),
            app_id: token.rec.app_id,
            scopes: token.rec.scopes.clone(),
        })
    }
}
