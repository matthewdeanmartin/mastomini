//! Mastodon entity JSON. Every field Mastodon defines is present, with a
//! neutral value where mastomini lacks the feature (spec/04 "Principles").

use super::time::{date, iso, iso_day};
use super::Ctx;
use crate::domain::query::Entry;
use crate::domain::records::{BoostRec, Role};
use crate::domain::{bit, Account, Notification, Service, Status};
use crate::ids;
use crate::store::Store;
use crate::text::{self, MentionTarget};
use serde_json::{json, Value};

pub fn avatar_url(ctx: &Ctx) -> String {
    format!("{}/avatars/original/missing.png", ctx.base_url)
}

pub fn header_url(ctx: &Ctx) -> String {
    format!("{}/headers/original/missing.png", ctx.base_url)
}

pub fn profile_url(ctx: &Ctx, username: &str) -> String {
    format!("{}/@{}", ctx.base_url, username)
}

fn fields_json(account: &Account) -> Vec<Value> {
    account
        .rec
        .fields
        .iter()
        .map(|f| json!({ "name": f.name, "value": text::escape(&f.value), "verified_at": null }))
        .collect()
}

pub fn account<S: Store>(svc: &Service<S>, ctx: &Ctx, account: &Account) -> Value {
    let rec = &account.rec;
    let (followers, following) = svc.follower_counts(account.slot);
    let url = profile_url(ctx, &rec.username);
    json!({
        "id": rec.id.to_string(),
        "username": rec.username,
        "acct": rec.username,
        "display_name": rec.display_name,
        "locked": rec.locked,
        "bot": rec.bot,
        "discoverable": rec.discoverable,
        "indexable": false,
        "group": false,
        "created_at": iso_day(ids::millis(rec.id)),
        "note": text::render_plain(&rec.note),
        "url": url,
        "uri": url,
        "avatar": avatar_url(ctx),
        "avatar_static": avatar_url(ctx),
        "header": header_url(ctx),
        "header_static": header_url(ctx),
        "followers_count": followers,
        "following_count": following,
        "statuses_count": svc.statuses_count(account.slot),
        "last_status_at": svc.last_status_ms(account.slot).map(date),
        "hide_collections": false,
        "noindex": true,
        "emojis": [],
        "roles": [],
        "fields": fields_json(account),
    })
}

fn role_json(role: Role) -> Value {
    match role {
        Role::Member => {
            json!({ "id": "-99", "name": "", "permissions": "0", "color": "", "highlighted": false })
        }
        Role::Admin => {
            json!({ "id": "3", "name": "Admin", "permissions": "1048575", "color": "", "highlighted": true })
        }
        Role::Owner => {
            json!({ "id": "0", "name": "Owner", "permissions": "1048575", "color": "", "highlighted": true })
        }
    }
}

/// `CredentialAccount`: the account plus its editable `source` and role.
pub fn credential_account<S: Store>(svc: &Service<S>, ctx: &Ctx, a: &Account) -> Value {
    let mut value = account(svc, ctx, a);
    let rec = &a.rec;
    let fields: Vec<Value> = rec
        .fields
        .iter()
        .map(|f| json!({ "name": f.name, "value": f.value, "verified_at": null }))
        .collect();
    value["source"] = json!({
        "privacy": rec.privacy.as_str(),
        "sensitive": rec.sensitive,
        "language": rec.language,
        "note": rec.note,
        "fields": fields,
        "follow_requests_count": 0,
        "discoverable": rec.discoverable,
        "indexable": false,
        "hide_collections": false,
        "attribution_domains": [],
    });
    value["role"] = role_json(rec.role);
    value
}

fn resolver<'a, S: Store>(
    svc: &'a Service<S>,
    ctx: &'a Ctx,
) -> impl Fn(&str) -> Option<MentionTarget> + 'a {
    move |username: &str| {
        svc.state
            .account_by_username(username)
            .map(|a| MentionTarget {
                url: profile_url(ctx, &a.rec.username),
                username: a.rec.username.clone(),
            })
    }
}

fn status_url(ctx: &Ctx, username: &str, id: u64) -> String {
    format!("{}/@{}/{}", ctx.base_url, username, id)
}

