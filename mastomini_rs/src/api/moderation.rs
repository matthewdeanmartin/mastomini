//! Blocks, mutes, reports, and the Mastodon admin API for accounts and
//! reports (`/api/v1/admin/*`), for household admins.

use super::{entities, fail, Call, Reply};
use crate::domain::query::{paginate, PageQuery, Scan};
use crate::domain::records::ReportCategory;
use crate::domain::{Account, AdminAction, Error, NewReport};
use crate::http::Response;
use crate::store::Store;
use serde_json::{json, Value};

fn target_slot<S: Store>(c: &Call<'_, S>, id: &str) -> Result<u8, Response> {
    let id = c.id(id)?;
    c.svc.state.slot_of(id).ok_or_else(|| fail(Error::NotFound))
}

fn relationship<S: Store>(c: &Call<'_, S>, viewer: u8, slot: u8) -> Reply {
    let account = c
        .svc
        .state
        .account(slot)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(entities::relationship(c.svc, viewer, account)))
}

pub(crate) fn block<S: Store>(c: &mut Call<'_, S>, id: &str, on: bool) -> Reply {
    let viewer = c
        .user_scoped("write:blocks")
        .or_else(|_| c.user_scoped("follow"))?;
    let slot = target_slot(c, id)?;
    c.svc.set_block(viewer, slot, on, c.now).map_err(fail)?;
    relationship(c, viewer, slot)
}

pub(crate) fn mute<S: Store>(c: &mut Call<'_, S>, id: &str, on: bool) -> Reply {
    let viewer = c
        .user_scoped("write:mutes")
        .or_else(|_| c.user_scoped("follow"))?;
    let slot = target_slot(c, id)?;
    let notifications = c.params.bool("notifications");
    let duration = c.params.u64("duration");
    c.svc
        .set_mute(viewer, slot, on, notifications, duration, c.now)
        .map_err(fail)?;
    relationship(c, viewer, slot)
}

fn relation_list<S: Store>(c: &Call<'_, S>, blocks: bool) -> Reply {
    let scope = if blocks { "read:blocks" } else { "read:mutes" };
    let viewer = c.user_scoped(scope).or_else(|_| c.user_scoped("follow"))?;
    let q = c.page_query();
    let page = c.svc.relation_list(viewer, blocks, &q);
    let ids: Vec<u64> = page.iter().map(|(id, _)| *id).collect();
    let body: Vec<Value> = page
        .into_iter()
        .filter_map(|(_, slot)| c.svc.state.account(slot))
        .map(|a| {
            let mut v = entities::account(c.svc, c.ctx, a);
            if !blocks {
                v["mute_expires_at"] = c
                    .svc
                    .state
                    .mute(viewer, a.slot)
                    .and_then(|m| m.expires_ms)
                    .map_or(Value::Null, |ms| json!(super::time::iso(ms)));
            }
            v
        })
        .collect();
    Ok(c.paged(Value::Array(body), &ids, q.limit()))
}

fn file_report<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let reporter = c.user_scoped("write:reports")?;
    let p = &c.params;
    let target = match p.text("account_id") {
        Some(id) => target_slot(c, id)?,
        None => {
            return Err(Response::error(
                422,
                "Validation failed: Account is missing",
            ))
        }
    };
    let status_ids = p
        .all("status_ids")
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| crate::ids::parse(s).ok_or_else(|| fail(Error::NotFound)))
        .collect::<Result<Vec<u64>, Response>>()?;
    let category = match p.text("category") {
        Some(text) => Some(
            ReportCategory::parse(text)
                .ok_or_else(|| Response::error(422, "Validation failed: Category is invalid"))?,
        ),
        None => None,
    };
    let rule_ids = p
        .all("rule_ids")
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u8>())
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|_| Response::error(422, "Validation failed: Rules are invalid"))?;
    let new = NewReport {
        target,
        status_ids,
        comment: p.get("comment").unwrap_or("").to_string(),
        category,
        rule_ids,
        forward: p.flag("forward"),
    };
    let id = c.svc.file_report(reporter, new, c.now).map_err(fail)?;
    let report = c
        .svc
        .state
        .reports
        .get(&id)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(entities::report(c.svc, c.ctx, report)))
}

