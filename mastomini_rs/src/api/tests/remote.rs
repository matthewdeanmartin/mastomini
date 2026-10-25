//! Accounts on other servers: search, follow, the bots board's pushes, the
//! home timeline, limits, device keys and images.

use super::*;
use crate::api::remote_search::{finish as finish_lookup, pending, Outbound};
use crate::api::time;
use crate::http::Response;
use serde_json::Value;

const BOB: &str = "@bob@example.org";

/// Alice's token, after a search that found `@bob@example.org`; Bob's id.
fn found_bob(s: &mut Server) -> (String, String) {
    let alice = s.login(
        "alice",
        "alicepw",
        "read write follow admin:read admin:write",
    );
    let url = format!("/api/v2/search?q={}&resolve=true", crate::http::encode(BOB));
    let req = Server::authed(Request::new("GET", &url), Some(&alice));
    let lookup = pending(&mut s.svc, &s.ctx, &req, s.now).expect("asks example.org");
    assert_eq!(
        lookup.url,
        "https://example.org/api/v1/accounts/lookup?acct=bob"
    );
    let answer =
        json!({"id": "1", "acct": "bob", "display_name": "Bob B", "note": "<p>Hi &amp; bye</p>"});
    finish_lookup(
        &mut s.svc,
        lookup,
        Ok((200, answer.to_string().into_bytes())),
        s.now,
    );
    assert!(
        pending(&mut s.svc, &s.ctx, &req, s.now).is_none(),
        "answer is cached"
    );
    let r = s.get(&url, Some(&alice));
    let accounts = r.json_body()["accounts"].clone();
    assert_eq!(accounts[0]["acct"], "bob@example.org", "{accounts}");
    assert_eq!(accounts[0]["display_name"], "Bob B");
    assert_eq!(accounts[0]["note"], "<p>Hi &amp; bye</p>");
    assert_eq!(accounts[0]["url"], "https://example.org/@bob");
    let id = accounts[0]["id"].as_str().unwrap().to_string();
    (alice, id)
}

