use minicloud::{
    http::{self, Body, Request},
    Cloud,
};
use serde_json::{json, Value};
use std::{
    io::Cursor,
    sync::{Arc, Mutex},
};

const TOKEN: &str = "test-admin-token-12345678";
thread_local! { static NOW: std::cell::Cell<u64> = const { std::cell::Cell::new(1_700_000_000) }; }
fn now() -> u64 {
    NOW.with(std::cell::Cell::get)
}
fn time(value: u64) {
    NOW.with(|clock| clock.set(value));
}

#[test]
fn expiry_survives_restart_and_updates_never_extend_lifetime() {
    let dir = tempfile::tempdir().unwrap();
    time(1_700_000_000);
    let mut c = Cloud::open_with_clock(dir.path(), TOKEN.into(), now).unwrap();
    c.enqueue("minicloud/screen/notify", notification("one", "1"), None)
        .unwrap();
    c.work_once().unwrap();
    assert_eq!(c.state.notices[0].expires_at, now() + 86400);
    time(now() + 3600);
    c.enqueue("minicloud/screen/notify", notification("two", "1"), None)
        .unwrap();
    c.work_once().unwrap();
    assert_eq!(c.state.notices[0].expires_at, 1_700_086_400);
    drop(c);
    time(1_700_086_400);
    let mut c = Cloud::open_with_clock(dir.path(), TOKEN.into(), now).unwrap();
    c.expire().unwrap();
    assert!(c.state.notices.is_empty());
    c.enqueue("minicloud/screen/notify", notification("late", "1"), None)
        .unwrap();
    c.work_once().unwrap();
    assert!(c.state.notices.is_empty());
}
#[test]
fn queued_notifications_expire_before_display_and_unsynchronized_clock_refuses_them() {
    let dir = tempfile::tempdir().unwrap();
    time(1_700_000_000);
    let mut c = Cloud::open_with_clock(dir.path(), TOKEN.into(), now).unwrap();
    c.enqueue("minicloud/screen/notify", notification("one", "1"), None)
        .unwrap();
    time(now() + 86400);
    c.work_once().unwrap();
    assert!(c.state.notices.is_empty());
    time(0);
    assert_eq!(
        c.enqueue("minicloud/screen/notify", notification("two", "2"), None)
            .unwrap_err()
            .status,
        503
    );
}
#[test]
fn notify_and_read_are_public_while_blob_management_requires_authentication() {
    let dir = tempfile::tempdir().unwrap();
    let shared = Arc::new(Mutex::new(Cloud::open(dir.path(), TOKEN.into()).unwrap()));
    for (path, value) in [
        ("/api/screen/notify", notification("one", "1")),
        (
            "/api/screen/read",
            json!({"event_id":"read","source":"nanacoin","id":"1"}),
        ),
    ] {
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            http::handle(
                &shared,
                &request("POST", path, &bytes, "application/json", false),
                &mut Cursor::new(bytes)
            )
            .unwrap()
            .status,
            202
        );
        shared.lock().unwrap().work_once().unwrap();
    }
    assert!(shared.lock().unwrap().state.notices.is_empty());
    assert_eq!(
        http::handle(
            &shared,
            &request("GET", "/api/blobs", &[], "", false),
            &mut Cursor::new([])
        )
        .err()
        .unwrap()
        .status,
        401
    );
}
fn request<'a>(
    method: &'a str,
    path: &'a str,
    bytes: &[u8],
    mime: &'a str,
    auth: bool,
) -> Request<'a> {
    Request {
        method,
        path,
        authorization: if auth {
            "Bearer test-admin-token-12345678"
        } else {
            ""
        },
        content_type: mime,
        length: bytes.len() as u64,
        etag: "",
        gzip: true,
    }
}

fn notification(event: &str, id: &str) -> Value {
    json!({"event_id":event,"source":"nanacoin","id":id,"recipient":"Katie","text":"New message","size":"large"})
}

