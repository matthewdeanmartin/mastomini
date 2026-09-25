//! Household state in RAM, persisted through a [`Store`].
//!
//! Every mutation writes its durable record first, then updates RAM. Each
//! user action has one commit point; multi-key clean-up that a power cut can
//! interrupt is finished by [`Service::open`] (spec/02-storage.md).

mod accounts;
mod codes;
mod collections;
mod conversations;
mod dm;
mod edits;
mod filters;
mod follow_requests;
mod lists;
mod moderation;
mod oauth;
mod polls;
pub mod query;
pub mod records;
mod server;
pub mod social;
mod statuses;

use crate::codec::{self, Kind};
use crate::ids::{self, IdGen};
use crate::store::{Key, Ns, Store, StoreError};
use records::*;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;

pub use accounts::{NewMember, ProfileUpdate};
pub use codes::{Redemption, CODE_LIFETIME_MS, MAX_CODES_LIVE};
pub use collections::{CollectionUpdate, NewCollection};
pub use conversations::Conversation;
pub use dm::{Held, UserKeyRec, LOCKED_TEXT};
pub use edits::StatusEdit;
pub use filters::{filter_context_bit, FilterMatch};
pub use lists::ListUpdate;
pub use moderation::{AdminAction, NewReport};
pub use oauth::{parse_scopes, AuthCodeGrant, Principal, KNOWN_SCOPES, OOB};
pub use polls::NewPoll;
pub use server::ServerUpdate;
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
/// Reports kept in flash. At the limit the oldest resolved one is dropped.
pub const MAX_REPORTS: usize = 32;
pub const MAX_COLLECTIONS_PER_ACCOUNT: usize = 8;
/// Evict before the store passes this fraction of its entries.
pub const LOW_WATERMARK: f32 = 0.80;
/// Flash write governor (spec/02 "Flash write governor").
pub const GOVERNOR_PER_ACCOUNT_HOUR: u32 = 120;
pub const GOVERNOR_GLOBAL_HOUR: u32 = 600;
const HOUR_MS: u64 = 60 * 60 * 1000;
/// 2: blocks, mutes, conversation mutes, moderation, reports, collections.
/// 3: invite and reset codes (`mm_inv`).
/// 4: edit history (`mm_hist`), polls, follow requests, lists (`mm_list`),
/// filters (`mm_filt`).
/// An older store is upgraded in place at boot (only the marker changes),
/// so older firmware refuses the store instead of misreading new records.
/// 5: local tag preferences, notes, suggestion dismissals and announcements.
pub const SCHEMA: u16 = 5;
/// Previous versions kept per edited status, and in all (spec/03).
pub const MAX_REVISIONS_PER_STATUS: u8 = 3;
pub const MAX_REVISIONS: usize = 1024;
pub const MAX_LISTS_PER_ACCOUNT: u8 = 8;
pub const MAX_FILTERS_PER_ACCOUNT: u8 = 8;
pub const MAX_FILTER_KEYWORDS: usize = 4;
pub const MAX_FILTER_STATUSES: usize = 4;
pub const MAX_POLL_OPTIONS: usize = 4;
pub const MAX_POLL_OPTION_CHARS: usize = 50;
pub const POLL_MIN_SECONDS: u64 = 300;
pub const POLL_MAX_SECONDS: u64 = 7 * 24 * 60 * 60;

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
    /// Accounts that muted the conversation, recorded on the status they
    /// muted (the thread root when it was still there).
    pub muted: u16,
    pub replies: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReactionKind {
    Favourite,
    Bookmark,
    Pin,
    /// Conversation mute (`POST /api/v1/statuses/:id/mute`).
    Mute,
}

impl ReactionKind {
    fn prefix(self) -> u8 {
        match self {
            ReactionKind::Favourite => b'f',
            ReactionKind::Bookmark => b'b',
            ReactionKind::Pin => b'p',
            ReactionKind::Mute => b'm',
        }
    }

