//! Timelines, notifications, markers, search, favourites and bookmarks.
//! All reads come from RAM; notifications and markers never touch flash.

use super::time::iso;
use super::{entities, fail, Call, Reply};
use crate::domain::query::Entry;
use crate::domain::{Error, Notification, ReactionKind};
use crate::http::Response;
use crate::store::Store;
use serde_json::{json, Map, Value};

const SEARCH_LIMIT: usize = 20;
const TIMELINES: [&str; 2] = ["home", "notifications"];

/// A page of statuses, annotated with the viewer's filters for `context`.
fn entries<S: Store>(
    c: &Call<'_, S>,
    page: Vec<(u64, Entry)>,
    viewer: u8,
    limit: usize,
    context: &str,
) -> Response {
    let ids: Vec<u64> = page.iter().map(|(id, _)| *id).collect();
    let body: Vec<Value> = page
        .into_iter()
        .map(|(_, e)| entities::entry(c.svc, c.ctx, e, Some(viewer)))
        .filter(|v| !v.is_null())
        .collect();
    let body = super::filters::annotate(c, viewer, context, body);
    c.paged(Value::Array(body), &ids, limit)
}

fn home<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user_scoped("read:statuses")?;
    let q = c.page_query();
    let page = c.svc.home(viewer, &q);
    Ok(entries(c, page, viewer, q.limit(), "home"))
}

fn direct<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user_scoped("read:statuses")?;
    let q = c.page_query();
    let page = c.svc.status_page(&q, |s| {
        s.rec.visibility == crate::domain::records::Visibility::Direct
            && c.svc.can_see(Some(viewer), s)
            && !c.svc.state.hides(viewer, s.rec.author)
    });
    Ok(entries(c, page, viewer, q.limit(), "home"))
}

fn public<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user_scoped("read:statuses")?;
    let q = c.page_query();
    // There is nothing remote, and no media.
    if c.params.flag("remote") || c.params.flag("only_media") {
        return Ok(Response::ok(json!([])));
    }
    let page = c.svc.public(viewer, &q);
    Ok(entries(c, page, viewer, q.limit(), "public"))
}

fn tag<S: Store>(c: &Call<'_, S>, name: &str) -> Reply {
    let viewer = c.user_scoped("read:statuses")?;
    let q = c.page_query();
    if c.params.flag("remote") || c.params.flag("only_media") {
        return Ok(Response::ok(json!([])));
    }
    let name = percent_encoding::percent_decode_str(name)
        .decode_utf8()
        .map_err(|_| Response::error(400, "Invalid UTF-8 tag"))?;
    let page = c.svc.tag_timeline(viewer, &name, &q);
    Ok(entries(c, page, viewer, q.limit(), "public"))
}

fn reactions<S: Store>(c: &Call<'_, S>, kind: ReactionKind) -> Reply {
    let scope = if kind == ReactionKind::Favourite {
        "read:favourites"
    } else {
        "read:bookmarks"
    };
    let viewer = c.user_scoped(scope)?;
    let q = c.page_query();
    let page = c.svc.reactions_of(viewer, kind, &q);
    let ids: Vec<u64> = page.iter().map(|(id, _)| *id).collect();
    let body: Vec<Value> = page
        .into_iter()
        .filter_map(|(_, status)| c.svc.state.statuses.get(&status))
        .map(|s| entities::status(c.svc, c.ctx, s, Some(viewer)))
        .collect();
    Ok(c.paged(Value::Array(body), &ids, q.limit()))
}

fn notification_filter<S: Store>(c: &Call<'_, S>) -> (Vec<String>, Vec<String>, Option<u8>) {
    let list =
        |name: &str| -> Vec<String> { c.params.all(name).iter().map(|s| s.to_string()).collect() };
    let from = c
        .params
        .u64("account_id")
        .and_then(|id| c.svc.state.slot_of(id));
    (list("types"), list("exclude_types"), from)
}