fn own_reports<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user_scoped("read:reports")?;
    let rows: Vec<Value> = c
        .svc
        .state
        .reports
        .values()
        .rev()
        .filter(|r| r.reporter == viewer)
        .map(|r| entities::report(c.svc, c.ctx, r))
        .collect();
    Ok(Response::ok(Value::Array(rows)))
}

// --- Admin API ---------------------------------------------------------------

/// An admin with the given admin scope.
fn admin<S: Store>(c: &Call<'_, S>, scope: &str) -> Result<u8, Response> {
    let actor = c.user_scoped(scope)?;
    c.svc.require_admin(actor).map_err(fail)?;
    Ok(actor)
}

fn admin_target<'s, S: Store>(c: &'s Call<'_, S>, id: &str) -> Result<&'s Account, Response> {
    let id = c.id(id)?;
    c.svc
        .state
        .account_by_id(id)
        .ok_or_else(|| fail(Error::NotFound))
}

fn page_of<T: Copy + 'static>(q: &PageQuery, rows: Vec<(u64, T)>) -> Vec<(u64, T)> {
    paginate(q, |bounds, asc| {
        let rows = rows.clone();
        let it: Scan<'_, T> = if asc {
            Box::new(rows.into_iter().filter(move |(id, _)| bounds.contains(*id)))
        } else {
            Box::new(
                rows.into_iter()
                    .rev()
                    .filter(move |(id, _)| bounds.contains(*id)),
            )
        };
        it
    })
}

/// v1 and v2 account filters. There are no remote or pending accounts, and
/// no e-mail addresses or IPs to match.
fn admin_accounts<S: Store>(c: &Call<'_, S>, v2: bool) -> Reply {
    admin(c, "admin:read:accounts")?;
    let p = &c.params;
    let text = |name: &str| p.text(name).map(str::to_lowercase);
    let (username, display_name) = (text("username"), text("display_name"));
    let status = if v2 {
        p.text("status").map(str::to_string)
    } else {
        [
            "active",
            "pending",
            "disabled",
            "silenced",
            "suspended",
            "sensitized",
        ]
        .into_iter()
        .find(|f| p.flag(f))
        .map(str::to_string)
    };
    let remote = if v2 {
        p.text("origin") == Some("remote")
    } else {
        p.flag("remote") || p.text("by_domain").is_some()
    };
    let staff = if v2 {
        p.text("permissions") == Some("staff")
    } else {
        p.flag("staff")
    };
    let impossible = remote
        || p.text("email").is_some()
        || p.text("ip").is_some()
        || status.as_deref() == Some("pending");
    let state = &c.svc.state;
    let mut rows: Vec<(u64, u8)> = state
        .active_accounts()
        .filter(|_| !impossible)
        .filter(|a| {
            username
                .as_ref()
                .is_none_or(|u| a.rec.username.contains(u.as_str()))
        })
        .filter(|a| {
            display_name
                .as_ref()
                .is_none_or(|d| a.rec.display_name.to_lowercase().contains(d.as_str()))
        })
        .filter(|a| !staff || a.rec.role.is_admin())
        .filter(|a| {
            let m = state.moderation(a.slot);
            match status.as_deref() {
                None => true,
                Some("active") => !a.rec.disabled && m.suspended_ms.is_none(),
                Some("disabled") => a.rec.disabled,
                Some("silenced") => m.silenced,
                Some("suspended") => m.suspended_ms.is_some(),
                Some("sensitized") => m.sensitized,
                Some(_) => false,
            }
        })
        .map(|a| (a.rec.id, a.slot))
        .collect();
    rows.sort_unstable();
    let q = c.page_query();
    let page = page_of(&q, rows);
    let ids: Vec<u64> = page.iter().map(|(id, _)| *id).collect();
    let body: Vec<Value> = page
        .into_iter()
        .filter_map(|(_, slot)| state.account(slot))
        .map(|a| entities::admin_account(c.svc, c.ctx, a))
        .collect();
    Ok(c.paged(Value::Array(body), &ids, q.limit()))
}

