//! Preferences, suggestions, and neutral answers for Tier 2/3 endpoints that
//! clients call in passing (spec/04 "Tier 3"). Empty lists, never 404s, where
//! a client polls them.

use super::{entities, fail, Call, Reply};
use crate::domain::Error;
use crate::http::Response;
use crate::store::Store;
use serde_json::{json, Value};

fn preferences<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user_scoped("read:accounts")?;
    let rec = &c
        .svc
        .state
        .account(viewer)
        .ok_or_else(|| fail(Error::NotFound))?
        .rec;
    Ok(Response::ok(json!({
        "posting:default:visibility": rec.privacy.as_str(),
        "posting:default:sensitive": rec.sensitive,
        "posting:default:language": rec.language,
        "posting:default:quote_policy": "nobody",
        "reading:expand:media": "default",
        "reading:expand:spoilers": false,
        "reading:autoplay:gifs": false,
    })))
}

/// Household members the viewer does not follow yet.
fn not_followed<S: Store>(c: &Call<'_, S>, viewer: u8) -> Vec<Value> {
    c.svc
        .state
        .active_accounts()
        .filter(|a| a.slot != viewer && !c.svc.state.follows(viewer, a.slot) && !a.rec.disabled)
        .filter(|a| !c.svc.state.hides(viewer, a.slot) && !c.svc.state.suspended(a.slot))
        .filter(|a| {
            !c.svc
                .state
                .social
                .get(&viewer)
                .is_some_and(|p| p.dismissed.contains(&a.rec.id))
        })
        .map(|a| entities::account(c.svc, c.ctx, a))
        .collect()
}

fn suggestions_v2<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user()?;
    let rows: Vec<Value> = not_followed(c, viewer)
        .into_iter()
        .map(|account| json!({ "source": "staff", "sources": ["featured"], "account": account }))
        .collect();
    Ok(Response::ok(Value::Array(rows)))
}

/// `order=active` (default: most recent post first) or `order=new`.
fn directory<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user()?;
    let mut accounts: Vec<&crate::domain::Account> = c
        .svc
        .state
        .active_accounts()
        .filter(|a| a.rec.discoverable && !a.rec.disabled)
        .filter(|a| !c.svc.state.blocked_either(viewer, a.slot) && !c.svc.state.suspended(a.slot))
        .collect();
    if c.params.get("order") == Some("new") {
        accounts.sort_by_key(|a| std::cmp::Reverse(a.rec.id));
    } else {
        accounts.sort_by_key(|a| std::cmp::Reverse(c.svc.last_status_ms(a.slot)));
    }
    let offset = c.params.u64("offset").unwrap_or(0) as usize;
    let limit = c
        .params
        .u64("limit")
        .map_or(40, |l| l.clamp(1, 80) as usize);
    let rows: Vec<Value> = accounts
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|a| entities::account(c.svc, c.ctx, a))
        .collect();
    Ok(Response::ok(Value::Array(rows)))
}

fn notification_policy() -> Value {
    json!({
        "for_not_following": "accept",
        "for_not_followers": "accept",
        "for_new_accounts": "accept",
        "for_private_mentions": "accept",
        "for_limited_accounts": "accept",
        "for_bots": "accept",
        "filter_not_following": false,
        "filter_not_followers": false,
        "filter_new_accounts": false,
        "filter_private_mentions": false,
        "summary": { "pending_requests_count": 0, "pending_notifications_count": 0 },
    })
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    let empty = |c: &Call<'_, S>| c.user().map(|_| Response::ok(json!([])));
    Some(match (method, seg) {
        ("GET", ["api", "v1", "preferences"]) => preferences(c),
        ("GET", ["health"]) => Ok(Response::new(200, "text/plain", "OK")),
        ("GET", ["api", "v2", "suggestions"]) => suggestions_v2(c),
        ("GET", ["api", "v1", "suggestions"]) => {
            let viewer = match c.user() {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            Ok(Response::ok(Value::Array(not_followed(c, viewer))))
        }
        ("GET", ["api", "v1", "directory"]) => directory(c),
        // v1 updates with PUT, v2 with PATCH. Everyone who can notify a
        // member is a member, so nothing is ever filtered.
        ("GET" | "PUT" | "PATCH", ["api", "v1" | "v2", "notifications", "policy"]) => {
            c.user().map(|_| Response::ok(notification_policy()))
        }
        // Notification requests (filtered notifications): never any, as
        // above. Mastodon's shapes, so apps that ask don't see errors.
        ("GET", ["api", "v1", "notifications", "requests", "merged"]) => {
            c.user().map(|_| Response::ok(json!({ "merged": true })))
        }
        ("POST", ["api", "v1", "notifications", "requests", "accept" | "dismiss"]) => {
            c.user().map(|_| Response::ok(json!({})))
        }
        ("GET", ["api", "v1", "notifications", "requests", _])
        | ("POST", ["api", "v1", "notifications", "requests", _, "accept" | "dismiss"]) => c
            .user()
            .and_then(|_| Err(Response::error(404, "Record not found"))),
        // Tier 2, not yet implemented: empty lists so clients render nothing.
        (
            "GET",
            ["api", "v1", "domain_blocks" | "endorsements" | "scheduled_statuses"]
            | ["api", "v1", "trends", ..]
            | ["api", "v1", "timelines", "link"],
        ) => empty(c),
        ("POST", ["api", "v1" | "v2", "media"]) => Err(Response::error(
            422,
            "Media attachments are not supported on this server",
        )),
        _ => return None,
    })
}