    fn from_prefix(byte: u8) -> Option<ReactionKind> {
        match byte {
            b'f' => Some(ReactionKind::Favourite),
            b'b' => Some(ReactionKind::Bookmark),
            b'p' => Some(ReactionKind::Pin),
            b'm' => Some(ReactionKind::Mute),
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
    /// To every admin; `object` is the report id.
    AdminReport,
    /// To a featured account; `object` is the collection id.
    AddedToCollection,
    /// A poll you made or voted in has ended.
    Poll,
    /// A status you boosted was edited.
    Update,
    /// Someone asked to follow your locked account.
    FollowRequest,
}

impl NotificationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NotificationKind::Mention => "mention",
            NotificationKind::Favourite => "favourite",
            NotificationKind::Reblog => "reblog",
            NotificationKind::Follow => "follow",
            NotificationKind::AdminReport => "admin.report",
            NotificationKind::AddedToCollection => "added_to_collection",
            NotificationKind::Poll => "poll",
            NotificationKind::Update => "update",
            NotificationKind::FollowRequest => "follow_request",
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
    /// Report or collection id, depending on `kind`.
    pub object: Option<u64>,
}

/// A member's view of one direct-message conversation (RAM only, like
/// markers: a restart shows everything as read and nothing as hidden).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConversationState {
    /// Newest status the member has marked read.
    pub read_up_to: u64,
    /// "Delete": hidden until a newer message arrives.
    pub hidden_up_to: u64,
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
    /// The member's secret, unlocked by the password on the sign-in form,
    /// to seal for the token this code turns into.
    secret: Option<Held>,
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
    /// Exact derived read index. Invalidated by durable account/status/follow
    /// mutations and rebuilt lazily in one pass; never a timed/stale cache.
    pub(crate) account_stats: std::cell::RefCell<Option<[query::AccountStats; MAX_ACCOUNTS]>>,
    /// Only enabled inside api::handle, cleared before and after every request.
    pub(crate) rendered_accounts: std::cell::RefCell<Option<BTreeMap<u8, serde_json::Value>>>,
    pub social: BTreeMap<u8, social::SocialRec>,
    pub account_notes: BTreeMap<(u8, u8), social::NoteRec>,
    pub announcements: BTreeMap<u64, social::AnnouncementRec>,
    pub server: ServerRec,
    pub terms: Option<TermsRec>,
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
    /// (blocker, blocked)
    pub blocks: BTreeMap<(u8, u8), BlockRec>,
    /// (muter, muted)
    pub mutes: BTreeMap<(u8, u8), MuteRec>,
    pub moderation: [Option<AccountModRec>; MAX_ACCOUNTS],
    pub reports: BTreeMap<u64, ReportRec>,
    pub collections: BTreeMap<u64, CollectionRec>,
    /// Invite and password-reset codes, by id.
    pub invites: BTreeMap<u64, CodeRec>,
    /// Previous versions of edited statuses: (status id, 0-2).
    pub history: BTreeMap<(u64, u8), RevisionRec>,
    /// Polls by status id.
    pub polls: BTreeMap<u64, PollRec>,
    /// Polls whose end has been announced (RAM; primed at the first tick).
    pub polls_announced: std::collections::BTreeSet<u64>,
    polls_primed: bool,
    /// None means recompute the next deadline after a poll mutation.
    next_poll_check: Option<u64>,
    /// (requester, locked account)
    pub follow_requests: BTreeMap<(u8, u8), FollowRec>,
    /// (owner slot, 0-7)
    pub lists: BTreeMap<(u8, u8), ListRec>,
    /// (owner slot, 0-7)
    pub filters: BTreeMap<(u8, u8), FilterRec>,
    /// (member slot, conversation root status id)
    pub conversations: BTreeMap<(u8, u64), ConversationState>,
    /// A time set by an admin's browser while the clock isn't synced:
    /// (wall clock ms, platform uptime ms when set). RAM only.
    pub clock_anchor: Option<(u64, u64)>,
    /// Wall clock of the request being handled, for mute expiry.
    pub now_ms: u64,
    pub user_keys: [Option<UserKeyRec>; MAX_ACCOUNTS],
    /// Device seals by their `mm_key` key (`w` + token key suffix).
    pub token_seals: BTreeMap<Key, crate::crypto::Sealed>,
    /// Envelopes of direct messages, by status id.
    pub dms: BTreeMap<u64, crate::crypto::Envelope>,
    /// The caller's unlocked secret, for the current request only.
    pub session: Option<dm::Session>,
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

