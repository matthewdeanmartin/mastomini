//! `/api/mastomini/v1`: what the Mastodon API cannot do (spec/06): status,
//! provisioning, admins managing members and deleting posts, invite and
//! reset codes, and members managing their password and devices.

use super::{entities, fail, time, Call, Reply};
use crate::domain::records::{CodeKind, CodeRec, Role};
use crate::domain::{AdminAction, Error, NewMember, Redemption, ServerUpdate};
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
        "clock": c.clock.as_str(),
        "mode": super::diag::transport_mode(c.ctx),
        "https": c.ctx.tls.is_some(),
        "secure": c.req.secure,
        "https_url": c.ctx.tls.as_ref().map(|t| &t.https_url),
        "ca_fingerprint": c.ctx.tls.as_ref().map(|t| &t.ca_fingerprint),
        "household_app": crate::web::bundled(),
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
        .map(|a| member_json(c, a))
        .collect();
    Ok(Response::ok(Value::Array(rows)))
}

fn member_json<S: Store>(c: &Call<'_, S>, a: &crate::domain::Account) -> Value {
    let m = c.svc.state.moderation(a.slot);
    let mut v = entities::account(c.svc, c.ctx, a);
    v["role"] = json!(a.rec.role.as_str());
    v["disabled"] = json!(a.rec.disabled);
    v["silenced"] = json!(m.silenced);
    v["suspended"] = json!(m.suspended_ms.is_some());
    v
}

fn member_slot<S: Store>(c: &Call<'_, S>, id: &str) -> Result<u8, Response> {
    let id = c.id(id)?;
    c.svc.state.slot_of(id).ok_or_else(|| fail(Error::NotFound))
}

fn member_reply<S: Store>(c: &Call<'_, S>, slot: u8) -> Reply {
    let account = c
        .svc
        .state
        .account(slot)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(member_json(c, account)))
}

/// `POST /admin/members/:id/{disable,enable,silence,unsilence,suspend,unsuspend}`.
/// Disabled and suspended members can't sign in; enabling brings their
/// devices back without signing in again.
fn member_action<S: Store>(c: &mut Call<'_, S>, id: &str, action: AdminAction) -> Reply {
    let actor = c.user()?;
    let slot = member_slot(c, id)?;
    c.svc
        .moderate(actor, slot, action, None, c.now)
        .map_err(fail)?;
    member_reply(c, slot)
}

fn member_role<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let actor = c.user()?;
    let slot = member_slot(c, id)?;
    let role = c
        .params
        .text("role")
        .and_then(Role::parse)
        .ok_or_else(|| Response::error(422, "Validation failed: Role is invalid"))?;
    c.svc.set_role(actor, slot, role, c.now).map_err(fail)?;
    member_reply(c, slot)
}

/// `DELETE /admin/members/:id` with `{confirm: "<username>"}`.
fn member_delete<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let actor = c.user()?;
    let slot = member_slot(c, id)?;
    let username = c
        .svc
        .state
        .account(slot)
        .map(|a| a.rec.username.clone())
        .unwrap_or_default();
    if c.params.get("confirm") != Some(username.as_str()) {
        return Err(Response::error(
            422,
            "Validation failed: Type the member's username to confirm",
        ));
    }
    c.svc.delete_account(actor, slot, c.now).map_err(fail)?;
    Ok(Response::ok(json!({})))
}

fn server_json<S: Store>(c: &Call<'_, S>) -> Value {
    let server = &c.svc.state.server;
    let (terms, effective_ms) = c.svc.terms_of_service();
    json!({
        "title": server.title,
        "description": server.description,
        "rules": server.rules,
        "terms": terms,
        "terms_customized": c.svc.state.terms.is_some(),
        "terms_effective_date": super::time::date(effective_ms),
    })
}

fn get_server<S: Store>(c: &Call<'_, S>) -> Reply {
    let actor = c.user()?;
    c.svc.require_admin(actor).map_err(fail)?;
    Ok(Response::ok(server_json(c)))
}

/// `PUT /admin/server`: any of `title`, `description`, `rules[]`, `terms`
/// (`""` restores the generated terms).
fn put_server<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let actor = c.user()?;
    let p = &c.params;
    let has_rules = p.names().any(|n| n == "rules" || n.starts_with("rules["));
    let update = ServerUpdate {
        title: p.get("title").map(str::to_string),
        description: p.get("description").map(str::to_string),
        rules: has_rules.then(|| p.all("rules").iter().map(|r| r.to_string()).collect()),
        terms: p.get("terms").map(str::to_string),
    };
    c.svc.update_server(actor, update, c.now).map_err(fail)?;
    Ok(Response::ok(server_json(c)))
}

fn delete_status<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let actor = c.user()?;
    let id = c.id(id)?;
    c.svc.admin_delete_status(actor, id, c.now).map_err(fail)?;
    Ok(Response::ok(json!({})))
}

fn code_json<S: Store>(c: &Call<'_, S>, rec: &CodeRec, code: Option<&str>) -> Value {
    let mut v = json!({
        "id": rec.id.to_string(),
        "kind": "invite",
        "created_at": time::iso(crate::ids::millis(rec.id)),
        "expires_at": time::iso(rec.expires_ms),
    });
    if let CodeKind::Reset { account_id, slot } = rec.kind {
        v["kind"] = json!("reset");
        v["account_id"] = json!(account_id.to_string());
        v["username"] = json!(c.svc.state.account(slot).map(|a| a.rec.username.as_str()));
    }
    if let Some(code) = code {
        v["code"] = json!(code);
        v["url"] = json!(format!("{}/setup/{code}", c.ctx.base_url));
    }
    v
}

