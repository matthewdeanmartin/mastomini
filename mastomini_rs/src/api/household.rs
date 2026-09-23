//! `/api/mastomini/v1`: what the Mastodon API cannot do (spec/06). Sprint 2
//! covers bootstrap only: status, provisioning, and an admin adding members.

use super::{entities, fail, Call, Reply};
use crate::domain::records::Role;
use crate::domain::{Error, NewMember};
use crate::http::Response;
use crate::store::Store;
use serde_json::{json, Value};

fn status<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let (writes, _) = c.svc.governor_counts();
    let stats = c.svc.store().stats().ok();
    let svc = &c.svc;
    Ok(Response::ok(json!({
        "provisioned": svc.state.provisioned,
        "title": svc.state.server.title,
        "version": env!("CARGO_PKG_VERSION"),
        "host": c.ctx.host(),
        "available": svc.latched().is_none(),
        "accounts": svc.state.active_accounts().count(),
        "statuses": svc.state.statuses.len(),
        "evictions": svc.state.evictions,
        "writes_this_hour": writes,
        "store_used": stats.map(|s| s.used_fraction()),
    })))
}

fn member_from<S: Store>(c: &Call<'_, S>, default_role: Role) -> Result<NewMember, Response> {
    let p = &c.params;
    let role = match p.get("role") {
        Some(r) => Role::parse(r)
            .ok_or_else(|| Response::error(422, "Validation failed: Role is invalid"))?,
        None => default_role,
    };
    Ok(NewMember {
        username: p.get("username").unwrap_or("").trim().to_string(),
        password: p.get("password").unwrap_or("").to_string(),
        display_name: p.get("display_name").unwrap_or("").trim().to_string(),
        role,
    })
}

fn provision<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let owner = member_from(c, Role::Owner)?;
    let title = c.params.text("title").map(str::to_string);
    let slot = c.svc.provision(owner, title, c.now).map_err(fail)?;
    let account = c
        .svc
        .state
        .account(slot)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::json(
        200,
        &entities::account(c.svc, c.ctx, account),
    ))
}

fn create_member<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let actor = c.user()?;
    let member = member_from(c, Role::Member)?;
    let slot = c.svc.create_member(actor, member, c.now).map_err(fail)?;
    let account = c
        .svc
        .state
        .account(slot)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(entities::account(c.svc, c.ctx, account)))
}

fn members<S: Store>(c: &Call<'_, S>) -> Reply {
    let actor = c.user()?;
    if !c
        .svc
        .state
        .account(actor)
        .is_some_and(|a| a.rec.role.is_admin())
    {
        return Err(fail(Error::Forbidden(
            "Only an admin can list members".into(),
        )));
    }
    let rows: Vec<Value> = c
        .svc
        .state
        .active_accounts()
        .map(|a| {
            let mut v = entities::account(c.svc, c.ctx, a);
            v["role"] = json!(a.rec.role.as_str());
            v["disabled"] = json!(a.rec.disabled);
            v
        })
        .collect();
    Ok(Response::ok(Value::Array(rows)))
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, seg) {
        ("GET", ["status"]) => status(c),
        ("POST", ["provision"]) => provision(c),
        ("GET", ["admin", "members"]) => members(c),
        ("POST", ["admin", "members"]) => create_member(c),
        _ => return None,
    })
}
