//! Minicloud through miniframework's request pipeline (the same `Site` the
//! desktop and the C6 run), without sockets.
use minicloud::{
    http::{self, Minicloud},
    Cloud,
};
use miniframework::{desktop::DesktopPlatform, http as wire, site::Scratch, Site};
use std::sync::{Arc, Mutex};

const TOKEN: &str = "test-admin-token-12345678";

struct Wire {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Wire {
    fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map_or("", |(_, v)| v.as_str())
    }
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

fn site(dir: &std::path::Path) -> Site<Minicloud> {
    let shared = Arc::new(Mutex::new(Cloud::open(dir, TOKEN.into()).unwrap()));
    Site::new(
        http::config("minicloud.local", "desktop"),
        Minicloud { shared },
        DesktopPlatform,
    )
}

fn render(mut response: wire::Response) -> Wire {
    let mut out = Vec::new();
    while !response.next().is_empty() {
        let n = response.next().len();
        out.extend_from_slice(response.next());
        response.advance(n);
    }
    assert!(!response.failed());
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let head = std::str::from_utf8(&out[..split]).unwrap();
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Wire {
        status,
        headers,
        body: out[split..].to_vec(),
    }
}

/// Sends a request; a body larger than the buffer limit is streamed, as
/// the connection loop would.
fn send(site: &Site<Minicloud>, method: &str, path: &str, extra: &str, body: &[u8]) -> Wire {
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: minicloud.local\r\n{extra}Content-Length: {}\r\n\r\n",
        body.len()
    );
    let stream = |m: &str, u: &str| site.stream_limit(m, u);
    let mut raw = head.into_bytes();
    let request = match wire::parse_streaming(&raw, site.config.body_limit, &stream).unwrap() {
        Some(request) if request.streamed.is_some() => request,
        _ => {
            raw.extend_from_slice(body);
            wire::parse(&raw, site.config.body_limit).unwrap().unwrap()
        }
    };
    let mut scratch = Scratch::new(site.config.response_limit);
    if request.streamed.is_some() {
        let mut source = body;
        render(site.respond_with(&request, false, &mut scratch, Some(&mut source)))
    } else {
        render(site.respond(&request, false, &mut scratch))
    }
}

fn admin() -> String {
    format!("Authorization: Bearer {TOKEN}\r\n")
}

#[test]
fn metrics_are_public_and_tagged() {
    let dir = tempfile::tempdir().unwrap();
    let site = site(dir.path());
    let revision = site.service.shared.lock().unwrap().state.revision;
    let w = send(&site, "GET", "/metrics", "", b"");
    assert_eq!(w.status, 200);
    assert!(w.header("Content-Type").starts_with("text/plain"));
    let line = String::from_utf8(w.body).unwrap();
    assert!(
        line.starts_with("board,host=minicloud.local,app=minicloud,target=desktop "),
        "{line}"
    );
    assert!(line.contains(" uptime_s=") && !line.contains(TOKEN));
    assert_eq!(site.service.shared.lock().unwrap().state.revision, revision);
}

#[test]
fn a_large_upload_streams_in_and_streams_back_out() {
    let dir = tempfile::tempdir().unwrap();
    let site = site(dir.path());
    let file: Vec<u8> = (0..200_000u32).map(|i| (i % 253) as u8).collect();
    let extra = format!("{}Content-Type: image/jpeg\r\n", admin());
    let up = send(&site, "PUT", "/api/blobs/photos/big.jpg", &extra, &file);
    assert_eq!(up.status, 201, "{}", String::from_utf8_lossy(&up.body));
    let url = up.json()["url"].as_str().unwrap().to_string();
    let down = send(&site, "GET", &url, "", b"");
    assert_eq!(down.status, 200);
    assert_eq!(down.header("Content-Type"), "image/jpeg");
    assert_eq!(down.header("Content-Length"), "200000");
    assert_eq!(down.header("Cache-Control"), "no-cache");
    assert_eq!(down.header("X-Content-Type-Options"), "nosniff");
    assert_eq!(
        down.header("Content-Security-Policy"),
        "sandbox; default-src 'none'"
    );
    assert!(down.body == file, "byte-for-byte");
    let etag = down.header("ETag").to_string();
    let cached = send(
        &site,
        "GET",
        &url,
        &format!("If-None-Match: {etag}\r\n"),
        b"",
    );
    assert_eq!(cached.status, 304);
    let head = send(&site, "HEAD", &url, "", b"");
    assert_eq!((head.status, head.body.len()), (200, 0));
    assert_eq!(head.header("Content-Length"), "200000");
}

#[test]
fn an_oversized_upload_gets_minicloud_s_own_error() {
    let dir = tempfile::tempdir().unwrap();
    let site = site(dir.path());
    let too_big = vec![0u8; 256 * 1024 + 1];
    let w = send(&site, "PUT", "/api/blobs/photos/x.bin", &admin(), &too_big);
    assert_eq!(w.status, 413);
    assert_eq!(w.json()["error"], "blob storage limit reached");
}

#[test]
fn cors_is_for_registered_banks_only() {
    let dir = tempfile::tempdir().unwrap();
    let site = site(dir.path());
    let origin = "Origin: https://nanacoin.local\r\n";
    assert_eq!(send(&site, "GET", "/api/status", origin, b"").status, 403);
    let bank = br#"{"id":"s3","secret":"0123456789abcdef0123456789abcdef","origin":"https://nanacoin.local","audience":"https://minicloud.local"}"#;
    let w = send(
        &site,
        "PUT",
        "/api/banks/s3",
        &format!("{}Content-Type: application/json\r\n", admin()),
        bank,
    );
    assert_eq!(w.status, 200, "{}", String::from_utf8_lossy(&w.body));
    let w = send(&site, "GET", "/api/status", origin, b"");
    assert_eq!(w.status, 200);
    assert_eq!(
        w.header("Access-Control-Allow-Origin"),
        "https://nanacoin.local"
    );
    assert_eq!(
        w.header("Access-Control-Allow-Methods"),
        "GET, PUT, DELETE, OPTIONS"
    );
    let preflight = send(&site, "OPTIONS", "/api/blobs/art/x", origin, b"");
    assert_eq!(preflight.status, 204);
    // No Origin (server-to-server, curl): served without CORS headers.
    let plain = send(&site, "GET", "/api/status", "", b"");
    assert_eq!(
        (plain.status, plain.header("Access-Control-Allow-Origin")),
        (200, "")
    );
}

#[test]
fn the_ui_routes_are_the_static_site() {
    let dir = tempfile::tempdir().unwrap();
    let site = site(dir.path());
    if minicloud::http::ASSETS.is_empty() {
        // Without a bundle, pages explain how to build it.
        assert_eq!(send(&site, "GET", "/", "", b"").status, 503);
        return;
    }
    for path in ["/", "/admin", "/screen"] {
        let w = send(&site, "GET", path, "Accept-Encoding: gzip\r\n", b"");
        assert_eq!(w.status, 200, "{path}");
        assert_eq!(w.header("Content-Encoding"), "gzip");
    }
    let asset = minicloud::http::ASSETS
        .iter()
        .find(|a| a.path.ends_with(".js"))
        .unwrap();
    let w = send(&site, "GET", asset.path, "Accept-Encoding: gzip\r\n", b"");
    assert_eq!(
        w.header("Cache-Control"),
        "public, max-age=31536000, immutable"
    );
    assert_eq!(send(&site, "GET", "/nothing-here", "", b"").status, 404);
}
