//! Accounts on other servers, read through their public RSS feeds
//! (spec/remote_follow_rss.md). Not federation: nothing is ever sent to
//! them, and they never sign in.
//!
//! A remote account is data, not an account slot. It is stored only while a
//! household member follows it; search results live in RAM until then. The
//! bots board fetches the feeds at night and pushes the posts here with a
//! device credential; this board never fetches a feed itself.

use super::*;
use crate::media::MediaDir;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Posts kept per followed account (a Mastodon feed holds 20).
pub const ITEMS_PER_FEED: usize = 20;
/// The highest limits an admin may set.
pub const BOARD_FEEDS_CEILING: u16 = 64;
pub const ACCOUNT_FEEDS_CEILING: u16 = 32;
pub const MAX_REMOTE_ITEMS: usize = ITEMS_PER_FEED * BOARD_FEEDS_CEILING as usize;
pub const MAX_DEVICES: usize = 4;
/// Post text and content warning, in bytes; longer text is cut.
pub const MAX_ITEM_TEXT: usize = 2000;
pub const MAX_ITEM_CW: usize = 200;
pub const MAX_ITEM_IMAGES: usize = 4;
/// A pushed image: a JPEG preview the bots board made.
pub const MAX_IMAGE_BYTES: usize = 128 * 1024;
pub const MAX_IMAGE_PX: u16 = 640;
/// Device credentials start with this, so they are easy to recognise.
pub const DEVICE_PREFIX: &str = "mmdev_";
/// The only scope a device credential has.
pub const FEEDS_SCOPE: &str = "feeds:sync";
const MAX_URL: usize = 512;
const MAX_NAME: usize = 100;
const MAX_NOTE: usize = 500;
/// Search results remembered (RAM) so a member can follow one.
const SEEN_MAX: usize = 128;
/// Posts found by a search (RAM), so a member can open one.
const SEEN_POSTS_MAX: usize = 80;
/// A sign-in to the search server must finish within this time.
const LOGIN_MS: u64 = 15 * 60 * 1000;
/// What the household's sign-in to its search server may do: read only.
pub const SEARCH_SCOPES: &str = "read:search read:statuses read:accounts";
const SEARCHES_MAX: usize = 16;
const SEARCH_TTL_MS: u64 = 10 * 60 * 1000;
/// A failed search is tried again sooner.
const SEARCH_FAILED_TTL_MS: u64 = 60 * 1000;
/// Searches that reach another server, per member per hour.
pub const SEARCHES_PER_HOUR: u32 = 30;
/// Uploaded images waiting for the result that names them.
const PENDING_MEDIA_MAX: usize = 96;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSettings {
    /// Search and follow accounts on other servers.
    pub enabled: bool,
    /// The public server whose account search answers name searches
    /// (`mastodon.nu`). Empty: only exact `@name@server` handles.
    pub search_server: String,
    pub feeds_max_board: u16,
    pub feeds_max_account: u16,
    /// Images from other servers on the SD card, at most this many MiB.
    pub media_quota_mb: u32,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        RemoteSettings {
            enabled: true,
            search_server: "mastodon.nu".into(),
            feeds_max_board: 30,
            feeds_max_account: 10,
            media_quota_mb: 256,
        }
    }
}

/// What the bots board last saw when it fetched the feed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedState {
    pub etag: String,
    pub last_modified: String,
    pub last_ok_ms: Option<u64>,
    pub last_try_ms: Option<u64>,
    /// Failed nights in a row.
    pub failures: u16,
    /// Short and fixed: never the server's own text.
    pub error: Option<String>,
    /// The server said the account or feed is gone (404/410).
    pub gone: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteMedia {
    pub id: u64,
    pub bytes: u32,
    pub width: u16,
    pub height: u16,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteAccount {
    pub id: u64,
    /// `name@server`, lowercase.
    pub acct: String,
    pub display_name: String,
    /// Plain text.
    pub note: String,
    /// Household members who follow it. Never empty while stored.
    pub followers: AccountMask,
    pub created_ms: u64,
    pub avatar: Option<RemoteMedia>,
    pub feed: FeedState,
}

impl RemoteAccount {
    pub fn user(&self) -> &str {
        self.acct.split_once('@').map_or(&self.acct, |(u, _)| u)
    }
    pub fn host(&self) -> &str {
        self.acct.split_once('@').map_or("", |(_, h)| h)
    }
    pub fn feed_url(&self) -> String {
        feed_url(&self.acct)
    }
}

/// A post from a followed account, as plain text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteItem {
    /// From the id generator when it arrived, so a night's posts come in
    /// above what clients have already seen (spec "Timeline weaving").
    pub id: u64,
    pub account: u64,
    /// Hash of the feed's guid: a post arrives once.
    pub guid: u64,
    pub published_ms: u64,
    /// The original post.
    pub url: String,
    pub text: String,
    pub cw: String,
    pub sensitive: bool,
    pub media: Vec<RemoteMedia>,
}

/// A machine credential: no account slot, no sign-in, one scope.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceRec {
    pub id: u64,
    pub name: String,
    pub hash: [u8; 32],
    pub created_ms: u64,
}

/// A search result: what another server said about an account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteProfile {
    pub acct: String,
    pub display_name: String,
    pub note: String,
}

/// A remote account by id: followed (stored) or only seen in a search.
#[derive(Clone, Copy, Debug)]
pub struct RemoteRef<'a> {
    pub id: u64,
    pub acct: &'a str,
    pub display_name: &'a str,
    pub note: &'a str,
    pub account: Option<&'a RemoteAccount>,
}

