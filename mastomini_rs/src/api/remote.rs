//! Accounts on other servers ([`crate::domain::remote`]): their Mastodon
//! JSON, follow and search from any app, the household app's and admin's
//! pages, and the bots board's nightly pushes.
//!
//! Searching another server happens before the request reaches this module:
//! the server adapter asks [`pending_lookup`] what to fetch, fetches it
//! without the service lock, and stores the answer with [`finish_lookup`].
//! Handlers here only read that cache, so no handler ever waits on the
//! network.

use super::time::{date, iso, iso_day};
use super::{entities, fail, Call, Ctx, Reply};
use crate::domain::query::Entry;
use crate::domain::remote::{
    normalize_acct, FeedResult, IncomingItem, Outcome, RemoteItem, RemoteMedia, RemoteRef,
    RemoteSettings, SeenPost, ACCOUNT_FEEDS_CEILING, BOARD_FEEDS_CEILING, FEEDS_SCOPE,
    ITEMS_PER_FEED, MAX_IMAGE_BYTES, MAX_IMAGE_PX, MAX_ITEM_IMAGES, MAX_ITEM_TEXT,
};
use crate::domain::{bit, Error, Service};
use crate::http::Response;
use crate::ids;
use crate::store::Store;
use serde_json::{json, Value};

pub use super::remote_search::LOOKUP_LIMIT;
use super::remote_search::{hashtag, search_key, tag_key};

pub fn media_url(ctx: &Ctx, id: u64) -> String {
    format!("{}/media/remote/{}.jpg", ctx.base_url, ids::b32_str(id))
}

fn profile_url(r: &RemoteRef<'_>) -> String {
    format!("https://{}/@{}", r.host(), r.user())
}

/// `Account` for an account on another server.
pub fn account<S: Store>(svc: &Service<S>, ctx: &Ctx, r: RemoteRef<'_>) -> Value {
    let url = profile_url(&r);
    let avatar = r.account.and_then(|a| a.avatar.as_ref()).map_or_else(
        || format!("{}/avatars/original/missing.png", ctx.base_url),
        |m| media_url(ctx, m.id),
    );
    let items: Vec<&RemoteItem> = svc.remote.items_of(r.id).collect();
    let display_name = if r.display_name.is_empty() {
        r.user()
    } else {
        r.display_name
    };
    json!({
        "id": r.id.to_string(),
        "username": r.user(),
        "acct": r.acct,
        "display_name": display_name,
        "locked": false,
        "bot": false,
        "discoverable": true,
        "indexable": false,
        "group": false,
        "created_at": iso_day(r.account.map_or(ids::millis(r.id), |a| a.created_ms)),
        "note": crate::text::render_plain(r.note),
        "url": url,
        "uri": url,
        "avatar": avatar,
        "avatar_static": avatar,
        "header": entities::header_url(ctx),
        "header_static": entities::header_url(ctx),
        "followers_count": r.followers().count_ones(),
        "following_count": 0,
        "statuses_count": items.len(),
        "last_status_at": items.last().map(|i| date(i.published_ms)),
        "hide_collections": true,
        "noindex": true,
        "emojis": [],
        "roles": [],
        "fields": [],
    })
}

fn media_json(ctx: &Ctx, m: &RemoteMedia) -> Value {
    let url = media_url(ctx, m.id);
    let size = json!({
        "width": m.width,
        "height": m.height,
        "size": format!("{}x{}", m.width, m.height),
        "aspect": f64::from(m.width) / f64::from(m.height.max(1)),
    });
    json!({
        "id": m.id.to_string(),
        "type": "image",
        "url": url,
        "preview_url": url,
        "remote_url": null,
        "text_url": null,
        "meta": { "original": size, "small": size },
        "description": (!m.description.is_empty()).then_some(&m.description),
        "blurhash": null,
    })
}

/// A post from another server, as plain parts.
struct Post<'a> {
    id: u64,
    account: u64,
    url: &'a str,
    text: &'a str,
    cw: &'a str,
    sensitive: bool,
    published_ms: u64,
    media: Vec<Value>,
    /// Pictures not kept here: the post says so and links to the original.
    linked_images: u16,
}

