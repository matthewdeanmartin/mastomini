//! Tags, private notes, profile aliases and announcements.
use super::{entities, fail, Call, Reply};
use crate::domain::query::paginate;
use crate::domain::records::Visibility;
use crate::domain::social::{tag_name, AnnouncementRec, FeaturedTag};
use crate::domain::{Error, Service};
use crate::http::Response;
use crate::store::Store;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn decode(value: &str) -> Result<String, Response> {
    percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .map(|v| v.into_owned())
        .map_err(|_| Response::error(400, "Invalid UTF-8 path segment"))
}

fn target<S: Store>(c: &Call<'_, S>, id: &str) -> Result<u8, Response> {
    c.svc
        .state
        .slot_of(c.id(id)?)
        .ok_or_else(|| fail(Error::NotFound))
}

fn tag<S: Store>(c: &Call<'_, S>, viewer: u8, name: &str) -> Value {
    let prefs = c.svc.social(viewer);
    let today = c.now / 86_400_000;
    let history: Vec<_> = (0..7).map(|offset| {
        let day = today.saturating_sub(offset);
        let mut accounts = 0u16;
        let uses = c.svc.state.statuses.values().filter(|s| {
            let matched = s.rec.visibility == Visibility::Public && s.tags.iter().any(|t| t == name)
                && crate::ids::millis(s.rec.id) / 86_400_000 == day
                && c.svc.can_see(Some(viewer),s) && !c.svc.state.hides(viewer,s.rec.author);
            if matched { accounts |= crate::domain::bit(s.rec.author); }
            matched
        }).count();
        json!({"day": (day*86400).to_string(), "uses": uses.to_string(), "accounts": accounts.count_ones().to_string()})
    }).collect();
    json!({"name":name, "url":format!("{}/tags/{}",c.ctx.base_url,crate::http::encode(name)),
        "following":prefs.followed.iter().any(|t|t.name==name),
        "featuring":prefs.featured.iter().any(|t|t.name==name),"history":history})
}

fn tag_usage<S: Store>(
    svc: &Service<S>,
    viewer: u8,
    owner: u8,
    name: &str,
) -> (usize, Option<u64>) {
    let mut count = 0;
    let mut last = None;
    for s in svc.state.statuses.values().filter(|s| {
        s.rec.author == owner
            && s.rec.visibility != Visibility::Direct
            && s.tags.iter().any(|t| t == name)
            && svc.can_see(Some(viewer), s)
    }) {
        count += 1;
        last = Some(crate::ids::millis(s.rec.id));
    }
    (count, last)
}

fn featured<S: Store>(c: &Call<'_, S>, viewer: u8, owner: u8, item: &FeaturedTag) -> Value {
    let (count, last) = tag_usage(c.svc, viewer, owner, &item.name);
    let username = &c.svc.state.account(owner).unwrap().rec.username;
    json!({"id":item.id.to_string(),"name":item.name,
        "url":format!("{}/@{}/tagged/{}",c.ctx.base_url,username,crate::http::encode(&item.name)),
        "statuses_count":count.to_string(),"last_status_at":last.map(super::time::iso)})
}

