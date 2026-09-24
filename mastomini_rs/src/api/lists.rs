//! `/api/v1/lists`, `/api/v1/timelines/list/:id`, `/api/v1/accounts/:id/lists`
//! and `/api/v1/follow_requests` (spec/04 "Tier 2").

use super::{entities, fail, Call, Reply};
use crate::domain::records::{ListRec, RepliesPolicy};
use crate::domain::{Error, ListUpdate};
use crate::http::Response;
use crate::store::Store;
use serde_json::{json, Value};

fn list_json(l: &ListRec) -> Value {
    json!({
        "id": l.id.to_string(),
        "title": l.title,
        "replies_policy": l.replies_policy.as_str(),
        "exclusive": l.exclusive,
    })
}

fn policy_param<S: Store>(c: &Call<'_, S>) -> Result<Option<RepliesPolicy>, Response> {
    match c.params.text("replies_policy") {
        None => Ok(None),
        Some(p) => RepliesPolicy::parse(p)
            .map(Some)
            .ok_or_else(|| Response::error(422, "Validation failed: replies_policy is invalid")),
    }
}

fn show<S: Store>(c: &Call<'_, S>, slot: u8, id: u64) -> Reply {
    let list = c.svc.list(slot, id).map_err(fail)?;
    Ok(Response::ok(list_json(list)))
}

fn index<S: Store>(c: &Call<'_, S>) -> Reply {
    let slot = c.user_scoped("read:lists")?;
    let rows: Vec<Value> = c.svc.lists_of(slot).map(list_json).collect();
    Ok(Response::ok(Value::Array(rows)))
}

fn create<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let slot = c.user_scoped("write:lists")?;
    let policy = policy_param(c)?.unwrap_or(RepliesPolicy::List);
    let title = c.params.get("title").unwrap_or("").to_string();
    let exclusive = c.params.flag("exclusive");
    let id = c
        .svc
        .create_list(slot, &title, policy, exclusive, c.now)
        .map_err(fail)?;
    show(c, slot, id)
}

fn update<S: Store>(c: &mut Call<'_, S>, id: u64) -> Reply {
    let slot = c.user_scoped("write:lists")?;
    let update = ListUpdate {
        title: c.params.get("title").map(str::to_string),
        replies_policy: policy_param(c)?,
        exclusive: c.params.bool("exclusive"),
    };
    c.svc.update_list(slot, id, update, c.now).map_err(fail)?;
    show(c, slot, id)
}

fn account_ids<S: Store>(c: &Call<'_, S>) -> Vec<u64> {
    c.params
        .all("account_ids")
        .iter()
        .filter_map(|id| crate::ids::parse(id))
        .collect()
}

fn members<S: Store>(c: &Call<'_, S>, id: u64) -> Reply {
    let slot = c.user_scoped("read:lists")?;
    let list = c.svc.list(slot, id).map_err(fail)?;
    let rows: Vec<Value> = list
        .members
        .iter()
        .filter_map(|id| c.svc.state.account_by_id(*id))
        .map(|a| entities::account(c.svc, c.ctx, a))
        .collect();
    Ok(Response::ok(Value::Array(rows)))
}

fn timeline<S: Store>(c: &Call<'_, S>, id: u64) -> Reply {
    let viewer = c.user_scoped("read:lists")?;
    let q = c.page_query();
    let page = c.svc.list_timeline(viewer, id, &q).map_err(fail)?;
    let ids: Vec<u64> = page.iter().map(|(id, _)| *id).collect();
    let rows: Vec<Value> = page
        .into_iter()
        .map(|(_, e)| entities::entry(c.svc, c.ctx, e, Some(viewer)))
        .collect();
    let rows = super::filters::annotate(c, viewer, "home", rows);
    Ok(c.paged(Value::Array(rows), &ids, q.limit()))
}

/// The viewer's lists that include an account.
fn containing<S: Store>(c: &Call<'_, S>, account: &str) -> Reply {
    let viewer = c.user_scoped("read:lists")?;
    let account_id = c.id(account)?;
    let rows: Vec<Value> = c
        .svc
        .lists_of(viewer)
        .filter(|l| l.members.contains(&account_id))
        .map(list_json)
        .collect();
    Ok(Response::ok(Value::Array(rows)))
}

fn follow_requests<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user_scoped("read:follows")?;
    let q = c.page_query();
    let page = c.svc.follow_requests_to(viewer, &q);
    let ids: Vec<u64> = page.iter().map(|(id, _)| *id).collect();
    let rows: Vec<Value> = page
        .into_iter()
        .filter_map(|(_, s)| c.svc.state.account(s))
        .map(|a| entities::account(c.svc, c.ctx, a))
        .collect();
    Ok(c.paged(Value::Array(rows), &ids, q.limit()))
}

/// `POST /api/v1/follow_requests/:account_id/authorize` or `/reject`.
fn answer<S: Store>(c: &mut Call<'_, S>, account: &str, accept: bool) -> Reply {
    let viewer = c.user_scoped("write:follows")?;
    let requester = c
        .svc
        .state
        .slot_of(c.id(account)?)
        .ok_or_else(|| fail(Error::NotFound))?;
    let result = if accept {
        c.svc.authorize_follow(viewer, requester, c.now)
    } else {
        c.svc.reject_follow(viewer, requester, c.now)
    };
    result.map_err(fail)?;
    let account = c
        .svc
        .state
        .account(requester)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(entities::relationship(c.svc, viewer, account)))
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    let id = |text: &str| crate::ids::parse(text).ok_or_else(|| fail(Error::NotFound));
    Some(match (method, seg) {
        ("GET", ["api", "v1", "lists"]) => index(c),
        ("POST", ["api", "v1", "lists"]) => create(c),
        ("GET", ["api", "v1", "lists", l]) => id(l).and_then(|l| {
            let slot = c.user_scoped("read:lists")?;
            show(c, slot, l)
        }),
        ("PUT", ["api", "v1", "lists", l]) => id(l).and_then(|l| update(c, l)),
        ("DELETE", ["api", "v1", "lists", l]) => id(l).and_then(|l| {
            let slot = c.user_scoped("write:lists")?;
            c.svc.delete_list(slot, l, c.now).map_err(fail)?;
            Ok(Response::ok(json!({})))
        }),
        ("GET", ["api", "v1", "lists", l, "accounts"]) => id(l).and_then(|l| members(c, l)),
        ("POST" | "DELETE", ["api", "v1", "lists", l, "accounts"]) => id(l).and_then(|l| {
            let slot = c.user_scoped("write:lists")?;
            let ids = account_ids(c);
            let result = if method == "POST" {
                c.svc.add_to_list(slot, l, &ids, c.now)
            } else {
                c.svc.remove_from_list(slot, l, &ids, c.now)
            };
            result.map_err(fail)?;
            Ok(Response::ok(json!({})))
        }),
        ("GET", ["api", "v1", "timelines", "list", l]) => id(l).and_then(|l| timeline(c, l)),
        ("GET", ["api", "v1", "accounts", a, "lists"]) => containing(c, a),
        ("GET", ["api", "v1", "follow_requests"]) => follow_requests(c),
        ("POST", ["api", "v1", "follow_requests", a, "authorize"]) => answer(c, a, true),
        ("POST", ["api", "v1", "follow_requests", a, "reject"]) => answer(c, a, false),
        _ => return None,
    })
}