#[test]
fn durable_queue_deduplicates_and_read_before_notify_does_not_resurrect() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Cloud::open(dir.path(), TOKEN.into()).unwrap();
    c.enqueue("minicloud/screen/notify", notification("one", "1"), None)
        .unwrap();
    c.enqueue("minicloud/screen/notify", notification("one", "1"), None)
        .unwrap();
    assert_eq!(c.state.jobs.len(), 1);
    drop(c);
    let mut c = Cloud::open(dir.path(), TOKEN.into()).unwrap();
    c.work_once().unwrap();
    assert_eq!(c.state.notices.len(), 1);
    c.enqueue(
        "minicloud/screen/read",
        json!({"event_id":"read-2","source":"nanacoin","id":"2"}),
        None,
    )
    .unwrap();
    c.work_once().unwrap();
    c.enqueue("minicloud/screen/notify", notification("two", "2"), None)
        .unwrap();
    c.work_once().unwrap();
    assert_eq!(c.state.notices.len(), 1);
    c.dismiss("nanacoin", "1").unwrap();
    drop(c);
    let c = Cloud::open(dir.path(), TOKEN.into()).unwrap();
    assert!(c.state.notices.is_empty());
    assert_eq!(c.state.receipts.len(), 3);
}
#[test]
fn invalid_plugin_payload_is_failed_without_partial_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Cloud::open(dir.path(), TOKEN.into()).unwrap();
    c.enqueue(
        "minicloud/screen/notify",
        json!({"event_id":"invalid","text":"Hello"}),
        None,
    )
    .unwrap();
    c.work_once().unwrap();
    assert!(c.state.notices.is_empty());
    assert_eq!(c.state.receipts[0].status, "failed");
    assert!(c.state.jobs.is_empty());
}
#[test]
fn interrupted_state_slot_recovers_previous_committed_generation() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Cloud::open(dir.path(), TOKEN.into()).unwrap();
    c.enqueue("minicloud/screen/notify", notification("one", "1"), None)
        .unwrap();
    c.work_once().unwrap();
    let revision = c.state.revision;
    drop(c);
    std::fs::write(
        dir.path().join(format!("state-{}.json", revision % 2)),
        b"interrupted",
    )
    .unwrap();
    let mut c = Cloud::open(dir.path(), TOKEN.into()).unwrap();
    assert_eq!(c.state.jobs.len(), 1);
    c.work_once().unwrap();
    assert_eq!(c.state.notices.len(), 1);
}
#[test]
fn blob_upload_streams_exact_bytes_with_mime_and_auth_and_keeps_rollback_files() {
    let dir = tempfile::tempdir().unwrap();
    let shared = Arc::new(Mutex::new(Cloud::open(dir.path(), TOKEN.into()).unwrap()));
    let data = b"hello file";
    assert_eq!(
        http::handle(
            &shared,
            &request(
                "PUT",
                "/api/blobs/app/folder/file.txt",
                data,
                "text/plain",
                false
            ),
            &mut Cursor::new(data)
        )
        .err()
        .unwrap()
        .status,
        401
    );
    for bytes in [data.as_slice(), b"replacement".as_slice()] {
        http::handle(
            &shared,
            &request(
                "PUT",
                "/api/blobs/app/folder/file.txt",
                bytes,
                "text/plain",
                true,
            ),
            &mut Cursor::new(bytes),
        )
        .unwrap();
    }
    let reply = http::handle(
        &shared,
        &request("GET", "/blobs/app/folder/file.txt", b"", "", false),
        &mut Cursor::new([]),
    )
    .unwrap();
    assert_eq!(reply.mime, "text/plain");
    assert_eq!(reply.length, 11);
    let Body::File(mut file) = reply.body else {
        panic!("blob must be streamed")
    };
    let mut out = String::new();
    std::io::Read::read_to_string(&mut file, &mut out).unwrap();
    assert_eq!(out, "replacement");
    let c = shared.lock().unwrap();
    c.store.collect(&c.state).unwrap();
    // Both generations' bytes survive collection and the newest may be torn.
    std::fs::write(
        dir.path()
            .join(format!("state-{}.json", c.state.revision % 2)),
        b"torn",
    )
    .unwrap();
    assert_eq!(c.store.load().unwrap().blobs[0].size, data.len() as u64);
}
#[test]
fn storage_rejects_traversal_truncated_uploads_and_wrong_rgb565_size() {
    let dir = tempfile::tempdir().unwrap();
    let shared = Arc::new(Mutex::new(Cloud::open(dir.path(), TOKEN.into()).unwrap()));
    for path in ["/api/blobs/app/../escape", "/api/blobs/app/%2e%2e/escape"] {
        assert_eq!(
            http::handle(
                &shared,
                &request("PUT", path, b"abc", "text/plain", true),
                &mut Cursor::new(b"abc")
            )
            .err()
            .unwrap()
            .status,
            400
        );
    }
    assert_eq!(
        http::handle(
            &shared,
            &request(
                "PUT",
                "/api/blobs/app/img.rgb565",
                b"abc",
                "application/x-rgb565",
                true
            ),
            &mut Cursor::new(b"abc")
        )
        .err()
        .unwrap()
        .status,
        400
    );
    let c = shared.lock().unwrap();
    assert!(c
        .store
        .upload("app", "file", "text/plain", &mut Cursor::new(b"a"), 20)
        .is_err());
    assert!(c.state.blobs.is_empty());
}
#[test]
fn public_dismiss_does_not_require_an_admin_token() {
    let dir = tempfile::tempdir().unwrap();
    let shared = Arc::new(Mutex::new(Cloud::open(dir.path(), TOKEN.into()).unwrap()));
    http::handle(
        &shared,
        &request("POST", "/api/screen/mastomini/123/dismiss", b"", "", false),
        &mut Cursor::new([]),
    )
    .unwrap();
    assert!(shared
        .lock()
        .unwrap()
        .state
        .dismissed
        .contains(&"mastomini/123".into()));
}

