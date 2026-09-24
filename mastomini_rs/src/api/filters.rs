//! Content filters: `/api/v2/filters` (with keywords and statuses), the
//! older `/api/v1/filters` view (one v1 filter per keyword), and
//! `Status.filtered` on timelines, notifications and threads.

use super::time::iso;
use super::{fail, Call, Reply};
use crate::domain::records::{
    FilterAction, FilterKeywordRec, FilterRec, FilterStatusRec, FILTER_CONTEXTS,
};
use crate::domain::{filter_context_bit, Error, FilterMatch};
use crate::http::Response;
use crate::store::Store;
use serde_json::{json, Value};

fn contexts(bits: u8) -> Vec<&'static str> {
    FILTER_CONTEXTS
        .iter()
        .enumerate()
        .filter(|(i, _)| bits & (1 << i) != 0)
        .map(|(_, c)| *c)
        .collect()
}

fn keyword_json(k: &FilterKeywordRec) -> Value {
    json!({ "id": k.id.to_string(), "keyword": k.keyword, "whole_word": k.whole_word })
}

fn status_json(s: &FilterStatusRec) -> Value {
    json!({ "id": s.id.to_string(), "status_id": s.status_id.to_string() })
}

fn filter_json(f: &FilterRec) -> Value {
    json!({
        "id": f.id.to_string(),
        "title": f.title,
        "context": contexts(f.context),
        "expires_at": f.expires_ms.map(iso),
        "filter_action": f.action.as_str(),
        "keywords": f.keywords.iter().map(keyword_json).collect::<Vec<_>>(),
        "statuses": f.statuses.iter().map(status_json).collect::<Vec<_>>(),
    })
}

/// A v1 filter: one keyword of a v2 filter.
fn v1_json(f: &FilterRec, k: &FilterKeywordRec) -> Value {
    json!({
        "id": k.id.to_string(),
        "phrase": k.keyword,
        "context": contexts(f.context),
        "expires_at": f.expires_ms.map(iso),
        "irreversible": f.action == FilterAction::Hide,
        "whole_word": k.whole_word,
    })
}

fn match_json(m: &FilterMatch<'_>) -> Value {
    json!({
        "filter": filter_json(m.filter),
        "keyword_matches": (!m.keywords.is_empty()).then(|| m.keywords.clone()),
        "status_matches": (!m.statuses.is_empty())
            .then(|| m.statuses.iter().map(|id| id.to_string()).collect::<Vec<_>>()),
    })
}

fn annotate_status<S: Store>(c: &Call<'_, S>, viewer: u8, bit: u8, status: &mut Value) {
    let Some(id) = status["id"].as_str().and_then(crate::ids::parse) else {
        return;
    };
    if let Some(s) = c.svc.state.statuses.get(&id) {
        let matches: Vec<Value> = c
            .svc
            .filter_matches(viewer, bit, s)
            .iter()
            .map(match_json)
            .collect();
        status["filtered"] = Value::Array(matches);
    }
}

/// Fill `filtered` on rendered statuses (and boosted ones) for `context`.
/// Nothing is removed: clients warn, blur or hide, as on Mastodon.
pub(crate) fn annotate<S: Store>(
    c: &Call<'_, S>,
    viewer: u8,
    context: &str,
    mut rows: Vec<Value>,
) -> Vec<Value> {
    let Some(bit) = filter_context_bit(context) else {
        return rows;
    };
    if c.svc.filters_of(viewer).next().is_none() {
        return rows;
    }
    for row in &mut rows {
        if row["reblog"].is_object() {
            annotate_status(c, viewer, bit, &mut row["reblog"]);
            row["filtered"] = row["reblog"]["filtered"].clone();
        } else if row["status"].is_object() {
            // A notification.
            annotate_status(c, viewer, bit, &mut row["status"]);
        } else {
            annotate_status(c, viewer, bit, row);
        }
    }
    rows
}

