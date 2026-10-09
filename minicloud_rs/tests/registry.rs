use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use minicloud::{
    http::{self, Body, Request},
    registry::Claims,
    store::{Store, Tier},
    Cloud, Shared,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read},
    sync::{Arc, Mutex},
};
const ADMIN: &str = "registry-test-admin-credential";
const SECRET: &str = "test-bank-secret-at-least-32-bytes-long";
const NOW: u64 = 1_800_000_000;
fn now() -> u64 {
    NOW
}
fn token(user: &str, bank: &str, audience: &str, iat: u64, exp: u64) -> String {
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&Claims {
            username: user.into(),
            bank: bank.into(),
            aud: audience.into(),
            iat,
            exp,
        })
        .unwrap(),
    );
    let message = format!("mc1.{payload}");
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(message.as_bytes());
    format!(
        "{message}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}
fn call(
    s: &Shared,
    method: &str,
    path: &str,
    auth: &str,
    data: &[u8],
    mime: &str,
) -> Result<Value, u16> {
    let authorization = format!("Bearer {auth}");
    let reply = http::handle(
        s,
        &Request {
            method,
            path,
            authorization: &authorization,
            content_type: mime,
            length: data.len() as u64,
            etag: "",
            gzip: false,
        },
        &mut Cursor::new(data),
    )
    .map_err(|e| e.status)?;
    let Body::Bytes(bytes) = reply.body else {
        panic!("expected JSON");
    };
    Ok(serde_json::from_slice(&bytes).unwrap())
}
fn json_call(s: &Shared, method: &str, path: &str, auth: &str, value: Value) -> Result<Value, u16> {
    call(
        s,
        method,
        path,
        auth,
        &serde_json::to_vec(&value).unwrap(),
        "application/json",
    )
}
fn setup() -> (tempfile::TempDir, Shared) {
    let dir = tempfile::tempdir().unwrap();
    let s = Arc::new(Mutex::new(
        Cloud::open_with_clock(dir.path(), ADMIN.into(), now).unwrap(),
    ));
    json_call(&s,"PUT","/api/banks/bank",ADMIN,json!({"id":"bank","secret":SECRET,"origin":"https://bank.local","audience":"https://cloud.local"})).unwrap();
    (dir, s)
}
#[test]
fn trusted_bank_users_crud_search_transfer_and_restart() {
    let (dir, s) = setup();
    let alice = token("alice", "bank", "https://cloud.local", NOW, NOW + 300);
    let bob = token("bob", "bank", "https://cloud.local", NOW, NOW + 300);
    let namespace = format!("{:x}", Sha256::digest(b"alice@bank"));
    let key = format!("{}/moon.png", &namespace[..16]);
    let blobpath = format!("/api/blobs/art/{key}");
    let path = format!("/api/copyrights/art/{key}");
    assert_eq!(
        call(
            &s,
            "PUT",
            "/api/blobs/other/moon.png",
            &alice,
            b"image",
            "image/png"
        ),
        Err(403)
    );
    let uploaded = call(&s, "PUT", &blobpath, &alice, b"image", "image/png").unwrap();
    let mut record = json!({"bucket":"art","key":key,"hash":uploaded["blob"]["hash"],"owner":"alice@bank","title":"Moon","license":"Personal display","revision":0});
    assert_eq!(json_call(&s, "PUT", &path, &bob, record.clone()), Err(403));
    let wrong = {
        let mut r = record.clone();
        r["hash"] = json!("0".repeat(64));
        r
    };
    assert_eq!(json_call(&s, "PUT", &path, &alice, wrong), Err(409));
    let created = json_call(&s, "PUT", &path, &alice, record.clone()).unwrap();
    assert_eq!(
        json_call(&s, "PUT", &path, &alice, record.clone()),
        Err(409)
    );
    assert_eq!(
        call(&s, "PUT", &blobpath, ADMIN, b"changed", "image/png"),
        Err(409)
    );
    assert_eq!(call(&s, "DELETE", &blobpath, ADMIN, b"", ""), Err(409));
    assert_eq!(
        call(&s, "GET", "/api/copyrights?q=moon", &alice, b"", "").unwrap()["copyrights"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        call(&s, "GET", "/api/blobs?q=moon", &alice, b"", "").unwrap()["blobs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    record = created["copyright"].clone();
    record["owner"] = json!("bob@bank");
    let transferred = json_call(&s, "PUT", &path, &alice, record).unwrap();
    assert_eq!(
        json_call(
            &s,
            "DELETE",
            &path,
            &alice,
            json!({"revision":transferred["copyright"]["revision"]})
        ),
        Err(403)
    );
    drop(s);
    let s = Arc::new(Mutex::new(
        Cloud::open_with_clock(dir.path(), ADMIN.into(), now).unwrap(),
    ));
    assert_eq!(
        call(&s, "GET", &path, &bob, b"", "").unwrap()["copyright"]["owner"],
        "bob@bank"
    );
    json_call(
        &s,
        "DELETE",
        &path,
        &bob,
        json!({"revision":transferred["copyright"]["revision"]}),
    )
    .unwrap();
    // A recreated record must not reuse a deleted record's revision.
    let mut recreated = transferred["copyright"].clone();
    recreated["revision"] = json!(0);
    let new = json_call(&s, "PUT", &path, ADMIN, recreated).unwrap();
    assert!(new["copyright"]["revision"].as_u64() > transferred["copyright"]["revision"].as_u64());
}
#[test]
fn delegation_rejects_forgery_wrong_audience_expiry_and_revocation() {
    let (_dir, s) = setup();
    for invalid in [
        token("alice", "unknown", "https://cloud.local", NOW, NOW + 300),
        token("alice", "bank", "https://other.local", NOW, NOW + 300),
        token("alice", "bank", "https://cloud.local", NOW - 300, NOW),
        token("alice", "bank", "https://cloud.local", NOW, NOW + 301),
        token("alice", "bank", "https://cloud.local", NOW + 31, NOW + 300),
        token("", "bank", "https://cloud.local", NOW, NOW + 300),
    ] {
        assert_eq!(call(&s, "GET", "/api/blobs", &invalid, b"", ""), Err(401));
    }
    let valid = token("alice", "bank", "https://cloud.local", NOW, NOW + 300);
    let mut forged = valid.clone().into_bytes();
    forged[10] = if forged[10] == b'A' { b'B' } else { b'A' };
    assert_eq!(
        call(
            &s,
            "GET",
            "/api/blobs",
            std::str::from_utf8(&forged).unwrap(),
            b"",
            ""
        ),
        Err(401)
    );
    assert_eq!(call(&s, "GET", "/api/banks", &valid, b"", ""), Err(403));
    assert!(!call(&s, "GET", "/api/banks", ADMIN, b"", "")
        .unwrap()
        .to_string()
        .contains(SECRET));
    call(&s, "DELETE", "/api/banks/bank", ADMIN, b"", "").unwrap();
    assert_eq!(call(&s, "GET", "/api/blobs", &valid, b"", ""), Err(401));
}
#[test]
fn migration_verifies_bytes_preserves_urls_and_reclaims_only_safe_sources() {
    let (dir, s) = setup();
    let card = tempfile::tempdir().unwrap();
    s.lock().unwrap().store = Store::with_card(dir.path(), Some(card.path().into())).unwrap();
    let blob = call(
        &s,
        "PUT",
        "/api/blobs/art/moon.png",
        ADMIN,
        b"picture",
        "image/png",
    )
    .unwrap()["blob"]
        .clone();
    let hash = blob["hash"].as_str().unwrap();
    let source = s.lock().unwrap().store.blob_path(hash);
    json_call(&s,"PUT","/api/copyrights/art/moon.png",ADMIN,json!({"bucket":"art","key":"moon.png","hash":hash,"owner":"alice@bank","title":"Moon","license":"Display","revision":0})).unwrap();
    let copyright = s.lock().unwrap().state.copyrights[0].clone();
    let migrated = json_call(
        &s,
        "POST",
        "/api/storage/migrate",
        ADMIN,
        json!({"bucket":"art","key":"moon.png","tier":"card"}),
    )
    .unwrap();
    assert_eq!(migrated["blob"]["hash"], hash);
    assert!(!source.exists());
    let store = Store::with_card(dir.path(), Some(card.path().into())).unwrap();
    let state = store.load().unwrap();
    assert_eq!(state.blobs[0].tier, Tier::Card);
    assert_eq!(state.copyrights[0].revision, copyright.revision);
    let mut bytes = Vec::new();
    store
        .open_blob(&state.blobs[0])
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"picture");
    // Missing card must fail closed, never roll back ownership or reset state.
    assert!(Store::with_card(dir.path(), None).unwrap().load().is_err());
    let dest = store.path_for(&state.blobs[0]).unwrap();
    std::fs::write(&dest, b"corrupt").unwrap();
    assert_eq!(
        json_call(
            &s,
            "POST",
            "/api/storage/migrate",
            ADMIN,
            json!({"bucket":"art","key":"moon.png","tier":"onboard"})
        ),
        Err(507)
    );
    assert_eq!(s.lock().unwrap().state.blobs[0].tier, Tier::Card);
    std::fs::write(&dest, b"picture").unwrap();
    json_call(
        &s,
        "POST",
        "/api/storage/migrate",
        ADMIN,
        json!({"bucket":"art","key":"moon.png","tier":"onboard"}),
    )
    .unwrap();
    assert!(source.exists());
    assert!(!dest.exists());
}