/// `Status` for a post from another server. Read-only: every counter is 0.
pub fn status<S: Store>(svc: &Service<S>, ctx: &Ctx, item: &RemoteItem) -> Value {
    post_json(
        svc,
        ctx,
        Post {
            id: item.id,
            account: item.account,
            url: &item.url,
            text: &item.text,
            cw: &item.cw,
            sensitive: item.sensitive,
            published_ms: item.published_ms,
            media: item.media.iter().map(|m| media_json(ctx, m)).collect(),
            linked_images: 0,
        },
    )
}

/// `Status` for a post a search found (not followed, not stored). Its
/// pictures are never fetched: apps would load them from that server.
pub fn seen_status<S: Store>(svc: &Service<S>, ctx: &Ctx, p: &SeenPost) -> Value {
    post_json(
        svc,
        ctx,
        Post {
            id: p.id,
            account: p.account,
            url: &p.url,
            text: &p.text,
            cw: &p.cw,
            sensitive: p.sensitive,
            published_ms: p.published_ms,
            media: Vec::new(),
            linked_images: p.images,
        },
    )
}

fn post_json<S: Store>(svc: &Service<S>, ctx: &Ctx, post: Post<'_>) -> Value {
    let Some(author) = svc.remote_ref(post.account) else {
        return Value::Null;
    };
    let tags = format!("https://{}/tags", author.host());
    let no_mentions = |_: &str| None;
    let mut content = crate::text::render(post.text, &no_mentions, &tags);
    let link = |label: &str| {
        format!(
            "<p><a href=\"{}\" rel=\"nofollow noopener\" target=\"_blank\">{label}</a></p>",
            crate::text::escape(post.url)
        )
    };
    if post.linked_images > 0 {
        let n = post.linked_images;
        content.push_str(&link(&format!(
            "{n} picture{} on the original post",
            if n == 1 { "" } else { "s" }
        )));
    } else if post.media.is_empty() && post.text.is_empty() {
        content = link("Open the original post");
    }
    json!({
        "id": post.id.to_string(),
        "uri": post.url,
        "url": post.url,
        "created_at": iso(post.published_ms),
        "edited_at": null,
        "account": account(svc, ctx, author),
        "content": content,
        "text": null,
        "visibility": "public",
        "sensitive": post.sensitive || !post.cw.is_empty(),
        "spoiler_text": post.cw,
        "language": null,
        "in_reply_to_id": null,
        "in_reply_to_account_id": null,
        "reblog": null,
        "application": null,
        "mentions": [],
        "tags": [],
        "emojis": [],
        "media_attachments": post.media,
        "card": null,
        "poll": null,
        "quote": null,
        "replies_count": 0,
        "reblogs_count": 0,
        "favourites_count": 0,
        "quotes_count": 0,
        "favourited": false,
        "reblogged": false,
        "bookmarked": false,
        "pinned": false,
        "muted": false,
        "filtered": [],
    })
}

pub fn entry<S: Store>(svc: &Service<S>, ctx: &Ctx, e: Entry) -> Value {
    match e {
        Entry::Remote(id) => svc
            .remote
            .items
            .get(&id)
            .map_or(Value::Null, |i| status(svc, ctx, i)),
        _ => Value::Null,
    }
}

pub fn relationship(viewer: u8, r: RemoteRef<'_>) -> Value {
    relationship_of(r.id, r.followers() & bit(viewer) != 0)
}

fn relationship_of(id: u64, following: bool) -> Value {
    json!({
        "id": id.to_string(),
        "following": following,
        "showing_reblogs": following,
        "notifying": false,
        "languages": null,
        "followed_by": false,
        "blocking": false,
        "blocked_by": false,
        "muting": false,
        "muting_notifications": false,
        "muting_expires_at": null,
        "requested": false,
        "requested_by": false,
        "domain_blocking": false,
        "endorsed": false,
        "note": "",
    })
}

/// The remote account an API id names, if any.
fn remote_id<S: Store>(c: &Call<'_, S>, text: &str) -> Option<u64> {
    ids::parse(text).filter(|id| c.svc.remote_ref(*id).is_some())
}