fn context_param<S: Store>(c: &Call<'_, S>) -> Result<Option<u8>, Response> {
    let names = c.params.all("context");
    if names.is_empty() {
        return Ok(None);
    }
    names
        .iter()
        .try_fold(0u8, |bits, n| filter_context_bit(n).map(|b| bits | b))
        .map(Some)
        .ok_or_else(|| Response::error(422, "Validation failed: context is invalid"))
}

/// `expires_in` seconds: empty clears it; zero or less has already expired.
fn expires_param<S: Store>(c: &Call<'_, S>) -> Result<Option<Option<u64>>, Response> {
    match c.params.get("expires_in") {
        None => Ok(None),
        Some("") => Ok(Some(None)),
        Some(v) => {
            let secs: i64 = v
                .parse()
                .map_err(|_| Response::error(422, "Validation failed: expires_in is invalid"))?;
            Ok(Some(Some(
                (c.now as i64)
                    .saturating_add(secs.saturating_mul(1000))
                    .max(0) as u64,
            )))
        }
    }
}

fn action_param<S: Store>(c: &Call<'_, S>) -> Result<Option<FilterAction>, Response> {
    match c.params.text("filter_action") {
        None => Ok(None),
        Some(a) => FilterAction::parse(a)
            .map(Some)
            .ok_or_else(|| Response::error(422, "Validation failed: filter_action is invalid")),
    }
}

/// One keyword change: `id == 0` adds a keyword.
struct KeywordChange {
    id: u64,
    keyword: Option<String>,
    whole_word: Option<bool>,
    destroy: bool,
}

impl KeywordChange {
    fn new(id: u64, keyword: Option<String>, whole_word: Option<bool>) -> KeywordChange {
        KeywordChange {
            id,
            keyword,
            whole_word,
            destroy: false,
        }
    }
}

/// Fields sent for one keyword, and the index it was sent under.
type AttributeRow = (Option<String>, Vec<(String, String)>);

/// `keywords_attributes[i][field]` (JSON) or `keywords_attributes[][field]`
/// (forms).
fn keyword_attributes<S: Store>(c: &Call<'_, S>) -> Vec<KeywordChange> {
    let mut rows: Vec<AttributeRow> = Vec::new();
    for (name, value) in c.params.pairs() {
        let Some(rest) = name.strip_prefix("keywords_attributes[") else {
            continue;
        };
        let Some((index, field)) = rest.split_once("][") else {
            continue;
        };
        let field = field.trim_end_matches(']').to_string();
        let index = (!index.is_empty()).then(|| index.to_string());
        let start_new = match rows.last() {
            None => true,
            Some((last, fields)) => match &index {
                Some(i) => last.as_ref() != Some(i),
                None => fields.iter().any(|(f, _)| *f == field),
            },
        };
        if start_new {
            rows.push((index, Vec::new()));
        }
        if let Some(row) = rows.last_mut() {
            row.1.push((field, value.to_string()));
        }
    }
    rows.into_iter()
        .map(|(_, fields)| {
            let get = |name: &str| {
                fields
                    .iter()
                    .find(|(f, _)| f == name)
                    .map(|(_, v)| v.as_str())
            };
            let truthy = |v: &str| matches!(v, "true" | "1" | "on" | "yes");
            KeywordChange {
                id: get("id").and_then(crate::ids::parse).unwrap_or(0),
                keyword: get("keyword").map(str::to_string),
                whole_word: get("whole_word").map(truthy),
                destroy: get("_destroy").is_some_and(truthy),
            }
        })
        .collect()
}

