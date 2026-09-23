//! `/api/v1/statuses/*`.

use super::{entities, fail, Call, Reply};
use crate::domain::records::Visibility;
use crate::domain::{Error, NewStatus, ReactionKind};
use crate::http::Response;
use crate::ids;
use crate::store::Store;
use serde_json::{json, Value};

/// A path id may name a status or a boost of one; actions apply to the
/// original, as in Mastodon.
fn original<S: Store>(c: &Call<'_, S>, id: &str) -> Result<u64, Response> {
    let id = c.id(id)?;
    Ok(c.svc.state.boosts.get(&id).map_or(id, |b| b.target))
}

fn render<S: Store>(c: &Call<'_, S>, id: u64, viewer: u8) -> Reply {
    let s = c
        .svc
        .visible(Some(viewer), id)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(entities::status(
        c.svc,
        c.ctx,
        s,
        Some(viewer),
    )))
}

fn create<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let slot = c.user_scoped("write:statuses")?;
    let p = &c.params;
    if p.all("media_ids").iter().any(|m| !m.is_empty()) {
        return Err(Response::error(
            422,
            "Media attachments are not supported on this server",
        ));
    }
    if p.names().any(|n| n.starts_with("poll")) {
        return Err(Response::error(
            422,
            "Polls are not supported on this server yet",
        ));
    }
    if p.text("scheduled_at").is_some() {
        return Err(Response::error(
            422,
            "Scheduled posts are not supported on this server",
        ));
    }
    let visibility = match p.text("visibility") {
        Some(v) => Some(
            Visibility::parse(v)
                .ok_or_else(|| Response::error(422, "Validation failed: Visibility is invalid"))?,
        ),
        None => None,
    };
    let in_reply_to_id = match p.text("in_reply_to_id") {
        Some(id) => Some(original(c, id)?),
        None => None,
    };
    let new = NewStatus {
        text: p.get("status").unwrap_or("").to_string(),
        spoiler_text: p.get("spoiler_text").unwrap_or("").to_string(),
        visibility,
        sensitive: p.flag("sensitive"),
        language: p.text("language").map(str::to_string),
        in_reply_to_id,
        idempotency_key: c.req.header("Idempotency-Key").map(str::to_string),
        app_id: c.principal.as_ref().map(|p| p.app_id),
    };
    let id = c.svc.post_status(slot, new, c.now).map_err(fail)?;
    render(c, id, slot)
}

fn show<S: Store>(c: &Call<'_, S>, id: &str) -> Reply {
    let viewer = c.user()?;
    let id = c.id(id)?;
    if let Some(boost) = c.svc.state.boosts.get(&id) {
        if c.svc.visible(Some(viewer), boost.target).is_some() {
            return Ok(Response::ok(entities::boost(
                c.svc,
                c.ctx,
                boost,
                Some(viewer),
            )));
        }
    }
    render(c, id, viewer)
}

fn show_many<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user()?;
    let rows: Vec<Value> = c
        .params
        .all("id")
        .iter()
        .take(40)
        .filter_map(|id| ids::parse(id))
        .filter_map(|id| c.svc.visible(Some(viewer), id))
        .map(|s| entities::status(c.svc, c.ctx, s, Some(viewer)))
        .collect();
    Ok(Response::ok(Value::Array(rows)))
}

fn delete<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let slot = c.user_scoped("write:statuses")?;
    let id = c.id(id)?;
    let status = c
        .svc
        .visible(Some(slot), id)
        .ok_or_else(|| fail(Error::NotFound))?;
    // Render before deleting so "delete & redraft" gets the full entity.
    let mut body = entities::status(c.svc, c.ctx, status, Some(slot));
    let text = status.rec.text.clone();
    c.svc.delete_status(slot, id, c.now).map_err(fail)?;
    body["text"] = json!(text);
    Ok(Response::ok(body))
}

fn context<S: Store>(c: &Call<'_, S>, id: &str) -> Reply {
    let viewer = c.user()?;
    let id = original(c, id)?;
    let (ancestors, descendants) = c
        .svc
        .context(Some(viewer), id)
        .ok_or_else(|| fail(Error::NotFound))?;
    let render = |ids: Vec<u64>| -> Vec<Value> {
        ids.into_iter()
            .filter_map(|id| c.svc.state.statuses.get(&id))
            .map(|s| entities::status(c.svc, c.ctx, s, Some(viewer)))
            .collect()
    };
    Ok(Response::ok(json!({
        "ancestors": render(ancestors),
        "descendants": render(descendants),
    })))
}