/// `POST /admin/invites` -> a one-time code and the URL to open.
fn create_invite<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let actor = c.user()?;
    let (rec, code) = c.svc.issue_invite(actor, c.now).map_err(fail)?;
    Ok(Response::ok(code_json(c, &rec, Some(&code))))
}

/// `POST /admin/members/:id/reset` -> a one-time reset code. The password
/// doesn't change until the member redeems it.
fn create_reset<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let actor = c.user()?;
    let slot = member_slot(c, id)?;
    let (rec, code) = c.svc.issue_reset(actor, slot, c.now).map_err(fail)?;
    Ok(Response::ok(code_json(c, &rec, Some(&code))))
}

/// `GET /admin/codes`: live invite and reset codes (never the codes
/// themselves, which are only stored hashed).
fn list_codes<S: Store>(c: &Call<'_, S>) -> Reply {
    let actor = c.user()?;
    c.svc.require_admin(actor).map_err(fail)?;
    let rows: Vec<Value> = c
        .svc
        .live_codes(c.now)
        .map(|rec| code_json(c, rec, None))
        .collect();
    Ok(Response::ok(Value::Array(rows)))
}

fn revoke_code<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let actor = c.user()?;
    let id = c.id(id)?;
    c.svc.revoke_code(actor, id, c.now).map_err(fail)?;
    Ok(Response::ok(json!({})))
}

/// `POST /codes/redeem` with `{code, password, username?, display_name?}`.
/// No sign-in: the code is the credential.
fn redeem<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let p = &c.params;
    let code = p.get("code").unwrap_or("").trim().to_string();
    let redemption = Redemption {
        password: p.get("password").unwrap_or("").to_string(),
        username: p.get("username").unwrap_or("").to_string(),
        display_name: p.get("display_name").unwrap_or("").to_string(),
    };
    let slot = c.svc.redeem_code(&code, redemption, c.now).map_err(fail)?;
    member_reply(c, slot)
}

/// `POST /me/password` with `{current, new}`. Every device, this one
/// included, is signed out and signs in again with the new password.
fn change_password<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let slot = c.user()?;
    let current = c.params.get("current").unwrap_or("").to_string();
    let new = c.params.get("new").unwrap_or("").to_string();
    c.svc
        .change_password(slot, &current, &new, c.now)
        .map_err(fail)?;
    Ok(Response::ok(json!({})))
}

fn sign_out_everywhere<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let slot = c.user()?;
    c.svc.sign_out_everywhere(slot, c.now).map_err(fail)?;
    Ok(Response::ok(json!({})))
}

/// `GET /me/devices`: the member's tokens as devices, newest first.
/// `last_used_at` is since the last restart (RAM only).
fn devices<S: Store>(c: &Call<'_, S>) -> Reply {
    let slot = c.user()?;
    let mine = c.req.bearer().map(crate::auth::sha256);
    let rows: Vec<Value> = c
        .svc
        .devices(slot)
        .into_iter()
        .map(|t| {
            let app = c.svc.state.apps.get(&t.rec.app_id);
            json!({
                "id": t.rec.id.to_string(),
                "app": {
                    "name": app.map(|a| a.name.as_str()),
                    "website": app.and_then(|a| a.website.as_deref()),
                },
                "scopes": t.rec.scopes,
                "created_at": time::iso(crate::ids::millis(t.rec.id)),
                "last_used_at": (t.last_used_ms > 0).then(|| time::iso(t.last_used_ms)),
                "current": mine == Some(t.rec.hash),
            })
        })
        .collect();
    Ok(Response::ok(Value::Array(rows)))
}

fn revoke_device<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let slot = c.user()?;
    let id = c.id(id)?;
    c.svc.revoke_device(slot, id, c.now).map_err(fail)?;
    Ok(Response::ok(json!({})))
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, seg) {
        ("GET", ["status"]) => status(c),
        ("POST", ["provision"]) => provision(c),
        ("POST", ["codes", "redeem"]) => redeem(c),
        ("POST", ["me", "password"]) => change_password(c),
        ("POST", ["me", "sign_out_everywhere"]) => sign_out_everywhere(c),
        ("GET", ["me", "devices"]) => devices(c),
        ("DELETE", ["me", "devices", id]) => revoke_device(c, id),
        ("POST", ["admin", "invites"]) => create_invite(c),
        ("GET", ["admin", "codes"]) => list_codes(c),
        ("DELETE", ["admin", "codes", id]) => revoke_code(c, id),
        ("GET", ["admin", "members"]) => members(c),
        ("POST", ["admin", "members"]) => create_member(c),
        ("POST", ["admin", "members", id, action]) => {
            let action = match *action {
                "role" => return Some(member_role(c, id)),
                "reset" => return Some(create_reset(c, id)),
                "disable" => AdminAction::Disable,
                "enable" => AdminAction::Enable,
                "silence" => AdminAction::Silence,
                "unsilence" => AdminAction::Unsilence,
                "suspend" => AdminAction::Suspend,
                "unsuspend" => AdminAction::Unsuspend,
                _ => return None,
            };
            member_action(c, id, action)
        }
        ("DELETE", ["admin", "members", id]) => member_delete(c, id),
        ("DELETE", ["admin", "statuses", id]) => delete_status(c, id),
        ("GET", ["admin", "server"]) => get_server(c),
        ("PUT" | "PATCH", ["admin", "server"]) => put_server(c),
        _ => return None,
    })
}