fn apply_keywords(rec: &mut FilterRec, changes: Vec<KeywordChange>) {
    for KeywordChange {
        id,
        keyword,
        whole_word,
        destroy,
    } in changes
    {
        if id != 0 {
            if destroy {
                rec.keywords.retain(|k| k.id != id);
            } else if let Some(k) = rec.keywords.iter_mut().find(|k| k.id == id) {
                if let Some(text) = keyword {
                    k.keyword = text;
                }
                if let Some(w) = whole_word {
                    k.whole_word = w;
                }
            }
        } else if let Some(text) = keyword.filter(|_| !destroy) {
            rec.keywords.push(FilterKeywordRec {
                id: 0,
                keyword: text,
                whole_word: whole_word.unwrap_or(true),
            });
        }
    }
}

fn reply_filter<S: Store>(c: &Call<'_, S>, slot: u8, id: u64) -> Reply {
    let f = c.svc.filter(slot, id).map_err(fail)?;
    Ok(Response::ok(filter_json(f)))
}

fn create_v2<S: Store>(c: &mut Call<'_, S>) -> Reply {
    let slot = c.user_scoped("write:filters")?;
    let mut rec = FilterRec {
        id: 0,
        title: c.params.get("title").unwrap_or("").to_string(),
        context: context_param(c)?.unwrap_or(0),
        action: action_param(c)?.unwrap_or(FilterAction::Warn),
        expires_ms: expires_param(c)?.flatten(),
        keywords: Vec::new(),
        statuses: Vec::new(),
    };
    apply_keywords(&mut rec, keyword_attributes(c));
    let id = c.svc.save_filter(slot, rec, c.now).map_err(fail)?;
    reply_filter(c, slot, id)
}

fn update_v2<S: Store>(c: &mut Call<'_, S>, id: u64) -> Reply {
    let slot = c.user_scoped("write:filters")?;
    let mut rec = c.svc.filter(slot, id).map_err(fail)?.clone();
    if let Some(title) = c.params.get("title") {
        rec.title = title.to_string();
    }
    if let Some(context) = context_param(c)? {
        rec.context = context;
    }
    if let Some(action) = action_param(c)? {
        rec.action = action;
    }
    if let Some(expires) = expires_param(c)? {
        rec.expires_ms = expires;
    }
    apply_keywords(&mut rec, keyword_attributes(c));
    c.svc.save_filter(slot, rec, c.now).map_err(fail)?;
    reply_filter(c, slot, id)
}

/// A keyword or status entry, by its own id, with the filter that has it.
fn part<S: Store>(c: &Call<'_, S>, slot: u8, id: &str) -> Result<(u64, FilterRec), Response> {
    let id = c.id(id)?;
    let f = c.svc.filter_with_part(slot, id).map_err(fail)?.clone();
    Ok((id, f))
}

fn keyword_reply(f: &FilterRec, id: u64) -> Reply {
    f.keywords
        .iter()
        .find(|k| k.id == id)
        .map(|k| Response::ok(keyword_json(k)))
        .ok_or_else(|| fail(Error::NotFound))
}

