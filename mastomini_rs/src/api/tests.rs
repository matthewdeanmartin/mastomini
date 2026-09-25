//! API tests through the full request path.

use super::testkit::*;
use crate::http::Request;
use serde_json::json;

mod social;

#[test]
fn instance_advertises_household_limits() {
    let mut s = Server::provisioned();
    let v1 = s.get("/api/v1/instance", None).json_body();
    assert_eq!(v1["title"], "Home");
    assert_eq!(v1["uri"], "mastomini.test");
    assert_eq!(v1["registrations"], false);
    assert_eq!(v1["configuration"]["statuses"]["max_characters"], 140);
    assert_eq!(v1["contact_account"]["username"], "alice");
    let v2 = s.get("/api/v2/instance", None).json_body();
    assert_eq!(v2["domain"], "mastomini.test");
    assert_eq!(
        v2["configuration"]["statuses"]["characters_reserved_per_url"],
        23
    );
    assert!(v2["version"].as_str().unwrap().starts_with("4.3.0"));
    let meta = s
        .get("/.well-known/oauth-authorization-server", None)
        .json_body();
    assert_eq!(meta["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(s.get("/api/v1/custom_emojis", None).json_body(), json!([]));
}

#[test]
fn provisioning_happens_once() {
    let mut s = Server::provisioned();
    let r = s.post_form(
        "/api/mastomini/v1/provision",
        None,
        &[("username", "mallory"), ("password", "pwpw")],
    );
    assert_eq!(r.status, 403);
    let status = s.get("/api/mastomini/v1/status", None).json_body();
    assert_eq!(status["provisioned"], true);
    assert_eq!(status["accounts"], 1);
}

#[test]
fn oauth_code_flow_with_pkce() {
    let mut s = Server::provisioned();
    let token = s.login("alice", "alicepw", "read write follow");
    let me = s.get("/api/v1/accounts/verify_credentials", Some(&token));
    assert_eq!(me.status, 200);
    let me = me.json_body();
    assert_eq!(me["username"], "alice");
    assert_eq!(me["source"]["privacy"], "public");
    assert_eq!(me["role"]["name"], "Owner");
    let app = s
        .get("/api/v1/apps/verify_credentials", Some(&token))
        .json_body();
    assert_eq!(app["name"], "Test");
}

#[test]
fn oauth_code_flow_with_client_secret_and_single_use_code() {
    let mut s = Server::provisioned();
    let (client_id, secret) = s.register_app("read");
    let r = s.post_form(
        "/oauth/authorize",
        None,
        &[
            ("client_id", &client_id),
            ("redirect_uri", REDIRECT),
            ("scope", "read"),
            ("username", "alice"),
            ("password", "alicepw"),
        ],
    );
    let location = r.header("Location").unwrap().to_string();
    let code = location.split("code=").nth(1).unwrap().to_string();
    let exchange = |s: &mut Server, secret: &str| {
        s.post_form(
            "/oauth/token",
            None,
            &[
                ("grant_type", "authorization_code"),
                ("client_id", &client_id),
                ("client_secret", secret),
                ("code", &code),
                ("redirect_uri", REDIRECT),
            ],
        )
    };
    let bad = exchange(&mut s, "wrong");
    assert_eq!(bad.status, 401);
    assert_eq!(bad.json_body()["error"], "invalid_client");
    let ok = exchange(&mut s, &secret);
    assert_eq!(ok.status, 200);
    assert_eq!(ok.json_body()["scope"], "read");
    let again = exchange(&mut s, &secret);
    assert_eq!(again.status, 400, "codes are single use");
    assert_eq!(again.json_body()["error"], "invalid_grant");
}

#[test]
fn authorize_page_rejects_bad_passwords_and_bad_redirects() {
    let mut s = Server::provisioned();
    let (client_id, _) = s.register_app("read");
    let page = s.get(
        &format!(
            "/oauth/authorize?response_type=code&client_id={client_id}&redirect_uri={}&scope=read",
            crate::http::encode(REDIRECT)
        ),
        None,
    );
    assert_eq!(page.status, 200);
    let html = String::from_utf8(page.body).unwrap();
    assert!(html.contains("Sign in to Home") && html.contains("name=\"password\""));

    let wrong = s.post_form(
        "/oauth/authorize",
        None,
        &[
            ("client_id", &client_id),
            ("redirect_uri", REDIRECT),
            ("username", "alice"),
            ("password", "nope"),
        ],
    );
    assert_eq!(wrong.status, 401);
    assert!(String::from_utf8(wrong.body)
        .unwrap()
        .contains("Wrong username or password"));

    let evil = s.get(
        &format!(
            "/oauth/authorize?client_id={client_id}&redirect_uri=https%3A%2F%2Fevil.example%2F"
        ),
        None,
    );
    assert_eq!(evil.status, 400);
    assert!(
        evil.header("Location").is_none(),
        "never redirect to an unregistered URI"
    );

    let deny = s.post_form(
        "/oauth/authorize",
        None,
        &[
            ("client_id", &client_id),
            ("redirect_uri", REDIRECT),
            ("state", "s1"),
            ("decision", "deny"),
        ],
    );
    assert_eq!(deny.status, 302);
    assert_eq!(
        deny.header("Location"),
        Some("mastomini-test://oauth?error=access_denied&state=s1")
    );
}

#[test]
fn lockout_after_five_failures() {
    let mut s = Server::provisioned();
    let (client_id, _) = s.register_app("read");
    for _ in 0..5 {
        s.post_form(
            "/oauth/authorize",
            None,
            &[
                ("client_id", &client_id),
                ("redirect_uri", REDIRECT),
                ("username", "alice"),
                ("password", "x"),
            ],
        );
    }
    let r = s.post_form(
        "/oauth/authorize",
        None,
        &[
            ("client_id", &client_id),
            ("redirect_uri", REDIRECT),
            ("username", "alice"),
            ("password", "alicepw"),
        ],
    );
    assert_eq!(r.status, 401);
    assert!(String::from_utf8(r.body)
        .unwrap()
        .contains("wait five minutes"));
}

#[test]
fn tokens_survive_restart_and_can_be_revoked() {
    let mut s = Server::provisioned();
    let token = s.login("alice", "alicepw", "read");
    let mut s = s.restart();
    assert_eq!(
        s.get("/api/v1/accounts/verify_credentials", Some(&token))
            .status,
        200
    );
    let r = s.post_form("/oauth/revoke", None, &[("token", &token)]);
    assert_eq!(r.status, 200);
    let r = s.get("/api/v1/accounts/verify_credentials", Some(&token));
    assert_eq!(r.status, 401);
    assert!(r.header("WWW-Authenticate").is_some());
}

#[test]
fn client_credentials_token_is_app_only() {
    let mut s = Server::provisioned();
    let (client_id, secret) = s.register_app("read write");
    let r = s.post_form(
        "/oauth/token",
        None,
        &[
            ("grant_type", "client_credentials"),
            ("client_id", &client_id),
            ("client_secret", &secret),
            ("scope", "read"),
        ],
    );
    assert_eq!(r.status, 200);
    let token = r.json_body()["access_token"].as_str().unwrap().to_string();
    assert_eq!(
        s.get("/api/v1/apps/verify_credentials", Some(&token))
            .status,
        200
    );
    assert_eq!(
        s.get("/api/v1/accounts/verify_credentials", Some(&token))
            .status,
        422
    );
}

#[test]
fn scopes_are_enforced() {
    let mut s = Server::provisioned();
    let read_only = s.login("alice", "alicepw", "read");
    let r = s.send_json(
        "PATCH",
        "/api/v1/accounts/update_credentials",
        Some(&read_only),
        &json!({ "display_name": "A" }),
    );
    assert_eq!(r.status, 403);
}

#[test]
fn profile_updates_and_members() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let r = s.send_json(
        "PATCH",
        "/api/v1/accounts/update_credentials",
        Some(&bob),
        &json!({
            "display_name": "Bob",
            "note": "Likes <tea>",
            "fields_attributes": { "0": { "name": "Room", "value": "Attic" } },
            "source": { "privacy": "private" }
        }),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let v = r.json_body();
    assert_eq!(v["display_name"], "Bob");
    assert_eq!(v["note"], "<p>Likes &lt;tea&gt;</p>");
    assert_eq!(v["fields"][0]["name"], "Room");
    assert_eq!(v["source"]["privacy"], "private");

    let too_long = s.send_json(
        "PATCH",
        "/api/v1/accounts/update_credentials",
        Some(&bob),
        &json!({ "display_name": "x".repeat(31) }),
    );
    assert_eq!(too_long.status, 422);

    let found = s
        .get(
            "/api/v1/accounts/lookup?acct=bob@mastomini.test",
            Some(&alice),
        )
        .json_body();
    assert_eq!(found["display_name"], "Bob");
    let id = found["id"].as_str().unwrap().to_string();
    assert_eq!(
        s.get(&format!("/api/v1/accounts/{id}"), Some(&alice))
            .json_body()["username"],
        "bob"
    );
    let search = s
        .get("/api/v1/accounts/search?q=bo", Some(&alice))
        .json_body();
    assert_eq!(search.as_array().unwrap().len(), 1);

    // Members can't add members.
    let r = s.post_form(
        "/api/mastomini/v1/admin/members",
        Some(&bob),
        &[("username", "eve"), ("password", "pwpw")],
    );
    assert_eq!(r.status, 403);
    assert_eq!(s.post_form("/api/v1/accounts", None, &[]).status, 403);
}

#[test]
fn follow_and_relationships() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let bob_id = s
        .get("/api/v1/accounts/verify_credentials", Some(&bob))
        .json_body()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = s.post_form(
        &format!("/api/v1/accounts/{bob_id}/follow"),
        Some(&alice),
        &[],
    );
    assert_eq!(r.json_body()["following"], true);
    let rel = s
        .get(
            &format!("/api/v1/accounts/relationships?id[]={bob_id}"),
            Some(&alice),
        )
        .json_body();
    assert_eq!(rel[0]["following"], true);
    assert_eq!(rel[0]["followed_by"], false);
    let followers = s
        .get(
            &format!("/api/v1/accounts/{bob_id}/followers"),
            Some(&alice),
        )
        .json_body();
    assert_eq!(followers[0]["username"], "alice");
    let r = s.post_form(
        &format!("/api/v1/accounts/{bob_id}/unfollow"),
        Some(&alice),
        &[],
    );
    assert_eq!(r.json_body()["following"], false);
}

#[test]
fn transport_rules() {
    let mut s = Server::provisioned();
    let r = s.get("/api/v1/nope", None);
    assert_eq!(r.status, 404);
    assert_eq!(r.json_body()["error"], "Record not found");
    assert_eq!(r.header("Access-Control-Allow-Origin"), Some("*"));
    let preflight = s.send(Request::new("OPTIONS", "/api/v1/statuses"));
    assert_eq!(preflight.status, 200);
    let big = Request::new("POST", "/api/v1/apps").with_body("application/json", vec![b' '; 5000]);
    assert_eq!(s.send(big).status, 413);
    // Without a valid clock, reads work and writes are refused.
    let req = Request::new("POST", "/api/v1/apps");
    let r = super::handle(&mut s.svc, &s.ctx, &req, None);
    assert_eq!(r.status, 503);
    let r = super::handle(
        &mut s.svc,
        &s.ctx,
        &Request::new("GET", "/api/v1/instance"),
        None,
    );
    assert_eq!(r.status, 200);
    let png = s.get("/avatars/original/missing.png", None);
    assert_eq!(png.header("Content-Type"), Some("image/png"));
    assert!(png.body.starts_with(b"\x89PNG"));
}

#[test]
fn accounts_have_generated_avatars() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read");
    let me = s
        .get("/api/v1/accounts/verify_credentials", Some(&alice))
        .json_body();
    let url = me["avatar"].as_str().unwrap();
    assert_eq!(me["avatar_static"], url);
    let path = url.strip_prefix("http://mastomini.test").unwrap();
    let png = s.get(path, None);
    assert_eq!(png.status, 200);
    assert_eq!(png.header("Content-Type"), Some("image/png"));
    assert!(png.body.starts_with(b"\x89PNG"));
    assert_eq!(s.get("/avatars/123.png", None).status, 404);
    assert_eq!(s.get("/avatars/nonsense.png", None).status, 404);
}