/// A status as `viewer` sees it.
pub fn status<S: Store>(svc: &Service<S>, ctx: &Ctx, s: &Status, viewer: Option<u8>) -> Value {
    let rec = &s.rec;
    let Some(author) = svc.state.account(rec.author) else {
        return Value::Null;
    };
    let viewer_bit = viewer.map_or(0, bit);
    let mentions: Vec<Value> = rec
        .mentions
        .iter()
        .filter_map(|id| svc.state.account_by_id(*id))
        .map(|a| {
            json!({
                "id": a.rec.id.to_string(),
                "username": a.rec.username,
                "acct": a.rec.username,
                "url": profile_url(ctx, &a.rec.username),
            })
        })
        .collect();
    let tags: Vec<Value> = s
        .tags
        .iter()
        .map(|t| json!({ "name": t, "url": format!("{}/tags/{}", ctx.base_url, t) }))
        .collect();
    let application = rec
        .app_id
        .and_then(|id| svc.state.apps.get(&id))
        .map(|app| json!({ "name": app.name, "website": app.website }));
    let url = status_url(ctx, &author.rec.username, rec.id);
    let resolve = resolver(svc, ctx);
    json!({
        "id": rec.id.to_string(),
        "uri": url,
        "url": url,
        "created_at": iso(ids::millis(rec.id)),
        "edited_at": rec.edited_at_ms.map(iso),
        "account": account(svc, ctx, author),
        "content": text::render(&rec.text, &resolve, &format!("{}/tags", ctx.base_url)),
        "text": null,
        "visibility": rec.visibility.as_str(),
        "sensitive": rec.sensitive || !rec.spoiler_text.is_empty(),
        "spoiler_text": rec.spoiler_text,
        "language": rec.language,
        "in_reply_to_id": rec.in_reply_to_id.map(|id| id.to_string()),
        "in_reply_to_account_id": rec.in_reply_to_account_id.map(|id| id.to_string()),
        "reblog": null,
        "application": application,
        "mentions": mentions,
        "tags": tags,
        "emojis": [],
        "media_attachments": [],
        "card": null,
        "poll": null,
        "quote": null,
        "replies_count": s.replies,
        "reblogs_count": s.boosted.count_ones(),
        "favourites_count": s.favourited.count_ones(),
        "quotes_count": 0,
        "favourited": s.favourited & viewer_bit != 0,
        "reblogged": s.boosted & viewer_bit != 0,
        "bookmarked": s.bookmarked & viewer_bit != 0,
        "pinned": s.pinned & bit(rec.author) != 0,
        "muted": false,
        "filtered": [],
    })
}

/// A boost: its own id and author, with the original nested in `reblog`.
pub fn boost<S: Store>(svc: &Service<S>, ctx: &Ctx, b: &BoostRec, viewer: Option<u8>) -> Value {
    let (Some(target), Some(booster)) = (
        svc.state.statuses.get(&b.target),
        svc.state.account(b.booster),
    ) else {
        return Value::Null;
    };
    let inner = status(svc, ctx, target, viewer);
    let url = status_url(ctx, &booster.rec.username, b.id);
    json!({
        "id": b.id.to_string(),
        "uri": url,
        "url": url,
        "created_at": iso(ids::millis(b.id)),
        "edited_at": null,
        "account": account(svc, ctx, booster),
        "content": "",
        "text": null,
        "visibility": inner["visibility"],
        "sensitive": false,
        "spoiler_text": "",
        "language": null,
        "in_reply_to_id": null,
        "in_reply_to_account_id": null,
        "application": null,
        "mentions": [],
        "tags": [],
        "emojis": [],
        "media_attachments": [],
        "card": null,
        "poll": null,
        "quote": null,
        "replies_count": 0,
        "reblogs_count": 0,
        "favourites_count": 0,
        "quotes_count": 0,
        "favourited": inner["favourited"],
        "reblogged": inner["reblogged"],
        "bookmarked": inner["bookmarked"],
        "pinned": false,
        "muted": false,
        "filtered": [],
        "reblog": inner,
    })
}

pub fn entry<S: Store>(svc: &Service<S>, ctx: &Ctx, e: Entry, viewer: Option<u8>) -> Value {
    match e {
        Entry::Status(id) => svc
            .state
            .statuses
            .get(&id)
            .map_or(Value::Null, |s| status(svc, ctx, s, viewer)),
        Entry::Boost { id, .. } => svc
            .state
            .boosts
            .get(&id)
            .map_or(Value::Null, |b| boost(svc, ctx, b, viewer)),
    }
}

pub fn relationship<S: Store>(svc: &Service<S>, viewer: u8, target: &Account) -> Value {
    let follow = svc.state.follows.get(&(viewer, target.slot));
    json!({
        "id": target.rec.id.to_string(),
        "following": follow.is_some(),
        "showing_reblogs": follow.is_some_and(|f| f.reblogs),
        "notifying": follow.is_some_and(|f| f.notify),
        "languages": null,
        "followed_by": svc.state.follows(target.slot, viewer),
        "blocking": false,
        "blocked_by": false,
        "muting": false,
        "muting_notifications": false,
        "requested": false,
        "requested_by": false,
        "domain_blocking": false,
        "endorsed": false,
        "note": "",
    })
}

pub fn notification<S: Store>(svc: &Service<S>, ctx: &Ctx, n: &Notification) -> Value {
    let from = svc.state.account(n.from);
    let status = n
        .status
        .and_then(|id| svc.state.statuses.get(&id))
        .map(|s| status(svc, ctx, s, Some(n.to)));
    json!({
        "id": n.id.to_string(),
        "type": n.kind.as_str(),
        "created_at": iso(ids::millis(n.id)),
        "group_key": format!("ungrouped-{}", n.id),
        "account": from.map(|a| account(svc, ctx, a)),
        "status": status,
    })
}

pub fn tag(ctx: &Ctx, name: &str, following: bool) -> Value {
    json!({
        "name": name,
        "url": format!("{}/tags/{}", ctx.base_url, name),
        "history": [],
        "following": following,
    })
}
