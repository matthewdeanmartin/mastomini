use super::{fail, time, Call, Reply};
use crate::domain::{
    scheduled::{Bridge, Scheduled},
    Service,
};
use crate::http::Response;
use crate::store::Store;
use serde_json::{json, Value};

pub(super) fn entity<S: Store>(j: &Scheduled, svc: &Service<S>) -> Value {
    json!({"id": j.id.to_string(), "scheduled_at": time::iso(j.at),
        "params": {"text": j.post.text, "spoiler_text": j.post.spoiler_text,
            "visibility": j.post.visibility.map(|v| v.as_str()), "sensitive": j.post.sensitive,
            "language": j.post.language, "in_reply_to_id": j.post.in_reply_to_id.map(|id| id.to_string()),
            "application_id": j.post.app_id, "poll": j.post.poll.as_ref().map(|p| json!({"options":p.options, "expires_in":p.expires_in_s,
                "multiple":p.multiple, "hide_totals":p.hide_totals})), "media_ids": []},
        "media_attachments": [], "mastomini_error": j.failure,
        "mastomini_delivery": if j.failure.is_some() { "failed" } else if j.done { "published" } else if j.cancelled { "cancelled" }
            else if svc.scheduling.acknowledged.contains(&(j.id, j.revision)) { "scheduled" } else { "pending_handoff" }
    })
}

pub(super) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    if !matches!(
        seg,
        ["api", "v1", "scheduled_statuses", ..]
            | ["api", "mastomini", "v1", "admin", "scheduler"]
            | ["api", "mastomini", "v1", "scheduler", "publish", _]
    ) {
        return None;
    }
    Some((|| {
        if matches!(seg, ["api", "mastomini", "v1", "admin", "scheduler"]) {
            let slot = c.user()?;
            c.svc.require_admin(slot).map_err(fail)?;
            if method == "PUT" {
                let bridge = Bridge {
                    bots_url: c
                        .params
                        .get("bots_url")
                        .unwrap_or("")
                        .trim_end_matches('/')
                        .to_string(),
                    api_key: c.params.get("api_key").unwrap_or("").to_string(),
                };
                // Reuse a real, revocable local API key, not a new machine secret.
                let principal = c
                    .svc
                    .principal(&bridge.api_key, c.now)
                    .ok_or_else(|| Response::error(422, "Invalid scheduler API key"))?;
                let owner = principal
                    .slot
                    .ok_or_else(|| Response::error(422, "A member API key is required"))?;
                let hash = crate::auth::sha256(&bridge.api_key);
                if !c
                    .svc
                    .api_keys(owner)
                    .iter()
                    .any(|(t, _)| t.rec.hash == hash)
                {
                    return Err(Response::error(
                        422,
                        "Use a persistent API key from My account",
                    ));
                }
                if !principal.allows("write:statuses") || principal.disabled {
                    return Err(Response::error(
                        422,
                        "Scheduler key requires write:statuses",
                    ));
                }
                c.svc.set_scheduler(bridge).map_err(fail)?;
            } else if method != "GET" {
                return Err(Response::error(405, "Use GET or PUT"));
            }
            return Ok(Response::ok(
                json!({"bots_url": c.svc.scheduling.bridge.bots_url,
                "key_set": !c.svc.scheduling.bridge.api_key.is_empty(), "capacity": 16,
                "last_error": c.svc.scheduling.last_error,
                "pending_handoff": c.svc.scheduling.jobs.values().filter(|j| !j.done && (j.failure.is_none() || j.cancelled) &&
                    !c.svc.scheduling.acknowledged.contains(&(j.id, j.revision))).count()}),
            ));
        }
        if let ["api", "mastomini", "v1", "scheduler", "publish", id] = seg {
            if method != "POST" {
                return Err(Response::error(405, "Use POST"));
            }
            c.user_scoped("write:statuses")?;
            if !crate::auth::ct_eq(
                c.req.bearer().unwrap_or("").as_bytes(),
                c.svc.scheduling.bridge.api_key.as_bytes(),
            ) {
                return Err(Response::error(
                    403,
                    "This key is not configured for the scheduler",
                ));
            }
            let id = c.id(id)?;
            let revision = c
                .params
                .u64("revision")
                .ok_or_else(|| Response::error(422, "revision is required"))?;
            let published = c.svc.publish_scheduled(id, revision, c.now).map_err(fail)?;
            return Ok(Response::ok(
                json!({"id": published.map(|v| v.to_string())}),
            ));
        }
        let slot = c.user_scoped(if method == "GET" {
            "read:statuses"
        } else {
            "write:statuses"
        })?;
        let account_id = c.svc.state.account(slot).unwrap().rec.id;
        if seg.len() == 3 && method == "GET" {
            let limit = c.params.u64("limit").unwrap_or(20).clamp(1, 40) as usize;
            let max = c.params.u64("max_id").unwrap_or(u64::MAX);
            let min = c
                .params
                .u64("min_id")
                .or_else(|| c.params.u64("since_id"))
                .unwrap_or(0);
            let ids: Vec<_> = c
                .svc
                .scheduling
                .jobs
                .values()
                .rev()
                .filter(|j| {
                    j.account_id == account_id
                        && !j.done
                        && !j.cancelled
                        && j.id < max
                        && j.id > min
                })
                .take(limit)
                .map(|j| j.id)
                .collect();
            let mut rows = Vec::new();
            for &id in &ids {
                let job = c.svc.scheduled_post(id).map_err(fail)?;
                rows.push(entity(&job, c.svc));
            }
            return Ok(c.paged(json!(rows), &ids, limit));
        }
        if seg.len() != 4 {
            return Err(Response::error(
                405,
                "Unsupported scheduled status operation",
            ));
        }
        let id = c.id(seg[3])?;
        let job = c
            .svc
            .scheduling
            .jobs
            .get(&id)
            .filter(|j| j.account_id == account_id && !j.done && !j.cancelled)
            .ok_or_else(|| Response::error(404, "Record not found"))?;
        if method == "GET" {
            let job = c.svc.scheduled_post(job.id).map_err(fail)?;
            return Ok(Response::ok(entity(&job, c.svc)));
        }
        let at = match method {
            "DELETE" => None,
            "PUT" => Some(
                c.params
                    .text("scheduled_at")
                    .and_then(time::parse_iso)
                    .ok_or_else(|| Response::error(422, "Invalid scheduled_at; use RFC3339"))?,
            ),
            _ => return Err(Response::error(405, "Use GET, PUT or DELETE")),
        };
        c.svc.change_schedule(slot, id, at, c.now).map_err(fail)?;
        let body = if method == "DELETE" {
            json!({})
        } else {
            let job = c.svc.scheduled_post(id).map_err(fail)?;
            entity(&job, c.svc)
        };
        Ok(Response::ok(body))
    })())
}
