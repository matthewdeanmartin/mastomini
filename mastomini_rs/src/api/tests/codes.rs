//! Invite and reset codes, password change, sign out everywhere and
//! devices, through the household API and the plain HTML pages.

use super::*;

fn body(r: &crate::http::Response) -> String {
    String::from_utf8_lossy(&r.body).to_string()
}

fn member_id(s: &mut Server, admin: &str, username: &str) -> String {
    let members = s
        .get("/api/mastomini/v1/admin/members", Some(admin))
        .json_body();
    members
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["username"] == username)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn delete(s: &mut Server, url: &str, token: &str) -> crate::http::Response {
    s.send(Request::new("DELETE", url).with_header("Authorization", &format!("Bearer {token}")))
}

/// The `/setup/<code>` path from text ending in `"`.
fn link_path(page: &str) -> String {
    let base = "http://mastomini.test";
    let start = page.find(&format!("{base}/setup/")).unwrap() + base.len();
    page[start..].chars().take_while(|c| *c != '"').collect()
}

#[test]
fn invite_redeem_and_sign_in() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    let bob = s.add_member(&alice, "bob");
    let r = s.post_form("/api/mastomini/v1/admin/invites", Some(&bob), &[]);
    assert_eq!(r.status, 403);
    let invite = s
        .post_form("/api/mastomini/v1/admin/invites", Some(&alice), &[])
        .json_body();
    assert_eq!(invite["kind"], "invite");
    let code = invite["code"].as_str().unwrap().to_string();
    assert_eq!(
        invite["url"],
        format!("http://mastomini.test/setup/{code}").as_str()
    );
    let listed = s
        .get("/api/mastomini/v1/admin/codes", Some(&alice))
        .json_body();
    assert_eq!(listed[0]["id"], invite["id"]);
    assert!(listed[0].get("code").is_none());

    let r = s.post_form(
        "/api/mastomini/v1/codes/redeem",
        None,
        &[
            ("code", &code),
            ("username", "dave"),
            ("password", "davepw"),
            ("display_name", "Dave"),
        ],
    );
    assert_eq!(r.status, 200, "{}", body(&r));
    assert_eq!(r.json_body()["username"], "dave");
    assert_eq!(r.json_body()["display_name"], "Dave");
    assert_eq!(r.json_body()["role"], "member");
    let r = s.post_form(
        "/api/mastomini/v1/codes/redeem",
        None,
        &[
            ("code", &code),
            ("username", "erin"),
            ("password", "erinpw"),
        ],
    );
    assert_eq!(r.status, 422);
    s.login("dave", "davepw", "read");
    let listed = s
        .get("/api/mastomini/v1/admin/codes", Some(&alice))
        .json_body();
    assert_eq!(listed, json!([]));
}

#[test]
fn reset_code_and_revoke() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    let bob = s.add_member(&alice, "bob");
    let bob_id = member_id(&mut s, &alice, "bob");
    let alice_id = member_id(&mut s, &alice, "alice");
    let url = format!("/api/mastomini/v1/admin/members/{alice_id}/reset");
    assert_eq!(s.post_form(&url, Some(&alice), &[]).status, 403);
    let url = format!("/api/mastomini/v1/admin/members/{bob_id}/reset");
    assert_eq!(s.post_form(&url, Some(&bob), &[]).status, 403);
    let reset = s.post_form(&url, Some(&alice), &[]).json_body();
    assert_eq!(reset["kind"], "reset");
    assert_eq!(reset["username"], "bob");
    assert_eq!(reset["account_id"], bob_id.as_str());

    // A revoked code can't be redeemed.
    let id = reset["id"].as_str().unwrap();
    let r = delete(
        &mut s,
        &format!("/api/mastomini/v1/admin/codes/{id}"),
        &alice,
    );
    assert_eq!(r.status, 200);
    let code = reset["code"].as_str().unwrap();
    let r = s.post_form(
        "/api/mastomini/v1/codes/redeem",
        None,
        &[("code", code), ("password", "bobnew")],
    );
    assert_eq!(r.status, 422);

    let code = s.post_form(&url, Some(&alice), &[]).json_body()["code"]
        .as_str()
        .unwrap()
        .to_string();
    let r = s.post_form(
        "/api/mastomini/v1/codes/redeem",
        None,
        &[("code", &code), ("password", "bobnew")],
    );
    assert_eq!(r.status, 200, "{}", body(&r));
    assert_eq!(
        s.get("/api/v1/accounts/verify_credentials", Some(&bob))
            .status,
        401
    );
    s.login("bob", "bobnew", "read");
}