fn device_key(s: &mut Server, admin: &str) -> String {
    let r = s.post_form(
        "/api/mastomini/v1/admin/device_keys",
        Some(admin),
        &[("name", "Bots board")],
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let key = r.json_body()["key"].as_str().unwrap().to_string();
    assert!(key.starts_with("mmdev_"));
    key
}

fn post(text: &str, guid: &str, published: u64) -> Value {
    json!({"guid": guid, "published_ms": published, "url": format!("https://example.org/@bob/{guid}"),
           "text": text, "cw": "", "sensitive": false, "media": []})
}

fn push(s: &mut Server, key: &str, account: &str, items: Value) -> Response {
    s.send_json(
        "POST",
        &format!("/api/mastomini/v1/feeds/{account}/result"),
        Some(key),
        &json!({"outcome": "ok", "etag": "\"e1\"", "last_modified": "", "title": "Bob Blog",
                "note": "About Bob", "items": items}),
    )
}

#[test]
fn search_follow_push_and_read_in_home() {
    let mut s = Server::provisioned();
    let (alice, bob) = found_bob(&mut s);
    let r = s.post_form(&format!("/api/v1/accounts/{bob}/follow"), Some(&alice), &[]);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json_body()["following"], true);
    let local = s.post_form(
        "/api/v1/statuses",
        Some(&alice),
        &[("status", "home first")],
    );
    assert_eq!(local.status, 200);

    let key = device_key(&mut s, &alice);
    let jobs = s
        .get("/api/mastomini/v1/feeds/jobs", Some(&key))
        .json_body();
    assert_eq!(jobs["jobs"][0]["url"], "https://example.org/@bob.rss");
    assert_eq!(jobs["jobs"][0]["account"], bob.as_str());
    assert_eq!(jobs["images"], false);

    let published = s.now - 3_600_000;
    let items = json!([
        post("older", "g1", published),
        post("newer https://example.org/x", "g2", published + 60_000)
    ]);
    let r = push(&mut s, &key, &bob, items.clone());
    assert_eq!(
        r.json_body()["added"],
        2,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    assert_eq!(
        push(&mut s, &key, &bob, items).json_body()["added"],
        0,
        "a post arrives once"
    );

    let home = s.get("/api/v1/timelines/home", Some(&alice)).json_body();
    let home = home.as_array().unwrap();
    assert_eq!(home.len(), 3);
    assert!(home[0]["content"].as_str().unwrap().contains("newer"));
    assert_eq!(home[1]["content"], "<p>older</p>");
    assert_eq!(
        home[2]["content"], "<p>home first</p>",
        "the night's posts arrive on top"
    );
    assert_eq!(home[0]["account"]["acct"], "bob@example.org");
    assert_eq!(home[0]["account"]["display_name"], "Bob Blog");
    assert_eq!(home[0]["url"], "https://example.org/@bob/g2");
    assert_eq!(home[1]["created_at"], time::iso(published));

    // Post search finds followed posts from elsewhere too.
    let found = s
        .get("/api/v2/search?q=older&type=statuses", Some(&alice))
        .json_body();
    assert_eq!(found["statuses"][0]["url"], "https://example.org/@bob/g1");

    // Paging across both kinds of post.
    let top = home[0]["id"].as_str().unwrap();
    let mid = home[1]["id"].as_str().unwrap();
    let since = s.get(
        &format!("/api/v1/timelines/home?since_id={top}"),
        Some(&alice),
    );
    assert_eq!(since.json_body(), json!([]));
    let older = s.get(
        &format!("/api/v1/timelines/home?max_id={mid}"),
        Some(&alice),
    );
    assert_eq!(older.json_body()[0]["content"], "<p>home first</p>");
    let first = s.get("/api/v1/timelines/home?limit=1", Some(&alice));
    assert_eq!(first.json_body()[0]["id"], top);

    // A remote post opens, but cannot be favourited, boosted or replied to.
    let one = s.get(&format!("/api/v1/statuses/{top}"), Some(&alice));
    assert_eq!(one.status, 200);
    let context = s.get(&format!("/api/v1/statuses/{top}/context"), Some(&alice));
    assert_eq!(
        context.json_body(),
        json!({"ancestors": [], "descendants": []})
    );
    let fav = s.post_form(
        &format!("/api/v1/statuses/{top}/favourite"),
        Some(&alice),
        &[],
    );
    assert_eq!(fav.status, 422);
    let account = s.get(&format!("/api/v1/accounts/{bob}/statuses"), Some(&alice));
    assert_eq!(account.json_body().as_array().unwrap().len(), 2);
    let rel = s.get(
        &format!("/api/v1/accounts/relationships?id[]={bob}"),
        Some(&alice),
    );
    assert_eq!(rel.json_body()[0]["following"], true);

    // Someone who does not follow Bob sees none of it.
    let carol = s.add_member(&alice, "carol");
    let home = s.get("/api/v1/timelines/home", Some(&carol)).json_body();
    assert!(home
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["account"]["acct"] != "bob@example.org"));
    assert_eq!(
        s.get(&format!("/api/v1/statuses/{top}"), Some(&carol))
            .status,
        404
    );
    // Public and local timelines stay household-only.
    let public = s.get("/api/v1/timelines/public", Some(&alice)).json_body();
    assert_eq!(public.as_array().unwrap().len(), 1);

    // Everything survives a reboot; the feed remembers its etag.
    let mut s = s.restart();
    let home = s.get("/api/v1/timelines/home", Some(&alice)).json_body();
    assert_eq!(home.as_array().unwrap().len(), 3);
    let jobs = s
        .get("/api/mastomini/v1/feeds/jobs", Some(&key))
        .json_body();
    assert_eq!(jobs["jobs"][0]["etag"], "\"e1\"");
    let mine = s.get("/api/mastomini/v1/remote", Some(&alice)).json_body();
    assert_eq!(mine["following"][0]["feed"]["posts"], 2);

    // The last unfollow deletes the account and its posts.
    let r = s.post_form(
        &format!("/api/v1/accounts/{bob}/unfollow"),
        Some(&alice),
        &[],
    );
    assert_eq!(r.json_body()["following"], false);
    assert!(s.svc.remote.accounts.is_empty() && s.svc.remote.items.is_empty());
    // Still found by the same search, and can be followed again.
    let again = s.get(
        &format!("/api/v2/search?q={}", crate::http::encode(BOB)),
        Some(&alice),
    );
    assert_eq!(again.json_body()["accounts"][0]["id"], bob.as_str());
    assert_eq!(push(&mut s, &key, &bob, json!([])).status, 410);
    let s = s.restart();
    assert!(s.svc.remote.items.is_empty());
}