/// Alice (owner) and bob, following each other. Returns their tokens.
fn pair() -> (Server, String, String) {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let id_of = |s: &mut Server, token: &str| {
        s.get("/api/v1/accounts/verify_credentials", Some(token))
            .json_body()["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (alice_id, bob_id) = (id_of(&mut s, &alice), id_of(&mut s, &bob));
    s.post_form(
        &format!("/api/v1/accounts/{bob_id}/follow"),
        Some(&alice),
        &[],
    );
    s.post_form(
        &format!("/api/v1/accounts/{alice_id}/follow"),
        Some(&bob),
        &[],
    );
    (s, alice, bob)
}

fn post(s: &mut Server, token: &str, pairs: &[(&str, &str)]) -> serde_json::Value {
    let r = s.post_form("/api/v1/statuses", Some(token), pairs);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json_body()
}

fn id(v: &serde_json::Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

fn authed(method: &str, url: &str, token: &str) -> Request {
    Request::new(method, url).with_header("Authorization", &format!("Bearer {token}"))
}

#[test]
fn post_render_and_timelines() {
    let (mut s, alice, bob) = pair();
    let status = post(
        &mut s,
        &alice,
        &[(
            "status",
            "Dinner at 6 @bob #family https://example.com/menu",
        )],
    );
    let sid = id(&status);
    let content = status["content"].as_str().unwrap();
    assert!(content.contains("class=\"u-url mention\""), "{content}");
    assert!(content.contains("class=\"mention hashtag\""));
    assert_eq!(status["mentions"][0]["username"], "bob");
    assert_eq!(status["tags"][0]["name"], "family");
    assert_eq!(status["account"]["username"], "alice");
    assert_eq!(status["application"]["name"], "Test");
    assert_eq!(status["media_attachments"], json!([]));

    assert_eq!(
        s.get("/api/v1/timelines/home", Some(&bob)).json_body()[0]["id"],
        sid.as_str()
    );
    assert_eq!(
        s.get("/api/v1/timelines/public", Some(&bob)).json_body()[0]["id"],
        sid.as_str()
    );
    assert_eq!(
        s.get("/api/v1/timelines/tag/Family", Some(&bob))
            .json_body()[0]["id"],
        sid.as_str()
    );
    let found = s.get("/api/v2/search?q=dinner", Some(&bob)).json_body();
    assert_eq!(found["statuses"][0]["id"], sid.as_str());
    let tags = s
        .get("/api/v2/search?q=fam&type=hashtags", Some(&bob))
        .json_body();
    assert_eq!(tags["hashtags"][0]["name"], "family");

    let too_long = s.post_form(
        "/api/v1/statuses",
        Some(&alice),
        &[("status", &"x".repeat(141))],
    );
    assert_eq!(too_long.status, 422);
    let media = s.post_form(
        "/api/v1/statuses",
        Some(&alice),
        &[("status", "pic"), ("media_ids[]", "1")],
    );
    assert_eq!(media.status, 422);
}

#[test]
fn reactions_boosts_and_notifications() {
    let (mut s, alice, bob) = pair();
    let sid = id(&post(&mut s, &alice, &[("status", "hello")]));
    let fav = s
        .post_form(
            &format!("/api/v1/statuses/{sid}/favourite"),
            Some(&bob),
            &[],
        )
        .json_body();
    assert_eq!(fav["favourited"], true);
    assert_eq!(fav["favourites_count"], 1);
    let boost = s
        .post_form(&format!("/api/v1/statuses/{sid}/reblog"), Some(&bob), &[])
        .json_body();
    assert_eq!(boost["reblog"]["id"], sid.as_str());
    assert_eq!(boost["reblogged"], true);
    s.post_form(&format!("/api/v1/statuses/{sid}/bookmark"), Some(&bob), &[]);
    assert_eq!(
        s.get("/api/v1/bookmarks", Some(&bob)).json_body()[0]["id"],
        sid.as_str()
    );
    assert_eq!(
        s.get("/api/v1/favourites", Some(&bob)).json_body()[0]["id"],
        sid.as_str()
    );
    let by = s
        .get(
            &format!("/api/v1/statuses/{sid}/favourited_by"),
            Some(&alice),
        )
        .json_body();
    assert_eq!(by[0]["username"], "bob");

    let notes = s.get("/api/v1/notifications", Some(&alice)).json_body();
    let kinds: Vec<&str> = notes
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["reblog", "favourite", "follow"]);
    let only = s
        .get("/api/v1/notifications?types[]=favourite", Some(&alice))
        .json_body();
    assert_eq!(only.as_array().unwrap().len(), 1);
    let grouped = s.get("/api/v2/notifications", Some(&alice)).json_body();
    assert_eq!(grouped["notification_groups"].as_array().unwrap().len(), 3);
    assert_eq!(
        s.get("/api/v1/notifications/unread_count", Some(&alice))
            .json_body()["count"],
        3
    );

    let newest = id(&notes[0]);
    let marker = s.post_form(
        "/api/v1/markers",
        Some(&alice),
        &[("notifications[last_read_id]", &newest)],
    );
    assert_eq!(
        marker.json_body()["notifications"]["last_read_id"],
        newest.as_str()
    );
    assert_eq!(
        s.get("/api/v1/notifications/unread_count", Some(&alice))
            .json_body()["count"],
        0
    );
    let markers = s
        .get("/api/v1/markers?timeline[]=notifications", Some(&alice))
        .json_body();
    assert_eq!(markers["notifications"]["version"], 0);

    s.post_form(
        &format!("/api/v1/notifications/{newest}/dismiss"),
        Some(&alice),
        &[],
    );
    assert_eq!(
        s.get("/api/v1/notifications", Some(&alice))
            .json_body()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    s.post_form("/api/v1/notifications/clear", Some(&alice), &[]);
    assert_eq!(
        s.get("/api/v1/notifications", Some(&alice)).json_body(),
        json!([])
    );

    let undone = s
        .post_form(&format!("/api/v1/statuses/{sid}/unreblog"), Some(&bob), &[])
        .json_body();
    assert_eq!(undone["reblogged"], false);

    // Ephemeral state vanishes on restart; durable state stays.
    s.post_form(
        &format!("/api/v1/statuses/{sid}/favourite"),
        Some(&bob),
        &[],
    );
    let mut s = s.restart();
    let after = s
        .get(&format!("/api/v1/statuses/{sid}"), Some(&bob))
        .json_body();
    assert_eq!(after["favourited"], true);
    assert_eq!(after["bookmarked"], true);
    assert_eq!(
        s.get("/api/v1/markers", Some(&alice)).json_body(),
        json!({})
    );
    assert_eq!(
        s.get("/api/v1/notifications", Some(&alice)).json_body(),
        json!([])
    );
}

#[test]
fn replies_context_delete_and_idempotency() {
    let (mut s, alice, bob) = pair();
    let root = id(&post(&mut s, &alice, &[("status", "root")]));
    let reply = post(
        &mut s,
        &bob,
        &[("status", "@alice reply"), ("in_reply_to_id", &root)],
    );
    assert_eq!(reply["in_reply_to_id"], root.as_str());
    let ctx = s
        .get(&format!("/api/v1/statuses/{root}/context"), Some(&alice))
        .json_body();
    assert_eq!(ctx["descendants"][0]["id"], reply["id"]);
    let parent = s
        .get(&format!("/api/v1/statuses/{root}"), Some(&alice))
        .json_body();
    assert_eq!(parent["replies_count"], 1);

    let once = |s: &mut Server| {
        let req = authed("POST", "/api/v1/statuses", &alice)
            .with_header("Idempotency-Key", "abc")
            .with_body("application/json", json!({ "status": "once" }).to_string());
        s.send(req).json_body()["id"].clone()
    };
    let a = once(&mut s);
    assert_eq!(a, once(&mut s));

    let url = format!("/api/v1/statuses/{root}");
    assert_eq!(s.send(authed("DELETE", &url, &bob)).status, 403);
    let deleted = s.send(authed("DELETE", &url, &alice)).json_body();
    assert_eq!(deleted["text"], "root");
    assert_eq!(s.get(&url, Some(&alice)).status, 404);
}

#[test]
fn direct_messages_stay_private_and_pagination_links() {
    let (mut s, alice, bob) = pair();
    let carol = s.add_member(&alice, "carol");
    let dm = id(&post(
        &mut s,
        &alice,
        &[("status", "@bob secret"), ("visibility", "direct")],
    ));
    assert_eq!(
        s.get(&format!("/api/v1/statuses/{dm}"), Some(&bob)).status,
        200
    );
    assert_eq!(
        s.get(&format!("/api/v1/statuses/{dm}"), Some(&carol))
            .status,
        404
    );
    let public = s.get("/api/v1/timelines/public", Some(&carol)).json_body();
    assert!(public.as_array().unwrap().is_empty());

    for i in 0..5 {
        post(&mut s, &alice, &[("status", &format!("n{i}"))]);
    }
    let page = s.get("/api/v1/timelines/public?limit=2", Some(&bob));
    let link = page.header("Link").unwrap().to_string();
    assert!(
        link.contains("rel=\"next\"") && link.contains("rel=\"prev\""),
        "{link}"
    );
    let next = link.split('<').nth(1).unwrap().split('>').next().unwrap();
    let path = next.strip_prefix("http://mastomini.test").unwrap();
    let second = s.get(path, Some(&bob)).json_body();
    assert_eq!(second[0]["content"], "<p>n2</p>");
}

fn html(r: &crate::http::Response) -> String {
    String::from_utf8(r.body.clone()).unwrap()
}

#[test]
fn first_visit_shows_setup_and_creates_the_household() {
    let mut s = Server::new();
    let first = s.get("/", None);
    assert_eq!(first.status, 200);
    assert!(html(&first).contains("action=\"/setup\""));

    let mismatch = s.post_form(
        "/setup",
        None,
        &[
            ("title", "The Martins"),
            ("username", "mom"),
            ("password", "abcd"),
            ("password_confirm", "abce"),
        ],
    );
    assert_eq!(mismatch.status, 422);
    assert!(html(&mismatch).contains("The two passwords are different"));
    assert!(
        html(&mismatch).contains("value=\"mom\""),
        "keeps what was typed"
    );

    let bad_name = s.post_form(
        "/setup",
        None,
        &[
            ("username", "Mom!"),
            ("password", "abcd"),
            ("password_confirm", "abcd"),
        ],
    );
    assert_eq!(bad_name.status, 422);
    assert!(html(&bad_name).contains("Username must be"));

    let ok = s.post_form(
        "/setup",
        None,
        &[
            ("title", "The Martins"),
            ("username", "Mom"),
            ("password", "abcd"),
            ("password_confirm", "abcd"),
        ],
    );
    assert_eq!(ok.status, 200);
    assert!(html(&ok).contains("<code>mom</code> is its owner"));
    assert!(html(&ok).contains("<code>mastomini.test</code>"));
    // Visiting by IP suggests that IP, which works on every phone.
    let by_ip = s.send(Request::new("GET", "/").with_header("Host", "192.168.1.161"));
    assert!(html(&by_ip).contains("<code>192.168.1.161</code> (or <code>mastomini.test</code>)"));
    assert_eq!(
        s.get("/api/v1/instance", None).json_body()["title"],
        "The Martins"
    );

    // Setup cannot run twice; the form is gone.
    let again = s.post_form(
        "/setup",
        None,
        &[
            ("username", "eve"),
            ("password", "abcd"),
            ("password_confirm", "abcd"),
        ],
    );
    assert_eq!(again.status, 302);
    assert_eq!(
        s.get("/api/mastomini/v1/status", None).json_body()["accounts"],
        1
    );
    let landing = s.get("/", None);
    assert!(html(&landing).contains("href=\"/app/\""));
    assert!(!html(&landing).contains("admin_password"));
    assert!(!html(&landing).contains("action=\"/setup\""));
    s.login("mom", "abcd", "read");
}

#[test]
fn setup_without_a_clock_explains_itself() {
    let mut s = Server::new();
    let req = Request::new("POST", "/setup").with_body(
        "application/x-www-form-urlencoded",
        "username=a&password=abcd&password_confirm=abcd",
    );
    let r = super::handle(&mut s.svc, &s.ctx, &req, None);
    assert_eq!(r.status, 503);
    assert!(html(&r).contains("setting its clock"));
    // The form itself still loads.
    let r = super::handle(&mut s.svc, &s.ctx, &Request::new("GET", "/"), None);
    assert_eq!(r.status, 200);
}

#[test]
fn streaming_is_advertised_as_absent() {
    // Clients such as Elk only open WebSockets when this is set (spec/04).
    let mut s = Server::provisioned();
    let v2 = s.get("/api/v2/instance", None).json_body();
    assert!(v2["configuration"]["urls"]["streaming"].is_null());
    let v1 = s.get("/api/v1/instance", None).json_body();
    assert!(v1["urls"]["streaming_api"].is_null());
    // Mastodon's health check answers "OK" when streaming works; never here.
    let health = s.get("/api/v1/streaming/health", None);
    assert_eq!(health.status, 404);
}

mod about;
mod codes;
mod conversations;
mod diag;
mod dm;
mod edits_polls;
mod lists_filters;
mod moderation;
mod trust;
