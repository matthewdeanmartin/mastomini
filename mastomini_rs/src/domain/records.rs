//! Records as they are persisted (spec/02-storage.md "Namespaces and keys").
//!
//! Anything derivable (counts, hashtags, notifications, timelines) is not
//! stored. Mentions *are* stored as account ids so a post's audience is fixed
//! when it is written, even if a username is later reused.

use crate::auth::Verifier;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Owner,
    Admin,
    Member,
}

impl Role {
    pub fn is_admin(self) -> bool {
        matches!(self, Role::Owner | Role::Admin)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }

    pub fn parse(text: &str) -> Option<Role> {
        match text {
            "owner" => Some(Role::Owner),
            "admin" => Some(Role::Admin),
            "member" | "user" => Some(Role::Member),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    Unlisted,
    Private,
    Direct,
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Unlisted => "unlisted",
            Visibility::Private => "private",
            Visibility::Direct => "direct",
        }
    }

    pub fn parse(text: &str) -> Option<Visibility> {
        match text {
            "public" => Some(Visibility::Public),
            "unlisted" => Some(Visibility::Unlisted),
            "private" => Some(Visibility::Private),
            "direct" => Some(Visibility::Direct),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRec {
    pub title: String,
    pub description: String,
    pub rules: Vec<String>,
}

/// Rules a new household starts with. Admins can change them
/// (`PUT /api/mastomini/v1/admin/server`).
pub const DEFAULT_RULES: [&str; 3] = [
    "Agree to be patient, the microcontroller is slow",
    "Agree to not unplug the microcontroller",
    "Agree to obey all laws in the jurisdiction of the microcontroller's physical location",
];

impl Default for ServerRec {
    fn default() -> Self {
        ServerRec {
            title: "mastomini".into(),
            description: "A private Mastodon-compatible server for one household.".into(),
            rules: DEFAULT_RULES.iter().map(|r| r.to_string()).collect(),
        }
    }
}

/// Terms of service set by an admin (`terms` in `mm_cfg`). Without one, the
/// server serves generated terms built from its rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermsRec {
    /// Plain text; paragraphs separated by blank lines.
    pub text: String,
    pub effective_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRec {
    /// Public API id (snowflake). Never reused, unlike the slot.
    pub id: u64,
    pub username: String,
    pub display_name: String,
    /// Bio as plain text; rendered to HTML on read.
    pub note: String,
    pub fields: Vec<Field>,
    pub role: Role,
    pub disabled: bool,
    /// Tombstone: purge in progress (spec/02 "Crash consistency").
    pub deleted: bool,
    pub locked: bool,
    pub bot: bool,
    pub discoverable: bool,
    pub verifier: Verifier,
    /// Bumping this revokes every token issued before, in one write.
    pub token_epoch: u32,
    pub privacy: Visibility,
    pub sensitive: bool,
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppRec {
    pub id: u64,
    pub client_id: String,
    pub secret_hash: [u8; 32],
    pub name: String,
    pub website: Option<String>,
    pub redirect_uris: Vec<String>,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenRec {
    pub id: u64,
    pub hash: [u8; 32],
    pub slot: u8,
    pub app_id: u64,
    pub scopes: Vec<String>,
    pub epoch: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusRec {
    pub id: u64,
    pub author: u8,
    pub text: String,
    pub spoiler_text: String,
    pub visibility: Visibility,
    pub sensitive: bool,
    pub language: Option<String>,
    pub in_reply_to_id: Option<u64>,
    /// Account id (not slot) of the replied-to author.
    pub in_reply_to_account_id: Option<u64>,
    /// Account ids mentioned when the post was written.
    pub mentions: Vec<u64>,
    pub app_id: Option<u64>,
    pub edited_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoostRec {
    pub id: u64,
    pub booster: u8,
    pub target: u64,
}

/// Favourite, bookmark or pin. The key holds kind, status and account; the
/// value holds the record id used for pagination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionRec {
    pub id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowRec {
    pub id: u64,
    pub reblogs: bool,
    pub notify: bool,
}

/// A block edge (`B` + blocker + blocked in `mm_rel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockRec {
    pub id: u64,
}

/// A mute edge (`M` + muter + muted in `mm_rel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuteRec {
    pub id: u64,
    /// Also hide notifications from the muted account (Mastodon's default).
    pub notifications: bool,
    /// Wall-clock expiry; `None` mutes indefinitely.
    pub expires_ms: Option<u64>,
}

/// Moderation state of one account (`m` + slot in `mm_mod`). Kept apart from
/// [`AccountRec`] so existing account records never change layout. The
/// account id guards against a stale record meeting a reused slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AccountModRec {
    pub account_id: u64,
    /// Mastodon's "limited": hidden from people who don't follow them.
    pub silenced: bool,
    /// When the account was suspended. Suspended accounts can't sign in and
    /// their posts are hidden, until unsuspended or deleted.
    pub suspended_ms: Option<u64>,
    /// Mastodon's "force sensitive". Only a flag here: there is no media.
    pub sensitized: bool,
}

impl AccountModRec {
    pub fn is_clear(&self) -> bool {
        !self.silenced && self.suspended_ms.is_none() && !self.sensitized
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReportCategory {
    Spam,
    Legal,
    Violation,
    Other,
}

impl ReportCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            ReportCategory::Spam => "spam",
            ReportCategory::Legal => "legal",
            ReportCategory::Violation => "violation",
            ReportCategory::Other => "other",
        }
    }

    pub fn parse(text: &str) -> Option<ReportCategory> {
        match text {
            "spam" => Some(ReportCategory::Spam),
            "legal" => Some(ReportCategory::Legal),
            "violation" => Some(ReportCategory::Violation),
            "other" => Some(ReportCategory::Other),
            _ => None,
        }
    }
}

/// A report to the household admins (`R` + id in `mm_mod`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportRec {
    pub id: u64,
    pub reporter: u8,
    pub target: u8,
    pub status_ids: Vec<u64>,
    pub comment: String,
    pub category: ReportCategory,
    /// Indexes into the server rules.
    pub rule_ids: Vec<u8>,
    pub forward: bool,
    pub assigned: Option<u8>,
    pub action_taken_ms: Option<u64>,
    pub action_taken_by: Option<u8>,
    pub updated_ms: u64,
}

/// One featured account in a collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionItemRec {
    pub id: u64,
    /// Account id, not slot: a collection outlives nothing, but ids are
    /// what the API speaks and they are never reused.
    pub account_id: u64,
    /// The featured account removed themselves. Kept so the curator can't
    /// simply add them back.
    pub revoked: bool,
}

/// A curated collection of accounts (`c` + id in `mm_coll`). Items live
/// inline: a household has at most 15 other members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionRec {
    pub id: u64,
    pub owner: u8,
    pub name: String,
    pub description: String,
    pub discoverable: bool,
    pub sensitive: bool,
    pub language: Option<String>,
    pub tag: Option<String>,
    pub items: Vec<CollectionItemRec>,
    pub updated_ms: u64,
}
