//! Accounts: provisioning, members, profiles and password checks.

use super::*;
use crate::auth::{self, Verifier};
use crate::text::MAX_USERNAME;

pub const MAX_DISPLAY_NAME_CHARS: usize = 30;
pub const MAX_DISPLAY_NAME_BYTES: usize = 120;
pub const MAX_NOTE_CHARS: usize = 160;
pub const MAX_NOTE_BYTES: usize = 640;
pub const MAX_FIELDS: usize = 4;
pub const MAX_FIELD_NAME: usize = 40;
pub const MAX_FIELD_VALUE: usize = 100;
pub const MAX_TITLE: usize = 40;

#[derive(Debug, Clone)]
pub struct NewMember {
    pub username: String,
    pub password: String,
    pub display_name: String,
    pub role: Role,
}

/// Fields `update_credentials` may change. `None` leaves a field unchanged.
#[derive(Debug, Clone, Default)]
pub struct ProfileUpdate {
    pub display_name: Option<String>,
    pub note: Option<String>,
    pub locked: Option<bool>,
    pub bot: Option<bool>,
    pub discoverable: Option<bool>,
    pub fields: Option<Vec<Field>>,
    pub privacy: Option<Visibility>,
    pub sensitive: Option<bool>,
    pub language: Option<Option<String>>,
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

pub fn validate_username(username: &str) -> Result<()> {
    if username.is_empty()
        || username.len() > MAX_USERNAME
        || !username
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return invalid("Validation failed: Username must be 1-20 characters of a-z, 0-9 and _");
    }
    Ok(())
}

fn validate_display_name(name: &str) -> Result<()> {
    if chars(name) > MAX_DISPLAY_NAME_CHARS || name.len() > MAX_DISPLAY_NAME_BYTES {
        return invalid("Validation failed: Display name is too long (maximum is 30 characters)");
    }
    Ok(())
}

impl<S: Store> Service<S> {
    fn free_slot(&self) -> Result<u8> {
        (0..MAX_ACCOUNTS as u8)
            .find(|slot| self.state.accounts[*slot as usize].is_none())
            .ok_or_else(|| Error::Invalid("The household is full (16 accounts)".into()))
    }

    fn insert_account(&mut self, member: NewMember, now_ms: u64) -> Result<u8> {
        validate_username(&member.username)?;
        validate_display_name(&member.display_name)?;
        auth::password_ok(&member.password).map_err(|m| Error::Invalid(m.into()))?;
        if self.state.account_by_username(&member.username).is_some() {
            return invalid("Validation failed: Username has already been taken");
        }
        let slot = self.free_slot()?;
        let rec = AccountRec {
            id: self.next_id(now_ms),
            username: member.username,
            display_name: member.display_name,
            note: String::new(),
            fields: Vec::new(),
            role: member.role,
            disabled: false,
            deleted: false,
            locked: false,
            bot: false,
            discoverable: true,
            verifier: Verifier::new(&member.password, self.config.password_rounds),
            token_epoch: 0,
            privacy: Visibility::Public,
            sensitive: false,
            language: None,
        };
        self.put(Ns::Acct, &keys::account(slot), Kind::Account, &rec)?;
        self.state.accounts[slot as usize] = Some(Account { slot, rec });
        Ok(slot)
    }

    /// First boot only: create the owner. The schema marker is the commit
    /// point and is written last. A power cut before it leaves only server
    /// settings and the owner record, which `open` recognises as an
    /// interrupted provisioning and rolls back.
    pub fn provision(
        &mut self,
        owner: NewMember,
        title: Option<String>,
        now_ms: u64,
    ) -> Result<u8> {
        if self.state.provisioned {
            return Err(Error::Forbidden("This server is already set up".into()));
        }
        if let Some(title) = &title {
            if title.trim().is_empty() || title.len() > MAX_TITLE {
                return invalid("Validation failed: Title must be 1-40 bytes");
            }
        }
        // Validate everything before the first write.
        validate_username(&owner.username)?;
        validate_display_name(&owner.display_name)?;
        auth::password_ok(&owner.password).map_err(|m| Error::Invalid(m.into()))?;
        self.govern(None, now_ms)?;
        if let Some(title) = title {
            let mut server = self.state.server.clone();
            server.title = title;
            self.put(Ns::Cfg, &keys::server(), Kind::Server, &server)?;
            self.state.server = server;
        }
        let slot = self.insert_account(
            NewMember {
                role: Role::Owner,
                ..owner
            },
            now_ms,
        )?;
        self.put(Ns::Cfg, &keys::schema(), Kind::Schema, &SCHEMA)?;
        self.state.provisioned = true;
        Ok(slot)
    }

    pub fn create_member(&mut self, actor: u8, member: NewMember, now_ms: u64) -> Result<u8> {
        let role = self.state.account(actor).map(|a| a.rec.role);
        if !role.is_some_and(Role::is_admin) {
            return Err(Error::Forbidden(
                "Only an admin can add household members".into(),
            ));
        }
        if member.role == Role::Owner {
            return Err(Error::Forbidden("There is only one owner".into()));
        }
        self.govern(Some(actor), now_ms)?;
        self.insert_account(member, now_ms)
    }

