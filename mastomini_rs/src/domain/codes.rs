//! One-time invite and password-reset codes (spec/05 "Passwords"), and the
//! member's own devices.
//!
//! Nobody sets a password for someone else: an admin issues a code, and the
//! member redeems it at `/setup/<code>` with a password of their own. Codes
//! are single use, expire after seven days and are stored only as hashes.
//! Redeeming erases the code before creating the account or setting the
//! password, so a power cut can lose a code (the admin issues another) but
//! never lets one be used twice.

use super::*;
use crate::auth::{self, sha256};

/// Live codes at once (spec/03).
pub const MAX_CODES_LIVE: usize = 8;
pub const CODE_LIFETIME_MS: u64 = 7 * 24 * HOUR_MS;

/// What redeeming a code needs besides the code itself.
#[derive(Debug, Clone, Default)]
pub struct Redemption {
    pub password: String,
    /// Invites only.
    pub username: String,
    /// Invites only.
    pub display_name: String,
}

impl<S: Store> Service<S> {
    fn erase_code(&mut self, id: u64) -> Result<()> {
        self.erase(Ns::Inv, &keys::code(id))?;
        self.state.invites.remove(&id);
        Ok(())
    }

    /// Store a new code: expired ones go first, then the oldest if all
    /// eight are still live. A new reset code replaces an older one for
    /// the same member.
    fn issue_code(&mut self, actor: u8, kind: CodeKind, now_ms: u64) -> Result<(CodeRec, String)> {
        self.govern(Some(actor), now_ms)?;
        let stale: Vec<u64> = self
            .state
            .invites
            .values()
            .filter(|c| c.expires_ms <= now_ms || (kind != CodeKind::Invite && c.kind == kind))
            .map(|c| c.id)
            .collect();
        for id in stale {
            self.erase_code(id)?;
        }
        while self.state.invites.len() >= MAX_CODES_LIVE {
            let oldest = *self.state.invites.keys().next().ok_or(Error::NotFound)?;
            self.erase_code(oldest)?;
        }
        let code = auth::random_secret();
        let rec = CodeRec {
            id: self.next_id(now_ms),
            hash: sha256(&code),
            kind,
            expires_ms: now_ms + CODE_LIFETIME_MS,
        };
        self.put(Ns::Inv, &keys::code(rec.id), Kind::Code, &rec)?;
        self.state.invites.insert(rec.id, rec.clone());
        Ok((rec, code))
    }

    /// An admin invites a new member. Returns the record and the code.
    pub fn issue_invite(&mut self, actor: u8, now_ms: u64) -> Result<(CodeRec, String)> {
        self.require_admin(actor)?;
        self.issue_code(actor, CodeKind::Invite, now_ms)
    }

    /// An admin lets `target` choose a new password. The password does not
    /// change until the code is redeemed. The same rules as moderation
    /// apply: not yourself, not the owner, admins only by the owner.
    pub fn issue_reset(&mut self, actor: u8, target: u8, now_ms: u64) -> Result<(CodeRec, String)> {
        self.require_power_over(actor, target)?;
        let account_id = self.state.account(target).ok_or(Error::NotFound)?.rec.id;
        let kind = CodeKind::Reset {
            slot: target,
            account_id,
        };
        self.issue_code(actor, kind, now_ms)
    }

    /// Live codes, oldest first. Resets for deleted members are left out.
    pub fn live_codes(&self, now_ms: u64) -> impl Iterator<Item = &CodeRec> {
        self.state
            .invites
            .values()
            .filter(move |c| c.expires_ms > now_ms && self.code_target_ok(c))
    }

    fn code_target_ok(&self, rec: &CodeRec) -> bool {
        match rec.kind {
            CodeKind::Invite => true,
            CodeKind::Reset { slot, account_id } => {
                self.state.account(slot).map(|a| a.rec.id) == Some(account_id)
            }
        }
    }

    /// The live code matching `code`, if any.
    pub fn find_code(&self, code: &str, now_ms: u64) -> Option<&CodeRec> {
        let hash = sha256(code);
        self.live_codes(now_ms)
            .find(|c| auth::ct_eq(&c.hash, &hash))
    }

    pub fn revoke_code(&mut self, actor: u8, id: u64, now_ms: u64) -> Result<()> {
        self.require_admin(actor)?;
        if !self.state.invites.contains_key(&id) {
            return Err(Error::NotFound);
        }
        self.govern(Some(actor), now_ms)?;
        self.erase_code(id)
    }

    /// Redeem a code: create the invited member, or set a new password.
    /// Returns the member's slot.
    pub fn redeem_code(&mut self, code: &str, r: Redemption, now_ms: u64) -> Result<u8> {
        let rec = self
            .find_code(code, now_ms)
            .cloned()
            .ok_or_else(|| Error::Invalid("This code is not valid. It may have expired or been used already; ask an admin for a new one.".into()))?;
        match rec.kind {
            CodeKind::Invite => {
                let member = NewMember {
                    username: r.username.trim().to_ascii_lowercase(),
                    password: r.password,
                    display_name: r.display_name.trim().to_string(),
                    role: Role::Member,
                };
                self.check_new_member(&member)?;
                self.govern(None, now_ms)?;
                self.erase_code(rec.id)?;
                self.insert_account(member, now_ms)
            }
            CodeKind::Reset { slot, .. } => {
                auth::password_ok(&r.password).map_err(|m| Error::Invalid(m.into()))?;
                self.govern(Some(slot), now_ms)?;
                self.erase_code(rec.id)?;
                // Without the old password the member's key can't be
                // opened: they get a new one, and old direct messages stay
                // unreadable (spec/05 "Direct messages").
                self.replace_password(slot, &r.password, None)?;
                Ok(slot)
            }
        }
    }

    /// The member's signed-in devices (their tokens), newest first.
    pub fn devices(&self, slot: u8) -> Vec<&Token> {
        let mut out: Vec<&Token> = self
            .state
            .tokens
            .iter()
            .filter(|t| t.rec.slot == slot)
            .collect();
        out.sort_by_key(|t| core::cmp::Reverse(t.rec.id));
        out
    }

    /// Sign one of the member's own devices out.
    pub fn revoke_device(&mut self, slot: u8, token_id: u64, now_ms: u64) -> Result<()> {
        let hash = self
            .state
            .tokens
            .iter()
            .find(|t| t.rec.slot == slot && t.rec.id == token_id)
            .map(|t| t.rec.hash)
            .ok_or(Error::NotFound)?;
        self.govern(Some(slot), now_ms)?;
        self.forget_token(&hash)
    }
}
