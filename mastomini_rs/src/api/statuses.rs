//! `/api/v1/statuses/*`.

use super::{entities, fail, Call, Reply};
use crate::domain::records::Visibility;
use crate::domain::{Error, NewPoll, NewStatus, ReactionKind, StatusEdit};
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

/// `poll[options][]`, `poll[expires_in]`, `poll[multiple]`,
/// `poll[hide_totals]` (form or JSON).
fn poll_param<S: Store>(c: &Call<'_, S>) -> Result<Option<NewPoll>, Response> {
    let p = &c.params;
    let options: Vec<String> = p
        .all("poll[options]")
        .iter()
        .map(|o| o.to_string())
        .collect();
    if options.is_empty() {
        return Ok(None);
    }
    let expires_in_s = p
        .u64("poll[expires_in]")
        .ok_or_else(|| Response::error(422, "Validation failed: poll[expires_in] is required"))?;
    Ok(Some(NewPoll {
        options,
        expires_in_s,
        multiple: p.flag("poll[multiple]"),
        hide_totals: p.flag("poll[hide_totals]"),
    }))
}

fn no_media<S: Store>(c: &Call<'_, S>) -> Result<(), Response> {
    if c.params.all("media_ids").iter().any(|m| !m.is_empty()) {
        return Err(Response::error(
            422,
            "Media attachments are not supported on this server",
        ));
    }
    Ok(())
}

fn create<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let slot = c.user_scoped("write:statuses")?;
    no_media(c)?;
    let poll = poll_param(c)?;
    let p = &c.params;
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
        poll,
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
    let (text, _) = c.svc.readable_text(Some(slot), status);
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
        let rows = ids
            .into_iter()
            .filter_map(|id| c.svc.state.statuses.get(&id))
            .map(|s| entities::status(c.svc, c.ctx, s, Some(viewer)))
            .collect();
        super::filters::annotate(c, viewer, "thread", rows)
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
        ReactionKind::Mute => "write:mutes",
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
    let (text, spoiler_text) = c.svc.readable_text(Some(viewer), s);
    Ok(Response::ok(json!({
        "id": id.to_string(),
        "text": text,
        "spoiler_text": spoiler_text,
    })))
}

/// `PUT /api/v1/statuses/:id`: the author edits text, content warning,
/// sensitivity, language and poll.
fn update<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let slot = c.user_scoped("write:statuses")?;
    let id = c.id(id)?;
    no_media(c)?;
    let poll = poll_param(c)?;
    let p = &c.params;
    let edit = StatusEdit {
        text: p.get("status").unwrap_or("").to_string(),
        spoiler_text: p.get("spoiler_text").unwrap_or("").to_string(),
        sensitive: p.flag("sensitive"),
        language: p.text("language").map(str::to_string),
        poll,
    };
    c.svc.edit_status(slot, id, edit, c.now).map_err(fail)?;
    render(c, id, slot)
}

/// Every version, oldest first, ending with the current one. Direct
/// messages keep no history, so theirs is just the current version.
fn history<S: Store>(c: &Call<'_, S>, id: &str) -> Reply {
    let viewer = c.user()?;
    let id = c.id(id)?;
    let s = c
        .svc
        .visible(Some(viewer), id)
        .ok_or_else(|| fail(Error::NotFound))?;
    let full = entities::status(c.svc, c.ctx, s, Some(viewer));
    let mut rows: Vec<Value> = c
        .svc
        .revisions(id)
        .into_iter()
        .map(|r| {
            json!({
                "content": entities::render_text(c.svc, c.ctx, &r.text),
                "spoiler_text": r.spoiler_text,
                "sensitive": r.sensitive || !r.spoiler_text.is_empty(),
                "created_at": crate::api::time::iso(r.created_ms),
                "account": full["account"],
                "media_attachments": [],
                "emojis": [],
                "poll": null,
            })
        })
        .collect();
    rows.push(json!({
        "content": full["content"],
        "spoiler_text": full["spoiler_text"],
        "sensitive": full["sensitive"],
        "created_at": full["edited_at"].as_str().map_or(full["created_at"].clone(), |e| json!(e)),
        "account": full["account"],
        "media_attachments": [],
        "emojis": [],
        "poll": full["poll"],
    }));
    Ok(Response::ok(Value::Array(rows)))
}

/// Conversation mute: no more notifications from this thread.
fn mute_conversation<S: Store>(c: &mut Call<'_, S>, id: &str, on: bool) -> Reply {
    let slot = c.user_scoped("write:mutes")?;
    let id = original(c, id)?;
    c.svc
        .set_conversation_mute(slot, id, on, c.now)
        .map_err(fail)?;
    render(c, id, slot)
}

fn poll_reply<S: Store>(c: &Call<'_, S>, id: u64, viewer: u8) -> Reply {
    c.svc
        .visible(Some(viewer), id)
        .ok_or_else(|| fail(Error::NotFound))?;
    let poll = c
        .svc
        .state
        .polls
        .get(&id)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(entities::poll(c.svc, id, poll, Some(viewer))))
}

/// `/api/v1/polls/:id` and `/votes` (`choices[]`). A poll's id is its
/// status's id.
pub(crate) fn polls<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, seg) {
        ("GET", ["api", "v1", "polls", id]) => (|| {
            let viewer = c.user_scoped("read:statuses")?;
            poll_reply(c, c.id(id)?, viewer)
        })(),
        ("POST", ["api", "v1", "polls", id, "votes"]) => (|| {
            let viewer = c.user_scoped("write:statuses")?;
            let id = c.id(id)?;
            let choices: Vec<usize> = c
                .params
                .all("choices")
                .iter()
                .map(|v| v.parse::<usize>())
                .collect::<Result<_, _>>()
                .map_err(|_| Response::error(422, "Validation failed: choices must be numbers"))?;
            c.svc.vote(viewer, id, &choices, c.now).map_err(fail)?;
            poll_reply(c, id, viewer)
        })(),
        _ => return None,
    })
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, &seg[3..]) {
        ("POST", []) => create(c),
        ("GET", []) => show_many(c),
        ("GET", [id]) => show(c, id),
        ("DELETE", [id]) => delete(c, id),
        ("PUT", [id]) => update(c, id),
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
        ("POST", [id, "mute"]) => mute_conversation(c, id, true),
        ("POST", [id, "unmute"]) => mute_conversation(c, id, false),
        _ => return None,
    })
}
