use crate::{
    store::{self, BLOB_BUDGET},
    Error, Result, Shared, MAX_PAYLOAD,
};
use serde_json::{json, Value};
use sha2::Digest;
use std::{fs::File, io::Read};
include!(concat!(env!("OUT_DIR"), "/assets.rs"));

fn display() -> Value {
    #[cfg(target_os = "espidf")]
    {
        use esp_idf_svc::sys;
        // SAFETY: driver getters return constants and a process-lifetime C string.
        unsafe {
            json!({
                "width": sys::minicloud_display_width(),
                "height": sys::minicloud_display_height(),
                "driver": std::ffi::CStr::from_ptr(sys::minicloud_display_driver()).to_string_lossy()
            })
        }
    }
    #[cfg(not(target_os = "espidf"))]
    {
        json!({"width":320,"height":172,"driver":"desktop-preview"})
    }
}

fn memory() -> Value {
    #[cfg(target_os = "espidf")]
    {
        use esp_idf_svc::sys;
        // SAFETY: read-only heap counters; no pointers or ownership transfer.
        unsafe {
            json!({"free":sys::heap_caps_get_free_size(sys::MALLOC_CAP_8BIT),"minimum_free":sys::heap_caps_get_minimum_free_size(sys::MALLOC_CAP_8BIT),"largest_block":sys::heap_caps_get_largest_free_block(sys::MALLOC_CAP_8BIT)})
        }
    }
    #[cfg(not(target_os = "espidf"))]
    {
        Value::Null
    }
}

