//! Exercise the real framework routing/framing boundary, including auth and caching.
use crate::{
    api::Ctx,
    http::BODY_LIMIT,
    server::{self, Bots},
    service::Service,
    store::MemStore,
};
use miniframework::{desktop::DesktopPlatform, http, site::Scratch, Site};
use std::sync::{Arc, Mutex};

#[test]
fn calibrated_heap_accepts_observed_idle_but_keeps_pressure_and_failures_visible() {
    let config = server::config("bots.local", "test");
    let mut info = miniframework::sys::SysInfo::default();
    info.heap.internal_total = 320 * 1024;
    for (free, largest, failures, heap, block, allocations) in [
        (14183, 6912, 0, "ok", "ok", "ok"),
        (6000, 3072, 1, "warn", "warn", "warn"),
        (3072, 3072, 1, "fail", "warn", "warn"),
    ] {
        info.heap.internal_free = free;
        info.heap.internal_largest = largest;
        let checks = miniframework::ops::builtin(&info, &config.ops, failures);
        for (id, expected) in [
            ("heap.internal", heap),
            ("heap.largest", block),
            ("heap.allocations", allocations),
        ] {
            assert_eq!(checks.iter().find(|c| c.id == id).unwrap().status, expected);
        }
    }
}

fn site() -> Site<Bots<MemStore>> {
    let service = Service::open(MemStore::default(), crate::bots::all()).unwrap();
    Site::new(
        server::config("bots.local", "test"),
        Bots::new(
            Arc::new(Mutex::new(service)),
            Arc::new(Ctx {
                platform: crate::api::Platform::default,
                tls: None,
            }),
            || Some(1_800_000_000_000),
            || 1000,
        ),
        DesktopPlatform,
    )
}

fn call(
    site: &Site<Bots<MemStore>>,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: &[u8],
    secure: bool,
) -> String {
    let request = http::Request {
        method: method.into(),
        uri: uri.into(),
        headers: headers
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect(),
        body: body.into(),
        consumed: 0,
        close: false,
        streamed: None,
    };
    let mut scratch = Scratch::new(65536);
    let mut response = site.respond(&request, secure, &mut scratch);
    let mut bytes = Vec::new();
    while !response.next().is_empty() {
        let length = response.next().len();
        bytes.extend_from_slice(response.next());
        response.advance(length);
    }
    String::from_utf8(bytes).unwrap()
}

fn body(response: &str) -> serde_json::Value {
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[test]
fn framework_preserves_sessions_secure_flag_and_private_cache_policy() {
    let site = site();
    let plain = call(&site, "GET", "/api/v1/status", &[], &[], false);
    assert_eq!(body(&plain)["secure"], false);
    let secure = call(&site, "GET", "/api/v1/status", &[], &[], true);
    assert_eq!(body(&secure)["secure"], true);
    let setup = call(
        &site,
        "POST",
        "/api/v1/setup",
        &[("Content-Type", "application/json")],
        br#"{"password":"test password"}"#,
        true,
    );
    assert!(setup.starts_with("HTTP/1.1 200"), "{setup}");
    let token = body(&setup)["token"].as_str().unwrap().to_string();
    let authorized = format!("Bearer {token}");
    let denied = call(&site, "GET", "/api/v1/bots", &[], &[], true);
    assert!(denied.starts_with("HTTP/1.1 401"));
    let bots = call(
        &site,
        "GET",
        "/api/v1/bots?unused=value",
        &[("Authorization", &authorized)],
        &[],
        true,
    );
    assert!(bots.starts_with("HTTP/1.1 200"), "{bots}");
    assert!(bots.contains("Cache-Control: no-store\r\n"));
    assert!(!bots.contains(&token));
}

#[test]
fn framework_preserves_conditional_responses_and_head_framing() {
    let site = site();
    let version = call(&site, "GET", "/api/v1/version", &[], &[], false);
    let etag = version
        .lines()
        .find_map(|line| line.strip_prefix("ETag: "))
        .unwrap();
    let unchanged = call(
        &site,
        "GET",
        "/api/v1/version",
        &[("If-None-Match", etag)],
        &[],
        false,
    );
    assert!(unchanged.starts_with("HTTP/1.1 304"), "{unchanged}");
    assert!(unchanged.ends_with("\r\n\r\n"));
    assert!(!unchanged.contains("Content-Length:"));
    let head = call(&site, "HEAD", "/api/v1/version", &[], &[], false);
    assert!(head.starts_with("HTTP/1.1 200"));
    assert!(head.ends_with("\r\n\r\n"));
    assert!(head.contains("Content-Length:"));
    assert!(head.contains("must-revalidate"));
    let redirect = call(&site, "GET", "/", &[], &[], false);
    assert!(redirect.contains("Location: /app/\r\n"));
}

#[test]
fn framework_owns_machine_diagnostics_and_rejects_large_bodies() {
    let site = site();
    for route in ["/metrics", "/api/v1/sys"] {
        let response = call(&site, "GET", route, &[], &[], false);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("mastomini-bots"));
        assert!(!response.contains("good_morning"));
    }
    let oversized = call(
        &site,
        "POST",
        "/api/v1/setup",
        &[],
        &vec![b' '; BODY_LIMIT + 1],
        false,
    );
    assert!(oversized.starts_with("HTTP/1.1 413"));
    assert!(site.service.shared.lock().unwrap().verifier.is_none());
}
