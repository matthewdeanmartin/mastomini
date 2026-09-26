//! Mastodon entity JSON. Every field Mastodon defines is present, with a
//! neutral value where mastomini lacks the feature (spec/04 "Principles").

use super::time::{date, iso, iso_day};
use super::Ctx;
use crate::domain::query::Entry;
use crate::domain::records::{BoostRec, CollectionRec, PollRec, ReportRec, Role};
use crate::domain::{bit, Account, Notification, NotificationKind, Service, Status};
use crate::ids;
use crate::store::Store;
use crate::text::{self, MentionTarget};
use serde_json::{json, Value};

/// The generated avatar ([`crate::avatar`]).
pub fn avatar_url(ctx: &Ctx, account_id: u64) -> String {
    format!("{}/avatars/{account_id}.png", ctx.base_url)
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
    if let Some(value) = svc
        .state
        .rendered_accounts
        .borrow()
        .as_ref()
        .and_then(|c| c.get(&account.slot))
    {
        return value.clone();
    }
    let rec = &account.rec;
    let (followers, following) = svc.follower_counts(account.slot);
    let url = profile_url(ctx, &rec.username);
    let moderation = svc.state.moderation(account.slot);
    let mut value = json!({
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
        "avatar": avatar_url(ctx, rec.id),
        "avatar_static": avatar_url(ctx, rec.id),
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
    });
    // Mastodon's optional flags, present only when set. A suspended
    // account keeps its name but shows nothing else about itself.
    if moderation.silenced {
        value["limited"] = json!(true);
    }
    if moderation.suspended_ms.is_some() {
        value["suspended"] = json!(true);
        value["display_name"] = json!("");
        value["note"] = json!("");
        value["fields"] = json!([]);
        value["locked"] = json!(false);
        value["bot"] = json!(false);
        value["statuses_count"] = json!(0);
        value["followers_count"] = json!(0);
        value["following_count"] = json!(0);
        value["last_status_at"] = Value::Null;
    }
    if let Some(cache) = svc.state.rendered_accounts.borrow_mut().as_mut() {
        cache.insert(account.slot, value.clone());
    }
    value
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
        "follow_requests_count": svc
            .state
            .follow_requests
            .keys()
            .filter(|(_, dst)| *dst == a.slot)
            .count(),
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

/// Status text as Mastodon HTML.
pub fn render_text<S: Store>(svc: &Service<S>, ctx: &Ctx, text: &str) -> String {
    text::render(text, &resolver(svc, ctx), &format!("{}/tags", ctx.base_url))
}

/// `Poll`, as `viewer` sees it. Totals stay hidden until the end when the
/// author asked for that, from the author too (as on Mastodon).
pub fn poll<S: Store>(svc: &Service<S>, id: u64, poll: &PollRec, viewer: Option<u8>) -> Value {
    let expired = poll.expires_ms <= svc.state.now_ms;
    let show = expired || !poll.hide_totals;
    let voters = poll.voters();
    let author = svc.state.statuses.get(&id).map(|s| s.rec.author);
    let options: Vec<Value> = poll
        .options
        .iter()
        .zip(&poll.votes)
        .map(|(title, votes)| json!({ "title": title, "votes_count": show.then(|| votes.count_ones()) }))
        .collect();
    let own: Vec<usize> = viewer.map_or_else(Vec::new, |v| {
        (0..poll.votes.len())
            .filter(|i| poll.votes[*i] & bit(v) != 0)
            .collect()
    });
    json!({
        "id": id.to_string(),
        "expires_at": iso(poll.expires_ms),
        "expired": expired,
        "multiple": poll.multiple,
        "votes_count": poll.votes.iter().map(|v| v.count_ones()).sum::<u32>(),
        "voters_count": poll.multiple.then(|| voters.count_ones()),
        "options": options,
        "emojis": [],
        "voted": viewer.is_some_and(|v| Some(v) == author || voters & bit(v) != 0),
        "own_votes": own,
    })
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
        .and_then(|id| svc.state.application_name(id))
        .map(|(name, website)| json!({ "name": name, "website": website }));
    let url = status_url(ctx, &author.rec.username, rec.id);
    let resolve = resolver(svc, ctx);
    let (text, spoiler_text) = svc.readable_text(viewer, s);
    json!({
        "id": rec.id.to_string(),
        "uri": url,
        "url": url,
        "created_at": iso(ids::millis(rec.id)),
        "edited_at": rec.edited_at_ms.map(iso),
        "account": account(svc, ctx, author),
        "content": text::render(&text, &resolve, &format!("{}/tags", ctx.base_url)),
        "text": null,
        "visibility": rec.visibility.as_str(),
        "sensitive": rec.sensitive
            || !spoiler_text.is_empty()
            || (viewer != Some(rec.author) && svc.state.moderation(rec.author).sensitized),
        "spoiler_text": spoiler_text,
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
        "poll": svc.state.polls.get(&rec.id).map(|p| poll(svc, rec.id, p, viewer)),
        "quote": null,
        "replies_count": s.replies,
        "reblogs_count": s.boosted.count_ones(),
        "favourites_count": s.favourited.count_ones(),
        "quotes_count": 0,
        "favourited": s.favourited & viewer_bit != 0,
        "reblogged": s.boosted & viewer_bit != 0,
        "bookmarked": s.bookmarked & viewer_bit != 0,
        "pinned": s.pinned & bit(rec.author) != 0,
        "muted": viewer.is_some_and(|v| svc.thread_muted(v, rec.id)),
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
        "muted": inner["muted"],
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
    let mute = svc.state.mute(viewer, target.slot);
    json!({
        "id": target.rec.id.to_string(),
        "following": follow.is_some(),
        "showing_reblogs": follow.is_some_and(|f| f.reblogs),
        "notifying": follow.is_some_and(|f| f.notify),
        "languages": null,
        "followed_by": svc.state.follows(target.slot, viewer),
        "blocking": svc.state.blocks(viewer, target.slot),
        "blocked_by": svc.state.blocks(target.slot, viewer),
        "muting": mute.is_some(),
        "muting_notifications": mute.is_some_and(|m| m.notifications),
        "muting_expires_at": mute.and_then(|m| m.expires_ms).map(iso),
        "requested": svc.state.follow_requests.contains_key(&(viewer, target.slot)),
        "requested_by": svc.state.follow_requests.contains_key(&(target.slot, viewer)),
        "domain_blocking": false,
        "endorsed": false,
        "note": svc.account_note(viewer, target.slot),
    })
}

pub fn notification<S: Store>(svc: &Service<S>, ctx: &Ctx, n: &Notification) -> Value {
    let from = svc.state.account(n.from);
    let status = n
        .status
        .and_then(|id| svc.state.statuses.get(&id))
        .map(|s| status(svc, ctx, s, Some(n.to)));
    let mut value = json!({
        "id": n.id.to_string(),
        "type": n.kind.as_str(),
        "created_at": iso(ids::millis(n.id)),
        "group_key": super::notification_groups::key(n),
        "account": from.map(|a| account(svc, ctx, a)),
        "status": status,
    });
    match n.kind {
        NotificationKind::AdminReport => {
            value["report"] = n
                .object
                .and_then(|id| svc.state.reports.get(&id))
                .map_or(Value::Null, |r| report(svc, ctx, r));
        }
        NotificationKind::AddedToCollection => {
            value["collection"] = n
                .object
                .and_then(|id| svc.state.collections.get(&id))
                .map_or(Value::Null, |c| collection(svc, ctx, c, n.to));
        }
        _ => {}
    }
    value
}

fn rule_ids(r: &ReportRec) -> Vec<String> {
    r.rule_ids.iter().map(|id| id.to_string()).collect()
}

/// `Report`, as the reporter (and a notified admin) sees it.
pub fn report<S: Store>(svc: &Service<S>, ctx: &Ctx, r: &ReportRec) -> Value {
    let target = svc.state.account(r.target);
    json!({
        "id": r.id.to_string(),
        "action_taken": r.action_taken_ms.is_some(),
        "action_taken_at": r.action_taken_ms.map(iso),
        "category": r.category.as_str(),
        "comment": r.comment,
        "forwarded": false,
        "created_at": iso(ids::millis(r.id)),
        "status_ids": r.status_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        "rule_ids": rule_ids(r),
        "collection_ids": [],
        "target_account": target.map(|a| account(svc, ctx, a)),
    })
}

/// `Admin::Account`. There are no e-mail addresses or IPs to show.
pub fn admin_account<S: Store>(svc: &Service<S>, ctx: &Ctx, a: &Account) -> Value {
    let m = svc.state.moderation(a.slot);
    json!({
        "id": a.rec.id.to_string(),
        "username": a.rec.username,
        "domain": null,
        "created_at": iso(ids::millis(a.rec.id)),
        "email": "",
        "ip": null,
        "ips": [],
        "locale": a.rec.language.clone().unwrap_or_default(),
        "invite_request": null,
        "role": role_json(a.rec.role),
        "confirmed": true,
        "approved": true,
        "disabled": a.rec.disabled,
        "silenced": m.silenced,
        "suspended": m.suspended_ms.is_some(),
        "sensitized": m.sensitized,
        "account": account(svc, ctx, a),
        "created_by_application_id": null,
        "invited_by_account_id": null,
    })
}

/// `Admin::Report`.
pub fn admin_report<S: Store>(svc: &Service<S>, ctx: &Ctx, r: &ReportRec, viewer: u8) -> Value {
    let admin = |slot: Option<u8>| {
        slot.and_then(|s| svc.state.account(s))
            .map_or(Value::Null, |a| admin_account(svc, ctx, a))
    };
    // Reported posts are shown to admins even if they could not otherwise
    // see them, except direct messages they aren't part of (spec/04).
    let statuses: Vec<Value> = r
        .status_ids
        .iter()
        .filter_map(|id| svc.state.statuses.get(id))
        .filter(|s| {
            s.rec.visibility != crate::domain::records::Visibility::Direct
                || s.rec.author == viewer
                || s.mentions & bit(viewer) != 0
        })
        .map(|s| status(svc, ctx, s, None))
        .collect();
    let rules: Vec<Value> = r
        .rule_ids
        .iter()
        .filter_map(|id| {
            svc.state
                .server
                .rules
                .get((*id as usize).checked_sub(1)?)
                .map(|text| json!({ "id": id.to_string(), "text": text, "hint": "" }))
        })
        .collect();
    json!({
        "id": r.id.to_string(),
        "action_taken": r.action_taken_ms.is_some(),
        "action_taken_at": r.action_taken_ms.map(iso),
        "category": r.category.as_str(),
        "comment": r.comment,
        "forwarded": false,
        "created_at": iso(ids::millis(r.id)),
        "updated_at": iso(r.updated_ms),
        "account": admin(Some(r.reporter)),
        "target_account": admin(Some(r.target)),
        "assigned_account": admin(r.assigned),
        "action_taken_by_account": admin(r.action_taken_by),
        "statuses": statuses,
        "rules": rules,
    })
}

pub fn collection_url(ctx: &Ctx, id: u64) -> String {
    format!("{}/collections/{}", ctx.base_url, id)
}

pub fn collection_item(svc_item: &crate::domain::records::CollectionItemRec) -> Value {
    json!({
        "id": svc_item.id.to_string(),
        "account_id": svc_item.account_id.to_string(),
        "state": if svc_item.revoked { "revoked" } else { "accepted" },
        "created_at": iso(ids::millis(svc_item.id)),
    })
}

/// `Collection`, with the items `viewer` may see.
pub fn collection<S: Store>(svc: &Service<S>, ctx: &Ctx, c: &CollectionRec, viewer: u8) -> Value {
    let owner_id = svc.state.account(c.owner).map_or(0, |a| a.rec.id);
    let items: Vec<Value> = svc.visible_items(viewer, c).map(collection_item).collect();
    json!({
        "id": c.id.to_string(),
        "account_id": owner_id.to_string(),
        "uri": collection_url(ctx, c.id),
        "url": collection_url(ctx, c.id),
        "name": c.name,
        "description": c.description,
        "language": c.language,
        "local": true,
        "sensitive": c.sensitive,
        "discoverable": c.discoverable,
        "tag": c.tag.as_ref().map(|t| json!({ "name": t, "url": format!("{}/tags/{}", ctx.base_url, t) })),
        "item_count": svc.accepted_count(c),
        "items": items,
        "created_at": iso(ids::millis(c.id)),
        "updated_at": iso(c.updated_ms),
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