fn read_only() -> Response {
    Response::error(
        422,
        "Accounts and posts on other servers can only be followed and read here",
    )
}

/// `/api/v1/accounts/*` for remote ids and handles; `seg` starts after
/// `accounts`. `None`: not remote, the local handlers answer.
pub(crate) fn accounts_route<S: Store>(
    c: &mut Call<'_, S>,
    method: &str,
    seg: &[&str],
) -> Option<Reply> {
    Some(match (method, seg) {
        ("GET", ["lookup"]) => {
            let acct = c.params.get("acct").unwrap_or("").trim_start_matches('@');
            let (_, host) = acct.split_once('@')?;
            if host.eq_ignore_ascii_case(c.ctx.host()) {
                return None;
            }
            (|| {
                c.user()?;
                let acct = normalize_acct(acct).map_err(fail)?;
                let r = c
                    .svc
                    .remote_ref_by_acct(&acct)
                    .ok_or_else(|| fail(Error::NotFound))?;
                Ok(Response::ok(account(c.svc, c.ctx, r)))
            })()
        }
        ("GET", ["relationships"]) => relationships(c),
        ("GET", []) => show_many(c),
        ("GET", [id]) => {
            let id = remote_id(c, id)?;
            c.user().map(|_| {
                let r = c.svc.remote_ref(id).expect("checked");
                Response::ok(account(c.svc, c.ctx, r))
            })
        }
        ("GET", [id, "statuses"]) => {
            let id = remote_id(c, id)?;
            (|| {
                c.user()?;
                let p = &c.params;
                if p.flag("pinned") || (p.flag("only_media") && c.svc.remote.media.is_none()) {
                    return Ok(Response::ok(json!([])));
                }
                let q = c.page_query();
                let page = c.svc.remote_statuses(id, &q);
                let ids: Vec<u64> = page.iter().map(|(id, _)| *id).collect();
                let body: Vec<Value> = page
                    .into_iter()
                    .map(|(_, e)| entry(c.svc, c.ctx, e))
                    .filter(|v| {
                        !p.flag("only_media")
                            || v["media_attachments"]
                                .as_array()
                                .is_some_and(|m| !m.is_empty())
                    })
                    .collect();
                Ok(c.paged(Value::Array(body), &ids, q.limit()))
            })()
        }
        ("GET", [id, "followers"]) => {
            let id = remote_id(c, id)?;
            (|| {
                c.user()?;
                let mask = c.svc.remote_ref(id).map_or(0, |r| r.followers());
                let rows: Vec<Value> = c
                    .svc
                    .state
                    .active_accounts()
                    .filter(|a| mask & bit(a.slot) != 0)
                    .map(|a| entities::account(c.svc, c.ctx, a))
                    .collect();
                Ok(Response::ok(Value::Array(rows)))
            })()
        }
        ("GET", [id, _]) => {
            remote_id(c, id)?;
            c.user().map(|_| Response::ok(json!([])))
        }
        ("POST", [id, action @ ("follow" | "unfollow")]) => {
            let id = remote_id(c, id)?;
            (|| {
                let viewer = c.user_scoped("follow")?;
                c.svc
                    .remote_follow(viewer, id, *action == "follow", c.now)
                    .map_err(fail)?;
                // The last unfollow deletes the account.
                Ok(Response::ok(match c.svc.remote_ref(id) {
                    Some(r) => relationship(viewer, r),
                    None => relationship_of(id, false),
                }))
            })()
        }
        ("POST", [id, ..]) => {
            remote_id(c, id)?;
            Err(read_only())
        }
        _ => return None,
    })
}

/// Local and remote ids together, in the order asked.
fn relationships<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user()?;
    let rows: Vec<Value> = c
        .params
        .all("id")
        .iter()
        .take(40)
        .filter_map(|id| ids::parse(id))
        .filter_map(|id| match c.svc.state.account_by_id(id) {
            Some(a) => Some(entities::relationship(c.svc, viewer, a)),
            None => c.svc.remote_ref(id).map(|r| relationship(viewer, r)),
        })
        .collect();
    Ok(Response::ok(Value::Array(rows)))
}

