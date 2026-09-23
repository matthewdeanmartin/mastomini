//! `/api/v1/accounts/*`.

use super::{entities, fail, Call, Reply};
use crate::domain::query::AccountStatusesQuery;
use crate::domain::records::{Field, Visibility};
use crate::domain::{Account, Error, ProfileUpdate};
use crate::http::Response;
use crate::store::Store;
use serde_json::{json, Value};

const SEARCH_LIMIT: usize = 40;

fn target<'s, S: Store>(c: &'s Call<'_, S>, id: &str) -> Result<&'s Account, Response> {
    let id = c.id(id)?;
    c.svc
        .state
        .account_by_id(id)
        .ok_or_else(|| fail(Error::NotFound))
}

fn verify_credentials<S: Store>(c: &Call<'_, S>) -> Reply {
    let slot = c
        .user_scoped("read:accounts")
        .or_else(|_| c.user_scoped("profile"))?;
    let account = c
        .svc
        .state
        .account(slot)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(entities::credential_account(
        c.svc, c.ctx, account,
    )))
}

fn update_credentials<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let slot = c.user()?;
    if !c.params.files.is_empty() {
        return Err(Response::error(
            422,
            "Avatars and headers can only be changed in the household app for now",
        ));
    }
    let p = &c.params;
    let privacy = match p.get("source[privacy]") {
        Some(v) => Some(
            Visibility::parse(v)
                .filter(|v| *v != Visibility::Direct)
                .ok_or_else(|| Response::error(422, "Validation failed: Privacy is invalid"))?,
        ),
        None => None,
    };
    let has_fields = p.names().any(|n| n.starts_with("fields_attributes"));
    let fields = has_fields.then(|| {
        (0..8)
            .filter_map(|i| {
                let name = p.get(&format!("fields_attributes[{i}][name]"));
                let value = p.get(&format!("fields_attributes[{i}][value]"));
                (name.is_some() || value.is_some()).then(|| Field {
                    name: name.unwrap_or("").to_string(),
                    value: value.unwrap_or("").to_string(),
                })
            })
            .collect::<Vec<_>>()
    });
    let update = ProfileUpdate {
        display_name: p.get("display_name").map(str::to_string),
        note: p.get("note").map(str::to_string),
        locked: p.bool("locked"),
        bot: p.bool("bot"),
        discoverable: p.bool("discoverable"),
        fields,
        privacy,
        sensitive: p.bool("source[sensitive]"),
        language: p
            .get("source[language]")
            .map(|l| Some(l.to_string()).filter(|l| !l.is_empty())),
    };
    c.svc.update_profile(slot, update, c.now).map_err(fail)?;
    let account = c
        .svc
        .state
        .account(slot)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(entities::credential_account(
        c.svc, c.ctx, account,
    )))
}

fn show<S: Store>(c: &Call<'_, S>, id: &str) -> Reply {
    c.user()?;
    let account = target(c, id)?;
    Ok(Response::ok(entities::account(c.svc, c.ctx, account)))
}

fn lookup<S: Store>(c: &Call<'_, S>) -> Reply {
    c.user()?;
    let acct = c.params.get("acct").unwrap_or("").trim_start_matches('@');
    let (user, host) = acct.split_once('@').unwrap_or((acct, ""));
    if !host.is_empty() && !host.eq_ignore_ascii_case(c.ctx.host()) {
        return Err(fail(Error::NotFound));
    }
    let account = c
        .svc
        .state
        .account_by_username(user)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(entities::account(c.svc, c.ctx, account)))
}

fn search<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user()?;
    let limit = (c.params.u64("limit").unwrap_or(SEARCH_LIMIT as u64) as usize).min(SEARCH_LIMIT);
    let following = c.params.flag("following");
    let q = c.params.get("q").unwrap_or("");
    let rows: Vec<Value> = c
        .svc
        .search_accounts(q, limit)
        .into_iter()
        .filter(|slot| !following || c.svc.state.follows(viewer, *slot))
        .filter_map(|slot| c.svc.state.account(slot))
        .map(|a| entities::account(c.svc, c.ctx, a))
        .collect();
    Ok(Response::ok(Value::Array(rows)))
}