fn v2<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    let id = |text: &str| crate::ids::parse(text).ok_or_else(|| fail(Error::NotFound));
    Some(match (method, seg) {
        ("GET", []) => c.user_scoped("read:filters").map(|slot| {
            Response::ok(Value::Array(
                c.svc.filters_of(slot).map(filter_json).collect(),
            ))
        }),
        ("POST", []) => create_v2(c),
        ("GET", ["keywords", k]) => c.user_scoped("read:filters").and_then(|slot| {
            let (k, f) = part(c, slot, k)?;
            keyword_reply(&f, k)
        }),
        ("PUT", ["keywords", k]) => c.user_scoped("write:filters").and_then(|slot| {
            let (k, mut f) = part(c, slot, k)?;
            let keyword = c.params.get("keyword").map(str::to_string);
            let whole_word = c.params.bool("whole_word");
            apply_keywords(&mut f, vec![KeywordChange::new(k, keyword, whole_word)]);
            c.svc.save_filter(slot, f.clone(), c.now).map_err(fail)?;
            keyword_reply(c.svc.filter(slot, f.id).map_err(fail)?, k)
        }),
        ("DELETE", ["keywords", k]) => c.user_scoped("write:filters").and_then(|slot| {
            let (k, mut f) = part(c, slot, k)?;
            f.keywords.retain(|kw| kw.id != k);
            c.svc.save_filter(slot, f, c.now).map_err(fail)?;
            Ok(Response::ok(json!({})))
        }),
        ("GET", ["statuses", s]) => c.user_scoped("read:filters").and_then(|slot| {
            let (s, f) = part(c, slot, s)?;
            f.statuses
                .iter()
                .find(|x| x.id == s)
                .map(|x| Response::ok(status_json(x)))
                .ok_or_else(|| fail(Error::NotFound))
        }),
        ("DELETE", ["statuses", s]) => c.user_scoped("write:filters").and_then(|slot| {
            let (s, mut f) = part(c, slot, s)?;
            f.statuses.retain(|x| x.id != s);
            c.svc.save_filter(slot, f, c.now).map_err(fail)?;
            Ok(Response::ok(json!({})))
        }),
        ("GET", [f]) => id(f).and_then(|f| {
            let slot = c.user_scoped("read:filters")?;
            reply_filter(c, slot, f)
        }),
        ("PUT", [f]) => id(f).and_then(|f| update_v2(c, f)),
        ("DELETE", [f]) => id(f).and_then(|f| {
            let slot = c.user_scoped("write:filters")?;
            c.svc.delete_filter(slot, f, c.now).map_err(fail)?;
            Ok(Response::ok(json!({})))
        }),
        ("GET", [f, "keywords"]) => id(f).and_then(|f| {
            let slot = c.user_scoped("read:filters")?;
            let f = c.svc.filter(slot, f).map_err(fail)?;
            Ok(Response::ok(Value::Array(
                f.keywords.iter().map(keyword_json).collect(),
            )))
        }),
        ("POST", [f, "keywords"]) => id(f).and_then(|f| {
            let slot = c.user_scoped("write:filters")?;
            let mut rec = c.svc.filter(slot, f).map_err(fail)?.clone();
            let before: Vec<u64> = rec.keywords.iter().map(|k| k.id).collect();
            let keyword = c.params.get("keyword").unwrap_or("").to_string();
            let whole_word = c.params.bool("whole_word");
            apply_keywords(
                &mut rec,
                vec![KeywordChange::new(0, Some(keyword), whole_word)],
            );
            c.svc.save_filter(slot, rec, c.now).map_err(fail)?;
            let rec = c.svc.filter(slot, f).map_err(fail)?;
            rec.keywords
                .iter()
                .find(|k| !before.contains(&k.id))
                .map(|k| Response::ok(keyword_json(k)))
                .ok_or_else(|| fail(Error::NotFound))
        }),
        ("GET", [f, "statuses"]) => id(f).and_then(|f| {
            let slot = c.user_scoped("read:filters")?;
            let f = c.svc.filter(slot, f).map_err(fail)?;
            Ok(Response::ok(Value::Array(
                f.statuses.iter().map(status_json).collect(),
            )))
        }),
        ("POST", [f, "statuses"]) => id(f).and_then(|f| {
            let slot = c.user_scoped("write:filters")?;
            let status_id = c
                .params
                .text("status_id")
                .and_then(crate::ids::parse)
                .filter(|s| c.svc.visible(Some(slot), *s).is_some())
                .ok_or_else(|| fail(Error::NotFound))?;
            let mut rec = c.svc.filter(slot, f).map_err(fail)?.clone();
            if !rec.statuses.iter().any(|s| s.status_id == status_id) {
                rec.statuses.push(FilterStatusRec { id: 0, status_id });
            }
            c.svc.save_filter(slot, rec, c.now).map_err(fail)?;
            let rec = c.svc.filter(slot, f).map_err(fail)?;
            rec.statuses
                .iter()
                .find(|s| s.status_id == status_id)
                .map(|s| Response::ok(status_json(s)))
                .ok_or_else(|| fail(Error::NotFound))
        }),
        _ => return None,
    })
}