impl RemoteRef<'_> {
    pub fn user(&self) -> &str {
        self.acct.split_once('@').map_or(self.acct, |(u, _)| u)
    }
    pub fn host(&self) -> &str {
        self.acct.split_once('@').map_or("", |(_, h)| h)
    }
    pub fn followers(&self) -> AccountMask {
        self.account.map_or(0, |a| a.followers)
    }
}

/// The night's fetch of one feed, as the bots board reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    NotModified,
    Gone,
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct IncomingItem {
    pub guid: String,
    pub published_ms: u64,
    pub url: String,
    pub text: String,
    pub cw: String,
    pub sensitive: bool,
    /// Uploaded image ids and their descriptions.
    pub media: Vec<(u64, String)>,
}

#[derive(Clone, Debug)]
pub struct FeedResult {
    pub outcome: Outcome,
    pub error: Option<String>,
    pub etag: String,
    pub last_modified: String,
    pub title: Option<String>,
    pub note: Option<String>,
    pub avatar: Option<u64>,
    pub items: Vec<IncomingItem>,
}

/// One feed the bots board should fetch tonight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedJob {
    pub account: u64,
    pub url: String,
    pub etag: String,
    pub last_modified: String,
    /// No avatar stored yet (and images are on).
    pub wants_avatar: bool,
    /// The newest post already here: older ones need no images fetched.
    pub newest_ms: Option<u64>,
}

/// What a search on another server found: remote account ids and the ids
/// of posts seen in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub accounts: Vec<u64>,
    pub posts: Vec<u64>,
}

type SearchResult = core::result::Result<Found, String>;

/// An app registered on a server, (client id, client secret), or why not.
pub type Registration = core::result::Result<(String, String), String>;

/// A search server's answer, before ids are given out.
#[derive(Clone, Debug, Default)]
pub struct Answer {
    pub profiles: Vec<RemoteProfile>,
    pub posts: Vec<(RemoteProfile, IncomingPost)>,
}

/// A post as a search server showed it.
#[derive(Clone, Debug, Default)]
pub struct IncomingPost {
    pub url: String,
    pub text: String,
    pub cw: String,
    pub sensitive: bool,
    pub published_ms: u64,
    /// Attached pictures: not fetched, the post links to the original.
    pub images: u16,
}

/// A post found by a search, kept in RAM so it can be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeenPost {
    pub id: u64,
    /// The author: a remote id from `seen` or a followed account.
    pub account: u64,
    pub url: String,
    pub text: String,
    pub cw: String,
    pub sensitive: bool,
    pub published_ms: u64,
    pub images: u16,
}

/// The household's sign-in to its search server (stored): an app
/// registered there, and a read-only token from an admin's login.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchAuth {
    pub server: String,
    pub client_id: String,
    pub client_secret: String,
    /// Never shown to anyone; sent only to `server`.
    pub token: String,
    /// Who signed in, `name@server`.
    pub acct: String,
    pub since_ms: u64,
}

/// A sign-in in progress (RAM): PKCE verifier and the state the server
/// sends back to the callback.
#[derive(Clone, Debug)]
pub struct LoginFlow {
    pub state: String,
    pub verifier: String,
    pub server: String,
    pub client_id: String,
    pub client_secret: String,
    pub started_ms: u64,
    /// The token, between the code exchange and learning who signed in.
    pub token: Option<String>,
    /// Finished: who signed in, or why it failed.
    pub outcome: Option<core::result::Result<String, String>>,
}

#[derive(Default)]
pub struct Remote {
    pub settings: RemoteSettings,
    pub accounts: BTreeMap<u64, RemoteAccount>,
    pub items: BTreeMap<u64, RemoteItem>,
    guids: BTreeSet<(u64, u64)>,
    pub devices: BTreeMap<u64, DeviceRec>,
    /// RAM: when each device credential was last used.
    pub device_used: BTreeMap<u64, u64>,
    seen: VecDeque<(u64, RemoteProfile)>,
    seen_posts: VecDeque<SeenPost>,
    searches: VecDeque<(String, u64, SearchResult)>,
    pub search_auth: Option<SearchAuth>,
    pub login: Option<LoginFlow>,
    /// An app registered on a server that has no sign-in yet (RAM):
    /// (server, client id, client secret), or why registering failed.
    pub registration: Option<(String, Registration)>,
    /// Per member: (hour window start, searches in it).
    search_budget: BTreeMap<u8, (u64, u32)>,
    pending_media: VecDeque<(u64, RemoteMedia)>,
    /// The SD card (or desktop directory). None: no images.
    pub media: Option<MediaDir>,
    pub media_bytes: u64,
}

impl Remote {
    pub fn followed_by(&self, slot: u8) -> impl Iterator<Item = &RemoteAccount> {
        self.accounts
            .values()
            .filter(move |a| a.followers & bit(slot) != 0)
    }

    pub fn find_acct(&self, acct: &str) -> Option<&RemoteAccount> {
        self.accounts.values().find(|a| a.acct == acct)
    }

    pub fn items_of(&self, account: u64) -> impl DoubleEndedIterator<Item = &RemoteItem> {
        self.items.values().filter(move |i| i.account == account)
    }
}

/// `name@server`, lowercase, or why not. No ports, paths, IP addresses or
/// household (`.local`) names: only public servers.
pub fn normalize_acct(raw: &str) -> Result<String> {
    let raw = raw.trim().trim_start_matches('@');
    let Some((user, host)) = raw.split_once('@') else {
        return invalid("Use @name@server for an account on another server");
    };
    let host = host.to_ascii_lowercase();
    if user.is_empty()
        || user.len() > 64
        || !user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
        || host.len() > 253
        || !host.contains('.')
        || host.ends_with(".local")
        || host.ends_with(".lan")
        || host.ends_with(".home.arpa")
        || host.ends_with(".internal")
        || host.parse::<std::net::IpAddr>().is_ok()
        || host.split('.').any(|s| {
            s.is_empty()
                || s.len() > 63
                || s.starts_with('-')
                || s.ends_with('-')
                || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        || host
            .rsplit('.')
            .next()
            .is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()))
    {
        return invalid("Use @name@server for an account on a public server");
    }
    Ok(format!("{}@{host}", user.to_ascii_lowercase()))
}