fn react<S: Store>(c: &mut Call<'_, S>, id: &str, kind: ReactionKind, on: bool) -> Reply {
    let scope = match kind {
        ReactionKind::Favourite => "write:favourites",
        ReactionKind::Bookmark => "write:bookmarks",
        ReactionKind::Pin => "write:accounts",
    };
    let slot = c.user_scoped(scope)?;
    let id = original(c, id)?;
    c.svc
        .set_reaction(slot, kind, id, on, c.now)
        .map_err(fail)?;
    render(c, id, slot)
}

fn reblog<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let slot = c.user_scoped("write:statuses")?;
    let id = original(c, id)?;
    let boost_id = c.svc.boost(slot, id, c.now).map_err(fail)?;
    let boost = c
        .svc
        .state
        .boosts
        .get(&boost_id)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(entities::boost(
        c.svc,
        c.ctx,
        boost,
        Some(slot),
    )))
}

fn unreblog<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let slot = c.user_scoped("write:statuses")?;
    let id = original(c, id)?;
    c.svc.unboost(slot, id, c.now).map_err(fail)?;
    render(c, id, slot)
}

fn who<S: Store>(c: &Call<'_, S>, id: &str, favourites: bool) -> Reply {
    let viewer = c.user()?;
    let id = original(c, id)?;
    let s = c
        .svc
        .visible(Some(viewer), id)
        .ok_or_else(|| fail(Error::NotFound))?;
    let mask = if favourites { s.favourited } else { s.boosted };
    let rows: Vec<Value> = c
        .svc
        .reactors(mask)
        .into_iter()
        .filter_map(|slot| c.svc.state.account(slot))
        .map(|a| entities::account(c.svc, c.ctx, a))
        .collect();
    Ok(Response::ok(Value::Array(rows)))
}

fn source<S: Store>(c: &Call<'_, S>, id: &str) -> Reply {
    let viewer = c.user()?;
    let id = c.id(id)?;
    let s = c
        .svc
        .visible(Some(viewer), id)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(json!({
        "id": id.to_string(),
        "text": s.rec.text,
        "spoiler_text": s.rec.spoiler_text,
    })))
}

/// Edits arrive with the Tier 2 API; until then the history is the one
/// current version.
fn history<S: Store>(c: &Call<'_, S>, id: &str) -> Reply {
    let viewer = c.user()?;
    let id = c.id(id)?;
    let s = c
        .svc
        .visible(Some(viewer), id)
        .ok_or_else(|| fail(Error::NotFound))?;
    let full = entities::status(c.svc, c.ctx, s, Some(viewer));
    Ok(Response::ok(json!([{
        "content": full["content"],
        "spoiler_text": full["spoiler_text"],
        "sensitive": full["sensitive"],
        "created_at": full["created_at"],
        "account": full["account"],
        "media_attachments": [],
        "emojis": [],
        "poll": null,
    }])))
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, &seg[3..]) {
        ("POST", []) => create(c),
        ("GET", []) => show_many(c),
        ("GET", [id]) => show(c, id),
        ("DELETE", [id]) => delete(c, id),
        ("PUT", [_]) => Err(Response::error(422, "Editing posts is not supported yet")),
        ("GET", [id, "context"]) => context(c, id),
        ("GET", [id, "source"]) => source(c, id),
        ("GET", [id, "history"]) => history(c, id),
        ("GET", [id, "favourited_by"]) => who(c, id, true),
        ("GET", [id, "reblogged_by"]) => who(c, id, false),
        ("GET", [_, "card"]) => Ok(Response::ok(Value::Null)),
        ("POST", [id, "favourite"]) => react(c, id, ReactionKind::Favourite, true),
        ("POST", [id, "unfavourite"]) => react(c, id, ReactionKind::Favourite, false),
        ("POST", [id, "bookmark"]) => react(c, id, ReactionKind::Bookmark, true),
        ("POST", [id, "unbookmark"]) => react(c, id, ReactionKind::Bookmark, false),
        ("POST", [id, "pin"]) => react(c, id, ReactionKind::Pin, true),
        ("POST", [id, "unpin"]) => react(c, id, ReactionKind::Pin, false),
        ("POST", [id, "reblog"]) => reblog(c, id),
        ("POST", [id, "unreblog"]) => unreblog(c, id),
        ("POST", [id, "mute" | "unmute"]) => {
            let viewer = match c.user() {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            match original(c, id) {
                Ok(id) => render(c, id, viewer),
                Err(e) => Err(e),
            }
        }
        _ => return None,
    })
}
