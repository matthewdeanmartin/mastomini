//! Encrypted direct messages: nothing readable in flash, and only
//! participants holding their password or one of their tokens can read.

use super::*;
use crate::store::{Ns, Store};

fn me(s: &mut Server, token: &str) -> String {
    s.get("/api/v1/accounts/verify_credentials", Some(token))
        .json_body()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Every value in the store, concatenated.
fn flash(s: &mut Server) -> Vec<u8> {
    let mut all = Vec::new();
    for ns in Ns::ALL {
        s.svc
            .store()
            .for_each(ns, &mut |_, value| {
                all.extend_from_slice(value);
                Ok(())
            })
            .unwrap();
    }
    all
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

fn dm(s: &mut Server, from: &str, text: &str) -> String {
    let r = s.post_form(
        "/api/v1/statuses",
        Some(from),
        &[
            ("status", text),
            ("visibility", "direct"),
            ("spoiler_text", "cw secret"),
        ],
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json_body()["id"].as_str().unwrap().to_string()
}

#[test]
fn direct_messages_are_not_readable_in_flash() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let id = dm(&mut s, &bob, "@alice the pizza code is 4711");
    let raw = flash(&mut s);
    assert!(!contains(&raw, "pizza code"), "DM text in flash");
    assert!(!contains(&raw, "cw secret"), "content warning in flash");

    // Both participants read it, content warning included.
    for token in [&alice, &bob] {
        let v = s
            .get(&format!("/api/v1/statuses/{id}"), Some(token))
            .json_body();
        assert!(v["content"]
            .as_str()
            .unwrap()
            .contains("pizza code is 4711"));
        assert_eq!(v["spoiler_text"], "cw secret");
        assert_eq!(v["sensitive"], true);
    }
    let source = s
        .get(&format!("/api/v1/statuses/{id}/source"), Some(&bob))
        .json_body();
    assert_eq!(source["text"], "@alice the pizza code is 4711");
}

#[test]
fn only_participants_with_credentials_read_and_admins_are_not_participants() {
    let mut s = Server::provisioned();
    let alice = s.login(
        "alice",
        "alicepw",
        "read write follow admin:read admin:write",
    );
    let bob = s.add_member(&alice, "bob");
    let carol = s.add_member(&alice, "carol");
    let id = dm(&mut s, &bob, "@carol just between us");
    let url = format!("/api/v1/statuses/{id}");
    assert_eq!(
        s.get(&url, Some(&alice)).status,
        404,
        "the owner is not a participant"
    );
    assert!(s.get(&url, Some(&carol)).json_body()["content"]
        .as_str()
        .unwrap()
        .contains("just between us"));

    // Reporting it doesn't hand it to the admin either.
    let bob_id = me(&mut s, &bob);
    let r = s.send_json(
        "POST",
        "/api/v1/reports",
        Some(&carol),
        &json!({ "account_id": bob_id, "status_ids": [id] }),
    );
    assert_eq!(r.status, 200);
    let reports = s.get("/api/v1/admin/reports", Some(&alice)).json_body();
    assert_eq!(reports[0]["statuses"], json!([]));

    // The server itself, holding the store but no token, can't read it.
    s.svc.close_session();
    let status = s
        .svc
        .state
        .statuses
        .get(&id.parse().unwrap())
        .unwrap()
        .clone();
    assert_eq!(status.rec.text, "");
    assert_eq!(
        s.svc.readable_text(Some(2), &status).0,
        crate::domain::LOCKED_TEXT
    );
}

#[test]
fn keys_survive_restart_and_password_change_but_not_revocation() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let id = dm(&mut s, &alice, "@bob see you at six");
    let url = format!("/api/v1/statuses/{id}");
    let mut s = s.restart();
    assert!(s.get(&url, Some(&bob)).json_body()["content"]
        .as_str()
        .unwrap()
        .contains("see you at six"));

    // New password: every session ends, but the key moves to the new one.
    let slot = s.svc.state.account_by_username("bob").unwrap().slot;
    let now = s.now;
    s.svc
        .change_password(slot, "secret", "new-secret", now)
        .unwrap();
    assert_eq!(s.get(&url, Some(&bob)).status, 401);
    let bob = s.login("bob", "new-secret", "read write follow");
    assert!(s.get(&url, Some(&bob)).json_body()["content"]
        .as_str()
        .unwrap()
        .contains("see you at six"));

    // A revoked token takes its copy of the key with it.
    let seals = s.svc.state.token_seals.len();
    let r = s.post_form("/oauth/revoke", None, &[("token", &bob)]);
    assert_eq!(r.status, 200);
    assert_eq!(s.svc.state.token_seals.len(), seals - 1);
}

#[test]
fn members_without_a_key_get_one_at_their_next_sign_in() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    s.add_member(&alice, "bob");
    // As if bob's account predated encryption.
    let slot = s.svc.state.account_by_username("bob").unwrap().slot;
    s.svc.state.user_keys[slot as usize] = None;
    let r = s.post_form(
        "/api/v1/statuses",
        Some(&alice),
        &[("status", "@bob hi"), ("visibility", "direct")],
    );
    assert_eq!(r.status, 422);
    assert!(String::from_utf8_lossy(&r.body).contains("@bob needs to sign in again"));
    let bob = s.login("bob", "secret", "read write follow");
    let id = dm(&mut s, &alice, "@bob hi again");
    let v = s
        .get(&format!("/api/v1/statuses/{id}"), Some(&bob))
        .json_body();
    assert!(v["content"].as_str().unwrap().contains("hi again"));
}

#[test]
fn deleting_a_direct_message_removes_its_envelope() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    s.add_member(&alice, "bob");
    let id = dm(&mut s, &alice, "@bob gone soon");
    let deleted = s.send(authed("DELETE", &format!("/api/v1/statuses/{id}"), &alice));
    assert_eq!(
        deleted.json_body()["text"],
        "@bob gone soon",
        "delete & redraft"
    );
    assert!(s.svc.state.dms.is_empty());
    let s = s.restart();
    assert!(s.svc.state.dms.is_empty());
}