fn tags<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Reply {
    if let ("GET", ["api", "v1", "tags", name]) = (method, seg) {
        let viewer = c.authenticated_user()?;
        let name = tag_name(&decode(name)?).map_err(fail)?;
        return Ok(Response::ok(tag(c, viewer, &name)));
    }
    let is_featured =
        seg.contains(&"featured_tags") || matches!(seg.last(), Some(&"feature" | &"unfeature"));
    let scope = match (method == "GET", is_featured) {
        (true, true) => "read:accounts",
        (false, true) => "write:accounts",
        (true, false) => "read:follows",
        (false, false) => "write:follows",
    };
    let viewer = c.user_scoped(scope).or_else(|error| {
        if !is_featured
            && c.principal
                .as_ref()
                .is_some_and(|p| p.scopes.iter().any(|s| s == "follow"))
        {
            c.user_scoped("follow")
        } else {
            Err(error)
        }
    })?;
    match (method, seg) {
        (
            "POST",
            ["api", "v1", "tags", name, action @ ("follow" | "unfollow" | "feature" | "unfeature")],
        ) => {
            let name = tag_name(&decode(name)?).map_err(fail)?;
            c.svc
                .set_tag(
                    viewer,
                    &name,
                    action.ends_with("feature"),
                    !action.starts_with("un"),
                    c.now,
                )
                .map_err(fail)?;
            Ok(Response::ok(tag(c, viewer, &name)))
        }
        ("GET", ["api", "v1", "followed_tags"]) => {
            let prefs = c.svc.social(viewer);
            let q = c.page_query();
            let rows = paginate(&q, |b, asc| {
                let mut rows: Vec<_> = prefs
                    .followed
                    .iter()
                    .filter(|t| b.contains(t.id))
                    .map(|t| (t.id, t))
                    .collect();
                rows.sort_by_key(|(id, _)| *id);
                if !asc {
                    rows.reverse();
                }
                Box::new(rows.into_iter())
            });
            let ids: Vec<_> = rows.iter().map(|(id, _)| *id).collect();
            Ok(c.paged(
                json!(rows
                    .iter()
                    .map(|(_, t)| tag(c, viewer, &t.name))
                    .collect::<Vec<_>>()),
                &ids,
                q.limit(),
            ))
        }
        ("POST", ["api", "v1", "featured_tags"]) => {
            let name = tag_name(c.params.get("name").unwrap_or("")).map_err(fail)?;
            let id = c
                .svc
                .set_tag(viewer, &name, true, true, c.now)
                .map_err(fail)?
                .unwrap();
            Ok(Response::ok(featured(
                c,
                viewer,
                viewer,
                &FeaturedTag { id, name },
            )))
        }
        ("DELETE", ["api", "v1", "featured_tags", id]) => {
            let id = c.id(id)?;
            let name = c
                .svc
                .social(viewer)
                .featured
                .into_iter()
                .find(|t| t.id == id)
                .ok_or_else(|| fail(Error::NotFound))?
                .name;
            c.svc
                .set_tag(viewer, &name, true, false, c.now)
                .map_err(fail)?;
            Ok(Response::ok(json!({})))
        }
        ("GET", ["api", "v1", "featured_tags", "suggestions"]) => {
            let prefs = c.svc.social(viewer);
            let mut counts = BTreeMap::<String, usize>::new();
            for s in c
                .svc
                .state
                .statuses
                .values()
                .filter(|s| s.rec.author == viewer && s.rec.visibility != Visibility::Direct)
            {
                for name in &s.tags {
                    if !prefs.featured.iter().any(|t| t.name == *name) {
                        *counts.entry(name.clone()).or_default() += 1;
                    }
                }
            }
            let mut rows: Vec<_> = counts.into_iter().collect();
            rows.sort_by(|(a, x), (b, y)| y.cmp(x).then(a.cmp(b)));
            Ok(Response::ok(json!(rows
                .into_iter()
                .take(10)
                .map(|(name, _)| featured(c, viewer, viewer, &FeaturedTag { id: 0, name }))
                .collect::<Vec<_>>())))
        }
        ("GET", ["api", "v1", "featured_tags"])
        | ("GET", ["api", "v1", "accounts", _, "featured_tags"]) => {
            let owner = if seg.len() == 5 {
                target(c, seg[3])?
            } else {
                viewer
            };
            if c.svc.state.suspended(owner) || c.svc.state.blocked_either(viewer, owner) {
                return Err(fail(Error::NotFound));
            }
            let prefs = c.svc.social(owner);
            Ok(Response::ok(json!(prefs
                .featured
                .iter()
                .rev()
                .map(|t| featured(c, viewer, owner, t))
                .collect::<Vec<_>>())))
        }
        _ => Err(fail(Error::NotFound)),
    }
}