    pub fn blocks(&self, src: u8, dst: u8) -> bool {
        self.blocks.contains_key(&(src, dst))
    }

    /// Either account blocks the other.
    pub fn blocked_either(&self, a: u8, b: u8) -> bool {
        self.blocks(a, b) || self.blocks(b, a)
    }

    /// An unexpired mute of `dst` by `src`.
    pub fn mute(&self, src: u8, dst: u8) -> Option<&MuteRec> {
        self.mutes
            .get(&(src, dst))
            .filter(|m| m.expires_ms.is_none_or(|e| e > self.now_ms))
    }

    /// Should `viewer` not see `author` in timelines, threads and
    /// notifications: blocked either way, or muted by the viewer.
    pub fn hides(&self, viewer: u8, author: u8) -> bool {
        viewer != author
            && (self.blocked_either(viewer, author) || self.mute(viewer, author).is_some())
    }

    /// Moderation state of a live account (all clear if none recorded).
    pub fn moderation(&self, slot: u8) -> AccountModRec {
        let id = self.account(slot).map(|a| a.rec.id);
        self.moderation
            .get(slot as usize)
            .copied()
            .flatten()
            .filter(|m| Some(m.account_id) == id)
            .unwrap_or_default()
    }

    pub fn suspended(&self, slot: u8) -> bool {
        self.moderation(slot).suspended_ms.is_some()
    }

    pub fn silenced(&self, slot: u8) -> bool {
        self.moderation(slot).silenced
    }

