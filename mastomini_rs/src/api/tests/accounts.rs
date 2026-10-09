use super::*;

fn account_id(s: &mut Server, token: &str) -> String {
    s.get("/api/v1/accounts/verify_credentials", Some(token))
        .json_body()["id"]
        .as_str()
        .unwrap()
        .into()
}

fn usernames(value: &serde_json::Value) -> Vec<&str> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["username"].as_str().unwrap())
        .collect()
}

#[test]
fn lookup_and_batch_queries_reject_remote_or_malformed_accounts() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    let bob = s.add_member(&alice, "bob");
    let bob_id = account_id(&mut s, &bob);
    for acct in ["bob", "@bob", "bob@MASTOMINI.TEST"] {
        let r = s.get(
            &format!("/api/v1/accounts/lookup?acct={acct}"),
            Some(&alice),
        );
        assert_eq!(r.status, 200);
        assert_eq!(r.json_body()["id"], bob_id);
    }
    for acct in ["bob@elsewhere.test", "missing", ""] {
        assert_eq!(
            s.get(
                &format!("/api/v1/accounts/lookup?acct={acct}"),
                Some(&alice)
            )
            .status,
            404
        );
    }
    for endpoint in ["accounts", "accounts/relationships"] {
        let path = format!("/api/v1/{endpoint}?id[]=!&id[]=1&id[]={bob_id}");
        assert_eq!(s.get(&path, None).status, 401);
        let r = s.get(&path, Some(&alice));
        assert_eq!(r.status, 200);
        assert_eq!(r.json_body().as_array().unwrap().len(), 1);
        assert_eq!(r.json_body()[0]["id"], bob_id);
        let query = (0..45)
            .map(|_| format!("id[]={bob_id}"))
            .collect::<Vec<_>>()
            .join("&");
        assert_eq!(
            s.get(&format!("/api/v1/{endpoint}?{query}"), Some(&alice))
                .json_body()
                .as_array()
                .unwrap()
                .len(),
            40
        );
    }
}

#[test]
fn following_search_and_familiar_followers_respect_visibility_changes() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let carol = s.add_member(&alice, "carol");
    let bob_id = account_id(&mut s, &bob);
    let carol_id = account_id(&mut s, &carol);
    s.post_form(
        &format!("/api/v1/accounts/{bob_id}/follow"),
        Some(&alice),
        &[],
    );
    s.post_form(
        &format!("/api/v1/accounts/{carol_id}/follow"),
        Some(&bob),
        &[],
    );
    let rows = s
        .get("/api/v1/accounts/search?q=&following=true", Some(&alice))
        .json_body();
    assert_eq!(usernames(&rows), ["bob"]);
    let path = format!("/api/v1/accounts/familiar_followers?id[]=!&id[]={carol_id}");
    let rows = s.get(&path, Some(&alice)).json_body();
    assert_eq!(usernames(&rows[0]["accounts"]), ["bob"]);
    s.post_form(
        &format!("/api/v1/accounts/{bob_id}/mute"),
        Some(&alice),
        &[],
    );
    assert_eq!(
        s.get(&path, Some(&alice)).json_body()[0]["accounts"],
        json!([])
    );
    s.post_form(
        &format!("/api/v1/accounts/{bob_id}/unmute"),
        Some(&alice),
        &[],
    );
    s.post_form(
        &format!("/api/v1/accounts/{bob_id}/block"),
        Some(&alice),
        &[],
    );
    assert_eq!(
        s.get("/api/v1/accounts/search?q=bob", Some(&alice))
            .json_body(),
        json!([])
    );
    let mut s = s.restart();
    assert_eq!(
        s.get(&path, Some(&alice)).json_body()[0]["accounts"],
        json!([])
    );
    let limited = s.login("alice", "alicepw", "read:statuses");
    assert_eq!(s.get(&path, Some(&limited)).status, 403);
}