fn announcement_json<S: Store>(
    c: &Call<'_, S>,
    viewer: u8,
    rec: &AnnouncementRec,
    admin: bool,
) -> Value {
    let id = c.svc.state.account(viewer).unwrap().rec.id;
    let reactions: Vec<_> = rec.reactions.iter().map(|(name,users)|json!({"name":name,"count":users.len(),"me":users.contains(&id),"url":null,"static_url":null})).collect();
    let mut result = json!({"id":rec.id.to_string(),"content":entities::render_text(c.svc,c.ctx,&rec.text),
        "starts_at":rec.starts_ms.map(super::time::iso),"ends_at":rec.ends_ms.map(super::time::iso),
        "all_day":rec.all_day,"published_at":rec.published_ms.map(super::time::iso),
        "updated_at":super::time::iso(rec.updated_ms),"read":rec.read_by.contains(&id),
        "mentions":[],"statuses":[],"tags":crate::text::tags(&rec.text).iter().map(|t|entities::tag(c.ctx,t,false)).collect::<Vec<_>>(),"emojis":[],"reactions":reactions});
    if admin {
        result["published"] = json!(rec.published);
        result["text"] = json!(rec.text);
    }
    result
}

fn datetime<S: Store>(
    c: &Call<'_, S>,
    name: &str,
    old: Option<u64>,
) -> Result<Option<u64>, Response> {
    match c.params.get(name) {
        None => Ok(old),
        Some("") => Ok(None),
        Some(text) => super::time::parse_iso(text).map(Some).ok_or_else(|| {
            Response::error(422, "Validation failed: Invalid timestamp (use RFC3339)")
        }),
    }
}

fn announcements<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str], admin: bool) -> Reply {
    let viewer = c.user_scoped(if admin {
        if method == "GET" {
            "admin:read"
        } else {
            "admin:write"
        }
    } else if method == "GET" {
        "read:announcements"
    } else {
        "write:announcements"
    })?;
    if admin && !c.svc.state.account(viewer).unwrap().rec.role.is_admin() {
        return Err(Response::error(403, "Admin only"));
    }
    let rest = &seg[if admin { 4 } else { 3 }..];
    if method == "GET" && rest.is_empty() {
        let rows: Vec<_> = c
            .svc
            .state
            .announcements
            .values()
            .rev()
            .filter(|r| admin || r.visible(c.now))
            .filter(|r| {
                admin
                    || c.params.bool("with_dismissed").unwrap_or(false)
                    || !r
                        .read_by
                        .contains(&c.svc.state.account(viewer).unwrap().rec.id)
            })
            .map(|r| announcement_json(c, viewer, r, admin))
            .collect();
        return Ok(Response::ok(json!(rows)));
    }
    if admin
        && ((method == "POST" && rest.is_empty())
            || (matches!(method, "PATCH" | "PUT") && rest.len() == 1)
            || (method == "POST" && rest.len() == 2 && matches!(rest[1], "publish" | "unpublish")))
    {
        let mut rec = if rest.is_empty() {
            AnnouncementRec {
                id: 0,
                text: String::new(),
                published: true,
                all_day: false,
                starts_ms: None,
                ends_ms: None,
                published_ms: None,
                updated_ms: c.now,
                read_by: vec![],
                reactions: BTreeMap::new(),
            }
        } else {
            c.svc
                .state
                .announcements
                .get(&c.id(rest[0])?)
                .ok_or_else(|| fail(Error::NotFound))?
                .clone()
        };
        if rest.len() == 2 {
            rec.published = rest[1] == "publish";
        } else {
            if let Some(text) = c.params.get("text").or_else(|| c.params.get("content")) {
                rec.text = text.to_string();
            }
            if let Some(value) = c.params.bool("published") {
                rec.published = value;
            }
            if let Some(value) = c.params.bool("all_day") {
                rec.all_day = value;
            }
            rec.starts_ms = datetime(c, "starts_at", rec.starts_ms)?;
            rec.ends_ms = datetime(c, "ends_at", rec.ends_ms)?;
        }
        let id = c.svc.save_announcement(viewer, rec, c.now).map_err(fail)?;
        return Ok(Response::ok(announcement_json(
            c,
            viewer,
            &c.svc.state.announcements[&id],
            true,
        )));
    }
    match (method, rest) {
        ("GET", [id]) if admin => Ok(Response::ok(announcement_json(
            c,
            viewer,
            c.svc
                .state
                .announcements
                .get(&c.id(id)?)
                .ok_or_else(|| fail(Error::NotFound))?,
            true,
        ))),
        ("DELETE", [id]) if admin => {
            c.svc
                .delete_announcement(viewer, c.id(id)?, c.now)
                .map_err(fail)?;
            Ok(Response::ok(json!({})))
        }
        ("POST", [id, "dismiss"]) if !admin => {
            c.svc
                .react_announcement(viewer, c.id(id)?, None, c.now)
                .map_err(fail)?;
            Ok(Response::ok(json!({})))
        }
        ("PUT" | "DELETE", [id, "reactions", name]) if !admin => {
            c.svc
                .react_announcement(
                    viewer,
                    c.id(id)?,
                    Some((&decode(name)?, method == "PUT")),
                    c.now,
                )
                .map_err(fail)?;
            Ok(Response::ok(json!({})))
        }
        _ => Err(fail(Error::NotFound)),
    }
}

