//! Members' API keys: `/api/mastomini/v1/me/api_keys` (local_tokens.rs).

use super::*;

const KEYS: &str = "/api/mastomini/v1/me/api_keys";

fn create(s: &mut Server, token: &str, name: &str, password: &str) -> crate::http::Response {
    s.post_form(
        KEYS,
        Some(token),
        &[
            ("name", name),
            ("scopes", "read write"),
            ("password", password),
        ],
    )
}

fn make_key(s: &mut Server, token: &str, name: &str) -> (String, String) {
    let r = create(s, token, name, "alicepw");
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let v = r.json_body();
    (
        v["id"].as_str().unwrap().to_string(),
        v["key"].as_str().unwrap().to_string(),
    )
}

#[test]
fn a_key_posts_as_its_member_and_names_itself() {
    let mut s = Server::provisioned();
    let token = s.login("alice", "alicepw", "read write follow");
    let (id, key) = make_key(&mut s, &token, "Weather bot");

    let r = s.post_form("/api/v1/statuses", Some(&key), &[("status", "Sunny, 21°")]);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let post = r.json_body();
    assert_eq!(post["account"]["username"], "alice");
    assert_eq!(post["application"]["name"], "Weather bot");
    let app = s
        .get("/api/v1/apps/verify_credentials", Some(&key))
        .json_body();
    assert_eq!(app["name"], "Weather bot");

    // Listed as a key, not as a signed-in device; the key itself is shown
    // only when created.
    let keys = s.get(KEYS, Some(&token)).json_body();
    assert_eq!(keys[0]["id"], id.as_str());
    assert_eq!(keys[0]["name"], "Weather bot");
    assert!(keys[0].get("key").is_none());
    let devices = s
        .get("/api/mastomini/v1/me/devices", Some(&token))
        .json_body();
    assert!(devices
        .as_array()
        .unwrap()
        .iter()
        .all(|d| d["id"] != id.as_str()));

    // Survives a restart, then revoking ends it and its name on old posts.
    let mut s = s.restart();
    assert_eq!(
        s.get("/api/v1/accounts/verify_credentials", Some(&key))
            .status,
        200
    );
    let r = s.send(Server::authed(
        Request::new("DELETE", &format!("{KEYS}/{id}")),
        Some(&token),
    ));
    assert_eq!(r.status, 200);
    assert_eq!(
        s.get("/api/v1/accounts/verify_credentials", Some(&key))
            .status,
        401
    );
    let post = s
        .get(
            &format!("/api/v1/statuses/{}", post["id"].as_str().unwrap()),
            Some(&token),
        )
        .json_body();
    assert!(post["application"].is_null());
    let s = s.restart();
    assert_eq!(s.svc.state.local_tokens.len(), 0);
}

#[test]
fn making_a_key_needs_the_password_and_a_name() {
    let mut s = Server::provisioned();
    let token = s.login("alice", "alicepw", "read write follow");
    assert_eq!(create(&mut s, &token, "Bot", "wrong").status, 403);
    assert_eq!(create(&mut s, &token, "  ", "alicepw").status, 422);
    assert_eq!(create(&mut s, &token, "Bot", "").status, 403);
    // Scopes are checked like an app's.
    let r = s.post_form(
        KEYS,
        Some(&token),
        &[
            ("name", "Bot"),
            ("scopes", "admin:everything"),
            ("password", "alicepw"),
        ],
    );
    assert_eq!(r.status, 422);
    // A read-only key cannot post.
    let r = s.post_form(
        KEYS,
        Some(&token),
        &[
            ("name", "Reader"),
            ("scopes", "read"),
            ("password", "alicepw"),
        ],
    );
    let reader = r.json_body()["key"].as_str().unwrap().to_string();
    let r = s.post_form("/api/v1/statuses", Some(&reader), &[("status", "hi")]);
    assert_eq!(r.status, 403);
    // Signed out: nothing.
    assert_eq!(s.get(KEYS, None).status, 401);
}

#[test]
fn keys_are_capped_and_never_evicted_by_sign_ins() {
    let mut s = Server::provisioned();
    let token = s.login("alice", "alicepw", "read write follow");
    let keys: Vec<String> = (0..crate::domain::MAX_API_KEYS_PER_ACCOUNT)
        .map(|n| make_key(&mut s, &token, &format!("Bot {n}")).1)
        .collect();
    assert_eq!(
        create(&mut s, &token, "One too many", "alicepw").status,
        429
    );
    // More sign-ins than a member may keep sessions for.
    for _ in 0..crate::domain::MAX_TOKENS_PER_ACCOUNT + 2 {
        s.login("alice", "alicepw", "read");
    }
    for k in &keys {
        assert_eq!(
            s.get("/api/v1/accounts/verify_credentials", Some(k)).status,
            200
        );
    }
}

#[test]
fn a_password_change_or_signing_out_everywhere_ends_keys() {
    let mut s = Server::provisioned();
    let token = s.login("alice", "alicepw", "read write follow");
    let (_, key) = make_key(&mut s, &token, "Bot");
    let r = s.post_form(
        "/api/mastomini/v1/me/sign_out_everywhere",
        Some(&token),
        &[],
    );
    assert_eq!(r.status, 200);
    assert_eq!(
        s.get("/api/v1/accounts/verify_credentials", Some(&key))
            .status,
        401
    );
    assert!(s.svc.state.local_tokens.is_empty());

    let token = s.login("alice", "alicepw", "read write follow");
    let (_, key) = make_key(&mut s, &token, "Bot");
    let r = s.post_form(
        "/api/mastomini/v1/me/password",
        Some(&token),
        &[("current", "alicepw"), ("new", "newpass1")],
    );
    assert_eq!(r.status, 200);
    assert_eq!(
        s.get("/api/v1/accounts/verify_credentials", Some(&key))
            .status,
        401
    );
}

#[test]
fn a_member_only_sees_and_revokes_their_own_keys() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let (id, key) = make_key(&mut s, &alice, "Bot");
    assert_eq!(s.get(KEYS, Some(&bob)).json_body(), json!([]));
    let r = s.send(Server::authed(
        Request::new("DELETE", &format!("{KEYS}/{id}")),
        Some(&bob),
    ));
    assert_eq!(r.status, 404);
    assert_eq!(
        s.get("/api/v1/accounts/verify_credentials", Some(&key))
            .status,
        200
    );
}

#[test]
fn a_power_cut_between_the_two_writes_leaves_no_key() {
    let mut s = Server::provisioned();
    let token = s.login("alice", "alicepw", "read write follow");
    let (id, key) = make_key(&mut s, &token, "Bot");
    // As if the description never reached flash.
    let id: u64 = id.parse().unwrap();
    s.svc
        .erase(crate::store::Ns::Tok, &crate::domain::keys::local_token(id))
        .unwrap();
    let mut s = s.restart();
    assert_eq!(
        s.get("/api/v1/accounts/verify_credentials", Some(&key))
            .status,
        401
    );
    // And the orphaned token was tidied away.
    assert!(s.svc.state.tokens.iter().all(|t| t.rec.id != id));
}