#[test]
fn name_search_uses_the_configured_server() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let url = "/api/v2/search?q=bob&resolve=true&type=accounts";
    let req = Server::authed(Request::new("GET", url), Some(&alice));
    let lookup = pending(&mut s.svc, &s.ctx, &req, s.now).unwrap();
    assert_eq!(
        lookup.url,
        "https://mastodon.nu/api/v2/search?type=accounts&limit=5&q=bob"
    );
    let answer = json!({"accounts": [
        {"acct": "bob@other.social", "display_name": "Bob O"},
        {"acct": "bobby", "display_name": "Bobby"},
        {"acct": "evil@127.0.0.1", "display_name": "No"},
    ]});
    finish_lookup(
        &mut s.svc,
        lookup,
        Ok((200, answer.to_string().into_bytes())),
        s.now,
    );
    let accounts = s.get(url, Some(&alice)).json_body()["accounts"].clone();
    let accts: Vec<&str> = accounts
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["acct"].as_str().unwrap())
        .collect();
    assert_eq!(accts, ["bob@other.social", "bobby@mastodon.nu"]);

    // A submitted search asks even without resolve (apps rarely send it);
    // mention autocomplete does not, unless it asks to resolve.
    let plain = Server::authed(Request::new("GET", "/api/v2/search?q=carol"), Some(&alice));
    assert!(pending(&mut s.svc, &s.ctx, &plain, s.now).is_some());
    let typing = Server::authed(
        Request::new("GET", "/api/v1/accounts/search?q=dan"),
        Some(&alice),
    );
    assert!(pending(&mut s.svc, &s.ctx, &typing, s.now).is_none());
    let resolving = Server::authed(
        Request::new("GET", "/api/v1/accounts/search?q=bobby&resolve=true"),
        Some(&alice),
    );
    let lookup = pending(&mut s.svc, &s.ctx, &resolving, s.now).unwrap();
    finish_lookup(
        &mut s.svc,
        lookup,
        Ok((200, answer.to_string().into_bytes())),
        s.now,
    );
    let found = s.send(resolving).json_body();
    let accts: Vec<&str> = found
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["acct"].as_str().unwrap())
        .collect();
    assert!(accts.contains(&"bobby@mastodon.nu"), "{accts:?}");
    // No token, no search.
    let anon = Request::new("GET", "/api/v2/search?q=dave&resolve=true");
    assert!(pending(&mut s.svc, &s.ctx, &anon, s.now).is_none());
    // A failed search is cached briefly, and the search still answers locally.
    let req = Server::authed(
        Request::new("GET", "/api/v2/search?q=erin&resolve=true"),
        Some(&alice),
    );
    let lookup = pending(&mut s.svc, &s.ctx, &req, s.now).unwrap();
    finish_lookup(&mut s.svc, lookup, Err("timeout".into()), s.now);
    assert!(pending(&mut s.svc, &s.ctx, &req, s.now).is_none());
    assert!(
        pending(&mut s.svc, &s.ctx, &req, s.now + 61_000).is_some(),
        "a failure is tried again after a minute"
    );
    assert_eq!(s.send(req).status, 200);

    // An admin can turn name searches off, or the whole feature.
    let admin = s.login("alice", "alicepw", "read write admin:read admin:write");
    let r = s.send_json(
        "PUT",
        "/api/mastomini/v1/admin/remote",
        Some(&admin),
        &json!({"search_server": ""}),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let req = Server::authed(
        Request::new("GET", "/api/v2/search?q=frank&resolve=true"),
        Some(&alice),
    );
    assert!(pending(&mut s.svc, &s.ctx, &req, s.now).is_none());
    let r = s.send_json(
        "PUT",
        "/api/mastomini/v1/admin/remote",
        Some(&admin),
        &json!({"enabled": false}),
    );
    assert_eq!(r.json_body()["enabled"], false);
    let req = Server::authed(
        Request::new("GET", "/api/v2/search?q=@g@example.org"),
        Some(&alice),
    );
    assert!(pending(&mut s.svc, &s.ctx, &req, s.now).is_none());
}

#[test]
fn searches_are_rate_limited_per_member() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read");
    for i in 0..crate::domain::remote::SEARCHES_PER_HOUR {
        let req = Server::authed(
            Request::new("GET", &format!("/api/v2/search?q=name{i}&resolve=true")),
            Some(&alice),
        );
        assert!(pending(&mut s.svc, &s.ctx, &req, s.now).is_some(), "{i}");
    }
    let req = Server::authed(
        Request::new("GET", "/api/v2/search?q=late&resolve=true"),
        Some(&alice),
    );
    assert!(pending(&mut s.svc, &s.ctx, &req, s.now).is_none());
    assert!(pending(&mut s.svc, &s.ctx, &req, s.now + 3_600_000).is_some());
}