pub(super) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, seg) {
        (_, ["api", "v1", "tags" | "featured_tags" | "followed_tags", ..])
        | (_, ["api", "v1", "accounts", _, "featured_tags"]) => tags(c, method, seg),
        (_, ["api", "v1", "announcements", ..]) => announcements(c, method, seg, false),
        (_, ["api", "v1", "admin", "announcements", ..]) => announcements(c, method, seg, true),
        ("POST", ["api", "v1", "accounts", id, "note"]) => (|| {
            let viewer = c.user_scoped("write:accounts")?;
            let slot = target(c, id)?;
            c.svc.set_account_note(viewer, slot, c.params.get("comment").unwrap_or(""), c.now).map_err(fail)?;
            Ok(Response::ok(entities::relationship(c.svc, viewer, c.svc.state.account(slot).unwrap())))
        })(),
        ("DELETE", ["api", "v1", "suggestions", id]) => (|| {
            let viewer = c.user_scoped("read")?;
            let slot = target(c, id)?;
            c.svc.dismiss_suggestion(viewer, slot, c.now).map_err(fail)?;
            Ok(Response::ok(json!({})))
        })(),
        ("GET", ["api", "v1", "profile"]) => super::accounts::verify_credentials(c),
        ("PATCH", ["api", "v1", "profile"]) => super::accounts::update_credentials(c),
        ("DELETE", ["api", "v1", "profile", "avatar" | "header"]) => c.user_scoped("write:accounts").map(|slot| {
            Response::ok(entities::credential_account(c.svc, c.ctx, c.svc.state.account(slot).unwrap()))
        }),
        ("GET", ["oauth", "userinfo"]) => c.user_scoped("profile").or_else(|_| c.user_scoped("read:accounts")).map(|slot| {
            let a = c.svc.state.account(slot).unwrap();
            Response::ok(json!({"sub": a.rec.id.to_string(), "preferred_username": a.rec.username,
                "name": a.rec.display_name, "profile": entities::profile_url(c.ctx, &a.rec.username),
                "picture": entities::avatar_url(c.ctx, a.rec.id)}))
        }),
        ("GET", ["api", "v1", "search"]) => super::timelines::search(c).map(|mut r| {
            if let Ok(mut v) = serde_json::from_slice::<Value>(&r.body) {
                if let Some(tags) = v["hashtags"].as_array_mut() {
                    *tags = tags.iter().filter_map(|t| t["name"].as_str().map(|n| json!(n))).collect();
                }
                r.body = serde_json::to_vec(&v).unwrap();
            }
            r
        }),
        _ => return None,
    })
}