fn show_many<S: Store>(c: &Call<'_, S>) -> Reply {
    c.user()?;
    let rows: Vec<Value> = c
        .params
        .all("id")
        .iter()
        .take(40)
        .filter_map(|id| ids::parse(id))
        .filter_map(|id| match c.svc.state.account_by_id(id) {
            Some(a) => Some(entities::account(c.svc, c.ctx, a)),
            None => c.svc.remote_ref(id).map(|r| account(c.svc, c.ctx, r)),
        })
        .collect();
    Ok(Response::ok(Value::Array(rows)))
}

/// `/api/v1/statuses/:id/*` for posts from other servers.
pub(crate) fn statuses_route<S: Store>(
    c: &mut Call<'_, S>,
    method: &str,
    seg: &[&str],
) -> Option<Reply> {
    let id = seg.first().and_then(|id| ids::parse(id))?;
    if let Some(found) = c.svc.seen_post(id) {
        let found = found.clone();
        if let Err(r) = c.user() {
            return Some(Err(r));
        }
        return Some(match (method, &seg[1..]) {
            ("GET", []) => Ok(Response::ok(seen_status(c.svc, c.ctx, &found))),
            ("GET", ["context"]) => Ok(Response::ok(json!({"ancestors": [], "descendants": []}))),
            ("GET", ["favourited_by" | "reblogged_by" | "quotes"]) => Ok(Response::ok(json!([]))),
            ("GET", _) => Err(fail(Error::NotFound)),
            _ => Err(read_only()),
        });
    }
    let item = c.svc.remote.items.get(&id)?;
    let viewer = match c.user() {
        Ok(v) => v,
        Err(r) => return Some(Err(r)),
    };
    // Only followers see a post; anyone else is told it does not exist.
    let followed = c
        .svc
        .remote
        .accounts
        .get(&item.account)
        .is_some_and(|a| a.followers & bit(viewer) != 0);
    if !followed {
        return Some(Err(fail(Error::NotFound)));
    }
    Some(match (method, &seg[1..]) {
        ("GET", []) => Ok(Response::ok(status(c.svc, c.ctx, item))),
        ("GET", ["context"]) => Ok(Response::ok(json!({"ancestors": [], "descendants": []}))),
        ("GET", ["favourited_by" | "reblogged_by" | "quotes"]) => Ok(Response::ok(json!([]))),
        ("GET", _) => Err(fail(Error::NotFound)),
        _ => Err(read_only()),
    })
}

/// Remote accounts for a search, from what this board already knows:
/// followed accounts that match, an exact handle, and a cached answer from
/// the search server.
pub(crate) fn search_accounts<S: Store>(
    c: &Call<'_, S>,
    viewer: u8,
    q: &str,
    limit: usize,
) -> Vec<Value> {
    let svc = &c.svc;
    if !svc.remote.settings.enabled && svc.remote.followed_by(viewer).next().is_none() {
        return Vec::new();
    }
    let needle = q.trim().trim_start_matches('@').to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<u64> = Vec::new();
    if let Ok(acct) = normalize_acct(&needle) {
        if let Some(r) = svc.remote_ref_by_acct(&acct) {
            out.push(r.id);
        }
    }
    for a in svc.remote.accounts.values() {
        if a.acct.contains(&needle) || a.display_name.to_lowercase().contains(&needle) {
            out.push(a.id);
        }
    }
    if let Some(Ok(found)) = svc.remote_search_cached(&search_key(&needle), c.now) {
        out.extend(found.accounts.iter().copied());
    }
    if let Some(Ok(found)) = svc.remote_search_cached(&needle, c.now) {
        out.extend(found.accounts.iter().copied());
    }
    let mut seen = std::collections::BTreeSet::new();
    out.into_iter()
        .filter(|id| seen.insert(*id))
        .filter_map(|id| svc.remote_ref(id))
        .take(limit)
        .map(|r| account(svc, c.ctx, r))
        .collect()
}