/// One page of the viewer's notifications, after filters.
struct NotificationPage {
    viewer: u8,
    rows: Vec<(u64, Notification)>,
    limit: usize,
}

fn notifications_page<S: Store>(c: &Call<'_, S>) -> Result<NotificationPage, Response> {
    let viewer = c.user_scoped("read:notifications")?;
    let (include, exclude, from) = notification_filter(c);
    let q = c.page_query();
    let rows = if c.params.u64("account_id").is_some() && from.is_none() {
        Vec::new()
    } else {
        c.svc.notifications(viewer, &include, &exclude, from, &q)
    };
    Ok(NotificationPage {
        viewer,
        rows,
        limit: q.limit(),
    })
}

fn notifications<S: Store>(c: &Call<'_, S>) -> Reply {
    let NotificationPage {
        viewer,
        rows,
        limit,
    } = notifications_page(c)?;
    let ids: Vec<u64> = rows.iter().map(|(id, _)| *id).collect();
    let body: Vec<Value> = rows
        .iter()
        .map(|(_, n)| entities::notification(c.svc, c.ctx, n))
        .collect();
    let body = super::filters::annotate(c, viewer, "notifications", body);
    Ok(c.paged(Value::Array(body), &ids, limit))
}

fn notification<S: Store>(c: &Call<'_, S>, id: &str) -> Reply {
    let viewer = c.user_scoped("read:notifications")?;
    let id = c.id(id)?;
    let n = c
        .svc
        .state
        .notifications
        .iter()
        .find(|n| n.id == id && n.to == viewer)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(entities::notification(c.svc, c.ctx, n)))
}

fn unread_count<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user_scoped("read:notifications")?;
    let read = c.svc.state.markers[viewer as usize][1].map_or(0, |m| m.last_read_id);
    let count = c
        .svc
        .state
        .notifications
        .iter()
        .filter(|n| n.to == viewer && n.id > read)
        .count()
        .min(1000);
    Ok(Response::ok(json!({ "count": count })))
}

fn dismiss<S: Store>(c: &mut Call<'_, S>, id: Option<&str>) -> Reply {
    let viewer = c.user_scoped("write:notifications")?;
    let id = match id {
        Some(id) => c.id(id)?,
        None => c.params.u64("id").ok_or_else(|| fail(Error::NotFound))?,
    };
    c.svc.dismiss_notification(viewer, id).map_err(fail)?;
    Ok(Response::ok(json!({})))
}

fn clear<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let viewer = c.user_scoped("write:notifications")?;
    c.svc.clear_notifications(viewer);
    Ok(Response::ok(json!({})))
}

fn marker_json(marker: &crate::domain::Marker) -> Value {
    json!({
        "last_read_id": marker.last_read_id.to_string(),
        "version": marker.version,
        "updated_at": iso(marker.updated_ms),
    })
}

fn markers_get<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user_scoped("read:statuses")?;
    let wanted = c.params.all("timeline");
    let mut out = Map::new();
    for (i, name) in TIMELINES.iter().enumerate() {
        if !wanted.is_empty() && !wanted.contains(name) {
            continue;
        }
        if let Some(marker) = &c.svc.state.markers[viewer as usize][i] {
            out.insert(name.to_string(), marker_json(marker));
        }
    }
    Ok(Response::ok(Value::Object(out)))
}

fn markers_set<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let viewer = c.user_scoped("write:statuses")?;
    let mut out = Map::new();
    for (i, name) in TIMELINES.iter().enumerate() {
        if let Some(last) = c.params.u64(&format!("{name}[last_read_id]")) {
            let marker = c.svc.set_marker(viewer, i, last, c.now);
            out.insert(name.to_string(), marker_json(&marker));
        }
    }
    Ok(Response::ok(Value::Object(out)))
}