#[test]
fn password_change_sign_out_and_devices() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    let bob = s.add_member(&alice, "bob");
    let phone = s.login("bob", "secret", "read write");

    let devices = s
        .get("/api/mastomini/v1/me/devices", Some(&bob))
        .json_body();
    let devices = devices.as_array().unwrap();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0]["current"], false);
    assert_eq!(devices[1]["current"], true);
    assert_eq!(devices[0]["app"]["name"], "Test");
    let phone_id = devices[0]["id"].as_str().unwrap();
    let r = delete(
        &mut s,
        &format!("/api/mastomini/v1/me/devices/{phone_id}"),
        &bob,
    );
    assert_eq!(r.status, 200);
    let r = s.get("/api/v1/accounts/verify_credentials", Some(&phone));
    assert_eq!(r.status, 401);

    let r = s.post_form(
        "/api/mastomini/v1/me/password",
        Some(&bob),
        &[("current", "wrong"), ("new", "bobnew")],
    );
    assert_eq!(r.status, 403);
    let r = s.post_form(
        "/api/mastomini/v1/me/password",
        Some(&bob),
        &[("current", "secret"), ("new", "bobnew")],
    );
    assert_eq!(r.status, 200);
    let r = s.get("/api/v1/accounts/verify_credentials", Some(&bob));
    assert_eq!(r.status, 401);
    let bob = s.login("bob", "bobnew", "read write");
    let r = s.post_form("/api/mastomini/v1/me/sign_out_everywhere", Some(&bob), &[]);
    assert_eq!(r.status, 200);
    let r = s.get("/api/v1/accounts/verify_credentials", Some(&bob));
    assert_eq!(r.status, 401);
    // Alice is unaffected.
    let r = s.get("/api/v1/accounts/verify_credentials", Some(&alice));
    assert_eq!(r.status, 200);
}

#[test]
fn html_pages_redeem_and_change_password() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    s.add_member(&alice, "bob");
    let bob_id = member_id(&mut s, &alice, "bob");
    let issue = |s: &mut Server, url: &str| {
        let url = s.post_form(url, Some(&alice), &[]).json_body()["url"]
            .as_str()
            .unwrap()
            .to_string();
        link_path(&format!("{url}\""))
    };
    let path = issue(&mut s, "/api/mastomini/v1/admin/invites");

    let r = s.get(&path, None);
    assert_eq!(r.status, 200);
    assert!(body(&r).contains("name=\"username\""));
    let r = s.post_form(
        &path,
        None,
        &[
            ("username", "dave"),
            ("password", "davepw"),
            ("password_confirm", "other"),
        ],
    );
    assert_eq!(r.status, 422);
    let r = s.post_form(
        &path,
        None,
        &[
            ("username", "dave"),
            ("password", "davepw"),
            ("password_confirm", "davepw"),
        ],
    );
    assert_eq!(r.status, 200, "{}", body(&r));
    assert_eq!(s.get(&path, None).status, 404);
    s.login("dave", "davepw", "read");

    // A reset link for bob.
    let path = issue(
        &mut s,
        &format!("/api/mastomini/v1/admin/members/{bob_id}/reset"),
    );
    assert!(body(&s.get(&path, None)).contains("<code>bob</code>"));
    let r = s.post_form(
        &path,
        None,
        &[("password", "bobnew"), ("password_confirm", "bobnew")],
    );
    assert_eq!(r.status, 200, "{}", body(&r));

    // Changing your own password.
    assert_eq!(s.get("/setup/password", None).status, 200);
    let change = |current: &'static str| {
        [
            ("username", "bob"),
            ("current", current),
            ("password", "x1234"),
            ("password_confirm", "x1234"),
        ]
    };
    let r = s.post_form("/setup/password", None, &change("secret"));
    assert_eq!(r.status, 401);
    let r = s.post_form("/setup/password", None, &change("bobnew"));
    assert_eq!(r.status, 200, "{}", body(&r));
    s.login("bob", "x1234", "read");
}