/// Posts from elsewhere for a search: what the search server found (when
/// the household is signed in there, or for a `#hashtag`), then posts from
/// followed accounts on this board whose text matches.
pub(crate) fn search_statuses<S: Store>(
    c: &Call<'_, S>,
    viewer: u8,
    q: &str,
    limit: usize,
) -> Vec<Value> {
    let needle = q.trim().to_lowercase();
    if needle.chars().count() < 2 {
        return Vec::new();
    }
    let svc = &c.svc;
    let key = match hashtag(&needle) {
        Some(tag) => tag_key(tag),
        None => search_key(needle.trim_start_matches('@')),
    };
    let mut out: Vec<Value> = match svc.remote_search_cached(&key, c.now) {
        Some(Ok(found)) => found
            .posts
            .iter()
            .filter_map(|id| svc.seen_post(*id))
            .take(limit)
            .map(|p| seen_status(svc, c.ctx, p))
            .filter(|v| !v.is_null())
            .collect(),
        _ => Vec::new(),
    };
    let needle = hashtag(&needle).map_or(needle.clone(), |t| format!("#{t}"));
    let room = limit.saturating_sub(out.len());
    out.extend(
        svc.remote
            .items
            .values()
            .rev()
            .filter(|i| {
                svc.remote
                    .accounts
                    .get(&i.account)
                    .is_some_and(|a| a.followers & bit(viewer) != 0)
            })
            .filter(|i| {
                i.text.to_lowercase().contains(&needle) || i.cw.to_lowercase().contains(&needle)
            })
            .take(room)
            .map(|i| status(svc, c.ctx, i)),
    );
    out
}

/// Mastodon's HTML (notes, posts) as plain text: tags dropped, paragraphs
/// and line breaks kept, the common entities decoded.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            rest = "";
            break;
        };
        let tag = rest[start + 1..start + end].to_ascii_lowercase();
        if tag.starts_with("br") || tag == "/p" {
            out.push('\n');
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    let decoded = out
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&");
    decoded.trim().to_string()
}

// The household app and admin pages.

fn feed_json<S: Store>(svc: &Service<S>, a: &crate::domain::remote::RemoteAccount) -> Value {
    json!({
        "url": a.feed_url(),
        "last_ok_at": a.feed.last_ok_ms.map(iso),
        "last_try_at": a.feed.last_try_ms.map(iso),
        "failures": a.feed.failures,
        "error": a.feed.error,
        "gone": a.feed.gone,
        "posts": svc.remote.items_of(a.id).count(),
    })
}

fn settings_json<S: Store>(svc: &Service<S>) -> Value {
    let s = &svc.remote.settings;
    json!({
        "enabled": s.enabled,
        "search_server": s.search_server,
        "feeds_max_board": s.feeds_max_board,
        "feeds_max_account": s.feeds_max_account,
        "feeds_ceiling_board": BOARD_FEEDS_CEILING,
        "feeds_ceiling_account": ACCOUNT_FEEDS_CEILING,
        "media_quota_mb": s.media_quota_mb,
        "images": svc.remote.media.is_some(),
        "media_bytes": svc.remote.media_bytes,
        "board_count": svc.remote.accounts.len(),
        // Signed in to the search server (the token itself is never shown).
        "search_account": svc
            .remote
            .search_auth
            .as_ref()
            .filter(|a| !a.token.is_empty())
            .map(|a| a.acct.clone()),
        "search_signed_in": svc.search_token().is_some(),
    })
}

/// `GET /api/mastomini/v1/remote`: the settings, and what I follow.
fn mine<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user()?;
    let following: Vec<Value> = c
        .svc
        .remote
        .followed_by(viewer)
        .map(|a| {
            let mut v = account(c.svc, c.ctx, c.svc.remote_ref(a.id).expect("stored"));
            v["feed"] = feed_json(c.svc, a);
            v
        })
        .collect();
    let mut body = settings_json(c.svc);
    body["following"] = Value::Array(following);
    Ok(Response::ok(body))
}