    /// Mask of accounts `viewer` blocks, is blocked by, or mutes.
    pub fn hidden_mask(&self, viewer: u8) -> u16 {
        (0..MAX_ACCOUNTS as u8)
            .filter(|s| self.hides(viewer, *s))
            .fold(0, |m, s| m | bit(s))
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
    pub fn terms() -> Key {
        Key::new("terms").expect("static key")
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
    pub fn block(src: u8, dst: u8) -> Key {
        Key::from_parts(&[b"B", &[hex(src)], &[hex(dst)]]).expect("fits")
    }
    pub fn mute(src: u8, dst: u8) -> Key {
        Key::from_parts(&[b"M", &[hex(src)], &[hex(dst)]]).expect("fits")
    }
    pub fn account_mod(slot: u8) -> Key {
        Key::from_parts(&[b"m", &[hex(slot)]]).expect("fits")
    }
    pub fn report(id: u64) -> Key {
        Key::from_parts(&[b"R", &ids::b32(id)]).expect("fits")
    }
    pub fn collection(id: u64) -> Key {
        Key::from_parts(&[b"c", &ids::b32(id)]).expect("fits")
    }
    pub fn user_key(slot: u8) -> Key {
        Key::from_parts(&[b"k", &[hex(slot)]]).expect("fits")
    }
    /// The device seal for a token: `w` + the token key's suffix.
    pub fn token_seal(hash: &[u8; 32]) -> Key {
        let token = token(hash);
        Key::from_parts(&[b"w", &token.as_str().as_bytes()[1..]]).expect("fits")
    }
    pub fn dm(id: u64) -> Key {
        Key::from_parts(&[b"d", &ids::b32(id)]).expect("fits")
    }
    pub fn code(id: u64) -> Key {
        Key::from_parts(&[b"i", &ids::b32(id)]).expect("fits")
    }
    pub fn revision(status: u64, n: u8) -> Key {
        Key::from_parts(&[b"h", &ids::b32(status), &[b'0' + n]]).expect("fits")
    }
    pub fn poll(status: u64) -> Key {
        Key::from_parts(&[b"o", &ids::b32(status)]).expect("fits")
    }
    pub fn follow_request(src: u8, dst: u8) -> Key {
        Key::from_parts(&[b"Q", &[hex(src)], &[hex(dst)]]).expect("fits")
    }
    pub fn list(slot: u8, n: u8) -> Key {
        Key::from_parts(&[b"l", &[hex(slot)], &[hex(n)]]).expect("fits")
    }
    pub fn filter(slot: u8, n: u8) -> Key {
        Key::from_parts(&[b"x", &[hex(slot)], &[hex(n)]]).expect("fits")
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
                if schema < SCHEMA {
                    let marker = codec::encode(Kind::Schema, &SCHEMA)?;
                    store.set(Ns::Cfg, &keys::schema(), &marker)?;
                }
            }
            None => {
                // Without the schema marker the only legitimate leftovers are
                // from an interrupted `provision`: server settings and the
                // owner. Roll those back. Anything else is not ours to erase.
                let cfg = load(&mut store, Ns::Cfg)?;
                let accounts = load(&mut store, Ns::Acct)?;
                let others = Ns::ALL
                    .iter()
                    .filter(|ns| !matches!(ns, Ns::Cfg | Ns::Acct | Ns::Key))
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
                for (key, _) in load(&mut store, Ns::Key)? {
                    store.erase(Ns::Key, &key)?;
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
        if let Some(bytes) = store.get(Ns::Cfg, &keys::terms())? {
            state.terms = Some(codec::decode(Kind::Terms, &bytes)?);
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

        let mut envelopes = Vec::new();
        let mut polls = Vec::new();
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
                            muted: 0,
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
                Some(b'o') => {
                    let poll: PollRec = codec::decode(Kind::Poll, &bytes)?;
                    let id = key
                        .as_str()
                        .get(1..)
                        .and_then(ids::from_b32)
                        .ok_or_else(|| corrupt(&key, "bad status id"))?;
                    polls.push((key, id, poll));
                }
                Some(b'd') => {
                    let envelope: crate::crypto::Envelope = codec::decode(Kind::Dm, &bytes)?;
                    let id = key
                        .as_str()
                        .get(1..)
                        .and_then(ids::from_b32)
                        .ok_or_else(|| corrupt(&key, "bad status id"))?;
                    envelopes.push((key, id, envelope));
                }
                _ => return Err(corrupt(&key, "unexpected key")),
            }
        }
        // The status record is the commit point; an envelope without one
        // is from an interrupted post or delete.
        for (key, id, envelope) in envelopes {
            if state.statuses.contains_key(&id) {
                state.dms.insert(id, envelope);
            } else {
                repairs.push((Ns::Stat, key));
            }
        }
        // Polls are written before their status, too.
        for (key, id, poll) in polls {
            if state.statuses.contains_key(&id) {
                state.polls.insert(id, poll);
            } else {
                repairs.push((Ns::Stat, key));
            }
        }
        // History: a revision is written before the status it came from is
        // overwritten, so one that is not older than the current version is
        // from an interrupted edit (spec/02 "Crash consistency").
        for (key, bytes) in load(&mut store, Ns::Hist)? {
            let rec: RevisionRec = codec::decode(Kind::Revision, &bytes)?;
            let k = key.as_str();
            let parsed = (|| {
                let status = ids::from_b32(k.get(1..14)?)?;
                let n = k.as_bytes().get(14)?.checked_sub(b'0')?;
                (k.len() == 15 && k.starts_with('h') && n < MAX_REVISIONS_PER_STATUS)
                    .then_some((status, n))
            })()
            .ok_or_else(|| corrupt(&key, "unparsable revision key"))?;
            let current_since = state
                .statuses
                .get(&parsed.0)
                .map(|s| s.rec.edited_at_ms.unwrap_or(ids::millis(s.rec.id)));
            match current_since {
                Some(since) if rec.created_ms < since => {
                    state.history.insert(parsed, rec);
                }
                _ => repairs.push((Ns::Hist, key)),
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
                        ReactionKind::Mute => &mut status.muted,
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
            let k = key.as_str().as_bytes();
            let (kind, src, dst) = match k {
                [kind @ (b'F' | b'B' | b'M' | b'Q'), s, d] => (
                    *kind,
                    unhex(*s).ok_or_else(|| corrupt(&key, "bad slot"))?,
                    unhex(*d).ok_or_else(|| corrupt(&key, "bad slot"))?,
                ),
                _ => return Err(corrupt(&key, "unexpected key")),
            };
            let live = state.account(src).is_some() && state.account(dst).is_some() && src != dst;
            match kind {
                b'F' => {
                    let rec: FollowRec = codec::decode(Kind::Follow, &bytes)?;
                    idgen.observe(rec.id);
                    if live {
                        state.follows.insert((src, dst), rec);
                    }
                }
                b'Q' => {
                    let rec: FollowRec = codec::decode(Kind::FollowRequest, &bytes)?;
                    idgen.observe(rec.id);
                    if live {
                        state.follow_requests.insert((src, dst), rec);
                    }
                }
                b'B' => {
                    let rec: BlockRec = codec::decode(Kind::Block, &bytes)?;
                    idgen.observe(rec.id);
                    if live {
                        state.blocks.insert((src, dst), rec);
                    }
                }
                _ => {
                    let rec: MuteRec = codec::decode(Kind::Mute, &bytes)?;
                    idgen.observe(rec.id);
                    if live {
                        state.mutes.insert((src, dst), rec);
                    }
                }
            }
            if !live {
                repairs.push((Ns::Rel, key));
            }
        }
        // A block commits first; the follows and requests it ends are
        // erased after it.
        let blocked: Vec<(u8, u8)> = state.blocks.keys().copied().collect();
        for (a, b) in blocked {
            for (src, dst) in [(a, b), (b, a)] {
                if state.follows.remove(&(src, dst)).is_some() {
                    repairs.push((Ns::Rel, keys::follow(src, dst)));
                }
                if state.follow_requests.remove(&(src, dst)).is_some() {
                    repairs.push((Ns::Rel, keys::follow_request(src, dst)));
                }
            }
        }
        // Authorizing writes the follow, then erases the request.
        let answered: Vec<(u8, u8)> = state
            .follow_requests
            .keys()
            .filter(|pair| state.follows.contains_key(pair))
            .copied()
            .collect();
        for pair in answered {
            state.follow_requests.remove(&pair);
            repairs.push((Ns::Rel, keys::follow_request(pair.0, pair.1)));
        }

        for (key, bytes) in load(&mut store, Ns::Mod)? {
            match key.as_str().as_bytes() {
                [b'm', h] => {
                    let slot = unhex(*h).ok_or_else(|| corrupt(&key, "bad slot"))?;
                    let rec: AccountModRec = codec::decode(Kind::AccountMod, &bytes)?;
                    let owner = state.account(slot).map(|a| a.rec.id);
                    if owner == Some(rec.account_id) {
                        state.moderation[slot as usize] = Some(rec);
                    } else {
                        repairs.push((Ns::Mod, key));
                    }
                }
                [b'R', ..] => {
                    let rec: ReportRec = codec::decode(Kind::Report, &bytes)?;
                    if keys::report(rec.id) != key {
                        return Err(corrupt(&key, "id does not match key"));
                    }
                    idgen.observe(rec.id);
                    if state.account(rec.reporter).is_some() && state.account(rec.target).is_some()
                    {
                        state.reports.insert(rec.id, rec);
                    } else {
                        repairs.push((Ns::Mod, key));
                    }
                }
                _ => return Err(corrupt(&key, "unexpected key")),
            }
        }

        for (key, bytes) in load(&mut store, Ns::Coll)? {
            let mut rec: CollectionRec = codec::decode(Kind::Collection, &bytes)?;
            if keys::collection(rec.id) != key {
                return Err(corrupt(&key, "id does not match key"));
            }
            idgen.observe(rec.id);
            for item in &rec.items {
                idgen.observe(item.id);
            }
            if state.account(rec.owner).is_none() {
                repairs.push((Ns::Coll, key));
                continue;
            }
            // Items naming deleted accounts are dropped from RAM; the record
            // loses them the next time the collection is written.
            rec.items
                .retain(|i| state.account_by_id(i.account_id).is_some());
            state.collections.insert(rec.id, rec);
        }

        for (ns, kind) in [(Ns::List, Kind::List), (Ns::Filt, Kind::Filter)] {
            for (key, bytes) in load(&mut store, ns)? {
                let (slot, n) = match key.as_str().as_bytes() {
                    [b'l' | b'x', s, n] => (
                        unhex(*s).ok_or_else(|| corrupt(&key, "bad slot"))?,
                        unhex(*n).ok_or_else(|| corrupt(&key, "bad index"))?,
                    ),
                    _ => return Err(corrupt(&key, "unexpected key")),
                };
                if state.account(slot).is_none() {
                    repairs.push((ns, key));
                    continue;
                }
                if kind == Kind::List {
                    let rec: ListRec = codec::decode(kind, &bytes)?;
                    idgen.observe(rec.id);
                    state.lists.insert((slot, n), rec);
                } else {
                    let rec: FilterRec = codec::decode(kind, &bytes)?;
                    idgen.observe(rec.id);
                    for k in &rec.keywords {
                        idgen.observe(k.id);
                    }
                    for s in &rec.statuses {
                        idgen.observe(s.id);
                    }
                    state.filters.insert((slot, n), rec);
                }
            }
        }

        for (key, bytes) in load(&mut store, Ns::Key)? {
            match key.as_str().as_bytes() {
                [b'k', h] => {
                    let slot = unhex(*h).ok_or_else(|| corrupt(&key, "bad slot"))?;
                    let rec: UserKeyRec = codec::decode(Kind::UserKey, &bytes)?;
                    if state.account(slot).map(|a| a.rec.id) == Some(rec.account_id) {
                        state.user_keys[slot as usize] = Some(rec);
                    } else {
                        repairs.push((Ns::Key, key));
                    }
                }
                [b'w', ..] => {
                    let sealed: crate::crypto::Sealed = codec::decode(Kind::TokenKey, &bytes)?;
                    let live = state
                        .tokens
                        .iter()
                        .any(|t| keys::token_seal(&t.rec.hash) == key);
                    if live {
                        state.token_seals.insert(key, sealed);
                    } else {
                        repairs.push((Ns::Key, key));
                    }
                }
                _ => return Err(corrupt(&key, "unexpected key")),
            }
        }

        for (key, bytes) in load(&mut store, Ns::Inv)? {
            let rec: CodeRec = codec::decode(Kind::Code, &bytes)?;
            if keys::code(rec.id) != key {
                return Err(corrupt(&key, "id does not match key"));
            }
            idgen.observe(rec.id);
            let live = match rec.kind {
                CodeKind::Invite => true,
                CodeKind::Reset { slot, account_id } => {
                    state.account(slot).map(|a| a.rec.id) == Some(account_id)
                }
            };
            if live {
                state.invites.insert(rec.id, rec);
            } else {
                repairs.push((Ns::Inv, key));
            }
        }

        // Tombstoned accounts: everything that referenced them was skipped
        // above and queued for erasure; finally erase the account itself.
        for account in state.accounts.iter().flatten() {
            if account.rec.deleted {
                repairs.push((Ns::Acct, keys::account(account.slot)));
            }
        }

        // Votes are bits by slot: a tombstoned member's votes must not pass
        // to whoever reuses the slot.
        let mut rewrites: Vec<(u64, PollRec)> = Vec::new();
        for account in state.accounts.iter().flatten().filter(|a| a.rec.deleted) {
            for (id, poll) in &mut state.polls {
                if poll.voters() & bit(account.slot) != 0 {
                    for v in &mut poll.votes {
                        *v &= !bit(account.slot);
                    }
                    rewrites.push((*id, poll.clone()));
                }
            }
        }
        for (id, poll) in &rewrites {
            store.set(
                Ns::Stat,
                &keys::poll(*id),
                &codec::encode(Kind::Poll, poll)?,
            )?;
        }

        state.repairs += repairs.len() as u64 + rewrites.len() as u64;
        for (ns, key) in &repairs {
            store.erase(*ns, key)?;
        }
        for slot in 0..MAX_ACCOUNTS {
            if state.accounts[slot].as_ref().is_some_and(|a| a.rec.deleted) {
                state.accounts[slot] = None;
            }
        }

        let mut service = Service {
            store,
            state,
            ids: idgen,
            config,
            latched: None,
        };
        service.load_social()?;
        service.check_invariants()?;
        Ok(service)
    }

    /// Fail closed if RAM state breaks a bound or an internal link.
    pub fn check_invariants(&self) -> core::result::Result<(), StoreError> {
        let s = &self.state;
        let fail = |what: &str| Err(StoreError::Corrupt(format!("invariant: {what}")));
        if s.statuses.len() > MAX_STATUSES
            || s.reports.len() > MAX_REPORTS
            || s.collections.len() > MAX_COLLECTIONS_PER_ACCOUNT * MAX_ACCOUNTS
            || s.boosts.len() > MAX_BOOSTS
            || s.reactions.len() > MAX_REACTIONS
            || s.apps.len() > MAX_APPS
            || s.tokens.len() > MAX_TOKENS
            || s.invites.len() > MAX_CODES_LIVE
            || s.history.len() > MAX_REVISIONS
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

    /// Record the wall clock of the request being handled (mute expiry).
    pub fn tick(&mut self, now_ms: u64) {
        self.state.now_ms = now_ms;
        self.announce_polls(now_ms);
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
        if matches!(
            kind,
            Kind::Account | Kind::Status | Kind::Follow | Kind::Block
        ) {
            self.state.account_stats.get_mut().take();
        }
        if let Some(cache) = self.state.rendered_accounts.get_mut() {
            cache.clear();
        }
        if let Some(session) = &self.state.session {
            session.clear_cache();
        }
        if kind == Kind::Poll {
            self.state.next_poll_check = None;
        }
        self.writable()?;
        let bytes = codec::encode(kind, value).map_err(|e| self.map_store_error(e))?;
        self.store
            .set(ns, key, &bytes)
            .map_err(|e| self.map_store_error(e))
    }

    pub(crate) fn erase(&mut self, ns: Ns, key: &Key) -> Result<()> {
        if matches!(ns, Ns::Stat | Ns::Acct | Ns::Rel) {
            self.state.account_stats.get_mut().take();
        }
        if ns == Ns::Stat {
            self.state.next_poll_check = None;
        }
        if let Some(cache) = self.state.rendered_accounts.get_mut() {
            cache.clear();
        }
        if let Some(session) = &self.state.session {
            session.clear_cache();
        }
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
        self.notify_about(kind, to, from, status, None, now_ms);
    }

    /// Would `to` accept a notification from `from` (about `status`)?
    /// Blocks, notification mutes, muted threads, suspended senders, and
    /// silenced senders `to` doesn't follow all say no.
    pub fn wants_notification(&self, to: u8, from: u8, status: Option<u64>) -> bool {
        let s = &self.state;
        if to == from || s.blocked_either(to, from) || s.suspended(from) {
            return false;
        }
        if s.mute(to, from).is_some_and(|m| m.notifications) {
            return false;
        }
        if s.silenced(from) && !s.follows(to, from) {
            return false;
        }
        status.is_none_or(|id| !self.thread_muted(to, id))
    }

    pub(crate) fn notify_about(
        &mut self,
        kind: NotificationKind,
        to: u8,
        from: u8,
        status: Option<u64>,
        object: Option<u64>,
        now_ms: u64,
    ) {
        let staff = kind == NotificationKind::AdminReport;
        // The one notification about your own post: your poll ended.
        let own_poll = kind == NotificationKind::Poll && to == from;
        if (to == from && !own_poll)
            || (!staff && !own_poll && !self.wants_notification(to, from, status))
        {
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
            object,
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