/// v1: each keyword of each filter is one v1 filter, keyed by keyword id.
fn v1<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    Some(match (method, seg) {
        ("GET", []) => c.user_scoped("read:filters").map(|slot| {
            let rows: Vec<Value> = c
                .svc
                .filters_of(slot)
                .flat_map(|f| f.keywords.iter().map(move |k| v1_json(f, k)))
                .collect();
            Response::ok(Value::Array(rows))
        }),
        ("POST", []) => (|| {
            let slot = c.user_scoped("write:filters")?;
            let phrase = c.params.get("phrase").unwrap_or("").trim().to_string();
            let rec = FilterRec {
                id: 0,
                title: phrase.clone(),
                context: context_param(c)?.unwrap_or(0),
                action: if c.params.flag("irreversible") {
                    FilterAction::Hide
                } else {
                    FilterAction::Warn
                },
                expires_ms: expires_param(c)?.flatten(),
                keywords: vec![FilterKeywordRec {
                    id: 0,
                    keyword: phrase,
                    whole_word: c.params.bool("whole_word").unwrap_or(true),
                }],
                statuses: Vec::new(),
            };
            let id = c.svc.save_filter(slot, rec, c.now).map_err(fail)?;
            let f = c.svc.filter(slot, id).map_err(fail)?;
            Ok(Response::ok(v1_json(f, &f.keywords[0])))
        })(),
        ("GET", [k]) => c.user_scoped("read:filters").and_then(|slot| {
            let (k, f) = part(c, slot, k)?;
            let kw = f
                .keywords
                .iter()
                .find(|x| x.id == k)
                .ok_or_else(|| fail(Error::NotFound))?;
            Ok(Response::ok(v1_json(&f, kw)))
        }),
        ("PUT", [k]) => c.user_scoped("write:filters").and_then(|slot| {
            let (k, mut f) = part(c, slot, k)?;
            let phrase = c.params.get("phrase").map(|p| p.trim().to_string());
            apply_keywords(
                &mut f,
                vec![KeywordChange::new(k, phrase, c.params.bool("whole_word"))],
            );
            if let Some(context) = context_param(c)? {
                f.context = context;
            }
            if let Some(irreversible) = c.params.bool("irreversible") {
                f.action = if irreversible {
                    FilterAction::Hide
                } else {
                    FilterAction::Warn
                };
            }
            if let Some(expires) = expires_param(c)? {
                f.expires_ms = expires;
            }
            c.svc.save_filter(slot, f.clone(), c.now).map_err(fail)?;
            let f = c.svc.filter(slot, f.id).map_err(fail)?;
            let kw = f
                .keywords
                .iter()
                .find(|x| x.id == k)
                .ok_or_else(|| fail(Error::NotFound))?;
            Ok(Response::ok(v1_json(f, kw)))
        }),
        ("DELETE", [k]) => c.user_scoped("write:filters").and_then(|slot| {
            let (k, mut f) = part(c, slot, k)?;
            f.keywords.retain(|x| x.id != k);
            let result = if f.keywords.is_empty() && f.statuses.is_empty() {
                c.svc.delete_filter(slot, f.id, c.now)
            } else {
                c.svc.save_filter(slot, f, c.now).map(|_| ())
            };
            result.map_err(fail)?;
            Ok(Response::ok(json!({})))
        }),
        _ => return None,
    })
}

pub(crate) fn route<S: Store>(c: &mut Call<'_, S>, method: &str, seg: &[&str]) -> Option<Reply> {
    match seg {
        ["api", "v2", "filters", rest @ ..] => v2(c, method, rest),
        ["api", "v1", "filters", rest @ ..] => v1(c, method, rest),
        _ => None,
    }
}
