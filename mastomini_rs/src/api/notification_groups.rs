//! Group after filtering the bounded RAM ring, then paginate groups by their
//! newest notification. Stable keys are type+target; mentions remain individual.
use super::{entities, fail, Call, Reply};
use crate::domain::query::paginate;
use crate::domain::{Error, Notification, NotificationKind};
use crate::http::Response;
use crate::store::Store;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn key(n: &Notification) -> String {
    match n.kind {
        NotificationKind::Favourite | NotificationKind::Reblog => {
            format!("{}-{}", n.kind.as_str(), n.status.unwrap_or(n.id))
        }
        NotificationKind::Follow => "follow".into(),
        _ => format!("ungrouped-{}", n.id),
    }
}

fn groups<S: Store>(c: &Call<'_, S>, viewer: u8) -> Vec<Vec<Notification>> {
    let types = c.params.all("types");
    let exclude = c.params.all("exclude_types");
    let account = c.params.u64("account_id");
    let ungroup = c.params.all("ungroup_types");
    let mut grouped = BTreeMap::<String, Vec<Notification>>::new();
    for n in c
        .svc
        .state
        .notifications
        .iter()
        .rev()
        .filter(|n| n.to == viewer)
    {
        if (!types.is_empty() && !types.contains(&n.kind.as_str()))
            || exclude.contains(&n.kind.as_str())
        {
            continue;
        }
        if account.is_some_and(|id| c.svc.state.account(n.from).is_none_or(|a| a.rec.id != id)) {
            continue;
        }
        if c.svc.state.account(n.from).is_none()
            || !(n.kind == NotificationKind::AdminReport
                || (n.kind == NotificationKind::Poll && n.from == viewer)
                || c.svc.wants_notification(viewer, n.from, n.status))
            || n.status
                .is_some_and(|id| c.svc.visible(Some(viewer), id).is_none())
        {
            continue;
        }
        let k = if ungroup.contains(&n.kind.as_str()) {
            format!("ungrouped-{}", n.id)
        } else {
            key(n)
        };
        grouped.entry(k).or_default().push(*n);
    }
    let mut groups: Vec<_> = grouped.into_values().collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g[0].id));
    groups
}

fn render<S: Store>(c: &Call<'_, S>, viewer: u8, groups: &[Vec<Notification>]) -> Value {
    let mut accounts = BTreeSet::new();
    let mut statuses = BTreeSet::new();
    let ungroup = c.params.all("ungroup_types");
    let rows: Vec<_> = groups
        .iter()
        .map(|g| {
            let n = g[0];
            let mut samples = Vec::new();
            for member in g {
                if let Some(a) = c.svc.state.account(member.from) {
                    accounts.insert(a.slot);
                    let id = a.rec.id.to_string();
                    if !samples.contains(&id) {
                        samples.push(id);
                    }
                }
                if let Some(id) = member.status {
                    statuses.insert(id);
                }
            }
            let group_key = if ungroup.contains(&n.kind.as_str()) {
                format!("ungrouped-{}", n.id)
            } else {
                key(&n)
            };
            json!({"group_key": group_key, "notifications_count": g.len(), "type": n.kind.as_str(),
            "most_recent_notification_id": n.id.to_string(),
            "page_min_id": g.last().unwrap().id.to_string(), "page_max_id": n.id.to_string(),
            "latest_page_notification_at": super::time::iso(crate::ids::millis(n.id)),
            "sample_account_ids": samples, "status_id": n.status.map(|id| id.to_string())})
        })
        .collect();
    let rendered: Vec<_> = statuses
        .into_iter()
        .filter_map(|id| c.svc.visible(Some(viewer), id))
        .map(|s| entities::status(c.svc, c.ctx, s, Some(viewer)))
        .collect();
    json!({"accounts":accounts.into_iter().filter_map(|id|c.svc.state.account(id)).map(|a|entities::account(c.svc,c.ctx,a)).collect::<Vec<_>>(),
        "statuses":super::filters::annotate(c,viewer,"notifications",rendered),"notification_groups":rows})
}

