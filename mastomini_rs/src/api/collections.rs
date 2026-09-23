//! `/api/v1/collections` and the account collection listings (Mastodon
//! 4.6). Response shapes follow docs.joinmastodon.org/methods/collections.

use super::{entities, fail, Call, Reply};
use crate::domain::{CollectionUpdate, Error, NewCollection};
use crate::http::Response;
use crate::store::Store;
use serde_json::{json, Value};

const DEFAULT_LIMIT: usize = 40;
const MAX_LIMIT: usize = 80;

fn read<S: Store>(c: &Call<'_, S>) -> Result<u8, Response> {
    c.user_scoped("read:collections")
}

fn write<S: Store>(c: &Call<'_, S>) -> Result<u8, Response> {
    c.user_scoped("write:collections")
}

fn wrapped<S: Store>(c: &Call<'_, S>, id: u64, viewer: u8) -> Reply {
    let rec = c
        .svc
        .state
        .collections
        .get(&id)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(json!({
        "collection": entities::collection(c.svc, c.ctx, rec, viewer),
    })))
}

fn language<S: Store>(c: &Call<'_, S>) -> Option<Option<String>> {
    c.params
        .get("language")
        .map(|l| Some(l.to_string()).filter(|l| !l.is_empty()))
}

fn create<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let owner = write(c)?;
    let p = &c.params;
    let account_ids = p
        .all("account_ids")
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| {
            crate::ids::parse(s)
                .ok_or_else(|| Response::error(422, "Validation failed: Account is invalid"))
        })
        .collect::<Result<Vec<u64>, Response>>()?;
    let new = NewCollection {
        name: p.get("name").unwrap_or("").to_string(),
        description: p.get("description").unwrap_or("").to_string(),
        discoverable: p.bool("discoverable"),
        sensitive: p.flag("sensitive"),
        language: language(c).flatten(),
        tag: p.get("tag_name").map(str::to_string),
        account_ids,
    };
    let id = c.svc.create_collection(owner, new, c.now).map_err(fail)?;
    wrapped(c, id, owner)
}

fn show<S: Store>(c: &Call<'_, S>, id: &str) -> Reply {
    let viewer = read(c)?;
    let id = c.id(id)?;
    let rec = c
        .svc
        .state
        .collections
        .get(&id)
        .filter(|rec| c.svc.collection_visible(viewer, rec))
        .ok_or_else(|| fail(Error::NotFound))?;
    // The curator first, then every featured account the viewer may see.
    let mut slots = vec![rec.owner];
    slots.extend(
        c.svc
            .visible_items(viewer, rec)
            .filter(|i| !i.revoked)
            .filter_map(|i| c.svc.state.slot_of(i.account_id)),
    );
    let accounts: Vec<Value> = slots
        .into_iter()
        .filter_map(|s| c.svc.state.account(s))
        .map(|a| entities::account(c.svc, c.ctx, a))
        .collect();
    Ok(Response::ok(json!({
        "collection": entities::collection(c.svc, c.ctx, rec, viewer),
        "accounts": accounts,
    })))
}

fn update<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let actor = write(c)?;
    let id = c.id(id)?;
    let p = &c.params;
    let update = CollectionUpdate {
        name: p.get("name").map(str::to_string),
        description: p.get("description").map(str::to_string),
        discoverable: p.bool("discoverable"),
        sensitive: p.bool("sensitive"),
        language: language(c),
        tag: p.get("tag_name").map(|t| Some(t.to_string())),
    };
    c.svc
        .update_collection(actor, id, update, c.now)
        .map_err(fail)?;
    wrapped(c, id, actor)
}

fn delete<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let actor = write(c)?;
    let id = c.id(id)?;
    c.svc.delete_collection(actor, id, c.now).map_err(fail)?;
    Ok(Response::ok(json!({})))
}

fn add_item<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let actor = write(c)?;
    let id = c.id(id)?;
    let account_id = c
        .params
        .text("account_id")
        .ok_or_else(|| Response::error(422, "Validation failed: Account is missing"))?;
    let account_id = crate::ids::parse(account_id)
        .ok_or_else(|| Response::error(422, "Validation failed: Account is invalid"))?;
    let item_id = c
        .svc
        .add_collection_item(actor, id, account_id, c.now)
        .map_err(fail)?;
    let item = c
        .svc
        .state
        .collections
        .get(&id)
        .and_then(|rec| rec.items.iter().find(|i| i.id == item_id))
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(json!({
        "collection_item": entities::collection_item(item),
    })))
}

fn remove_item<S: Store>(c: &mut Call<'_, S>, id: &str, item: &str, revoke: bool) -> Reply {
    let actor = write(c)?;
    let (id, item) = (c.id(id)?, c.id(item)?);
    let result = if revoke {
        c.svc.revoke_collection_item(actor, id, item, c.now)
    } else {
        c.svc.remove_collection_item(actor, id, item, c.now)
    };
    result.map_err(fail)?;
    Ok(Response::ok(json!({})))
}

/// `{"collections": [...]}` with offset/limit paging.
fn listing<S: Store>(c: &Call<'_, S>, account: &str, featuring: bool) -> Reply {
    let viewer = read(c)?;
    let id = c.id(account)?;
    let slot = c
        .svc
        .state
        .slot_of(id)
        .ok_or_else(|| fail(Error::NotFound))?;
    let ids = if featuring {
        c.svc.collections_featuring(viewer, slot)
    } else {
        c.svc.collections_of(viewer, slot)
    };
    let limit = match c.params.u64("limit").unwrap_or(0) as usize {
        0 => DEFAULT_LIMIT,
        n => n.min(MAX_LIMIT),
    };
    let offset = c.params.u64("offset").unwrap_or(0) as usize;
    let rows: Vec<Value> = ids
        .into_iter()
        .skip(offset)
        .take(limit)
        .filter_map(|id| c.svc.state.collections.get(&id))
        .map(|rec| entities::collection(c.svc, c.ctx, rec, viewer))
        .collect();
    Ok(Response::ok(json!({ "collections": rows })))
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, seg) {
        ("POST", ["api", "v1", "collections"]) => create(c),
        ("GET", ["api", "v1", "collections", id]) => show(c, id),
        ("PATCH" | "PUT", ["api", "v1", "collections", id]) => update(c, id),
        ("DELETE", ["api", "v1", "collections", id]) => delete(c, id),
        ("POST", ["api", "v1", "collections", id, "items"]) => add_item(c, id),
        ("DELETE", ["api", "v1", "collections", id, "items", item]) => {
            remove_item(c, id, item, false)
        }
        ("POST", ["api", "v1", "collections", id, "items", item, "revoke"]) => {
            remove_item(c, id, item, true)
        }
        ("GET", ["api", "v1", "accounts", account, "collections"]) => listing(c, account, false),
        ("GET", ["api", "v1", "accounts", account, "in_collections"]) => listing(c, account, true),
        _ => return None,
    })
}