#[test]
fn a_handle_that_does_not_answer_lookup_can_still_be_followed() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let url = "/api/v1/accounts/lookup?acct=zed@blog.example";
    let req = Server::authed(Request::new("GET", url), Some(&alice));
    let lookup = pending(&mut s.svc, &s.ctx, &req, s.now).unwrap();
    finish_lookup(&mut s.svc, lookup, Ok((401, Vec::new())), s.now);
    let r = s.get(url, Some(&alice));
    assert_eq!(r.json_body()["acct"], "zed@blog.example");
    assert_eq!(r.json_body()["display_name"], "zed");
    // A server that says the account does not exist: not found.
    let url = "/api/v1/accounts/lookup?acct=nobody@example.org";
    let req = Server::authed(Request::new("GET", url), Some(&alice));
    let lookup = pending(&mut s.svc, &s.ctx, &req, s.now).unwrap();
    finish_lookup(&mut s.svc, lookup, Ok((404, Vec::new())), s.now);
    assert_eq!(s.get(url, Some(&alice)).status, 404);
}

#[test]
fn limits_are_set_by_an_admin() {
    let mut s = Server::provisioned();
    let (alice, bob) = found_bob(&mut s);
    let member = s.add_member(&alice, "carol");
    let r = s.send_json(
        "PUT",
        "/api/mastomini/v1/admin/remote",
        Some(&member),
        &json!({"feeds_max_account": 1}),
    );
    assert_eq!(r.status, 403);
    let r = s.send_json(
        "PUT",
        "/api/mastomini/v1/admin/remote",
        Some(&alice),
        &json!({"feeds_max_account": 99}),
    );
    assert_eq!(r.status, 422);
    let r = s.send_json(
        "PUT",
        "/api/mastomini/v1/admin/remote",
        Some(&alice),
        &json!({"feeds_max_account": 1, "feeds_max_board": 1}),
    );
    assert_eq!(r.json_body()["feeds_max_account"], 1);

    assert_eq!(
        s.post_form(&format!("/api/v1/accounts/{bob}/follow"), Some(&alice), &[])
            .status,
        200
    );
    // Carol may follow Bob too: one feed counts once for the household.
    assert_eq!(
        s.post_form(
            &format!("/api/v1/accounts/{bob}/follow"),
            Some(&member),
            &[]
        )
        .status,
        200
    );
    let other = s.svc.remote_seen(
        crate::domain::remote::RemoteProfile {
            acct: "dee@example.org".into(),
            display_name: String::new(),
            note: String::new(),
        },
        s.now,
    );
    let other = other.unwrap();
    let r = s.post_form(
        &format!("/api/v1/accounts/{other}/follow"),
        Some(&alice),
        &[],
    );
    assert_eq!(r.status, 422);
    assert!(r.json_body()["error"]
        .as_str()
        .unwrap()
        .contains("Unfollow one first"));
    let r = s.post_form(
        &format!("/api/v1/accounts/{other}/follow"),
        Some(&member),
        &[],
    );
    assert_eq!(r.status, 422, "the household is at its limit");
    let view = s
        .get("/api/mastomini/v1/admin/remote", Some(&alice))
        .json_body();
    assert_eq!(view["accounts"][0]["followers"], json!(["alice", "carol"]));

    // A deleted member stops following.
    let carol_slot = s.svc.state.account_by_username("carol").unwrap().slot;
    let alice_slot = s.svc.state.account_by_username("alice").unwrap().slot;
    s.svc.delete_account(alice_slot, carol_slot, s.now).unwrap();
    let view = s
        .get("/api/mastomini/v1/admin/remote", Some(&alice))
        .json_body();
    assert_eq!(view["accounts"][0]["followers"], json!(["alice"]));
}