pub(super) fn list<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user_scoped("read:notifications")?;
    let q = c.page_query();
    let groups = groups(c, viewer);
    let page = paginate(&q, |b, asc| {
        let mut rows: Vec<_> = groups
            .iter()
            .filter(|g| b.contains(g[0].id))
            .map(|g| (g[0].id, g.clone()))
            .collect();
        if asc {
            rows.reverse();
        }
        Box::new(rows.into_iter())
    });
    let ids: Vec<_> = page.iter().map(|(id, _)| *id).collect();
    Ok(c.paged(
        render(
            c,
            viewer,
            &page.into_iter().map(|(_, g)| g).collect::<Vec<_>>(),
        ),
        &ids,
        q.limit(),
    ))
}

fn selected<S: Store>(
    c: &Call<'_, S>,
    viewer: u8,
    wanted: &str,
) -> Result<Vec<Notification>, Response> {
    // Lookup/dismiss uses the same filtered, visible ring, never another viewer's rows.
    let singleton = wanted
        .strip_prefix("ungrouped-")
        .and_then(crate::ids::parse);
    let rows: Vec<_> = groups(c, viewer)
        .into_iter()
        .flatten()
        .filter(|n| singleton.map_or_else(|| key(n) == wanted, |id| n.id == id))
        .collect();
    if rows.is_empty() {
        Err(fail(Error::NotFound))
    } else {
        Ok(rows)
    }
}

pub(super) fn show<S: Store>(c: &Call<'_, S>, key: &str) -> Reply {
    let viewer = c.user_scoped("read:notifications")?;
    let rows = selected(c, viewer, key)?;
    let mut body = render(c, viewer, &[rows]);
    body["notification_groups"][0]["group_key"] = json!(key);
    Ok(Response::ok(body))
}

pub(super) fn accounts<S: Store>(c: &Call<'_, S>, key: &str) -> Reply {
    let viewer = c.user_scoped("read:notifications")?;
    let rows = selected(c, viewer, key)?;
    let slots: BTreeSet<_> = rows.iter().map(|n| n.from).collect();
    let q = c.page_query();
    let page = paginate(&q, |b, asc| {
        let mut rows: Vec<_> = slots
            .iter()
            .filter_map(|s| c.svc.state.account(*s))
            .filter(|a| b.contains(a.rec.id))
            .map(|a| (a.rec.id, a))
            .collect();
        rows.sort_by_key(|(id, _)| *id);
        if !asc {
            rows.reverse();
        }
        Box::new(rows.into_iter())
    });
    let ids: Vec<_> = page.iter().map(|(id, _)| *id).collect();
    Ok(c.paged(
        json!(page
            .iter()
            .map(|(_, a)| entities::account(c.svc, c.ctx, a))
            .collect::<Vec<_>>()),
        &ids,
        q.limit(),
    ))
}

pub(super) fn dismiss<S: Store>(c: &mut Call<'_, S>, key: &str) -> Reply {
    let viewer = c.user_scoped("write:notifications")?;
    let rows = selected(c, viewer, key)?;
    let ids: BTreeSet<_> = rows.iter().map(|n| n.id).collect();
    c.svc
        .state
        .notifications
        .retain(|n| n.to != viewer || !ids.contains(&n.id));
    Ok(Response::ok(json!({})))
}

pub(super) fn unread<S: Store>(c: &Call<'_, S>) -> Reply {
    let viewer = c.user_scoped("read:notifications")?;
    let read = c.svc.state.markers[viewer as usize][1].map_or(0, |m| m.last_read_id);
    Ok(Response::ok(
        json!({"count":groups(c,viewer).iter().filter(|g|g.iter().any(|n|n.id>read)).count()}),
    ))
}