pub enum Body {
    Bytes(Vec<u8>),
    File(File),
    Static(&'static [u8]),
}
pub struct Reply {
    pub status: u16,
    pub mime: String,
    pub length: u64,
    pub body: Body,
    pub headers: Vec<(String, String)>,
}
impl Reply {
    pub fn json(status: u16, value: Value) -> Self {
        let bytes = serde_json::to_vec(&value).unwrap();
        Self {
            status,
            mime: "application/json".into(),
            length: bytes.len() as u64,
            body: Body::Bytes(bytes),
            headers: Vec::new(),
        }
    }
    pub fn error(error: Error) -> Self {
        Self::json(error.status, json!({"error":error.message}))
    }
}
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub authorization: &'a str,
    pub content_type: &'a str,
    pub length: u64,
    pub etag: &'a str,
    pub gzip: bool,
}
fn read_json(body: &mut dyn Read, length: u64) -> Result<Value> {
    if length > MAX_PAYLOAD as u64 {
        return Err(Error::new(413, "JSON exceeds 2048 bytes"));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    body.take(length).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != length {
        return Err(Error::new(400, "truncated JSON body"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}
fn path_parts(path: &str) -> Result<Vec<String>> {
    path.split('?')
        .next()
        .unwrap_or("")
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            percent_encoding::percent_decode_str(s)
                .decode_utf8()
                .map(|s| s.into_owned())
                .map_err(|_| Error::new(400, "invalid URL UTF-8"))
        })
        .collect()
}
pub fn handle(shared: &Shared, request: &Request<'_>, body: &mut dyn Read) -> Result<Reply> {
    let parts = path_parts(request.path)?;
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    let method = if request.method == "HEAD" {
        "GET"
    } else {
        request.method
    };
    // The UI is miniframework's static site (`config`); a page request only
    // reaches here when no UI was bundled.
    if method == "GET" && parts.first() != Some(&"api") && parts.first() != Some(&"blobs") {
        return Err(if ASSETS.is_empty() {
            Error::new(503, "management UI not built; run make web")
        } else {
            Error::new(404, "not found")
        });
    }
    let public = method == "GET"
        && (parts == ["api", "status"]
            || parts == ["api", "screen"]
            || parts.first() == Some(&"blobs"))
        || method == "POST"
            && (parts == ["api", "screen", "next"]
                || parts == ["api", "screen", "notify"]
                || parts == ["api", "screen", "read"]
                || matches!(parts.as_slice(), ["api", "screen", _, _, "dismiss"]));
    let mut cloud = shared.lock().unwrap();
    cloud.expire()?;
    let bearer = request.authorization.strip_prefix("Bearer ").unwrap_or("");
    let admin = bearer == cloud.token;
    let actor = if !public && !admin {
        Some(crate::registry::authenticate(&cloud, bearer)?)
    } else {
        None
    };
    let delegated_route = matches!(
        parts.as_slice(),
        ["api", "blobs", ..] | ["api", "copyrights", ..]
    );
    if !public && !admin && !delegated_route {
        return Err(Error::new(403, "admin permission required"));
    }
    match (method, parts.as_slice()) {
        ("GET", ["api", "banks"]) => Ok(Reply::json(
            200,
            json!({"banks":cloud.state.banks.iter().map(|b| json!({"id":b.id,"origin":b.origin,"audience":b.audience})).collect::<Vec<_>>()}),
        )),
        ("PUT", ["api", "banks", id]) => {
            let bank: crate::registry::Bank =
                serde_json::from_value(read_json(body, request.length)?)?;
            bank.validate()?;
            if bank.id != *id {
                return Err(Error::new(400, "bank id differs from path"));
            }
            let mut next = cloud.state.clone();
            next.banks.retain(|b| b.id != *id);
            if next.banks.len() >= 8 {
                return Err(Error::new(429, "8 bank limit reached"));
            }
            next.banks.push(bank);
            cloud.commit(next)?;
            Ok(Reply::json(200, json!({"trusted":true})))
        }
        ("DELETE", ["api", "banks", id]) => {
            let mut next = cloud.state.clone();
            let count = next.banks.len();
            next.banks.retain(|b| b.id != *id);
            if next.banks.len() == count {
                return Err(Error::new(404, "bank not found"));
            }
            cloud.commit(next)?;
            Ok(Reply::json(200, json!({"deleted":true})))
        }
        ("GET", ["api", "storage"]) => Ok(Reply::json(
            200,
            json!({"card_available":cloud.store.card.as_ref().is_some_and(|p| p.is_dir()),"onboard_bytes":cloud.state.blobs.iter().filter(|b| b.tier == store::Tier::Onboard).map(|b| b.size).sum::<u64>(),"card_bytes":cloud.state.blobs.iter().filter(|b| b.tier == store::Tier::Card).map(|b| b.size).sum::<u64>()}),
        )),
        ("POST", ["api", "storage", "migrate"]) => {
            let value = read_json(body, request.length)?;
            let bucket = value["bucket"]
                .as_str()
                .ok_or_else(|| Error::new(400, "bucket required"))?;
            let key = value["key"]
                .as_str()
                .ok_or_else(|| Error::new(400, "key required"))?;
            let tier: store::Tier = serde_json::from_value(value["tier"].clone())?;
            let index = cloud
                .state
                .blobs
                .iter()
                .position(|b| b.bucket == bucket && b.key == key)
                .ok_or_else(|| Error::new(404, "file not found"))?;
            let blob = &cloud.state.blobs[index];
            if tier == store::Tier::Onboard
                && blob.tier != tier
                && cloud
                    .state
                    .blobs
                    .iter()
                    .filter(|b| b.tier == store::Tier::Onboard)
                    .map(|b| b.size)
                    .sum::<u64>()
                    + blob.size
                    > BLOB_BUDGET
            {
                return Err(Error::new(413, "onboard storage limit reached"));
            }
            let migrated = cloud.store.migrate(blob, tier)?;
            let mut next = cloud.state.clone();
            next.blobs[index] = migrated.clone();
            cloud.commit(next)?;
            // Both recovery slots now reference the verified destination before reclaiming the source.
            let next = cloud.state.clone();
            cloud.commit(next)?;
            cloud.store.collect(&cloud.state)?;
            Ok(Reply::json(200, json!({"blob":migrated})))
        }
        ("GET", ["api", "copyrights"]) => {
            let q = crate::registry::query(request.path);
            let records: Vec<_> = cloud
                .state
                .copyrights
                .iter()
                .filter(|c| {
                    format!(
                        "{} {} {} {} {}",
                        c.bucket, c.key, c.owner, c.title, c.license
                    )
                    .to_lowercase()
                    .contains(&q)
                })
                .collect();
            Ok(Reply::json(200, json!({"copyrights":records})))
        }
        ("GET", ["api", "copyrights", bucket, key @ ..]) if !key.is_empty() => {
            let key = key.join("/");
            let record = cloud
                .state
                .copyrights
                .iter()
                .find(|c| c.bucket == *bucket && c.key == key)
                .ok_or_else(|| Error::new(404, "copyright not found"))?;
            Ok(Reply::json(200, json!({"copyright":record})))
        }
        ("PUT" | "DELETE", ["api", "copyrights", bucket, key @ ..]) if !key.is_empty() => {
            let key = key.join("/");
            store::object_name(bucket, &key)?;
            let value = read_json(body, request.length)?;
            let existing = cloud
                .state
                .copyrights
                .iter()
                .find(|c| c.bucket == *bucket && c.key == key);
            if let Some(user) = &actor {
                let authorized = existing.map_or_else(
                    || {
                        cloud.state.blobs.iter().any(|b| {
                            b.bucket == *bucket && b.key == key && b.uploader.as_ref() == Some(user)
                        })
                    },
                    |c| &c.owner == user,
                );
                if !authorized {
                    return Err(Error::new(
                        403,
                        "file or copyright owner permission required",
                    ));
                }
            }
            let revision = value["revision"]
                .as_u64()
                .ok_or_else(|| Error::new(400, "revision required"))?;
            if revision != existing.map_or(0, |c| c.revision) {
                return Err(Error::new(409, "copyright revision changed"));
            }
            if method == "DELETE" && existing.is_none() {
                return Err(Error::new(404, "copyright not found"));
            }
            let mut next = cloud.state.clone();
            next.copyrights
                .retain(|c| c.bucket != *bucket || c.key != key);
            if method == "PUT" {
                let mut record: crate::registry::Copyright = serde_json::from_value(value)?;
                if record.bucket != *bucket || record.key != key {
                    return Err(Error::new(400, "file differs from path"));
                }
                record.validate(&cloud)?;
                if next.copyrights.iter().any(|c| c.hash == record.hash) {
                    return Err(Error::new(
                        409,
                        "file content is already registered under another file key",
                    ));
                }
                if existing.is_none() && actor.as_ref().is_some_and(|a| *a != record.owner) {
                    return Err(Error::new(403, "initial owner must be authenticated user"));
                }
                if next.copyrights.len() >= 48 {
                    return Err(Error::new(429, "48 copyright limit reached"));
                }
                record.revision = cloud
                    .state
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| Error::new(409, "revision exhausted"))?;
                next.copyrights.push(record.clone());
                cloud.commit(next)?;
                Ok(Reply::json(200, json!({"copyright":record})))
            } else {
                cloud.commit(next)?;
                Ok(Reply::json(200, json!({"deleted":true})))
            }
        }
        ("GET", ["api", "status"]) => Ok(Reply::json(
            200,
            json!({
                "service":"minicloud", "revision":cloud.state.revision, "plugins":cloud.plugins.iter().map(|p| p.name()).collect::<Vec<_>>(),
                "limits":{"blob_bytes":store::MAX_BLOB,"blob_budget":BLOB_BUDGET,"queue_jobs":crate::MAX_JOBS,"mqtt_clients":4,"mqtt_payload":MAX_PAYLOAD,"screen_notices":crate::MAX_NOTICES},
                "sqlite":cfg!(feature="sqlite"), "display":display(), "memory":memory(), "clock":{"unix_seconds":cloud.now(),"synchronized":cloud.now() >= 1_700_000_000}, "queued":cloud.state.jobs.len(),"storage_bytes":cloud.state.blobs.iter().map(|b| b.size).sum::<u64>()
            }),
        )),
        ("GET", ["api", "screen"]) => {
            let current = cloud.current();
            let pages = current
                .as_ref()
                .map(|n| crate::layout::pages(&n.text, &n.size))
                .unwrap_or_default();
            let page = cloud.screen_page().min(pages.len().saturating_sub(1));
            let page_text = current
                .as_ref()
                .and_then(|n| {
                    pages.get(page).map(|text| {
                        let columns = 318 / (8 * crate::layout::scale(&n.size));
                        text.as_bytes()
                            .chunks(columns)
                            .map(|line| std::str::from_utf8(line).unwrap().trim_end())
                            .collect::<Vec<_>>()
                            .join(
                                "
",
                            )
                    })
                })
                .unwrap_or_default();
            Ok(Reply::json(
                200,
                json!({"width":320,"height":172,"rotation_seconds":8,"current":current,"page":page+1,"pages":pages.len(),"page_text":page_text,"notices":cloud.state.notices,"revision":cloud.state.revision}),
            ))
        }
        ("POST", ["api", "screen", "next"]) => {
            cloud.advance();
            Ok(Reply::json(200, json!({"advanced":true})))
        }
        ("POST", ["api", "screen", source, id, "dismiss"]) => {
            cloud.dismiss(source, id)?;
            Ok(Reply::json(200, json!({"dismissed":true})))
        }
        ("GET", ["api", "blobs"]) => Ok(Reply::json(
            200,
            json!({"blobs":cloud.state.blobs.iter().filter(|b| format!("{} {} {} {}",b.bucket,b.key,b.mime,b.hash).to_lowercase().contains(&crate::registry::query(request.path))).collect::<Vec<_>>()}),
        )),
        ("GET", ["api", "jobs"]) => Ok(Reply::json(
            200,
            json!({"queued":cloud.state.jobs,"receipts":cloud.state.receipts}),
        )),
        ("POST", ["api", "publish"]) => {
            let value = read_json(body, request.length)?;
            let topic = value["topic"]
                .as_str()
                .ok_or_else(|| Error::new(400, "topic required"))?;
            if !mqttbytes::valid_topic(topic)
                || topic.len() > 128
                || matches!(topic, "minicloud/jobs/result" | "minicloud/screen/changed")
            {
                return Err(Error::new(400, "invalid/reserved topic"));
            }
            let payload = value
                .get("payload")
                .cloned()
                .ok_or_else(|| Error::new(400, "payload required"))?;
            let job = if cloud.plugins.iter().any(|p| p.accepts(topic)) {
                Some(cloud.enqueue(topic, payload.clone(), None)?)
            } else {
                None
            };
            cloud.publish(topic, payload)?;
            Ok(Reply::json(202, json!({"job_id":job,"published":true})))
        }
        ("POST", ["api", "plugins", plugin, "invoke"]) => {
            let value = read_json(body, request.length)?;
            let topic = value["topic"].as_str().unwrap_or("invoke");
            let payload = value
                .get("payload")
                .cloned()
                .ok_or_else(|| Error::new(400, "payload required"))?;
            let id = cloud.enqueue(topic, payload, Some(plugin))?;
            Ok(Reply::json(202, json!({"job_id":id})))
        }
        ("POST", ["api", "screen", action @ ("notify" | "read")]) => {
            let value = read_json(body, request.length)?;
            let id = cloud.enqueue(&format!("minicloud/screen/{action}"), value, None)?;
            Ok(Reply::json(202, json!({"job_id":id})))
        }
        ("PUT", ["api", "blobs", bucket, key @ ..]) if !key.is_empty() => {
            let key = key.join("/");
            store::object_name(bucket, &key)?;
            let old = cloud
                .state
                .blobs
                .iter()
                .find(|b| b.bucket == *bucket && b.key == key);
            if cloud
                .state
                .copyrights
                .iter()
                .any(|c| c.bucket == *bucket && c.key == key)
            {
                return Err(Error::new(
                    409,
                    "registered files are immutable; remove copyright first",
                ));
            }
            if let Some(user) = &actor {
                let namespace = format!("{:x}", sha2::Sha256::digest(user.as_bytes()));
                if *bucket != "art"
                    || !key.starts_with(&format!("{}/", &namespace[..16]))
                    || old.is_some_and(|b| b.uploader.as_ref() != Some(user))
                {
                    return Err(Error::new(403, "upload only to your art namespace"));
                }
            }
            let used = cloud
                .state
                .blobs
                .iter()
                .filter(|b| b.tier == store::Tier::Onboard)
                .map(|b| b.size)
                .sum::<u64>()
                - old
                    .filter(|b| b.tier == store::Tier::Onboard)
                    .map_or(0, |b| b.size);
            if request.length > store::MAX_BLOB || used + request.length > BLOB_BUDGET {
                return Err(Error::new(413, "blob storage limit reached"));
            }
            if old.is_none() && cloud.state.blobs.len() == 48 {
                return Err(Error::new(429, "48 object limit reached"));
            }
            cloud.store.collect(&cloud.state)?;
            let mut blob =
                cloud
                    .store
                    .upload(bucket, &key, request.content_type, body, request.length)?;
            blob.uploader = actor.clone();
            let mut next = cloud.state.clone();
            next.blobs.retain(|b| b.bucket != *bucket || b.key != key);
            next.blobs.push(blob.clone());
            cloud.commit(next)?;
            Ok(Reply::json(201, json!({"blob":blob,"url":blob.url()})))
        }
        ("GET", ["blobs", bucket, key @ ..]) if !key.is_empty() => {
            let key = key.join("/");
            store::object_name(bucket, &key)?;
            let blob = cloud
                .state
                .blobs
                .iter()
                .find(|b| b.bucket == *bucket && b.key == key)
                .ok_or_else(|| Error::new(404, "blob not found"))?;
            let etag = format!("\"{}\"", blob.hash);
            if request.etag == etag {
                return Ok(Reply {
                    status: 304,
                    mime: blob.mime.clone(),
                    length: 0,
                    body: Body::Bytes(vec![]),
                    headers: vec![("ETag".into(), etag)],
                });
            }
            Ok(Reply {
                status: 200,
                mime: blob.mime.clone(),
                length: blob.size,
                body: Body::File(cloud.store.open_blob(blob)?),
                headers: vec![
                    ("ETag".into(), etag),
                    (
                        "Content-Disposition".into(),
                        format!("inline; filename=\"{}\"", key.rsplit('/').next().unwrap()),
                    ),
                    (
                        "Content-Security-Policy".into(),
                        "sandbox; default-src 'none'".into(),
                    ),
                ],
            })
        }
        ("DELETE", ["api", "blobs", bucket, key @ ..]) if !key.is_empty() => {
            let key = key.join("/");
            store::object_name(bucket, &key)?;
            if cloud
                .state
                .copyrights
                .iter()
                .any(|c| c.bucket == *bucket && c.key == key)
            {
                return Err(Error::new(409, "remove copyright before deleting file"));
            }
            if actor.as_ref().is_some_and(|user| {
                !cloud.state.blobs.iter().any(|b| {
                    b.bucket == *bucket && b.key == key && b.uploader.as_ref() == Some(user)
                })
            }) {
                return Err(Error::new(403, "file owner permission required"));
            }
            if cloud.state.notices.iter().any(|n| {
                n.image
                    .as_ref()
                    .is_some_and(|i| i.bucket == *bucket && i.key == key)
            }) {
                return Err(Error::new(409, "dismiss notices using this image first"));
            }
            let mut next = cloud.state.clone();
            let previous = next.blobs.len();
            next.blobs.retain(|b| b.bucket != *bucket || b.key != key);
            if previous == next.blobs.len() {
                return Err(Error::new(404, "blob not found"));
            }
            cloud.commit(next)?;
            cloud.store.collect(&cloud.state)?;
            Ok(Reply::json(200, json!({"deleted":true})))
        }
        #[cfg(feature = "sqlite")]
        ("POST", ["api", "databases", database, "query"]) => {
            let value = read_json(body, request.length)?;
            Ok(Reply::json(
                200,
                crate::sql::query(&cloud.store, database, &value)?,
            ))
        }
        _ => Err(Error::new(404, "endpoint not found")),
    }
}

/// Minicloud as a miniframework app. The framework owns the connection loop,
/// keep-alive, the static UI, CORS and `/metrics`; this adapter passes API and
/// blob requests to [`handle`] unchanged. Blob uploads stream from the socket
/// into storage and downloads stream from storage to the socket, 4 KiB at a
/// time, so neither needs RAM for the whole file.
pub struct Minicloud {
    pub shared: Shared,
}

impl miniframework::Service for Minicloud {
    fn handle(&self, req: &miniframework::Request<'_>, reply: &mut miniframework::Reply<'_>) {
        let request = Request {
            method: req.method,
            path: req.uri,
            authorization: req.header("Authorization"),
            content_type: req.header("Content-Type"),
            length: req.length as u64,
            etag: req.header("If-None-Match"),
            gzip: miniframework::web::accepts_gzip(req.header("Accept-Encoding")),
        };
        let mut body = req.body_reader();
        let answer = handle(&self.shared, &request, &mut body).unwrap_or_else(Reply::error);
        let Reply {
            status,
            mime,
            length,
            body,
            headers,
        } = answer;
        match body {
            Body::File(file) => reply.stream(status, mime, length, Box::new(file)),
            Body::Static(bytes) => reply.bytes(status, mime, bytes),
            Body::Bytes(bytes) if bytes.len() > reply.limit() => {
                let error = Reply::error(Error::new(507, "response exceeds this board's limit"));
                if let Body::Bytes(text) = error.body {
                    reply.bytes(507, error.mime, &text);
                }
            }
            Body::Bytes(bytes) => reply.bytes(status, mime, &bytes),
        }
        // Blobs revalidate with their content hash (ETag); nothing is stored
        // without asking. Active content cannot be sniffed into something else.
        reply.header("Cache-Control", "no-cache");
        reply.header("X-Content-Type-Options", "nosniff");
        for (name, value) in headers {
            reply.header(name, value);
        }
    }

    fn streamed_body(&self, method: &str, path: &str) -> Option<usize> {
        // A little over the blob cap, so `handle` answers an oversized upload
        // with its own 413 while the framework drains the rest of the body
        // (refusing at the head would leave the client mid-send, and closing
        // on unread data resets the connection before it reads the error).
        (method == "PUT" && path.starts_with("/api/blobs/"))
            .then_some(store::MAX_BLOB as usize + 64 * 1024)
    }

    fn origin_allowed(&self, origin: &str) -> bool {
        self.shared
            .lock()
            .is_ok_and(|cloud| cloud.state.banks.iter().any(|b| b.origin == origin))
    }
}

/// Site settings shared by the desktop and the C6. `host` is
/// `minicloud.local` on the board, the listening address on a PC.
pub fn config(host: &str, target: &'static str) -> miniframework::Config {
    let mut config = miniframework::Config::new("minicloud", host);
    config.version = env!("CARGO_PKG_VERSION");
    // JSON bodies (plugin commands, copyright records) are capped at 2 KiB;
    // blob uploads are streamed, not buffered.
    config.body_limit = 4096;
    config.assets = ASSETS;
    config.spa = miniframework::web::Spa::Routes(&["", "/admin", "/screen"]);
    config.app_paths = &["/blobs"];
    config.cors_methods = "GET, PUT, DELETE, OPTIONS";
    config.cors_headers = "Authorization, Content-Type";
    config.influx_tags = vec![("target", target.into())];
    // CORS is for registered banks only (`Minicloud::origin_allowed`).
    config.origins.clear();
    config
}