#[test]
fn device_keys_reach_only_the_feed_routes() {
    let mut s = Server::provisioned();
    let (alice, bob) = found_bob(&mut s);
    s.post_form(&format!("/api/v1/accounts/{bob}/follow"), Some(&alice), &[]);
    let key = device_key(&mut s, &alice);
    let member = s.add_member(&alice, "carol");
    let r = s.post_form(
        "/api/mastomini/v1/admin/device_keys",
        Some(&member),
        &[("name", "x")],
    );
    assert_eq!(r.status, 403);

    assert_eq!(s.get("/api/mastomini/v1/feeds/jobs", None).status, 401);
    assert_eq!(
        s.get("/api/mastomini/v1/feeds/jobs", Some(&alice)).status,
        403
    );
    for path in [
        "/api/v1/timelines/home",
        "/api/v1/accounts/verify_credentials",
        "/api/mastomini/v1/remote",
    ] {
        let status = s.get(path, Some(&key)).status;
        assert!(matches!(status, 401 | 403 | 422), "{path}: {status}");
    }
    let r = s.post_form("/api/v1/statuses", Some(&key), &[("status", "hi")]);
    assert!(matches!(r.status, 401 | 403 | 422));
    assert_eq!(
        s.get("/api/mastomini/v1/feeds/jobs", Some("mmdev_wrong"))
            .status,
        401
    );
    let bad = s.send_json(
        "POST",
        &format!("/api/mastomini/v1/feeds/{bob}/result"),
        Some(&key),
        &json!({"outcome": "maybe"}),
    );
    assert_eq!(bad.status, 422);

    // The admin page shows when the key was used; revoking it shuts it out.
    let view = s
        .get("/api/mastomini/v1/admin/remote", Some(&alice))
        .json_body();
    let id = view["device_keys"][0]["id"].as_str().unwrap().to_string();
    assert!(view["device_keys"][0]["last_used_at"].is_string());
    let r = s.send(Server::authed(
        Request::new(
            "DELETE",
            &format!("/api/mastomini/v1/admin/device_keys/{id}"),
        ),
        Some(&alice),
    ));
    assert_eq!(r.status, 200);
    assert_eq!(
        s.get("/api/mastomini/v1/feeds/jobs", Some(&key)).status,
        401
    );
}

#[test]
fn failures_and_gone_feeds_are_recorded() {
    let mut s = Server::provisioned();
    let (alice, bob) = found_bob(&mut s);
    s.post_form(&format!("/api/v1/accounts/{bob}/follow"), Some(&alice), &[]);
    let key = device_key(&mut s, &alice);
    let path = format!("/api/mastomini/v1/feeds/{bob}/result");
    s.send_json(
        "POST",
        &path,
        Some(&key),
        &json!({"outcome": "failed", "error": "timeout"}),
    );
    s.send_json("POST", &path, Some(&key), &json!({"outcome": "gone"}));
    let feed =
        s.get("/api/mastomini/v1/remote", Some(&alice)).json_body()["following"][0]["feed"].clone();
    assert_eq!(feed["failures"], 2);
    assert_eq!(feed["gone"], true);
    s.send_json(
        "POST",
        &path,
        Some(&key),
        &json!({"outcome": "not_modified"}),
    );
    let feed =
        s.get("/api/mastomini/v1/remote", Some(&alice)).json_body()["following"][0]["feed"].clone();
    assert_eq!(feed["failures"], 0);
    assert_eq!(feed["gone"], false);
    assert!(feed["last_ok_at"].is_string());
}

/// A JPEG header the size check reads: APP0, then a SOF0 of `w`×`h`.
fn jpeg(w: u16, h: u16) -> Vec<u8> {
    let mut b = vec![
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x0B, 0x08,
    ];
    b.extend_from_slice(&h.to_be_bytes());
    b.extend_from_slice(&w.to_be_bytes());
    b.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
    b
}

fn upload(s: &mut Server, key: &str, account: &str, bytes: Vec<u8>) -> Response {
    let req = Request::new("POST", &format!("/api/mastomini/v1/feeds/{account}/media"))
        .with_body("image/jpeg", bytes);
    s.send(Server::authed(req, Some(key)))
}

