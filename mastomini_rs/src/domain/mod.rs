//! Household state in RAM, persisted through a [`Store`].
//!
//! Every mutation writes its durable record first, then updates RAM. Each
//! user action has one commit point; multi-key clean-up that a power cut can
//! interrupt is finished by [`Service::open`] (spec/02-storage.md).

mod accounts;
mod oauth;
pub mod query;
pub mod records;
mod statuses;

use crate::codec::{self, Kind};
use crate::ids::{self, IdGen};
use crate::store::{Key, Ns, Store, StoreError};
use records::*;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;

pub use accounts::{NewMember, ProfileUpdate};
pub use oauth::{parse_scopes, AuthCodeGrant, Principal, KNOWN_SCOPES, OOB};
pub use statuses::NewStatus;

pub const MAX_ACCOUNTS: usize = 16;
pub const MAX_STATUSES: usize = 4096;
pub const MAX_BOOSTS: usize = 2048;
pub const MAX_REACTIONS: usize = 16_384;
pub const MAX_PINS: usize = 5;
pub const MAX_APPS: usize = 32;
pub const MAX_TOKENS: usize = 64;
pub const MAX_TOKENS_PER_ACCOUNT: usize = 8;
pub const MAX_CODES: usize = 16;
pub const MAX_APP_TOKENS: usize = 32;
pub const MAX_NOTIFICATIONS: usize = 512;
pub const MAX_IDEMPOTENCY: usize = 256;
pub const IDEMPOTENCY_MS: u64 = 60 * 60 * 1000;
/// Evict before the store passes this fraction of its entries.
pub const LOW_WATERMARK: f32 = 0.80;
/// Flash write governor (spec/02 "Flash write governor").
pub const GOVERNOR_PER_ACCOUNT_HOUR: u32 = 120;
pub const GOVERNOR_GLOBAL_HOUR: u32 = 600;
const HOUR_MS: u64 = 60 * 60 * 1000;
pub const SCHEMA: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    NotFound,
    Unauthorized,
    Forbidden(String),
    /// 422: validation failed.
    Invalid(String),
    /// 429: governor or rate limit.
    TooMany(String),
    /// 503: clock invalid, or the store latched unavailable.
    Unavailable(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound => f.write_str("Record not found"),
            Error::Unauthorized => f.write_str("The access token is invalid"),
            Error::Forbidden(m) | Error::Invalid(m) | Error::TooMany(m) | Error::Unavailable(m) => {
                f.write_str(m)
            }
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;

pub(crate) fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(message.into()))
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Host used for `@user@host` mentions and URLs.
    pub host: String,
    pub password_rounds: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            host: "mastomini.local".into(),
            password_rounds: crate::auth::PASSWORD_ROUNDS,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Account {
    pub slot: u8,
    pub rec: AccountRec,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub rec: StatusRec,
    /// Slots of mentioned accounts.
    pub mentions: u16,
    pub tags: Vec<String>,
    pub favourited: u16,
    pub bookmarked: u16,
    pub pinned: u16,
    pub boosted: u16,
    pub replies: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReactionKind {
    Favourite,
    Bookmark,
    Pin,
}

impl ReactionKind {
    fn prefix(self) -> u8 {
        match self {
            ReactionKind::Favourite => b'f',
            ReactionKind::Bookmark => b'b',
            ReactionKind::Pin => b'p',
        }
    }

    fn from_prefix(byte: u8) -> Option<ReactionKind> {
        match byte {
            b'f' => Some(ReactionKind::Favourite),
            b'b' => Some(ReactionKind::Bookmark),
            b'p' => Some(ReactionKind::Pin),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reaction {
    pub kind: ReactionKind,
    pub status: u64,
    pub slot: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Mention,
    Favourite,
    Reblog,
    Follow,
}

impl NotificationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NotificationKind::Mention => "mention",
            NotificationKind::Favourite => "favourite",
            NotificationKind::Reblog => "reblog",
            NotificationKind::Follow => "follow",
        }
    }
}

/// Ephemeral: lives only in the RAM ring (spec/02 "Notifications are ephemeral").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notification {
    pub id: u64,
    pub kind: NotificationKind,
    pub to: u8,
    pub from: u8,
    pub status: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Marker {
    pub last_read_id: u64,
    pub version: u32,
    pub updated_ms: u64,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub rec: TokenRec,
    pub last_used_ms: u64,
}

#[derive(Debug, Clone)]
struct AuthCode {
    code_hash: [u8; 32],
    app_id: u64,
    slot: u8,
    redirect_uri: String,
    scopes: Vec<String>,
    challenge: Option<String>,
    expires_ms: u64,
}

/// App-only tokens from `client_credentials` are ephemeral.
#[derive(Debug, Clone)]
struct AppToken {
    hash: [u8; 32],
    app_id: u64,
    scopes: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct Governor {
    window_start_ms: u64,
    global: u32,
    per_account: [u32; MAX_ACCOUNTS],
}

/// Everything the server knows, in RAM.
#[derive(Debug, Default)]
pub struct State {
    pub server: ServerRec,
    pub provisioned: bool,
    pub accounts: [Option<Account>; MAX_ACCOUNTS],
    pub apps: BTreeMap<u64, AppRec>,
    pub tokens: Vec<Token>,
    app_tokens: VecDeque<AppToken>,
    pub statuses: BTreeMap<u64, Status>,
    pub boosts: BTreeMap<u64, BoostRec>,
    /// Reaction record id -> reaction.
    pub reactions: BTreeMap<u64, Reaction>,
    reaction_ids: BTreeMap<(ReactionKind, u64, u8), u64>,
    pub follows: BTreeMap<(u8, u8), FollowRec>,
    pub notifications: VecDeque<Notification>,
    /// Per account: [home, notifications].
    pub markers: [[Option<Marker>; 2]; MAX_ACCOUNTS],
    codes: Vec<AuthCode>,
    lockouts: Vec<(String, u32, u64)>,
    idempotency: VecDeque<(u8, String, u64, u64)>,
    governor: Governor,
    pub evictions: u64,
    pub repairs: u64,
}

impl State {
    pub fn account(&self, slot: u8) -> Option<&Account> {
        self.accounts
            .get(slot as usize)?
            .as_ref()
            .filter(|a| !a.rec.deleted)
    }

    pub fn active_accounts(&self) -> impl Iterator<Item = &Account> {
        self.accounts.iter().flatten().filter(|a| !a.rec.deleted)
    }

    pub fn account_by_id(&self, id: u64) -> Option<&Account> {
        self.active_accounts().find(|a| a.rec.id == id)
    }

    pub fn account_by_username(&self, username: &str) -> Option<&Account> {
        self.active_accounts()
            .find(|a| a.rec.username.eq_ignore_ascii_case(username))
    }

    pub fn slot_of(&self, account_id: u64) -> Option<u8> {
        self.account_by_id(account_id).map(|a| a.slot)
    }

    pub fn follows(&self, src: u8, dst: u8) -> bool {
        self.follows.contains_key(&(src, dst))
    }

    fn mention_mask(&self, ids: &[u64]) -> u16 {
        ids.iter()
            .filter_map(|id| self.slot_of(*id))
            .fold(0, |mask, slot| mask | bit(slot))
    }
}

pub fn bit(slot: u8) -> u16 {
    1 << slot
}

fn hex(slot: u8) -> u8 {
    b"0123456789abcdef"[(slot & 15) as usize]
}

fn unhex(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|d| d as u8)
}

pub(crate) mod keys {
    use super::*;

    pub fn schema() -> Key {
        Key::new("schema").expect("static key")
    }
    pub fn server() -> Key {
        Key::new("server").expect("static key")
    }
    pub fn account(slot: u8) -> Key {
        Key::from_parts(&[b"a", &[hex(slot)]]).expect("static key")
    }
    pub fn app(id: u64) -> Key {
        Key::from_parts(&[b"p", &ids::b32(id)]).expect("fits")
    }
    pub fn token(hash: &[u8; 32]) -> Key {
        let mut part = [0u8; 14];
        for (i, byte) in hash.iter().take(7).enumerate() {
            part[2 * i] = hex(byte >> 4);
            part[2 * i + 1] = hex(byte & 15);
        }
        Key::from_parts(&[b"t", &part]).expect("fits")
    }
    pub fn status(id: u64) -> Key {
        Key::from_parts(&[b"s", &ids::b32(id)]).expect("fits")
    }
    pub fn boost(id: u64) -> Key {
        Key::from_parts(&[b"r", &ids::b32(id)]).expect("fits")
    }
    pub fn reaction(kind: ReactionKind, status: u64, slot: u8) -> Key {
        Key::from_parts(&[&[kind.prefix()], &ids::b32(status), &[hex(slot)]]).expect("fits")
    }
    pub fn follow(src: u8, dst: u8) -> Key {
        Key::from_parts(&[b"F", &[hex(src)], &[hex(dst)]]).expect("fits")
    }
}

pub struct Service<S: Store> {
    store: S,
    pub state: State,
    ids: IdGen,
    pub config: Config,
    latched: Option<String>,
}

fn load(store: &mut impl Store, ns: Ns) -> core::result::Result<Vec<(Key, Vec<u8>)>, StoreError> {
    let mut out = Vec::new();
    store.for_each(ns, &mut |key, value| {
        out.push((*key, value.to_vec()));
        Ok(())
    })?;
    Ok(out)
}

fn corrupt(key: &Key, what: impl fmt::Display) -> StoreError {
    StoreError::Corrupt(format!("{key}: {what}"))
}

impl<S: Store> Service<S> {
    /// Boot: enumerate the store, rebuild RAM state, repair interrupted
    /// multi-key operations, then check invariants. Never formats.
    pub fn open(mut store: S, config: Config) -> core::result::Result<Self, StoreError> {
        let mut state = State::default();
        let mut idgen = IdGen::default();

        match store.get(Ns::Cfg, &keys::schema())? {
            Some(bytes) => {
                let schema: u16 = codec::decode(Kind::Schema, &bytes)?;
                if schema > SCHEMA {
                    return Err(StoreError::Corrupt(format!(
                        "store schema {schema} is newer than this firmware ({SCHEMA}); downgrade refused"
                    )));
                }
                state.provisioned = true;
            }
            None => {
                // Without the schema marker the only legitimate leftovers are
                // from an interrupted `provision`: server settings and the
                // owner. Roll those back. Anything else is not ours to erase.
                let cfg = load(&mut store, Ns::Cfg)?;
                let accounts = load(&mut store, Ns::Acct)?;
                let others = Ns::ALL
                    .iter()
                    .filter(|ns| !matches!(ns, Ns::Cfg | Ns::Acct))
                    .try_fold(0, |n, ns| load(&mut store, *ns).map(|v| n + v.len()))?;
                let only_owner = accounts.len() <= 1
                    && accounts.iter().all(|(_, bytes)| {
                        codec::decode::<AccountRec>(Kind::Account, bytes)
                            .is_ok_and(|a| a.role == Role::Owner)
                    });
                if others > 0 || !only_owner || cfg.iter().any(|(k, _)| *k != keys::server()) {
                    return Err(StoreError::Corrupt(
                        "store has data but no schema record".into(),
                    ));
                }
                for (key, _) in accounts {
                    store.erase(Ns::Acct, &key)?;
                    state.repairs += 1;
                }
                for (key, _) in cfg {
                    store.erase(Ns::Cfg, &key)?;
                    state.repairs += 1;
                }
            }
        }
        if let Some(bytes) = store.get(Ns::Cfg, &keys::server())? {
            state.server = codec::decode(Kind::Server, &bytes)?;
        }

        for (key, bytes) in load(&mut store, Ns::Acct)? {
            let rec: AccountRec = codec::decode(Kind::Account, &bytes)?;
            let k = key.as_str().as_bytes();
            let slot = match k {
                [b'a', h] => unhex(*h).ok_or_else(|| corrupt(&key, "bad slot"))?,
                _ => return Err(corrupt(&key, "unexpected key")),
            };
            idgen.observe(rec.id);
            state.accounts[slot as usize] = Some(Account { slot, rec });
        }

        for (key, bytes) in load(&mut store, Ns::App)? {
            let rec: AppRec = codec::decode(Kind::App, &bytes)?;
            if keys::app(rec.id) != key {
                return Err(corrupt(&key, "id does not match key"));
            }
            idgen.observe(rec.id);
            state.apps.insert(rec.id, rec);
        }

        let mut repairs: Vec<(Ns, Key)> = Vec::new();

        for (key, bytes) in load(&mut store, Ns::Tok)? {
            let rec: TokenRec = codec::decode(Kind::Token, &bytes)?;
            idgen.observe(rec.id);
            let valid = state
                .account(rec.slot)
                .is_some_and(|a| a.rec.token_epoch == rec.epoch)
                && state.apps.contains_key(&rec.app_id);
            if valid {
                state.tokens.push(Token {
                    rec,
                    last_used_ms: 0,
                });
            } else {
                repairs.push((Ns::Tok, key));
            }
        }

        for (key, bytes) in load(&mut store, Ns::Stat)? {
            match key.as_str().as_bytes().first() {
                Some(b's') => {
                    let rec: StatusRec = codec::decode(Kind::Status, &bytes)?;
                    idgen.observe(rec.id);
                    if state.account(rec.author).is_none() {
                        repairs.push((Ns::Stat, key));
                        continue;
                    }
                    let tags = crate::text::tags(&rec.text);
                    state.statuses.insert(
                        rec.id,
                        Status {
                            mentions: 0,
                            tags,
                            favourited: 0,
                            bookmarked: 0,
                            pinned: 0,
                            boosted: 0,
                            replies: 0,
                            rec,
                        },
                    );
                }
                Some(b'r') => {
                    let rec: BoostRec = codec::decode(Kind::Boost, &bytes)?;
                    idgen.observe(rec.id);
                    state.boosts.insert(rec.id, rec);
                }
                _ => return Err(corrupt(&key, "unexpected key")),
            }
        }
        // Derived fields need every status loaded first.
        let masks: Vec<(u64, u16, Option<u64>)> = state
            .statuses
            .values()
            .map(|s| {
                (
                    s.rec.id,
                    state.mention_mask(&s.rec.mentions),
                    s.rec.in_reply_to_id,
                )
            })
            .collect();
        for (id, mask, parent) in masks {
            if let Some(s) = state.statuses.get_mut(&id) {
                s.mentions = mask;
            }
            if let Some(p) = parent.and_then(|p| state.statuses.get_mut(&p)) {
                p.replies += 1;
            }
        }
        let boosts: Vec<BoostRec> = state.boosts.values().copied().collect();
        for boost in boosts {
            let live = state.account(boost.booster).is_some();
            match state.statuses.get_mut(&boost.target) {
                Some(target) if live && target.boosted & bit(boost.booster) == 0 => {
                    target.boosted |= bit(boost.booster);
                }
                _ => {
                    state.boosts.remove(&boost.id);
                    repairs.push((Ns::Stat, keys::boost(boost.id)));
                }
            }
        }

        for (key, bytes) in load(&mut store, Ns::Rx)? {
            let rec: ReactionRec = codec::decode(Kind::Reaction, &bytes)?;
            let k = key.as_str();
            let parsed = (|| {
                let kind = ReactionKind::from_prefix(*k.as_bytes().first()?)?;
                let status = ids::from_b32(k.get(1..14)?)?;
                let slot = unhex(*k.as_bytes().get(14)?)?;
                (k.len() == 15).then_some(Reaction { kind, status, slot })
            })()
            .ok_or_else(|| corrupt(&key, "unparsable reaction key"))?;
            idgen.observe(rec.id);
            let live = state.account(parsed.slot).is_some();
            match state.statuses.get_mut(&parsed.status) {
                Some(status) if live => {
                    let mask = match parsed.kind {
                        ReactionKind::Favourite => &mut status.favourited,
                        ReactionKind::Bookmark => &mut status.bookmarked,
                        ReactionKind::Pin => &mut status.pinned,
                    };
                    *mask |= bit(parsed.slot);
                    state.reactions.insert(rec.id, parsed);
                    state
                        .reaction_ids
                        .insert((parsed.kind, parsed.status, parsed.slot), rec.id);
                }
                _ => repairs.push((Ns::Rx, key)),
            }
        }

        for (key, bytes) in load(&mut store, Ns::Rel)? {
            let rec: FollowRec = codec::decode(Kind::Follow, &bytes)?;
            let k = key.as_str().as_bytes();
            let (src, dst) = match k {
                [b'F', s, d] => (
                    unhex(*s).ok_or_else(|| corrupt(&key, "bad slot"))?,
                    unhex(*d).ok_or_else(|| corrupt(&key, "bad slot"))?,
                ),
                _ => return Err(corrupt(&key, "unexpected key")),
            };
            idgen.observe(rec.id);
            if state.account(src).is_some() && state.account(dst).is_some() && src != dst {
                state.follows.insert((src, dst), rec);
            } else {
                repairs.push((Ns::Rel, key));
            }
        }

        // Tombstoned accounts: everything that referenced them was skipped
        // above and queued for erasure; finally erase the account itself.
        for account in state.accounts.iter().flatten() {
            if account.rec.deleted {
                repairs.push((Ns::Acct, keys::account(account.slot)));
            }
        }

        state.repairs += repairs.len() as u64;
        for (ns, key) in &repairs {
            store.erase(*ns, key)?;
        }
        for slot in 0..MAX_ACCOUNTS {
            if state.accounts[slot].as_ref().is_some_and(|a| a.rec.deleted) {
                state.accounts[slot] = None;
            }
        }

        let service = Service {
            store,
            state,
            ids: idgen,
            config,
            latched: None,
        };
        service.check_invariants()?;
        Ok(service)
    }

    /// Fail closed if RAM state breaks a bound or an internal link.
    pub fn check_invariants(&self) -> core::result::Result<(), StoreError> {
        let s = &self.state;
        let fail = |what: &str| Err(StoreError::Corrupt(format!("invariant: {what}")));
        if s.statuses.len() > MAX_STATUSES
            || s.boosts.len() > MAX_BOOSTS
            || s.reactions.len() > MAX_REACTIONS
            || s.apps.len() > MAX_APPS
            || s.tokens.len() > MAX_TOKENS
        {
            return fail("capacity exceeded");
        }
        let mut usernames: Vec<&str> = s
            .active_accounts()
            .map(|a| a.rec.username.as_str())
            .collect();
        usernames.sort_unstable();
        if usernames
            .windows(2)
            .any(|w| w[0].eq_ignore_ascii_case(w[1]))
        {
            return fail("duplicate username");
        }
        if s.provisioned
            && !s
                .active_accounts()
                .any(|a| a.rec.role == Role::Owner && !a.rec.disabled)
        {
            return fail("no enabled owner");
        }
        for (id, reaction) in &s.reactions {
            if s.reaction_ids
                .get(&(reaction.kind, reaction.status, reaction.slot))
                != Some(id)
            {
                return fail("reaction index");
            }
        }
        Ok(())
    }

    pub fn store(&mut self) -> &mut S {
        &mut self.store
    }

    pub fn into_store(self) -> S {
        self.store
    }

    pub fn latched(&self) -> Option<&str> {
        self.latched.as_deref()
    }

    pub fn last_id(&self) -> u64 {
        self.ids.last()
    }

    pub(crate) fn next_id(&mut self, now_ms: u64) -> u64 {
        self.ids.next(now_ms)
    }

    fn map_store_error(&mut self, e: StoreError) -> Error {
        match e {
            StoreError::Full => Error::Invalid("The server's storage is full".into()),
            StoreError::BadKey | StoreError::ValueTooLarge => {
                Error::Invalid(format!("Record rejected by storage: {e}"))
            }
            StoreError::Io(_) | StoreError::Corrupt(_) => {
                let message = format!("Storage failure, restart required: {e}");
                self.latched = Some(message.clone());
                Error::Unavailable(message)
            }
        }
    }

    /// Refuse writes after a storage failure until restart.
    pub(crate) fn writable(&self) -> Result<()> {
        match &self.latched {
            Some(message) => Err(Error::Unavailable(message.clone())),
            None => Ok(()),
        }
    }

    pub(crate) fn put<T: serde::Serialize>(
        &mut self,
        ns: Ns,
        key: &Key,
        kind: Kind,
        value: &T,
    ) -> Result<()> {
        self.writable()?;
        let bytes = codec::encode(kind, value).map_err(|e| self.map_store_error(e))?;
        self.store
            .set(ns, key, &bytes)
            .map_err(|e| self.map_store_error(e))
    }

    pub(crate) fn erase(&mut self, ns: Ns, key: &Key) -> Result<()> {
        self.writable()?;
        self.store
            .erase(ns, key)
            .map_err(|e| self.map_store_error(e))
    }

    /// Count one flash-writing action against the hourly budgets.
    pub(crate) fn govern(&mut self, slot: Option<u8>, now_ms: u64) -> Result<()> {
        let g = &mut self.state.governor;
        if now_ms >= g.window_start_ms + HOUR_MS || now_ms < g.window_start_ms {
            *g = Governor {
                window_start_ms: now_ms,
                ..Governor::default()
            };
        }
        if g.global >= GOVERNOR_GLOBAL_HOUR {
            return Err(Error::TooMany(
                "Too many writes this hour; try again later".into(),
            ));
        }
        if let Some(slot) = slot {
            if g.per_account[slot as usize] >= GOVERNOR_PER_ACCOUNT_HOUR {
                return Err(Error::TooMany(
                    "Too many writes this hour; try again later".into(),
                ));
            }
            g.per_account[slot as usize] += 1;
        }
        g.global += 1;
        Ok(())
    }

    pub fn governor_counts(&self) -> (u32, u64) {
        (
            self.state.governor.global,
            self.state.governor.window_start_ms,
        )
    }

    fn over_watermark(&mut self) -> Result<bool> {
        let stats = self.store.stats().map_err(|e| self.map_store_error(e))?;
        Ok(stats.used_fraction() >= LOW_WATERMARK)
    }

    /// Ring eviction before writing one more record of `kind`
    /// (spec/02 "Eviction").
    pub(crate) fn make_room(&mut self, kind: Room) -> Result<()> {
        for _ in 0..MAX_STATUSES {
            let boosts_full = kind == Room::Boost && self.state.boosts.len() >= MAX_BOOSTS;
            let full = match kind {
                Room::Status => self.state.statuses.len() >= MAX_STATUSES,
                Room::Boost => boosts_full,
                Room::Reaction => self.state.reactions.len() >= MAX_REACTIONS,
            };
            if !full && !self.over_watermark()? {
                return Ok(());
            }
            if boosts_full {
                let oldest = *self.state.boosts.keys().next().ok_or(Error::NotFound)?;
                self.remove_boost(oldest)?;
            } else {
                let victim = self
                    .state
                    .statuses
                    .values()
                    .find(|s| s.pinned == 0)
                    .map(|s| s.rec.id)
                    .ok_or_else(|| Error::Invalid("The server's storage is full".into()))?;
                self.remove_status(victim)?;
            }
            self.state.evictions += 1;
        }
        Err(Error::Invalid("The server's storage is full".into()))
    }

    pub(crate) fn notify(
        &mut self,
        kind: NotificationKind,
        to: u8,
        from: u8,
        status: Option<u64>,
        now_ms: u64,
    ) {
        if to == from {
            return;
        }
        if self.state.notifications.len() >= MAX_NOTIFICATIONS {
            self.state.notifications.pop_front();
        }
        let id = self.ids.next(now_ms);
        self.state.notifications.push_back(Notification {
            id,
            kind,
            to,
            from,
            status,
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Room {
    Status,
    Boost,
    Reaction,
}

#[cfg(test)]
mod tests;