fn admin_view<S: Store>(c: &Call<'_, S>) -> Reply {
    let actor = c.user_scoped("admin:read")?;
    c.svc.require_admin(actor).map_err(fail)?;
    let accounts: Vec<Value> = c
        .svc
        .remote
        .accounts
        .values()
        .map(|a| {
            let followers: Vec<&str> = c
                .svc
                .state
                .active_accounts()
                .filter(|m| a.followers & bit(m.slot) != 0)
                .map(|m| m.rec.username.as_str())
                .collect();
            json!({
                "id": a.id.to_string(),
                "acct": a.acct,
                "display_name": a.display_name,
                "followers": followers,
                "feed": feed_json(c.svc, a),
            })
        })
        .collect();
    let devices: Vec<Value> = c
        .svc
        .remote
        .devices
        .values()
        .map(|d| {
            json!({
                "id": d.id.to_string(),
                "name": d.name,
                "created_at": iso(d.created_ms),
                "last_used_at": c.svc.remote.device_used.get(&d.id).map(|ms| iso(*ms)),
            })
        })
        .collect();
    let mut body = settings_json(c.svc);
    body["accounts"] = Value::Array(accounts);
    body["device_keys"] = Value::Array(devices);
    Ok(Response::ok(body))
}

fn admin_update<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let actor = c.user_scoped("admin:write")?;
    let old = c.svc.remote.settings.clone();
    let p = &c.params;
    let number = |name: &str, old: u64| -> Result<u64, Response> {
        match p.get(name) {
            None => Ok(old),
            Some(v) => v
                .parse()
                .map_err(|_| Response::error(422, &format!("{name} must be a whole number"))),
        }
    };
    let settings = RemoteSettings {
        enabled: p.bool("enabled").unwrap_or(old.enabled),
        search_server: p
            .get("search_server")
            .map_or(old.search_server.clone(), str::to_string),
        feeds_max_board: number("feeds_max_board", old.feeds_max_board.into())?.min(u16::MAX.into())
            as u16,
        feeds_max_account: number("feeds_max_account", old.feeds_max_account.into())?
            .min(u16::MAX.into()) as u16,
        media_quota_mb: number("media_quota_mb", old.media_quota_mb.into())?.min(u32::MAX.into())
            as u32,
    };
    c.svc
        .set_remote_settings(actor, settings, c.now)
        .map_err(fail)?;
    admin_view(c)
}

fn create_device_key<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let actor = c.user_scoped("admin:write")?;
    let name = c.params.get("name").unwrap_or("").to_string();
    let (id, key) = c.svc.create_device_key(actor, &name, c.now).map_err(fail)?;
    Ok(Response::ok(
        json!({"id": id.to_string(), "name": name.trim(), "key": key}),
    ))
}

fn revoke_device_key<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let actor = c.user_scoped("admin:write")?;
    let id = c.id(id)?;
    c.svc.revoke_device_key(actor, id, c.now).map_err(fail)?;
    Ok(Response::ok(json!({})))
}

// The bots board.

/// Only a device credential with `feeds:sync`; nothing else gets in.
fn device<S: Store>(c: &Call<'_, S>) -> Result<(), Response> {
    match &c.principal {
        Some(p) if p.slot.is_none() && p.allows(FEEDS_SCOPE) => Ok(()),
        Some(_) => Err(Response::error(403, "Only a device key can do this")),
        None => Err(fail(Error::Unauthorized)),
    }
}

fn jobs<S: Store>(c: &Call<'_, S>) -> Reply {
    device(c)?;
    let jobs: Vec<Value> = c
        .svc
        .feed_jobs()
        .into_iter()
        .map(|j| {
            json!({
                "account": j.account.to_string(),
                "url": j.url,
                "etag": j.etag,
                "last_modified": j.last_modified,
                "wants_avatar": j.wants_avatar,
                "newest_ms": j.newest_ms,
            })
        })
        .collect();
    Ok(Response::ok(json!({
        "jobs": jobs,
        "images": c.svc.remote.media.is_some(),
        "max_items": ITEMS_PER_FEED,
        "max_text": MAX_ITEM_TEXT,
        "max_images": MAX_ITEM_IMAGES,
        "image": { "max_px": MAX_IMAGE_PX, "max_bytes": MAX_IMAGE_BYTES },
    })))
}