fn admin_account<S: Store>(c: &Call<'_, S>, id: &str) -> Reply {
    admin(c, "admin:read:accounts")?;
    let account = admin_target(c, id)?;
    Ok(Response::ok(entities::admin_account(c.svc, c.ctx, account)))
}

fn admin_account_action<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let actor = admin(c, "admin:write:accounts")?;
    let slot = admin_target(c, id)?.slot;
    let action = c
        .params
        .text("type")
        .and_then(AdminAction::parse)
        .ok_or_else(|| Response::error(422, "Validation failed: Type is invalid"))?;
    let report_id = match c.params.text("report_id") {
        Some(id) => Some(c.id(id)?),
        None => None,
    };
    c.svc
        .moderate(actor, slot, action, report_id, c.now)
        .map_err(fail)?;
    Ok(Response::ok(json!({})))
}

fn admin_account_undo<S: Store>(c: &mut Call<'_, S>, id: &str, action: AdminAction) -> Reply {
    let actor = admin(c, "admin:write:accounts")?;
    let slot = admin_target(c, id)?.slot;
    c.svc
        .moderate(actor, slot, action, None, c.now)
        .map_err(fail)?;
    admin_account(c, id)
}

/// Permanently delete a suspended account's data (Mastodon's rule: suspend
/// first). Returns the account as it was.
fn admin_account_delete<S: Store>(c: &mut Call<'_, S>, id: &str) -> Reply {
    let actor = admin(c, "admin:write:accounts")?;
    let account = admin_target(c, id)?;
    let (slot, body) = (account.slot, entities::admin_account(c.svc, c.ctx, account));
    if !c.svc.state.suspended(slot) {
        return Err(fail(Error::Forbidden("This action is not allowed".into())));
    }
    c.svc.delete_account(actor, slot, c.now).map_err(fail)?;
    Ok(Response::ok(body))
}

fn admin_reports<S: Store>(c: &Call<'_, S>) -> Reply {
    let actor = admin(c, "admin:read:reports")?;
    let p = &c.params;
    let resolved = p.flag("resolved");
    let reporter = p.u64("account_id").map(|id| c.svc.state.slot_of(id));
    let target = p.u64("target_account_id").map(|id| c.svc.state.slot_of(id));
    let rows: Vec<(u64, u64)> = c
        .svc
        .state
        .reports
        .values()
        .filter(|r| r.action_taken_ms.is_some() == resolved)
        .filter(|r| reporter.is_none_or(|s| s == Some(r.reporter)))
        .filter(|r| target.is_none_or(|s| s == Some(r.target)))
        .map(|r| (r.id, r.id))
        .collect();
    let q = c.page_query();
    let page = page_of(&q, rows);
    let ids: Vec<u64> = page.iter().map(|(id, _)| *id).collect();
    let body: Vec<Value> = page
        .into_iter()
        .filter_map(|(id, _)| c.svc.state.reports.get(&id))
        .map(|r| entities::admin_report(c.svc, c.ctx, r, actor))
        .collect();
    Ok(c.paged(Value::Array(body), &ids, q.limit()))
}

fn admin_report<S: Store>(c: &Call<'_, S>, id: &str) -> Reply {
    let actor = admin(c, "admin:read:reports")?;
    let id = c.id(id)?;
    let report = c
        .svc
        .state
        .reports
        .get(&id)
        .ok_or_else(|| fail(Error::NotFound))?;
    Ok(Response::ok(entities::admin_report(
        c.svc, c.ctx, report, actor,
    )))
}