#[test]
fn images_need_a_media_directory() {
    let mut s = Server::provisioned();
    let (alice, bob) = found_bob(&mut s);
    s.post_form(&format!("/api/v1/accounts/{bob}/follow"), Some(&alice), &[]);
    let key = device_key(&mut s, &alice);
    assert_eq!(
        upload(&mut s, &key, &bob, jpeg(64, 48)).status,
        503,
        "no SD card, no images"
    );

    let dir = tempfile::tempdir().unwrap();
    s.svc
        .set_media(Some(crate::media::MediaDir::open(dir.path()).unwrap()));
    let jobs = s
        .get("/api/mastomini/v1/feeds/jobs", Some(&key))
        .json_body();
    assert_eq!(jobs["images"], true);
    assert_eq!(jobs["jobs"][0]["wants_avatar"], true);
    assert_eq!(upload(&mut s, &key, &bob, b"GIF89a".to_vec()).status, 422);
    assert_eq!(upload(&mut s, &key, &bob, jpeg(2000, 100)).status, 422);
    let image = upload(&mut s, &key, &bob, jpeg(64, 48)).json_body()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let avatar = upload(&mut s, &key, &bob, jpeg(32, 32)).json_body()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let unused = upload(&mut s, &key, &bob, jpeg(10, 10)).json_body()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut item = post("a picture", "p1", s.now - 1000);
    item["media"] = json!([{"id": image, "description": "a cat"}]);
    let r = s.send_json(
        "POST",
        &format!("/api/mastomini/v1/feeds/{bob}/result"),
        Some(&key),
        &json!({"outcome": "ok", "avatar": avatar, "items": [item]}),
    );
    assert_eq!(r.json_body()["added"], 1);
    let home = s.get("/api/v1/timelines/home", Some(&alice)).json_body();
    let media = home[0]["media_attachments"][0].clone();
    assert_eq!(media["description"], "a cat");
    assert_eq!(media["meta"]["original"]["width"], 64);
    let url = media["url"].as_str().unwrap();
    let path = url
        .strip_prefix("http://mastomini.test")
        .unwrap()
        .to_string();
    let r = s.get(&path, None);
    assert_eq!(r.status, 200);
    assert!(r.file.as_ref().unwrap().exists());
    assert!(r.header("Cache-Control").unwrap().contains("immutable"));
    assert!(home[0]["account"]["avatar"]
        .as_str()
        .unwrap()
        .contains("/media/remote/"));
    // The upload nobody named was deleted; an unknown file is not served.
    let unused_path = s
        .svc
        .remote
        .media
        .as_ref()
        .unwrap()
        .path(unused.parse().unwrap());
    assert!(!unused_path.exists());
    assert_eq!(s.get("/media/remote/0000000000000.jpg", None).status, 404);

    // Unfollowing deletes the files too.
    s.post_form(
        &format!("/api/v1/accounts/{bob}/unfollow"),
        Some(&alice),
        &[],
    );
    assert_eq!(
        std::fs::read_dir(dir.path().join("remote"))
            .unwrap()
            .count(),
        0
    );
}

/// Answer each request the server adapter would send, in order, then handle
/// the request, as `server::Household` does with a real network. Returns
/// the response and what was sent.
fn drive(s: &mut Server, req: Request, answers: &[(u16, Value)]) -> (Response, Vec<Outbound>) {
    let mut sent = Vec::new();
    for (status, body) in answers {
        let Some(out) = pending(&mut s.svc, &s.ctx, &req, s.now) else {
            break;
        };
        sent.push(out.clone());
        finish_lookup(
            &mut s.svc,
            out,
            Ok((*status, body.to_string().into_bytes())),
            s.now,
        );
    }
    assert!(
        pending(&mut s.svc, &s.ctx, &req, s.now).is_none(),
        "every request answered"
    );
    (s.send(req), sent)
}