#[test]
fn two_stats_cards_coalesce_and_rotate_between_messages() {
    let dir = tempfile::tempdir().unwrap();
    time(1_700_000_000);
    let mut c = Cloud::open_with_clock(dir.path(), TOKEN.into(), now).unwrap();
    let mut send = |source: &str, id: &str, text: &str, event: &str| {
        c.enqueue("minicloud/screen/notify",json!({"event_id":event,"source":source,"id":id,"text":text,"recipient":"Household","size":"small"}),None).unwrap();
        c.work_once().unwrap();
    };
    send("deployment", "welcome", "Minicloud is ready!", "welcome");
    send("nanacoin", "mail", "Dinner is ready", "mail");
    send("nanacoin", "stats-0-s3-1", "Inflation: No data", "stats1");
    send(
        "nanacoin",
        "stats-1-s3-1",
        "Interest: No active loans",
        "stats2",
    );
    send("nanacoin", "stats-0-s3-2", "Inflation: 3%", "stats3");
    assert_eq!(c.state.notices.len(), 3);
    assert!(!c
        .state
        .notices
        .iter()
        .any(|n| n.id == "welcome" || n.id == "stats-0-s3-1"));
    let mut order = Vec::new();
    for _ in 0..4 {
        order.push(c.current().unwrap().id);
        c.advance();
    }
    assert_eq!(order[0], "mail");
    assert!(order[1].starts_with("stats-"));
    assert_eq!(order[2], "mail");
    assert!(order[3].starts_with("stats-"));
    assert_ne!(order[1], order[3]);
    drop(c);
    let c = Cloud::open_with_clock(dir.path(), TOKEN.into(), now).unwrap();
    assert_eq!(c.state.notices.len(), 3);
}