#[test]
fn removing_a_follower_preserves_the_opposite_follow_and_allows_refollow() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let alice_id = account_id(&mut s, &alice);
    let bob_id = account_id(&mut s, &bob);
    for (token, id) in [(&alice, &bob_id), (&bob, &alice_id)] {
        assert_eq!(
            s.post_form(&format!("/api/v1/accounts/{id}/follow"), Some(token), &[])
                .status,
            200
        );
    }
    let r = s.post_form(
        &format!("/api/v1/accounts/{bob_id}/remove_from_followers"),
        Some(&alice),
        &[],
    );
    assert_eq!(r.status, 200);
    let rel = r.json_body();
    assert_eq!(rel["followed_by"], false);
    assert_eq!(rel["following"], true);
    assert_eq!(rel["blocking"], false);
    let mut s = s.restart();
    assert_eq!(
        usernames(
            &s.get(
                &format!("/api/v1/accounts/{alice_id}/following"),
                Some(&alice)
            )
            .json_body()
        ),
        ["bob"]
    );
    assert_eq!(
        s.get(
            &format!("/api/v1/accounts/{alice_id}/followers"),
            Some(&alice)
        )
        .json_body(),
        json!([])
    );
    assert_eq!(
        s.post_form(
            &format!("/api/v1/accounts/{alice_id}/follow"),
            Some(&bob),
            &[]
        )
        .json_body()["following"],
        true
    );
}

#[test]
fn profile_validation_is_atomic_and_language_can_be_cleared() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    let path = "/api/v1/accounts/update_credentials";
    let baseline = s
        .get("/api/v1/accounts/verify_credentials", Some(&alice))
        .json_body();
    for body in [
        json!({"display_name":"must not persist", "source":{"privacy":"direct"}}),
        json!({"display_name":"must not persist", "note":"x".repeat(161)}),
        json!({"display_name":"must not persist", "fields_attributes":[{"name":"x".repeat(41),"value":"bad"}]}),
        json!({"display_name":"must not persist", "source":{"language":"too-long-language"}}),
    ] {
        assert_eq!(s.send_json("PATCH", path, Some(&alice), &body).status, 422);
        assert_eq!(
            s.get("/api/v1/accounts/verify_credentials", Some(&alice))
                .json_body(),
            baseline
        );
    }
    assert_eq!(
        s.send_json(
            "PATCH",
            path,
            Some(&alice),
            &json!({"source":{"language":"en","privacy":"private","sensitive":true}})
        )
        .status,
        200
    );
    let prefs = s.get("/api/v1/preferences", Some(&alice)).json_body();
    assert_eq!(prefs["posting:default:language"], "en");
    assert_eq!(prefs["posting:default:visibility"], "private");
    assert_eq!(prefs["posting:default:sensitive"], true);
    assert_eq!(
        s.send_json(
            "PATCH",
            path,
            Some(&alice),
            &json!({"source":{"language":""}})
        )
        .status,
        200
    );
    let mut s = s.restart();
    assert!(
        s.get("/api/v1/preferences", Some(&alice)).json_body()["posting:default:language"]
            .is_null()
    );
}

#[test]
fn account_status_queries_apply_flags_tags_pins_and_cursors() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    let id = account_id(&mut s, &alice);
    let bob = s.add_member(&alice, "bob");
    let root = s
        .post_form(
            "/api/v1/statuses",
            Some(&alice),
            &[("status", "first #rust")],
        )
        .json_body()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let bob_post = s
        .post_form(
            "/api/v1/statuses",
            Some(&bob),
            &[("status", "other author's root")],
        )
        .json_body()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let reply = s
        .post_form(
            "/api/v1/statuses",
            Some(&alice),
            &[("status", "reply"), ("in_reply_to_id", &bob_post)],
        )
        .json_body()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        s.post_form(&format!("/api/v1/statuses/{root}/pin"), Some(&alice), &[])
            .status,
        200
    );
    let path = format!("/api/v1/accounts/{id}/statuses");
    for query in ["exclude_replies=true", "pinned=true", "tagged=rust"] {
        let rows = s.get(&format!("{path}?{query}"), Some(&alice)).json_body();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["id"], root);
    }
    assert_eq!(
        s.get(&format!("{path}?only_media=true"), Some(&alice))
            .json_body(),
        json!([])
    );
    let newest = s.get(&format!("{path}?limit=1"), Some(&alice));
    assert_eq!(newest.json_body()[0]["id"], reply);
    assert!(newest.header("Link").is_some());
    assert_eq!(
        s.get(&format!("{path}?max_id={reply}"), Some(&alice))
            .json_body()[0]["id"],
        root
    );
}
