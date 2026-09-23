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

impl Default for ServerRec {
    fn default() -> Self {
        ServerRec {
            title: "mastomini".into(),
            description: "A private Mastodon-compatible server for one household.".into(),
            rules: Vec::new(),
        }
    }
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
