//! `/api/v1/conversations`: direct-message threads (spec/04 "Tier 2").

use super::{entities, fail, Call, Reply};
use crate::domain::query::{paginate, Scan};
use crate::domain::{bit, Conversation, Error, MAX_ACCOUNTS};
use crate::http::Response;
use crate::store::Store;
use serde_json::{json, Value};

fn conversation_json<S: Store>(c: &Call<'_, S>, viewer: u8, conv: &Conversation) -> Value {
    let accounts: Vec<Value> = (0..MAX_ACCOUNTS as u8)
        .filter(|s| *s != viewer && conv.participants & bit(*s) != 0)
        .filter_map(|s| c.svc.state.account(s))
        .map(|a| entities::account(c.svc, c.ctx, a))
        .collect();
    let last = c
        .svc
        .state
        .statuses
        .get(&conv.last)
        .map_or(Value::Null, |s| {
            entities::status(c.svc, c.ctx, s, Some(viewer))
        });
    json!({
        "id": conv.root.to_string(),
        "unread": conv.unread,
        "accounts": accounts,
        "last_status": last,
    })
}

/// Newest first, paged by the last message's id, as Mastodon does.
fn list<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user_scoped("read:statuses")?;
    let q = c.page_query();
    let all = c.svc.conversations(viewer);
    let page = paginate(&q, |bounds, asc| {
        let rows: Vec<(u64, Conversation)> = all
            .iter()
            .filter(|conv| bounds.contains(conv.last))
            .map(|conv| (conv.last, *conv))
            .collect();
        let it: Scan<'_, Conversation> = if asc {
            Box::new(rows.into_iter().rev())
        } else {
            Box::new(rows.into_iter())
        };
        it
    });
    let ids: Vec<u64> = page.iter().map(|(id, _)| *id).collect();
    let rows: Vec<Value> = page
        .iter()
        .map(|(_, conv)| conversation_json(c, viewer, conv))
        .collect();
    Ok(c.paged(Value::Array(rows), &ids, q.limit()))
}

fn root<S: Store>(c: &Call<'_, S>, id: &str) -> Result<u64, Response> {
    c.id(id)
}

fn read<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let viewer = c.user_scoped("write:conversations")?;
    let root = root(c, id)?;
    c.svc.mark_conversation(viewer, root, false).map_err(fail)?;
    let conv = c
        .svc
        .conversation(viewer, root)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(conversation_json(c, viewer, &conv)))
}

fn hide<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let viewer = c.user_scoped("write:conversations")?;
    let root = root(c, id)?;
    c.svc.mark_conversation(viewer, root, true).map_err(fail)?;
    Ok(Response::ok(json!({})))
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, seg) {
        ("GET", ["api", "v1", "conversations"]) => list(c),
        ("POST", ["api", "v1", "conversations", id, "read"]) => read(c, id),
        ("DELETE", ["api", "v1", "conversations", id]) => hide(c, id),
        _ => return None,
    })
}