fn account_param<S: Store>(c: &Call<'_, S>, id: &str) -> Result<u64, Response> {
    let id = c.id(id)?;
    if c.svc.remote.accounts.contains_key(&id) {
        Ok(id)
    } else {
        // Unfollowed since the jobs were read: the bots board drops it.
        Err(Response::error(410, "Nobody follows this account any more"))
    }
}

fn media_upload<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    device(c)?;
    let account = account_param(c, id)?;
    let id = c
        .svc
        .feed_media_put(account, &c.req.body, c.now)
        .map_err(fail)?;
    Ok(Response::ok(json!({"id": id.to_string()})))
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn result<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    device(c)?;
    let account = account_param(c, id)?;
    let v: Value = serde_json::from_slice(&c.req.body)
        .map_err(|e| Response::error(400, &format!("Invalid JSON: {e}")))?;
    let outcome = match v["outcome"].as_str() {
        Some("ok") => Outcome::Ok,
        Some("not_modified") => Outcome::NotModified,
        Some("gone") => Outcome::Gone,
        Some("failed") => Outcome::Failed,
        _ => {
            return Err(Response::error(
                422,
                "outcome must be ok, not_modified, gone or failed",
            ))
        }
    };
    let items = v["items"]
        .as_array()
        .into_iter()
        .flatten()
        .take(ITEMS_PER_FEED)
        .map(|i| IncomingItem {
            guid: text(&i["guid"]),
            published_ms: i["published_ms"].as_u64().unwrap_or(0),
            url: text(&i["url"]),
            text: text(&i["text"]),
            cw: text(&i["cw"]),
            sensitive: i["sensitive"].as_bool().unwrap_or(false),
            media: i["media"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| Some((ids::parse(m["id"].as_str()?)?, text(&m["description"]))))
                .collect(),
        })
        .collect();
    let result = FeedResult {
        outcome,
        error: v["error"].as_str().map(str::to_string),
        etag: text(&v["etag"]),
        last_modified: text(&v["last_modified"]),
        title: v["title"].as_str().map(str::to_string),
        note: v["note"].as_str().map(str::to_string),
        avatar: v["avatar"].as_str().and_then(ids::parse),
        items,
    };
    let added = c.svc.feed_result(account, result, c.now).map_err(fail)?;
    Ok(Response::ok(json!({"added": added})))
}

/// `/media/remote/<id>.jpg`: an image from the SD card, streamed. Public
/// like Mastodon's media URLs: apps load them without a token.
pub(crate) fn media<S: Store>(c: &Call<'_, S>, file: &str) -> Response {
    let id = file.strip_suffix(".jpg").and_then(ids::from_b32);
    let dir = c.svc.remote.media.as_ref();
    let known = id.is_some_and(|id| {
        let r = &c.svc.remote;
        r.accounts
            .values()
            .filter_map(|a| a.avatar.as_ref())
            .chain(r.items.values().flat_map(|i| i.media.iter()))
            .any(|m| m.id == id)
    });
    match (id, dir) {
        (Some(id), Some(dir)) if known => Response::file("image/jpeg", dir.path(id))
            .with_header("Cache-Control", "public, max-age=31536000, immutable"),
        _ => Response::error(404, "Record not found"),
    }
}

/// `/api/mastomini/v1/...` routes; `seg` starts after `v1`.
pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    if let Some(reply) = super::remote_search::route(c, method, seg) {
        return Some(reply);
    }
    Some(match (method, seg) {
        ("GET", ["remote"]) => mine(c),
        ("GET", ["admin", "remote"]) => admin_view(c),
        ("PUT" | "PATCH", ["admin", "remote"]) => admin_update(c),
        ("POST", ["admin", "device_keys"]) => create_device_key(c),
        ("DELETE", ["admin", "device_keys", id]) => revoke_device_key(c, id),
        ("GET", ["feeds", "jobs"]) => jobs(c),
        ("POST", ["feeds", id, "result"]) => result(c, id),
        ("POST" | "PUT", ["feeds", id, "media"]) => media_upload(c, id),
        _ => return None,
    })
}