#[derive(Clone, Copy)]
enum ReportOp {
    Assign,
    Unassign,
    Resolve,
    Reopen,
    Update,
}

fn admin_report_op<S: Store>(c: &mut Call<'_, S>, id: &str, op: ReportOp) -> Reply {
    let actor = admin(c, "admin:write:reports")?;
    let report_id = c.id(id)?;
    let result = match op {
        ReportOp::Assign => c.svc.assign_report(actor, report_id, true, c.now),
        ReportOp::Unassign => c.svc.assign_report(actor, report_id, false, c.now),
        ReportOp::Resolve => c.svc.resolve_report(actor, report_id, true, c.now),
        ReportOp::Reopen => c.svc.resolve_report(actor, report_id, false, c.now),
        ReportOp::Update => {
            let category = match c.params.text("category") {
                Some(text) => Some(ReportCategory::parse(text).ok_or_else(|| {
                    Response::error(422, "Validation failed: Category is invalid")
                })?),
                None => None,
            };
            let has_rules = c.params.names().any(|n| n.starts_with("rule_ids"));
            let rule_ids = if has_rules {
                Some(
                    c.params
                        .all("rule_ids")
                        .iter()
                        .filter(|s| !s.is_empty())
                        .map(|s| s.parse::<u8>())
                        .collect::<Result<Vec<u8>, _>>()
                        .map_err(|_| {
                            Response::error(422, "Validation failed: Rules are invalid")
                        })?,
                )
            } else {
                None
            };
            c.svc
                .recategorize_report(actor, report_id, category, rule_ids, c.now)
        }
    };
    result.map_err(fail)?;
    admin_report(c, id)
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, seg) {
        ("GET", ["api", "v1", "blocks"]) => relation_list(c, true),
        ("GET", ["api", "v1", "mutes"]) => relation_list(c, false),
        ("POST", ["api", "v1", "reports"]) => file_report(c),
        ("GET", ["api", "v1", "reports"]) => own_reports(c),
        ("GET", ["api", "v1", "admin", "accounts"]) => admin_accounts(c, false),
        ("GET", ["api", "v2", "admin", "accounts"]) => admin_accounts(c, true),
        ("GET", ["api", "v1", "admin", "accounts", id]) => admin_account(c, id),
        ("DELETE", ["api", "v1", "admin", "accounts", id]) => admin_account_delete(c, id),
        ("POST", ["api", "v1", "admin", "accounts", id, action]) => match *action {
            "action" => admin_account_action(c, id),
            "enable" => admin_account_undo(c, id, AdminAction::Enable),
            "unsilence" => admin_account_undo(c, id, AdminAction::Unsilence),
            "unsuspend" => admin_account_undo(c, id, AdminAction::Unsuspend),
            "unsensitive" => admin_account_undo(c, id, AdminAction::Unsensitive),
            // Nobody is ever pending approval here.
            "approve" | "reject" => admin(c, "admin:write:accounts")
                .and_then(|_| Err(fail(Error::Forbidden("This action is not allowed".into())))),
            _ => return None,
        },
        ("GET", ["api", "v1", "admin", "reports"]) => admin_reports(c),
        ("GET", ["api", "v1", "admin", "reports", id]) => admin_report(c, id),
        ("PUT", ["api", "v1", "admin", "reports", id]) => admin_report_op(c, id, ReportOp::Update),
        ("POST", ["api", "v1", "admin", "reports", id, action]) => {
            let op = match *action {
                "assign_to_self" => ReportOp::Assign,
                "unassign" => ReportOp::Unassign,
                "resolve" => ReportOp::Resolve,
                "reopen" => ReportOp::Reopen,
                _ => return None,
            };
            admin_report_op(c, id, op)
        }
        _ => return None,
    })
}
