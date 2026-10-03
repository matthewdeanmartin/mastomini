use crate::{
    store::{self, BLOB_BUDGET},
    Error, Result, Shared, MAX_PAYLOAD,
};
use serde_json::{json, Value};
use std::{
    fs::File,
    io::{Cursor, Read, Write},
    net::{TcpListener, TcpStream},
    time::{Duration, Instant},
};
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
    if method == "GET" && parts.first() != Some(&"api") && parts.first() != Some(&"blobs") {
        let asset = if parts.is_empty() || parts == ["admin"] || parts == ["screen"] {
            "index.html"
        } else {
            parts[0]
        };
        if parts.len() <= 1 {
            if let Some((_, bytes)) = ASSETS.iter().find(|(name, _)| *name == asset) {
                if !request.gzip {
                    return Err(Error::new(406, "UI assets require Accept-Encoding: gzip"));
                }
                let mime = if asset.ends_with(".js") {
                    "text/javascript"
                } else if asset.ends_with(".css") {
                    "text/css"
                } else {
                    "text/html"
                };
                return Ok(Reply {
                    status: 200,
                    mime: mime.into(),
                    length: bytes.len() as u64,
                    body: Body::Static(bytes),
                    headers: vec![
                        ("Content-Encoding".into(), "gzip".into()),
                        ("Vary".into(), "Accept-Encoding".into()),
                    ],
                });
            }
        }
        return Err(Error::new(503, "management UI not built; run make web"));
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
    if !public && request.authorization.strip_prefix("Bearer ") != Some(cloud.token.as_str()) {
        return Err(Error::new(401, "admin bearer token required"));
    }
    match (method, parts.as_slice()) {
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
        ("GET", ["api", "blobs"]) => Ok(Reply::json(200, json!({"blobs":cloud.state.blobs}))),
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
            let used =
                cloud.state.blobs.iter().map(|b| b.size).sum::<u64>() - old.map_or(0, |b| b.size);
            if request.length > store::MAX_BLOB || used + request.length > BLOB_BUDGET {
                return Err(Error::new(413, "blob storage limit reached"));
            }
            if old.is_none() && cloud.state.blobs.len() == 48 {
                return Err(Error::new(429, "48 object limit reached"));
            }
            cloud.store.collect(&cloud.state)?;
            let blob =
                cloud
                    .store
                    .upload(bucket, &key, request.content_type, body, request.length)?;
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

/// One HTTP socket at a time with deadlines and a fixed header buffer. File
/// bodies are copied in 4 KiB chunks. The same transport runs on ESP-IDF std.
pub fn serve(listener: TcpListener, shared: Shared) -> std::io::Result<()> {
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                if let Err(error) = connection(&mut stream, &shared) {
                    eprintln!("HTTP: {error}");
                }
            }
            Err(error) => eprintln!("HTTP accept: {error}"),
        }
    }
    Ok(())
}
fn connection(stream: &mut TcpStream, shared: &Shared) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let mut timed = Timed {
        socket: stream,
        started: Instant::now(),
    };
    let stream = &mut timed;
    let mut buffer = [0; 4096];
    let mut used = 0;
    let parsed = loop {
        let mut headers = [httparse::EMPTY_HEADER; 24];
        let mut parsed = httparse::Request::new(&mut headers);
        match parsed.parse(&buffer[..used]) {
            Ok(httparse::Status::Complete(offset)) => {
                let get = |name: &str| -> String {
                    parsed
                        .headers
                        .iter()
                        .find(|h| h.name.eq_ignore_ascii_case(name))
                        .and_then(|h| std::str::from_utf8(h.value).ok())
                        .unwrap_or("")
                        .to_owned()
                };
                let mut invalid = None;
                if !get("transfer-encoding").is_empty() {
                    invalid = Some(Error::new(
                        501,
                        "chunked requests unsupported; supply Content-Length",
                    ));
                }
                if parsed
                    .headers
                    .iter()
                    .filter(|h| h.name.eq_ignore_ascii_case("content-length"))
                    .count()
                    > 1
                {
                    invalid = Some(Error::new(400, "duplicate Content-Length"));
                }
                let method = parsed.method.unwrap_or("").to_owned();
                let len_text = get("content-length");
                let length = if len_text.is_empty() {
                    if method == "PUT" || method == "POST" && get("content-type").contains("json") {
                        invalid = Some(Error::new(411, "Content-Length required"));
                    }
                    0
                } else {
                    match len_text.parse::<u64>() {
                        Ok(n) => n,
                        Err(_) => {
                            invalid = Some(Error::new(400, "invalid Content-Length"));
                            0
                        }
                    }
                };
                break (
                    offset,
                    method,
                    parsed.path.unwrap_or("/").to_owned(),
                    get("authorization"),
                    get("content-type"),
                    length,
                    get("if-none-match"),
                    get("accept-encoding")
                        .split(',')
                        .any(|s| s.trim() == "gzip"),
                    invalid,
                );
            }
            Ok(httparse::Status::Partial) if used < buffer.len() => {}
            _ => {
                return send(
                    stream,
                    Reply::error(Error::new(400, "invalid/oversized HTTP headers")),
                    false,
                )
            }
        }
        let n = stream.read(&mut buffer[used..])?;
        if n == 0 {
            return Ok(());
        }
        used += n;
    };
    let (offset, method, path, authorization, content_type, length, etag, gzip, invalid) = parsed;
    let reply = if let Some(error) = invalid {
        Reply::error(error)
    } else {
        let mut body = Cursor::new(&buffer[offset..used])
            .chain(&mut *stream)
            .take(length);
        let reply = handle(
            shared,
            &Request {
                method: &method,
                path: &path,
                authorization: &authorization,
                content_type: &content_type,
                length,
                etag: &etag,
                gzip,
            },
            &mut body,
        )
        .unwrap_or_else(Reply::error);
        // Drain bounded rejected bodies before close, otherwise Windows sends
        // a TCP reset that can discard the JSON error response.
        if length <= store::MAX_BLOB + 4096 {
            let mut discard = [0; 1024];
            while body.read(&mut discard).unwrap_or(0) != 0 {}
        }
        reply
    };
    send(stream, reply, method == "HEAD")
}
struct Timed<'a> {
    socket: &'a mut TcpStream,
    started: Instant,
}
impl Timed<'_> {
    fn remaining(&self) -> std::io::Result<Duration> {
        Duration::from_secs(20)
            .checked_sub(self.started.elapsed())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| std::io::ErrorKind::TimedOut.into())
    }
}
impl Read for Timed<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.socket
            .set_read_timeout(Some(self.remaining()?.min(Duration::from_secs(10))))?;
        self.socket.read(bytes)
    }
}
impl Write for Timed<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.socket
            .set_write_timeout(Some(self.remaining()?.min(Duration::from_secs(10))))?;
        self.socket.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.socket.flush()
    }
}
fn send(stream: &mut impl Write, reply: Reply, head: bool) -> std::io::Result<()> {
    write!(stream,"HTTP/1.1 {} Response\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-cache\r\nX-Content-Type-Options: nosniff\r\n",reply.status,reply.mime,reply.length)?;
    for (key, value) in reply.headers {
        write!(stream, "{key}: {value}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    if !head {
        match reply.body {
            Body::Bytes(bytes) => stream.write_all(&bytes)?,
            Body::Static(bytes) => stream.write_all(bytes)?,
            Body::File(mut file) => {
                let mut bytes = [0; 4096];
                loop {
                    let n = file.read(&mut bytes)?;
                    if n == 0 {
                        break;
                    }
                    stream.write_all(&bytes[..n])?;
                }
            }
        }
    }
    stream.flush()
}