pub(super) fn search<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user_scoped("read:search")?;
    let q = c.params.get("q").unwrap_or("").trim().to_string();
    let kind = c.params.get("type").unwrap_or("");
    let limit = (c.params.u64("limit").unwrap_or(SEARCH_LIMIT as u64) as usize).min(SEARCH_LIMIT);
    let offset = c.params.u64("offset").unwrap_or(0) as usize;
    let want = |t: &str| kind.is_empty() || kind == t;
    let accounts: Vec<Value> = if want("accounts") {
        c.svc
            .search_accounts(&q, offset + limit)
            .into_iter()
            .skip(offset)
            .filter(|slot| !c.params.flag("following") || c.svc.state.follows(viewer, *slot))
            .filter(|slot| !c.svc.state.blocked_either(viewer, *slot))
            .filter(|slot| *slot == viewer || !c.svc.state.suspended(*slot))
            .filter_map(|slot| c.svc.state.account(slot))
            .map(|a| entities::account(c.svc, c.ctx, a))
            .collect()
    } else {
        Vec::new()
    };
    let statuses: Vec<Value> = if want("statuses") {
        c.svc
            .search_statuses(viewer, &q, offset + limit)
            .into_iter()
            .skip(offset)
            .filter_map(|id| c.svc.state.statuses.get(&id))
            .map(|s| entities::status(c.svc, c.ctx, s, Some(viewer)))
            .collect()
    } else {
        Vec::new()
    };
    let hashtags: Vec<Value> = if want("hashtags") && !q.is_empty() {
        c.svc
            .search_tags(&q, offset + limit)
            .into_iter()
            .skip(offset)
            .map(|t| entities::tag(c.ctx, &t, false))
            .collect()
    } else {
        Vec::new()
    };
    Ok(Response::ok(json!({
        "accounts": accounts,
        "statuses": statuses,
        "hashtags": hashtags,
    })))
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, seg) {
        ("GET", ["api", "v1", "timelines", "home"]) => home(c),
        ("GET", ["api", "v1", "timelines", "direct"]) => direct(c),
        ("GET", ["api", "v1", "timelines", "public"]) => public(c),
        ("GET", ["api", "v1", "timelines", "tag", name]) => tag(c, name),
        ("GET", ["api", "v1", "favourites"]) => reactions(c, ReactionKind::Favourite),
        ("GET", ["api", "v1", "bookmarks"]) => reactions(c, ReactionKind::Bookmark),
        ("GET", ["api", "v1", "notifications"]) => notifications(c),
        ("GET", ["api", "v1", "notifications", "unread_count"]) => unread_count(c),
        ("GET", ["api", "v2", "notifications", "unread_count"]) => {
            super::notification_groups::unread(c)
        }
        ("GET", ["api", "v1", "notifications", "requests"]) => {
            c.user().map(|_| Response::ok(json!([])))
        }
        ("GET", ["api", "v1", "notifications", id]) if *id != "policy" => notification(c, id),
        ("POST", ["api", "v1", "notifications", "clear"]) => clear(c),
        ("POST", ["api", "v1", "notifications", "dismiss"]) => dismiss(c, None),
        ("POST", ["api", "v1", "notifications", id, "dismiss"]) if *id != "requests" => {
            dismiss(c, Some(id))
        }
        ("GET", ["api", "v2", "notifications"]) => super::notification_groups::list(c),
        ("GET", ["api", "v2", "notifications", key]) if *key != "policy" => {
            super::notification_groups::show(c, key)
        }
        ("GET", ["api", "v2", "notifications", key, "accounts"]) => {
            super::notification_groups::accounts(c, key)
        }
        ("POST", ["api", "v2", "notifications", key, "dismiss"]) => {
            super::notification_groups::dismiss(c, key)
        }
        ("GET", ["api", "v1", "markers"]) => markers_get(c),
        ("POST", ["api", "v1", "markers"]) => markers_set(c),
        ("GET", ["api", "v2", "search"]) => search(c),
        _ => return None,
    })
}