fn header<'a>(out: &'a Outbound, name: &str) -> Option<&'a str> {
    out.headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn query(url: &str, name: &str) -> String {
    let q = url.split_once('?').map_or("", |(_, q)| q);
    form_urlencoded::parse(q.as_bytes())
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

/// Alice signs the household in to mastodon.social; the authorize URL.
fn sign_in(s: &mut Server, admin: &str) -> String {
    let start = Server::authed(
        Request::new("POST", "/api/mastomini/v1/admin/search_login").with_body(
            "application/json",
            json!({"server": "mastodon.social"}).to_string(),
        ),
        Some(admin),
    );
    let (r, sent) = drive(
        s,
        start,
        &[(200, json!({"client_id": "cid", "client_secret": "csecret"}))],
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(sent[0].url, "https://mastodon.social/api/v1/apps");
    let body = String::from_utf8(sent[0].body.clone()).unwrap();
    assert!(
        body.contains("scopes=read%3Asearch+read%3Astatuses+read%3Aaccounts"),
        "{body}"
    );
    assert!(
        body.contains("redirect_uris=http%3A%2F%2Fmastomini.test%2Fapi%2Fmastomini%2Fv1%2Fremote%2Foauth%2Fcallback"),
        "{body}"
    );
    let url = r.json_body()["authorize_url"].as_str().unwrap().to_string();
    assert!(url.starts_with("https://mastodon.social/oauth/authorize?"));
    assert_eq!(query(&url, "client_id"), "cid");
    assert_eq!(query(&url, "code_challenge_method"), "S256");
    assert_eq!(
        query(&url, "scope"),
        "read:search read:statuses read:accounts"
    );
    assert_eq!(
        query(&url, "redirect_uri"),
        "http://mastomini.test/api/mastomini/v1/remote/oauth/callback"
    );
    url
}

fn callback(url: &str, code: &str) -> Request {
    let state = crate::http::encode(&query(url, "state"));
    Request::new(
        "GET",
        &format!("/api/mastomini/v1/remote/oauth/callback?code={code}&state={state}"),
    )
}

#[test]
fn an_admin_signs_in_to_the_search_server_with_pkce() {
    let mut s = Server::provisioned();
    let alice = s.login(
        "alice",
        "alicepw",
        "read write follow admin:read admin:write",
    );
    let member = s.add_member(&alice, "max");
    let r = s.send_json(
        "POST",
        "/api/mastomini/v1/admin/search_login",
        Some(&member),
        &json!({"server": "mastodon.social"}),
    );
    assert_eq!(r.status, 403, "only an admin signs the household in");

    let url = sign_in(&mut s, &alice);
    let challenge = query(&url, "code_challenge");
    let (r, sent) = drive(
        &mut s,
        callback(&url, "thecode"),
        &[
            (200, json!({"access_token": "tok-secret"})),
            (200, json!({"acct": "matt", "username": "matt"})),
        ],
    );
    assert_eq!(r.status, 302);
    assert_eq!(
        r.header("Location"),
        Some("/app/#/admin/remote?search=connected")
    );
    // The code exchange proves the verifier behind the challenge.
    assert_eq!(sent[0].url, "https://mastodon.social/oauth/token");
    let body = String::from_utf8(sent[0].body.clone()).unwrap();
    let verifier: String = form_urlencoded::parse(body.as_bytes())
        .find(|(k, _)| k == "code_verifier")
        .map(|(_, v)| v.into_owned())
        .unwrap();
    use base64::Engine;
    assert_eq!(
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(crate::auth::sha256(&verifier)),
        challenge
    );
    assert!(body.contains("code=thecode") && body.contains("client_secret=csecret"));
    assert_eq!(
        sent[1].url,
        "https://mastodon.social/api/v1/accounts/verify_credentials"
    );
    assert_eq!(header(&sent[1], "Authorization"), Some("Bearer tok-secret"));

    // The admin page says who; the token is never shown.
    let view = s.get("/api/mastomini/v1/admin/remote", Some(&alice));
    assert_eq!(view.json_body()["search_account"], "matt@mastodon.social");
    assert_eq!(view.json_body()["search_server"], "mastodon.social");
    assert!(!String::from_utf8_lossy(&view.body).contains("tok-secret"));
    let mine = s.get("/api/mastomini/v1/remote", Some(&member));
    assert!(!String::from_utf8_lossy(&mine.body).contains("tok-secret"));
    // A used state goes nowhere.
    let (r, sent) = drive(&mut s, callback(&url, "again"), &[]);
    assert!(sent.is_empty());
    assert!(r.header("Location").unwrap().contains("search=failed"));

    // It survives a reboot, and signing out forgets it.
    let mut s = s.restart();
    assert!(s.svc.search_token().is_some());
    let r = s.send(Server::authed(
        Request::new("DELETE", "/api/mastomini/v1/admin/search_login"),
        Some(&alice),
    ));
    assert_eq!(r.status, 200);
    let s = s.restart();
    assert!(s.svc.search_token().is_none());
}

#[test]
fn a_refused_sign_in_reports_why() {
    let mut s = Server::provisioned();
    let alice = s.login(
        "alice",
        "alicepw",
        "read write follow admin:read admin:write",
    );
    let url = sign_in(&mut s, &alice);
    let state = crate::http::encode(&query(&url, "state"));
    let denied = Request::new(
        "GET",
        &format!("/api/mastomini/v1/remote/oauth/callback?error=access_denied&state={state}"),
    );
    let (r, sent) = drive(&mut s, denied, &[]);
    assert!(sent.is_empty());
    assert!(r
        .header("Location")
        .unwrap()
        .contains("search=failed&reason="));
    assert!(s.svc.search_token().is_none());
    // A server that will not register us: a clear error, and no retry loop.
    let start = Server::authed(
        Request::new("POST", "/api/mastomini/v1/admin/search_login").with_body(
            "application/json",
            json!({"server": "closed.example"}).to_string(),
        ),
        Some(&alice),
    );
    let (r, sent) = drive(&mut s, start, &[(403, json!({"error": "no"}))]);
    assert_eq!(sent.len(), 1);
    assert_eq!(r.status, 503);
    assert!(r.json_body()["error"]
        .as_str()
        .unwrap()
        .contains("closed.example"));
}

#[test]
fn signed_in_searches_find_posts_and_hashtags() {
    let mut s = Server::provisioned();
    let alice = s.login(
        "alice",
        "alicepw",
        "read write follow admin:read admin:write",
    );
    let url = sign_in(&mut s, &alice);
    drive(
        &mut s,
        callback(&url, "c"),
        &[
            (200, json!({"access_token": "tok"})),
            (200, json!({"acct": "matt"})),
        ],
    );
    let max = s.add_member(&alice, "max");
    let carver = json!({"acct": "carver@wood.example", "display_name": "Carver", "note": ""});
    let post = |id: &str, text: &str, vis: &str| {
        json!({
            "id": id, "url": format!("https://wood.example/@carver/{id}"), "visibility": vis,
            "content": format!("<p>{text}</p>"), "spoiler_text": "", "sensitive": false,
            "created_at": "2026-10-22T10:00:00.000Z", "account": carver,
            "media_attachments": [{"type": "image"}],
        })
    };
    // A word: accounts and posts in one signed-in request, resolved.
    let req = Server::authed(Request::new("GET", "/api/v2/search?q=spoons"), Some(&max));
    let answer = json!({
        "accounts": [carver],
        "statuses": [post("1", "Carving spoons", "public"), post("2", "secret", "private")],
        "hashtags": [],
    });
    let (r, sent) = drive(&mut s, req, &[(200, answer)]);
    assert_eq!(
        sent[0].url,
        "https://mastodon.social/api/v2/search?limit=5&q=spoons"
    );
    assert_eq!(header(&sent[0], "Authorization"), Some("Bearer tok"));
    let body = r.json_body();
    assert_eq!(body["accounts"][0]["acct"], "carver@wood.example");
    let statuses = body["statuses"].as_array().unwrap();
    assert_eq!(statuses.len(), 1, "followers-only posts are left out");
    assert_eq!(statuses[0]["url"], "https://wood.example/@carver/1");
    assert!(statuses[0]["content"]
        .as_str()
        .unwrap()
        .contains("1 picture on the original post"));
    assert_eq!(
        statuses[0]["media_attachments"],
        json!([]),
        "pictures are never fetched for a search"
    );
    // The post opens, and its author can be followed (by RSS).
    let id = statuses[0]["id"].as_str().unwrap();
    assert_eq!(
        s.get(&format!("/api/v1/statuses/{id}"), Some(&max)).status,
        200
    );
    let author = statuses[0]["account"]["id"].as_str().unwrap();
    let r = s.post_form(
        &format!("/api/v1/accounts/{author}/follow"),
        Some(&max),
        &[],
    );
    assert_eq!(r.json_body()["following"], true);
    // The posts tab alone reuses the answer.
    let posts = Server::authed(
        Request::new("GET", "/api/v2/search?q=spoons&type=statuses"),
        Some(&max),
    );
    let (r, sent) = drive(&mut s, posts, &[]);
    assert!(sent.is_empty());
    assert_eq!(
        r.json_body()["statuses"][0]["url"],
        "https://wood.example/@carver/1"
    );

    // A hashtag: its timeline on the search server.
    let tag = Server::authed(
        Request::new("GET", "/api/v2/search?q=%23woodworking&type=statuses"),
        Some(&max),
    );
    let (r, sent) = drive(
        &mut s,
        tag,
        &[(200, json!([post("3", "A bowl #woodworking", "public")]))],
    );
    assert_eq!(
        sent[0].url,
        "https://mastodon.social/api/v1/timelines/tag/woodworking?limit=20"
    );
    assert_eq!(header(&sent[0], "Authorization"), Some("Bearer tok"));
    assert_eq!(
        r.json_body()["statuses"][0]["url"],
        "https://wood.example/@carver/3"
    );

    // Without a sign-in a post search does not leave the board.
    let r = s.send(Server::authed(
        Request::new("DELETE", "/api/mastomini/v1/admin/search_login"),
        Some(&alice),
    ));
    assert_eq!(r.status, 200);
    let req = Server::authed(
        Request::new("GET", "/api/v2/search?q=bowls&type=statuses"),
        Some(&max),
    );
    assert!(pending(&mut s.svc, &s.ctx, &req, s.now).is_none());
    let req = Server::authed(Request::new("GET", "/api/v2/search?q=bowls"), Some(&max));
    let out = pending(&mut s.svc, &s.ctx, &req, s.now).unwrap();
    assert!(out.url.contains("type=accounts") && header(&out, "Authorization").is_none());
}