fn relationships<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user()?;
    let rows: Vec<Value> = c
        .params
        .all("id")
        .iter()
        .take(40)
        .filter_map(|id| crate::ids::parse(id))
        .filter_map(|id| c.svc.state.account_by_id(id))
        .map(|a| entities::relationship(c.svc, viewer, a))
        .collect();
    Ok(Response::ok(Value::Array(rows)))
}

fn statuses<S: Store>(c: &Call<'_, S>, id: &str) -> Reply {
    let viewer = c.user()?;
    let author = target(c, id)?.slot;
    let p = &c.params;
    let opts = AccountStatusesQuery {
        exclude_replies: p.flag("exclude_replies"),
        exclude_reblogs: p.flag("exclude_reblogs"),
        pinned: p.flag("pinned"),
        only_media: p.flag("only_media"),
        tagged: p.text("tagged").map(str::to_string),
    };
    let q = c.page_query();
    let page = c.svc.account_statuses(Some(viewer), author, &opts, &q);
    let ids: Vec<u64> = page.iter().map(|(id, _)| *id).collect();
    let body: Vec<Value> = page
        .into_iter()
        .map(|(_, e)| entities::entry(c.svc, c.ctx, e, Some(viewer)))
        .collect();
    Ok(c.paged(Value::Array(body), &ids, q.limit()))
}

fn follow_list<S: Store>(c: &Call<'_, S>, id: &str, followers: bool) -> Reply {
    c.user()?;
    let slot = target(c, id)?.slot;
    let q = c.page_query();
    let page = c.svc.follow_list(slot, followers, &q);
    let ids: Vec<u64> = page.iter().map(|(id, _)| *id).collect();
    let body: Vec<Value> = page
        .into_iter()
        .filter_map(|(_, s)| c.svc.state.account(s))
        .map(|a| entities::account(c.svc, c.ctx, a))
        .collect();
    Ok(c.paged(Value::Array(body), &ids, q.limit()))
}

fn set_follow<S: Store>(c: &mut Call<'_, S>, id: &str, on: bool) -> Reply {
    let viewer = c.user_scoped("follow")?;
    let dst = target(c, id)?.slot;
    let (reblogs, notify) = (c.params.bool("reblogs"), c.params.bool("notify"));
    c.svc
        .set_follow(viewer, dst, on, reblogs, notify, c.now)
        .map_err(fail)?;
    let account = c
        .svc
        .state
        .account(dst)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(entities::relationship(c.svc, viewer, account)))
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, &seg[3..]) {
        ("GET", ["verify_credentials"]) => verify_credentials(c),
        ("PATCH", ["update_credentials"]) => update_credentials(c),
        ("GET", ["lookup"]) => lookup(c),
        ("GET", ["search"]) => search(c),
        ("GET", ["relationships"]) => relationships(c),
        ("GET", ["familiar_followers"]) => c.user().map(|_| Response::ok(json!([]))),
        ("POST", []) => Err(Response::error(
            403,
            "Registrations are closed; ask your household admin",
        )),
        ("GET", [id]) => show(c, id),
        ("GET", [id, "statuses"]) => statuses(c, id),
        ("GET", [id, "followers"]) => follow_list(c, id, true),
        ("GET", [id, "following"]) => follow_list(c, id, false),
        ("GET", [_, "featured_tags" | "lists" | "identity_proofs" | "endorsements"]) => {
            c.user().map(|_| Response::ok(json!([])))
        }
        ("POST", [id, "follow"]) => set_follow(c, id, true),
        ("POST", [id, "unfollow"]) => set_follow(c, id, false),
        _ => return None,
    })
}