/// Mastodon's public RSS feed for an account.
pub fn feed_url(acct: &str) -> String {
    let (user, host) = acct.split_once('@').unwrap_or((acct, ""));
    format!("https://{host}/@{user}.rss")
}

/// A server name an admin may choose for searches: a bare public host.
pub fn valid_host(host: &str) -> bool {
    normalize_acct(&format!("a@{host}")).is_ok_and(|a| a == format!("a@{host}"))
}

/// Cut to at most `max` bytes on a character boundary, without control
/// characters other than newlines.
fn clip(text: &str, max: usize) -> String {
    let mut out = String::new();
    for c in text.trim().chars() {
        if c.is_control() && c != '\n' {
            continue;
        }
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out
}

/// An `https://` link without spaces, quotes or angle brackets.
fn clean_url(url: &str) -> Option<String> {
    let url = url.trim();
    (url.starts_with("https://")
        && url.len() <= MAX_URL
        && url
            .bytes()
            .all(|b| b.is_ascii_graphic() && !b"\"'<>\\`".contains(&b)))
    .then(|| url.to_string())
}

fn guid_hash(guid: &str) -> u64 {
    let d = crate::auth::sha256(guid);
    u64::from_le_bytes(d[..8].try_into().expect("8 bytes"))
}

fn settings_key() -> Key {
    Key::new("settings").expect("static key")
}
fn search_auth_key() -> Key {
    Key::new("searchauth").expect("static key")
}

fn pkce_challenge(verifier: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(crate::auth::sha256(verifier))
}
fn account_key(id: u64) -> Key {
    Key::from_parts(&[b"a", &ids::b32(id)]).expect("fits")
}
fn item_key(id: u64) -> Key {
    Key::from_parts(&[b"i", &ids::b32(id)]).expect("fits")
}
fn device_key(id: u64) -> Key {
    Key::from_parts(&[b"d", &ids::b32(id)]).expect("fits")
}

impl<S: Store> Service<S> {
    /// Boot: settings, followed accounts, posts and devices. Followers whose
    /// slot is no longer an account are dropped (a reused slot must not
    /// inherit them), and so is anything nobody follows any more.
    pub(crate) fn load_remote(&mut self) -> core::result::Result<(), StoreError> {
        let media = self.remote.media.take();
        self.remote = Remote {
            media,
            ..Remote::default()
        };
        let live: AccountMask = self.state.active_accounts().fold(0, |m, a| m | bit(a.slot));
        let mut erase = Vec::new();
        let mut rewrite = Vec::new();
        let mut items = Vec::new();
        for (key, bytes) in load(&mut self.store, Ns::Remote)? {
            if key == settings_key() {
                self.remote.settings = codec::decode(Kind::RemoteSettings, &bytes)?;
                continue;
            }
            if key == search_auth_key() {
                self.remote.search_auth = Some(codec::decode(Kind::SearchAuth, &bytes)?);
                continue;
            }
            match key.as_str().as_bytes().first() {
                Some(b'a') => {
                    let mut rec: RemoteAccount = codec::decode(Kind::RemoteAccount, &bytes)?;
                    if account_key(rec.id) != key {
                        return Err(corrupt(&key, "id does not match key"));
                    }
                    self.ids.observe(rec.id);
                    if rec.followers & !live != 0 {
                        rec.followers &= live;
                        rewrite.push(rec.clone());
                    }
                    if rec.followers == 0 {
                        erase.push(key);
                    } else {
                        self.remote.accounts.insert(rec.id, rec);
                    }
                }
                Some(b'i') => {
                    let rec: RemoteItem = codec::decode(Kind::RemoteItem, &bytes)?;
                    if item_key(rec.id) != key {
                        return Err(corrupt(&key, "id does not match key"));
                    }
                    self.ids.observe(rec.id);
                    items.push(rec);
                }
                Some(b'd') => {
                    let rec: DeviceRec = codec::decode(Kind::Device, &bytes)?;
                    if device_key(rec.id) != key {
                        return Err(corrupt(&key, "id does not match key"));
                    }
                    self.ids.observe(rec.id);
                    self.remote.devices.insert(rec.id, rec);
                }
                _ => return Err(corrupt(&key, "unexpected key")),
            }
        }
        for item in items {
            if self.remote.accounts.contains_key(&item.account) {
                self.remote.guids.insert((item.account, item.guid));
                self.remote.items.insert(item.id, item);
            } else {
                erase.push(item_key(item.id));
            }
        }
        for rec in rewrite.into_iter().filter(|r| r.followers != 0) {
            self.store.set(
                Ns::Remote,
                &account_key(rec.id),
                &codec::encode(Kind::RemoteAccount, &rec)?,
            )?;
            self.state.repairs += 1;
        }
        for key in erase {
            self.store.erase(Ns::Remote, &key)?;
            self.state.repairs += 1;
        }
        self.sweep_media_files();
        Ok(())
    }

    /// Use this directory for images (the SD card). Files nothing refers to
    /// are deleted.
    pub fn set_media(&mut self, media: Option<MediaDir>) {
        self.remote.media = media;
        self.sweep_media_files();
    }

    fn sweep_media_files(&mut self) {
        let referenced: BTreeSet<u64> = self
            .remote
            .accounts
            .values()
            .filter_map(|a| a.avatar.as_ref())
            .chain(self.remote.items.values().flat_map(|i| i.media.iter()))
            .map(|m| m.id)
            .collect();
        self.remote.media_bytes = self
            .remote
            .accounts
            .values()
            .filter_map(|a| a.avatar.as_ref())
            .chain(self.remote.items.values().flat_map(|i| i.media.iter()))
            .map(|m| u64::from(m.bytes))
            .sum();
        self.remote.pending_media.clear();
        if let Some(media) = &self.remote.media {
            match media.ids() {
                Ok(ids) => {
                    for id in ids.into_iter().filter(|id| !referenced.contains(id)) {
                        media.remove(id);
                    }
                }
                Err(e) => log::warn!("Reading the image directory failed: {e}"),
            }
        }
    }

    pub fn remote_settings(&self) -> &RemoteSettings {
        &self.remote.settings
    }

    pub fn set_remote_settings(
        &mut self,
        actor: u8,
        settings: RemoteSettings,
        now_ms: u64,
    ) -> Result<()> {
        self.require_admin(actor)?;
        let host = settings.search_server.trim().to_ascii_lowercase();
        if !host.is_empty() && !valid_host(&host) {
            return invalid("The search server must be a public server name, such as mastodon.nu");
        }
        if settings.feeds_max_board > BOARD_FEEDS_CEILING
            || settings.feeds_max_account > ACCOUNT_FEEDS_CEILING
        {
            return invalid(format!(
                "At most {BOARD_FEEDS_CEILING} feeds for the household and {ACCOUNT_FEEDS_CEILING} per person"
            ));
        }
        if settings.media_quota_mb > 16 * 1024 {
            return invalid("At most 16384 MiB of images");
        }
        let settings = RemoteSettings {
            search_server: host,
            ..settings
        };
        self.govern(Some(actor), now_ms)?;
        self.put(Ns::Remote, &settings_key(), Kind::RemoteSettings, &settings)?;
        self.remote.settings = settings;
        self.enforce_media_quota()?;
        Ok(())
    }

    /// A remote account by id: followed, or seen in a recent search.
    pub fn remote_ref(&self, id: u64) -> Option<RemoteRef<'_>> {
        if let Some(a) = self.remote.accounts.get(&id) {
            return Some(RemoteRef {
                id,
                acct: &a.acct,
                display_name: &a.display_name,
                note: &a.note,
                account: Some(a),
            });
        }
        self.remote
            .seen
            .iter()
            .find(|(seen, _)| *seen == id)
            .map(|(_, p)| RemoteRef {
                id,
                acct: &p.acct,
                display_name: &p.display_name,
                note: &p.note,
                account: None,
            })
    }

    pub fn remote_ref_by_acct(&self, acct: &str) -> Option<RemoteRef<'_>> {
        let id = self.remote.find_acct(acct).map(|a| a.id).or_else(|| {
            self.remote
                .seen
                .iter()
                .find(|(_, p)| p.acct == acct)
                .map(|(id, _)| *id)
        })?;
        self.remote_ref(id)
    }

    /// Remember a search result so it can be shown and followed; its id.
    /// A followed account keeps its id; its stored profile is not changed
    /// (the nightly fetch keeps that up to date).
    pub fn remote_seen(&mut self, profile: RemoteProfile, now_ms: u64) -> Option<u64> {
        if let Some(a) = self.remote.find_acct(&profile.acct) {
            return Some(a.id);
        }
        let profile = RemoteProfile {
            acct: profile.acct,
            display_name: clip(&profile.display_name, MAX_NAME),
            note: clip(&profile.note, MAX_NOTE),
        };
        if let Some(entry) = self
            .remote
            .seen
            .iter_mut()
            .find(|(_, p)| p.acct == profile.acct)
        {
            entry.1 = profile;
            return Some(entry.0);
        }
        let id = self.ids.next(now_ms)?;
        if self.remote.seen.len() >= SEEN_MAX {
            self.remote.seen.pop_front();
        }
        self.remote.seen.push_back((id, profile));
        Some(id)
    }

    /// A cached search, if it is recent.
    pub fn remote_search_cached(&self, key: &str, now_ms: u64) -> Option<&SearchResult> {
        self.remote
            .searches
            .iter()
            .find(|(k, at, r)| {
                let ttl = if r.is_ok() {
                    SEARCH_TTL_MS
                } else {
                    SEARCH_FAILED_TTL_MS
                };
                k == key && now_ms.saturating_sub(*at) < ttl
            })
            .map(|(_, _, r)| r)
    }

    /// Count a search that will reach another server; false when this
    /// member has used up the hour.
    pub fn remote_search_allowed(&mut self, slot: u8, now_ms: u64) -> bool {
        let (start, used) = self.remote.search_budget.entry(slot).or_insert((now_ms, 0));
        if now_ms.saturating_sub(*start) >= HOUR_MS {
            *start = now_ms;
            *used = 0;
        }
        if *used >= SEARCHES_PER_HOUR {
            return false;
        }
        *used += 1;
        true
    }

    /// Keep what another server answered for `key`.
    pub fn remote_search_store(
        &mut self,
        key: String,
        result: core::result::Result<Answer, String>,
        now_ms: u64,
    ) {
        let result = result.map(|answer| {
            let accounts = answer
                .profiles
                .into_iter()
                .filter_map(|p| self.remote_seen(p, now_ms))
                .collect();
            let posts = answer
                .posts
                .into_iter()
                .filter_map(|(author, post)| self.remote_seen_post(author, post, now_ms))
                .collect();
            Found { accounts, posts }
        });
        self.remote.searches.retain(|(k, _, _)| *k != key);
        if self.remote.searches.len() >= SEARCHES_MAX {
            self.remote.searches.pop_front();
        }
        self.remote.searches.push_back((key, now_ms, result));
    }

    /// Remember a post a search found; its id.
    fn remote_seen_post(
        &mut self,
        author: RemoteProfile,
        post: IncomingPost,
        now_ms: u64,
    ) -> Option<u64> {
        let url = clean_url(&post.url)?;
        if let Some(p) = self.remote.seen_posts.iter().find(|p| p.url == url) {
            return Some(p.id);
        }
        let account = self.remote_seen(author, now_ms)?;
        let id = self.ids.next(now_ms)?;
        if self.remote.seen_posts.len() >= SEEN_POSTS_MAX {
            self.remote.seen_posts.pop_front();
        }
        self.remote.seen_posts.push_back(SeenPost {
            id,
            account,
            url,
            text: clip(&post.text, MAX_ITEM_TEXT),
            cw: clip(&post.cw, MAX_ITEM_CW),
            sensitive: post.sensitive,
            published_ms: post.published_ms.min(now_ms),
            images: post.images,
        });
        Some(id)
    }

    pub fn seen_post(&self, id: u64) -> Option<&SeenPost> {
        self.remote.seen_posts.iter().find(|p| p.id == id)
    }

    // Signing in to the search server (PKCE, read-only).

    /// The token for searches, when the household is signed in to the
    /// server it searches.
    pub fn search_token(&self) -> Option<&str> {
        self.remote
            .search_auth
            .as_ref()
            .filter(|a| !a.token.is_empty() && a.server == self.remote.settings.search_server)
            .map(|a| a.token.as_str())
    }

    /// The app this board registered on `server`, if any.
    pub fn search_app(&self, server: &str) -> Option<(String, String)> {
        if let Some(a) = self
            .remote
            .search_auth
            .as_ref()
            .filter(|a| a.server == server && !a.client_id.is_empty())
        {
            return Some((a.client_id.clone(), a.client_secret.clone()));
        }
        match &self.remote.registration {
            Some((s, Ok(app))) if s == server => Some(app.clone()),
            _ => None,
        }
    }

    pub fn registration_failed(&self, server: &str) -> Option<&str> {
        match &self.remote.registration {
            Some((s, Err(e))) if s == server => Some(e),
            _ => None,
        }
    }

    pub fn set_registration(&mut self, server: String, app: Registration) {
        self.remote.registration = Some((server, app));
    }

    /// Start a sign-in; the URL the admin's browser goes to.
    pub fn start_search_login(
        &mut self,
        actor: u8,
        server: &str,
        redirect_uri: &str,
        now_ms: u64,
    ) -> Result<String> {
        self.require_admin(actor)?;
        if !valid_host(server) {
            return invalid("Choose a public server name, such as mastodon.social");
        }
        let Some((client_id, client_secret)) = self.search_app(server) else {
            let why = self
                .registration_failed(server)
                .unwrap_or("it did not answer")
                .to_string();
            self.remote.registration = None;
            return Err(Error::Unavailable(format!(
                "Could not register with {server}: {why}"
            )));
        };
        let state = crate::auth::random_secret();
        let verifier = crate::auth::random_secret();
        let url = format!(
            "https://{server}/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
            crate::http::encode(&client_id),
            crate::http::encode(redirect_uri),
            crate::http::encode(SEARCH_SCOPES),
            crate::http::encode(&state),
            pkce_challenge(&verifier),
        );
        self.remote.login = Some(LoginFlow {
            state,
            verifier,
            server: server.to_string(),
            client_id,
            client_secret,
            started_ms: now_ms,
            token: None,
            outcome: None,
        });
        Ok(url)
    }

    /// The sign-in a callback's `state` names, while it is young.
    pub fn login_flow(&self, state: &str, now_ms: u64) -> Option<&LoginFlow> {
        self.remote.login.as_ref().filter(|f| {
            crate::auth::ct_eq(f.state.as_bytes(), state.as_bytes())
                && now_ms.saturating_sub(f.started_ms) < LOGIN_MS
        })
    }

    fn login_mut(&mut self, state: &str) -> Option<&mut LoginFlow> {
        self.remote
            .login
            .as_mut()
            .filter(|f| crate::auth::ct_eq(f.state.as_bytes(), state.as_bytes()))
    }

    pub fn login_token(&mut self, state: &str, token: core::result::Result<String, String>) {
        if let Some(flow) = self.login_mut(state) {
            match token {
                Ok(t) => flow.token = Some(t),
                Err(e) => flow.outcome = Some(Err(e)),
            }
        }
    }

    /// The server said who signed in: keep the token and search there.
    pub fn login_done(
        &mut self,
        state: &str,
        acct: core::result::Result<String, String>,
        now_ms: u64,
    ) {
        let Some(flow) = self.login_mut(state).cloned() else {
            return;
        };
        let outcome = acct.and_then(|acct| {
            let auth = SearchAuth {
                server: flow.server.clone(),
                client_id: flow.client_id.clone(),
                client_secret: flow.client_secret.clone(),
                token: flow.token.clone().unwrap_or_default(),
                acct: acct.clone(),
                since_ms: now_ms,
            };
            let settings = RemoteSettings {
                search_server: flow.server.clone(),
                ..self.remote.settings.clone()
            };
            self.put(Ns::Remote, &search_auth_key(), Kind::SearchAuth, &auth)
                .and_then(|_| {
                    self.put(Ns::Remote, &settings_key(), Kind::RemoteSettings, &settings)
                })
                .map_err(|e| e.to_string())?;
            self.remote.search_auth = Some(auth);
            self.remote.settings = settings;
            self.remote.registration = None;
            // Answers from before the sign-in were anonymous.
            self.remote.searches.clear();
            Ok(acct)
        });
        if let Some(f) = self.login_mut(state) {
            f.token = None;
            f.outcome = Some(outcome);
        }
    }

    /// The finished sign-in's outcome; the flow is used up.
    pub fn take_login(&mut self, state: &str) -> Option<core::result::Result<String, String>> {
        let done = self.login_mut(state)?.outcome.clone();
        if done.is_some() {
            self.remote.login = None;
        }
        done
    }

    pub fn search_sign_out(&mut self, actor: u8, now_ms: u64) -> Result<()> {
        self.require_admin(actor)?;
        self.govern(Some(actor), now_ms)?;
        if self.remote.search_auth.take().is_some() {
            self.erase(Ns::Remote, &search_auth_key())?;
        }
        self.remote.searches.clear();
        Ok(())
    }

    /// Follow or unfollow a remote account. The last unfollow deletes it
    /// and its posts.
    pub fn remote_follow(&mut self, viewer: u8, id: u64, on: bool, now_ms: u64) -> Result<()> {
        self.state.account(viewer).ok_or(Error::NotFound)?;
        let target = self.remote_ref(id).ok_or(Error::NotFound)?;
        let following = target.followers() & bit(viewer) != 0;
        if following == on {
            return Ok(());
        }
        if on {
            let settings = &self.remote.settings;
            if !settings.enabled {
                return Err(Error::Forbidden(
                    "Following accounts on other servers is turned off".into(),
                ));
            }
            let mine = self.remote.followed_by(viewer).count();
            if mine >= settings.feeds_max_account as usize {
                return invalid(format!(
                    "You already follow {mine} accounts on other servers, the most allowed. Unfollow one first."
                ));
            }
            if target.account.is_none()
                && self.remote.accounts.len() >= settings.feeds_max_board as usize
            {
                return invalid(format!(
                    "This household already follows {} accounts on other servers, the most allowed",
                    self.remote.accounts.len()
                ));
            }
        }
        let mut rec = match target.account {
            Some(a) => a.clone(),
            None => RemoteAccount {
                id,
                acct: target.acct.to_string(),
                display_name: target.display_name.to_string(),
                note: target.note.to_string(),
                followers: 0,
                created_ms: now_ms,
                avatar: None,
                feed: FeedState::default(),
            },
        };
        self.govern(Some(viewer), now_ms)?;
        if on {
            rec.followers |= bit(viewer);
        } else {
            rec.followers &= !bit(viewer);
        }
        if rec.followers == 0 {
            return self.remove_remote_account(id);
        }
        self.put(Ns::Remote, &account_key(id), Kind::RemoteAccount, &rec)?;
        self.remote.seen.retain(|(seen, _)| *seen != id);
        self.remote.accounts.insert(id, rec);
        Ok(())
    }

    /// A deleted member stops following everything.
    pub(crate) fn remote_forget_slot(&mut self, slot: u8) -> Result<()> {
        let ids: Vec<u64> = self.remote.followed_by(slot).map(|a| a.id).collect();
        for id in ids {
            let mut rec = self.remote.accounts[&id].clone();
            rec.followers &= !bit(slot);
            if rec.followers == 0 {
                self.remove_remote_account(id)?;
            } else {
                self.put(Ns::Remote, &account_key(id), Kind::RemoteAccount, &rec)?;
                self.remote.accounts.insert(id, rec);
            }
        }
        Ok(())
    }

    fn remove_remote_account(&mut self, id: u64) -> Result<()> {
        let items: Vec<u64> = self.remote.items_of(id).map(|i| i.id).collect();
        for item in items {
            self.remove_remote_item(item)?;
        }
        self.erase(Ns::Remote, &account_key(id))?;
        if let Some(rec) = self.remote.accounts.remove(&id) {
            if let Some(avatar) = &rec.avatar {
                self.drop_media_file(avatar);
            }
            // Back among search results, with the same id: a cached search
            // still finds it, and it can be followed again.
            if self.remote.seen.len() >= SEEN_MAX {
                self.remote.seen.pop_front();
            }
            self.remote.seen.push_back((
                id,
                RemoteProfile {
                    acct: rec.acct,
                    display_name: rec.display_name,
                    note: rec.note,
                },
            ));
        }
        Ok(())
    }

    fn remove_remote_item(&mut self, id: u64) -> Result<()> {
        self.erase(Ns::Remote, &item_key(id))?;
        if let Some(item) = self.remote.items.remove(&id) {
            self.remote.guids.remove(&(item.account, item.guid));
            for m in &item.media {
                self.drop_media_file(m);
            }
        }
        Ok(())
    }

    fn drop_media_file(&mut self, m: &RemoteMedia) {
        if let Some(media) = &self.remote.media {
            media.remove(m.id);
        }
        self.remote.media_bytes = self.remote.media_bytes.saturating_sub(u64::from(m.bytes));
    }

    /// Posts of followed accounts for `viewer`'s home timeline, by id.
    pub(crate) fn remote_entries(
        &self,
        viewer: u8,
        bounds: query::Bounds,
        ascending: bool,
    ) -> query::Scan<'_, query::Entry> {
        let mask = bit(viewer);
        let followed: BTreeSet<u64> = self
            .remote
            .accounts
            .values()
            .filter(|a| a.followers & mask != 0)
            .map(|a| a.id)
            .collect();
        if followed.is_empty() {
            return Box::new(std::iter::empty());
        }
        let it = self
            .remote
            .items
            .range(bounds.range())
            .filter(move |(_, i)| followed.contains(&i.account))
            .map(|(id, _)| (*id, query::Entry::Remote(*id)));
        if ascending {
            Box::new(it)
        } else {
            Box::new(it.rev())
        }
    }

    /// One remote account's posts, newest first.
    pub fn remote_statuses(&self, account: u64, q: &query::PageQuery) -> Vec<(u64, query::Entry)> {
        query::paginate(q, |bounds, asc| {
            let it = self
                .remote
                .items
                .range(bounds.range())
                .filter(move |(_, i)| i.account == account)
                .map(|(id, _)| (*id, query::Entry::Remote(*id)));
            if asc {
                Box::new(it)
            } else {
                Box::new(it.rev())
            }
        })
    }

    // Device credentials.

    pub fn create_device_key(
        &mut self,
        actor: u8,
        name: &str,
        now_ms: u64,
    ) -> Result<(u64, String)> {
        self.require_admin(actor)?;
        let name = clip(name, 40);
        if name.is_empty() {
            return invalid("Give the device a name, such as Bots board");
        }
        if self.remote.devices.len() >= MAX_DEVICES {
            return invalid(format!(
                "At most {MAX_DEVICES} device keys; revoke one first"
            ));
        }
        self.govern(Some(actor), now_ms)?;
        let id = self.next_id(now_ms)?;
        let secret = format!("{DEVICE_PREFIX}{}", crate::auth::random_secret());
        let rec = DeviceRec {
            id,
            name,
            hash: crate::auth::sha256(&secret),
            created_ms: now_ms,
        };
        self.put(Ns::Remote, &device_key(id), Kind::Device, &rec)?;
        self.remote.devices.insert(id, rec);
        Ok((id, secret))
    }

    pub fn revoke_device_key(&mut self, actor: u8, id: u64, now_ms: u64) -> Result<()> {
        self.require_admin(actor)?;
        if !self.remote.devices.contains_key(&id) {
            return Err(Error::NotFound);
        }
        self.govern(Some(actor), now_ms)?;
        self.erase(Ns::Remote, &device_key(id))?;
        self.remote.devices.remove(&id);
        self.remote.device_used.remove(&id);
        Ok(())
    }

    /// The device a bearer token belongs to.
    pub(crate) fn device_principal(&mut self, token: &str, now_ms: u64) -> Option<Principal> {
        if !token.starts_with(DEVICE_PREFIX) {
            return None;
        }
        let hash = crate::auth::sha256(token);
        let id = self
            .remote
            .devices
            .values()
            .find(|d| crate::auth::ct_eq(&d.hash, &hash))?
            .id;
        self.remote.device_used.insert(id, now_ms);
        Some(Principal {
            slot: None,
            app_id: 0,
            scopes: vec![FEEDS_SCOPE.to_string()],
            disabled: false,
        })
    }

    // The bots board's side.

    /// Every followed feed. The bots board decides when to fetch them.
    pub fn feed_jobs(&self) -> Vec<FeedJob> {
        let images = self.remote.media.is_some();
        self.remote
            .accounts
            .values()
            .map(|a| FeedJob {
                account: a.id,
                url: a.feed_url(),
                etag: a.feed.etag.clone(),
                last_modified: a.feed.last_modified.clone(),
                wants_avatar: images && a.avatar.is_none(),
                newest_ms: self.remote.items_of(a.id).map(|i| i.published_ms).max(),
            })
            .collect()
    }

    /// Store a JPEG preview the bots board made for `account`'s post or
    /// avatar. It waits for the result that names it.
    pub fn feed_media_put(&mut self, account: u64, bytes: &[u8], now_ms: u64) -> Result<u64> {
        self.writable()?;
        let Some(media) = self.remote.media.clone() else {
            return Err(Error::Unavailable(
                "No SD card: images from other servers are off".into(),
            ));
        };
        if !self.remote.accounts.contains_key(&account) {
            return Err(Error::NotFound);
        }
        if bytes.len() > MAX_IMAGE_BYTES {
            return invalid(format!("Images must be at most {MAX_IMAGE_BYTES} bytes"));
        }
        let Some((width, height)) = crate::media::jpeg_size(bytes) else {
            return invalid("Images must be JPEG");
        };
        if width > MAX_IMAGE_PX || height > MAX_IMAGE_PX {
            return invalid(format!(
                "Images must be at most {MAX_IMAGE_PX} pixels wide and high"
            ));
        }
        let id = self.next_id(now_ms)?;
        media
            .put(id, bytes)
            .map_err(|e| Error::Unavailable(format!("Saving the image failed: {e}")))?;
        let m = RemoteMedia {
            id,
            bytes: bytes.len() as u32,
            width,
            height,
            description: String::new(),
        };
        self.remote.media_bytes += u64::from(m.bytes);
        if self.remote.pending_media.len() >= PENDING_MEDIA_MAX {
            if let Some((_, old)) = self.remote.pending_media.pop_front() {
                self.drop_media_file(&old);
            }
        }
        self.remote.pending_media.push_back((account, m));
        Ok(id)
    }

    fn take_pending(&mut self, account: u64, id: u64) -> Option<RemoteMedia> {
        let at = self
            .remote
            .pending_media
            .iter()
            .position(|(a, m)| *a == account && m.id == id)?;
        self.remote.pending_media.remove(at).map(|(_, m)| m)
    }

    /// Record one feed's fetch: its state, profile and new posts. Returns
    /// how many posts were new.
    pub fn feed_result(&mut self, account: u64, result: FeedResult, now_ms: u64) -> Result<usize> {
        self.writable()?;
        let mut rec = self
            .remote
            .accounts
            .get(&account)
            .cloned()
            .ok_or(Error::NotFound)?;
        rec.feed.last_try_ms = Some(now_ms);
        match result.outcome {
            Outcome::Ok | Outcome::NotModified => {
                rec.feed.last_ok_ms = Some(now_ms);
                rec.feed.failures = 0;
                rec.feed.error = None;
                rec.feed.gone = false;
                if result.outcome == Outcome::Ok {
                    rec.feed.etag = clip(&result.etag, 200);
                    rec.feed.last_modified = clip(&result.last_modified, 64);
                }
            }
            Outcome::Gone => {
                rec.feed.gone = true;
                rec.feed.failures = rec.feed.failures.saturating_add(1);
                rec.feed.error = Some("The account or its feed is gone".into());
            }
            Outcome::Failed => {
                rec.feed.failures = rec.feed.failures.saturating_add(1);
                rec.feed.error = Some(clip(
                    result.error.as_deref().unwrap_or("The fetch failed"),
                    120,
                ));
            }
        }
        if let Some(title) = result.title.map(|t| clip(&t, MAX_NAME)) {
            if !title.is_empty() {
                rec.display_name = title;
            }
        }
        if let Some(note) = result.note {
            rec.note = clip(&note, MAX_NOTE);
        }
        if let Some(avatar) = result.avatar.and_then(|id| self.take_pending(account, id)) {
            if let Some(old) = rec.avatar.replace(avatar) {
                self.drop_media_file(&old);
            }
        }
        self.put(Ns::Remote, &account_key(account), Kind::RemoteAccount, &rec)?;
        self.remote.accounts.insert(account, rec);

        let mut incoming = if result.outcome == Outcome::Ok {
            result.items
        } else {
            Vec::new()
        };
        incoming.truncate(ITEMS_PER_FEED);
        // Oldest first, so newer posts get higher ids.
        incoming.sort_by_key(|i| i.published_ms);
        let mut added = 0;
        for item in incoming {
            let guid = guid_hash(&item.guid);
            let Some(url) = clean_url(&item.url) else {
                continue;
            };
            if item.guid.is_empty() || self.remote.guids.contains(&(account, guid)) {
                continue;
            }
            let media: Vec<RemoteMedia> = item
                .media
                .iter()
                .take(MAX_ITEM_IMAGES)
                .filter_map(|(id, alt)| {
                    self.take_pending(account, *id).map(|m| RemoteMedia {
                        description: clip(alt, 400),
                        ..m
                    })
                })
                .collect();
            let id = self.next_id(now_ms)?;
            let rec = RemoteItem {
                id,
                account,
                guid,
                published_ms: if item.published_ms == 0 {
                    now_ms
                } else {
                    item.published_ms.min(now_ms)
                },
                url,
                text: clip(&item.text, MAX_ITEM_TEXT),
                cw: clip(&item.cw, MAX_ITEM_CW),
                sensitive: item.sensitive,
                media,
            };
            self.put(Ns::Remote, &item_key(id), Kind::RemoteItem, &rec)?;
            self.remote.guids.insert((account, guid));
            self.remote.items.insert(id, rec);
            added += 1;
        }
        // Images uploaded for this feed that no post named.
        let unused: Vec<RemoteMedia> = self
            .remote
            .pending_media
            .iter()
            .filter(|(a, _)| *a == account)
            .map(|(_, m)| m.clone())
            .collect();
        self.remote.pending_media.retain(|(a, _)| *a != account);
        for m in &unused {
            self.drop_media_file(m);
        }
        let mine: Vec<u64> = self.remote.items_of(account).map(|i| i.id).collect();
        for id in mine.iter().take(mine.len().saturating_sub(ITEMS_PER_FEED)) {
            self.remove_remote_item(*id)?;
        }
        while self.remote.items.len() > MAX_REMOTE_ITEMS {
            let oldest = *self.remote.items.keys().next().expect("not empty");
            self.remove_remote_item(oldest)?;
        }
        self.enforce_media_quota()?;
        Ok(added)
    }

    /// Over the image quota: the oldest posts' images go first.
    fn enforce_media_quota(&mut self) -> Result<()> {
        let quota = u64::from(self.remote.settings.media_quota_mb) * 1024 * 1024;
        while self.remote.media_bytes > quota {
            let Some(id) = self
                .remote
                .items
                .values()
                .find(|i| !i.media.is_empty())
                .map(|i| i.id)
            else {
                break;
            };
            let mut item = self.remote.items[&id].clone();
            for m in std::mem::take(&mut item.media) {
                self.drop_media_file(&m);
            }
            self.put(Ns::Remote, &item_key(id), Kind::RemoteItem, &item)?;
            self.remote.items.insert(id, item);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_must_be_public_handles() {
        assert_eq!(
            normalize_acct("@Alice@Example.org").unwrap(),
            "alice@example.org"
        );
        for bad in [
            "alice",
            "alice@example.org/path",
            "alice@example.org:443",
            "alice@mastomini.local",
            "alice@router.lan",
            "alice@127.0.0.1",
            "alice@10.0.0.1",
            "alice@example",
            "al ice@example.org",
            "alice@-bad.org",
        ] {
            assert!(normalize_acct(bad).is_err(), "{bad}");
        }
        assert_eq!(
            feed_url("alice@example.org"),
            "https://example.org/@alice.rss"
        );
        assert!(valid_host("mastodon.nu"));
        assert!(!valid_host("mastodon.nu/api"));
        assert!(!valid_host("Mastodon.NU"));
    }

    #[test]
    fn clipping_keeps_characters_whole() {
        assert_eq!(clip("  héllo\u{7}\n", 3), "hé");
        assert_eq!(clean_url("https://a.org/x"), Some("https://a.org/x".into()));
        assert_eq!(clean_url("javascript:alert(1)"), None);
        assert_eq!(clean_url("https://a.org/\"x"), None);
    }
}