    pub fn update_profile(&mut self, slot: u8, update: ProfileUpdate, now_ms: u64) -> Result<()> {
        let mut rec = self.state.account(slot).ok_or(Error::NotFound)?.rec.clone();
        if let Some(name) = update.display_name {
            validate_display_name(&name)?;
            rec.display_name = name;
        }
        if let Some(note) = update.note {
            if chars(&note) > MAX_NOTE_CHARS || note.len() > MAX_NOTE_BYTES {
                return invalid("Validation failed: Note is too long (maximum is 160 characters)");
            }
            rec.note = note;
        }
        if let Some(fields) = update.fields {
            if fields.len() > MAX_FIELDS {
                return invalid("Validation failed: Fields: at most 4 profile fields");
            }
            if fields
                .iter()
                .any(|f| f.name.len() > MAX_FIELD_NAME || f.value.len() > MAX_FIELD_VALUE)
            {
                return invalid("Validation failed: Fields are too long (40 / 100 bytes)");
            }
            rec.fields = fields
                .into_iter()
                .filter(|f| !f.name.is_empty() || !f.value.is_empty())
                .collect();
        }
        if let Some(v) = update.locked {
            rec.locked = v;
        }
        if let Some(v) = update.bot {
            rec.bot = v;
        }
        if let Some(v) = update.discoverable {
            rec.discoverable = v;
        }
        if let Some(v) = update.privacy {
            rec.privacy = v;
        }
        if let Some(v) = update.sensitive {
            rec.sensitive = v;
        }
        if let Some(v) = update.language {
            if v.as_ref().is_some_and(|l| l.len() > 8) {
                return invalid("Validation failed: Language is invalid");
            }
            rec.language = v;
        }
        let current = &self.state.account(slot).ok_or(Error::NotFound)?.rec;
        if *current == rec {
            return Ok(()); // nothing changed: no flash write
        }
        self.govern(Some(slot), now_ms)?;
        self.put(Ns::Acct, &keys::account(slot), Kind::Account, &rec)?;
        if let Some(account) = self.state.accounts[slot as usize].as_mut() {
            account.rec = rec;
        }
        Ok(())
    }

    /// Check a username/password, with the RAM-only lockout.
    pub fn check_password(&mut self, username: &str, password: &str, now_ms: u64) -> Result<u8> {
        let name = username.trim().to_ascii_lowercase();
        let lockouts = &mut self.state.lockouts;
        lockouts.retain(|(_, _, until)| *until == 0 || *until > now_ms);
        if lockouts
            .iter()
            .any(|(n, _, until)| *n == name && *until > now_ms)
        {
            return Err(Error::TooMany(
                "Too many failed sign-in attempts; wait five minutes".into(),
            ));
        }
        let found = self
            .state
            .account_by_username(&name)
            .filter(|a| !a.rec.disabled)
            .filter(|a| a.rec.verifier.verify(password))
            .map(|a| a.slot);
        match found {
            Some(slot) => {
                self.state.lockouts.retain(|(n, _, _)| *n != name);
                Ok(slot)
            }
            None => {
                let lockouts = &mut self.state.lockouts;
                match lockouts.iter().position(|(n, _, _)| *n == name) {
                    Some(i) => {
                        let entry = &mut lockouts[i];
                        entry.1 += 1;
                        if entry.1 >= auth::LOCKOUT_FAILURES {
                            entry.2 = now_ms + auth::LOCKOUT_MS;
                        }
                    }
                    None if lockouts.len() < 64 => lockouts.push((name, 1, 0)),
                    None => {}
                }
                Err(Error::Unauthorized)
            }
        }
    }

    /// Change a password and sign out every other session in one write.
    pub fn change_password(
        &mut self,
        slot: u8,
        current: &str,
        new: &str,
        now_ms: u64,
    ) -> Result<()> {
        let account = self.state.account(slot).ok_or(Error::NotFound)?;
        if !account.rec.verifier.verify(current) {
            return Err(Error::Forbidden("Current password is incorrect".into()));
        }
        auth::password_ok(new).map_err(|m| Error::Invalid(m.into()))?;
        let mut rec = account.rec.clone();
        rec.verifier = Verifier::new(new, self.config.password_rounds);
        rec.token_epoch = rec.token_epoch.wrapping_add(1);
        self.govern(Some(slot), now_ms)?;
        self.put(Ns::Acct, &keys::account(slot), Kind::Account, &rec)?;
        let epoch = rec.token_epoch;
        if let Some(a) = self.state.accounts[slot as usize].as_mut() {
            a.rec = rec;
        }
        // The epoch already invalidated them; erasing is tidy-up that boot
        // would otherwise do.
        let stale: Vec<[u8; 32]> = self
            .state
            .tokens
            .iter()
            .filter(|t| t.rec.slot == slot && t.rec.epoch != epoch)
            .map(|t| t.rec.hash)
            .collect();
        self.state
            .tokens
            .retain(|t| !(t.rec.slot == slot && t.rec.epoch != epoch));
        for hash in stale {
            self.erase(Ns::Tok, &keys::token(&hash))?;
        }
        Ok(())
    }

    /// Accounts matching a query by username or display name.
    pub fn search_accounts(&self, query: &str, limit: usize) -> Vec<u8> {
        let q = query.trim().trim_start_matches('@').to_lowercase();
        let q = q.split('@').next().unwrap_or("").to_string();
        self.state
            .active_accounts()
            .filter(|a| {
                q.is_empty()
                    || a.rec.username.contains(&q)
                    || a.rec.display_name.to_lowercase().contains(&q)
            })
            .map(|a| a.slot)
            .take(limit)
            .collect()
    }
}
