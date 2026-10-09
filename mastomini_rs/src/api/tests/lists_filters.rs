//! Follow requests, lists and filters through the full request path.

use super::*;

fn id_of(v: &serde_json::Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

#[test]
fn filter_updates_group_form_rows_and_preserve_state_on_validation_errors() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    let r = s.post_form(
        "/api/v2/filters",
        Some(&alice),
        &[
            ("title", "Animals"),
            ("context[]", "home"),
            ("expires_in", "60"),
            ("keywords_attributes[][keyword]", "cat"),
            ("keywords_attributes[][whole_word]", "true"),
            ("keywords_attributes[][keyword]", "dog"),
            ("keywords_attributes[][whole_word]", "false"),
        ],
    );
    assert_eq!(r.status, 200);
    let before = r.json_body();
    assert_eq!(before["keywords"].as_array().unwrap().len(), 2);
    assert_eq!(before["keywords"][0]["whole_word"], true);
    assert_eq!(before["keywords"][1]["whole_word"], false);
    let path = format!("/api/v2/filters/{}", id_of(&before));
    for body in [
        json!({"title":"must not persist", "context":["unknown"]}),
        json!({"title":"must not persist", "filter_action":"unknown"}),
        json!({"title":"must not persist", "expires_in":"not a number"}),
        json!({"title":"must not persist", "keywords_attributes":[{"keyword":"x".repeat(41)}]}),
    ] {
        assert_eq!(s.send_json("PUT", &path, Some(&alice), &body).status, 422);
        assert_eq!(s.get(&path, Some(&alice)).json_body(), before);
    }
    let cat = id_of(&before["keywords"][0]);
    let dog = id_of(&before["keywords"][1]);
    let updated = s.send_json("PUT", &path, Some(&alice), &json!({
        "title":"Updated", "context":["public","account"], "filter_action":"blur", "expires_in":"",
        "keywords_attributes":[{"id":cat,"_destroy":true},{"id":dog,"keyword":"hound","whole_word":true},{"keyword":"bird"}]
    }));
    assert_eq!(updated.status, 200);
    let updated = updated.json_body();
    assert_eq!(updated["context"], json!(["public", "account"]));
    assert_eq!(updated["filter_action"], "blur");
    assert!(updated["expires_at"].is_null());
    assert_eq!(updated["keywords"].as_array().unwrap().len(), 2);
    assert_eq!(updated["keywords"][0]["id"], dog);
    assert_eq!(updated["keywords"][0]["keyword"], "hound");
    assert_eq!(updated["keywords"][0]["whole_word"], true);
    let mut s = s.restart();
    assert_eq!(s.get(&path, Some(&alice)).json_body(), updated);
    assert_eq!(
        s.get(&format!("/api/v2/filters/keywords/{cat}"), Some(&alice))
            .status,
        404
    );
}

#[test]
fn filter_parts_are_owner_scoped_deduplicated_and_independently_removable() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    let bob = s.add_member(&alice, "bob");
    let filter = s
        .post_form(
            "/api/v2/filters",
            Some(&alice),
            &[("title", "Posts"), ("context[]", "public")],
        )
        .json_body();
    let path = format!("/api/v2/filters/{}", id_of(&filter));
    let post = s
        .post_form("/api/v1/statuses", Some(&bob), &[("status", "ordinary")])
        .json_body();
    let post_id = id_of(&post);
    let kw = s
        .post_form(
            &format!("{path}/keywords"),
            Some(&alice),
            &[("keyword", "never-match")],
        )
        .json_body();
    let status = s
        .post_form(
            &format!("{path}/statuses"),
            Some(&alice),
            &[("status_id", &post_id)],
        )
        .json_body();
    assert_eq!(
        s.post_form(
            &format!("{path}/statuses"),
            Some(&alice),
            &[("status_id", &post_id)]
        )
        .json_body(),
        status
    );
    let keyword_path = format!("/api/v2/filters/keywords/{}", id_of(&kw));
    let status_path = format!("/api/v2/filters/statuses/{}", id_of(&status));
    for child in [&keyword_path, &status_path] {
        assert_eq!(s.get(child, Some(&bob)).status, 404);
        assert_eq!(
            s.send_json("DELETE", child, Some(&bob), &json!({})).status,
            404
        );
        assert_eq!(s.get(child, Some(&alice)).status, 200);
    }
    assert_eq!(
        s.get(&format!("{path}/keywords"), Some(&alice)).json_body(),
        json!([kw])
    );
    assert_eq!(
        s.get(&format!("{path}/statuses"), Some(&alice)).json_body(),
        json!([status])
    );
    let timeline = s.get("/api/v1/timelines/public", Some(&alice)).json_body();
    assert_eq!(
        timeline[0]["filtered"][0]["status_matches"],
        json!([post_id])
    );
    assert!(timeline[0]["filtered"][0]["keyword_matches"].is_null());
    let private = s
        .post_form(
            "/api/v1/statuses",
            Some(&bob),
            &[("status", "followers only"), ("visibility", "private")],
        )
        .json_body();
    assert_eq!(
        s.post_form(
            &format!("{path}/statuses"),
            Some(&alice),
            &[("status_id", &id_of(&private))]
        )
        .status,
        404
    );
    assert_eq!(
        s.send_json("DELETE", &keyword_path, Some(&alice), &json!({}))
            .status,
        200
    );
    assert_eq!(
        s.send_json("DELETE", &status_path, Some(&alice), &json!({}))
            .status,
        200
    );
    let mut s = s.restart();
    assert_eq!(s.get(&status_path, Some(&alice)).status, 404);
    assert_eq!(
        s.get(&format!("{path}/statuses"), Some(&alice)).json_body(),
        json!([])
    );
    assert_eq!(
        s.send_json("DELETE", &path, Some(&alice), &json!({}))
            .status,
        200
    );
    assert_eq!(s.get(&path, Some(&alice)).status, 404);
}

#[test]
fn v1_filter_updates_share_v2_state_without_deleting_sibling_keywords() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    let v1 = s
        .post_form(
            "/api/v1/filters",
            Some(&alice),
            &[("phrase", "cat"), ("context[]", "home")],
        )
        .json_body();
    let path = format!("/api/v1/filters/{}", id_of(&v1));
    let f = s.get("/api/v2/filters", Some(&alice)).json_body()[0].clone();
    let filter_path = format!("/api/v2/filters/{}", id_of(&f));
    assert_eq!(
        s.post_form(
            &format!("{filter_path}/keywords"),
            Some(&alice),
            &[("keyword", "dog")]
        )
        .status,
        200
    );
    let updated = send(
        &mut s,
        "PUT",
        &path,
        &alice,
        &[
            ("phrase", " kitten "),
            ("whole_word", "false"),
            ("irreversible", "true"),
            ("context[]", "account"),
            ("expires_in", "-1"),
        ],
    );
    assert_eq!(updated.status, 200);
    assert_eq!(updated.json_body()["phrase"], "kitten");
    assert_eq!(updated.json_body()["irreversible"], true);
    assert_eq!(updated.json_body()["whole_word"], false);
    assert_eq!(s.get(&path, Some(&alice)).json_body(), updated.json_body());
    let changed = s.get(&filter_path, Some(&alice)).json_body();
    assert_eq!(changed["context"], json!(["account"]));
    assert_eq!(changed["filter_action"], "hide");
    assert_eq!(changed["keywords"].as_array().unwrap().len(), 2);
    assert_eq!(send(&mut s, "DELETE", &path, &alice, &[]).status, 200);
    let remaining = s.get(&filter_path, Some(&alice)).json_body();
    assert_eq!(remaining["keywords"].as_array().unwrap().len(), 1);
    assert_eq!(remaining["keywords"][0]["keyword"], "dog");
    assert_eq!(
        send(
            &mut s,
            "DELETE",
            &format!("/api/v1/filters/{}", id_of(&remaining["keywords"][0])),
            &alice,
            &[]
        )
        .status,
        200
    );
    assert_eq!(s.get(&filter_path, Some(&alice)).status, 404);
}

fn me(s: &mut Server, token: &str) -> String {
    id_of(
        &s.get("/api/v1/accounts/verify_credentials", Some(token))
            .json_body(),
    )
}

fn send(
    s: &mut Server,
    method: &str,
    url: &str,
    token: &str,
    form: &[(&str, &str)],
) -> crate::http::Response {
    let req = Request::new(method, url)
        .with_body(
            "application/x-www-form-urlencoded",
            super::super::testkit::form(form),
        )
        .with_header("Authorization", &format!("Bearer {token}"));
    s.send(req)
}

#[test]
fn locked_accounts_answer_follow_requests() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let carol = s.add_member(&alice, "carol");
    let (alice_id, bob_id, carol_id) = (me(&mut s, &alice), me(&mut s, &bob), me(&mut s, &carol));
    let r = send(
        &mut s,
        "PATCH",
        "/api/v1/accounts/update_credentials",
        &carol,
        &[("locked", "true")],
    );
    assert_eq!(r.status, 200);

    let rel = s
        .post_form(
            &format!("/api/v1/accounts/{carol_id}/follow"),
            Some(&alice),
            &[],
        )
        .json_body();
    assert_eq!(rel["requested"], true);
    assert_eq!(rel["following"], false);
    s.post_form(
        &format!("/api/v1/accounts/{carol_id}/follow"),
        Some(&bob),
        &[],
    );

    let waiting = s.get("/api/v1/follow_requests", Some(&carol)).json_body();
    let ids: Vec<String> = waiting.as_array().unwrap().iter().map(id_of).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&alice_id) && ids.contains(&bob_id));
    let me_cred = s
        .get("/api/v1/accounts/verify_credentials", Some(&carol))
        .json_body();
    assert_eq!(me_cred["source"]["follow_requests_count"], 2);
    let notes = s
        .get("/api/v1/notifications?types[]=follow_request", Some(&carol))
        .json_body();
    assert_eq!(notes.as_array().unwrap().len(), 2);
    let seen_by_carol = s
        .get(
            &format!("/api/v1/accounts/relationships?id[]={alice_id}"),
            Some(&carol),
        )
        .json_body();
    assert_eq!(seen_by_carol[0]["requested_by"], true);

    // Private posts stay private until accepted.
    s.post_form(
        "/api/v1/statuses",
        Some(&carol),
        &[("status", "for followers"), ("visibility", "private")],
    );
    assert!(s
        .get("/api/v1/timelines/home", Some(&alice))
        .json_body()
        .as_array()
        .unwrap()
        .is_empty());

    let rel = s
        .post_form(
            &format!("/api/v1/follow_requests/{alice_id}/authorize"),
            Some(&carol),
            &[],
        )
        .json_body();
    assert_eq!(rel["followed_by"], true);
    let rel = s
        .get(
            &format!("/api/v1/accounts/relationships?id[]={carol_id}"),
            Some(&alice),
        )
        .json_body();
    assert_eq!(rel[0]["following"], true);
    assert_eq!(rel[0]["requested"], false);
    assert_eq!(
        s.get("/api/v1/timelines/home", Some(&alice))
            .json_body()
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let r = s.post_form(
        &format!("/api/v1/follow_requests/{bob_id}/reject"),
        Some(&carol),
        &[],
    );
    assert_eq!(r.status, 200);
    let r = s.post_form(
        &format!("/api/v1/follow_requests/{bob_id}/reject"),
        Some(&carol),
        &[],
    );
    assert_eq!(r.status, 404);

    // Unlocking lets waiting people in.
    s.post_form(
        &format!("/api/v1/accounts/{carol_id}/follow"),
        Some(&bob),
        &[],
    );
    let mut s = s.restart();
    let rel = s
        .get(
            &format!("/api/v1/accounts/relationships?id[]={carol_id}"),
            Some(&bob),
        )
        .json_body();
    assert_eq!(rel[0]["requested"], true);
    send(
        &mut s,
        "PATCH",
        "/api/v1/accounts/update_credentials",
        &carol,
        &[("locked", "false")],
    );
    let rel = s
        .get(
            &format!("/api/v1/accounts/relationships?id[]={carol_id}"),
            Some(&bob),
        )
        .json_body();
    assert_eq!(rel[0]["following"], true);
    assert!(s
        .get("/api/v1/follow_requests", Some(&carol))
        .json_body()
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn lists_and_their_timelines() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let carol = s.add_member(&alice, "carol");
    let (bob_id, carol_id) = (me(&mut s, &bob), me(&mut s, &carol));

    let list = s
        .post_form(
            "/api/v1/lists",
            Some(&alice),
            &[("title", "kids"), ("replies_policy", "list")],
        )
        .json_body();
    let list_id = id_of(&list);
    assert_eq!(list["replies_policy"], "list");
    // Only people you follow.
    let r = s.post_form(
        &format!("/api/v1/lists/{list_id}/accounts"),
        Some(&alice),
        &[("account_ids[]", &bob_id)],
    );
    assert_eq!(r.status, 422);
    s.post_form(
        &format!("/api/v1/accounts/{bob_id}/follow"),
        Some(&alice),
        &[],
    );
    s.post_form(
        &format!("/api/v1/accounts/{carol_id}/follow"),
        Some(&alice),
        &[],
    );
    let r = s.post_form(
        &format!("/api/v1/lists/{list_id}/accounts"),
        Some(&alice),
        &[("account_ids[]", &bob_id)],
    );
    assert_eq!(r.status, 200);
    let members = s
        .get(&format!("/api/v1/lists/{list_id}/accounts"), Some(&alice))
        .json_body();
    assert_eq!(members[0]["username"], "bob");
    let lists = s
        .get(&format!("/api/v1/accounts/{bob_id}/lists"), Some(&alice))
        .json_body();
    assert_eq!(lists[0]["id"], list_id.as_str());
    // Not bob's to see.
    assert_eq!(
        s.get(&format!("/api/v1/lists/{list_id}"), Some(&bob))
            .status,
        404
    );

    let bobs = id_of(
        &s.post_form("/api/v1/statuses", Some(&bob), &[("status", "from bob")])
            .json_body(),
    );
    s.post_form(
        "/api/v1/statuses",
        Some(&carol),
        &[("status", "from carol")],
    );
    let timeline = s
        .get(&format!("/api/v1/timelines/list/{list_id}"), Some(&alice))
        .json_body();
    let ids: Vec<String> = timeline.as_array().unwrap().iter().map(id_of).collect();
    assert_eq!(ids, std::slice::from_ref(&bobs));

    // Exclusive: bob's posts leave home.
    let r = send(
        &mut s,
        "PUT",
        &format!("/api/v1/lists/{list_id}"),
        &alice,
        &[("title", "the kids"), ("exclusive", "true")],
    );
    assert_eq!(r.json_body()["title"], "the kids");
    let home: Vec<String> = s
        .get("/api/v1/timelines/home", Some(&alice))
        .json_body()
        .as_array()
        .unwrap()
        .iter()
        .map(id_of)
        .collect();
    assert!(!home.contains(&bobs));
    assert_eq!(home.len(), 1);

    // Unfollowing takes bob out of the list; the list survives a restart.
    s.post_form(
        &format!("/api/v1/accounts/{bob_id}/unfollow"),
        Some(&alice),
        &[],
    );
    let mut s = s.restart();
    let members = s
        .get(&format!("/api/v1/lists/{list_id}/accounts"), Some(&alice))
        .json_body();
    assert_eq!(members, json!([]));
    assert_eq!(
        s.get("/api/v1/lists", Some(&alice)).json_body()[0]["exclusive"],
        true
    );
    let r = send(
        &mut s,
        "DELETE",
        &format!("/api/v1/lists/{list_id}"),
        &alice,
        &[],
    );
    assert_eq!(r.status, 200);
    assert_eq!(s.get("/api/v1/lists", Some(&alice)).json_body(), json!([]));
}

#[test]
fn filters_mark_matching_posts() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let bob_id = me(&mut s, &bob);
    s.post_form(
        &format!("/api/v1/accounts/{bob_id}/follow"),
        Some(&alice),
        &[],
    );

    let filter = s.send_json(
        "POST",
        "/api/v2/filters",
        Some(&alice),
        &json!({
            "title": "spoilers",
            "context": ["home", "notifications"],
            "filter_action": "hide",
            "keywords_attributes": [{"keyword": "Dragon", "whole_word": true}],
        }),
    );
    assert_eq!(
        filter.status,
        200,
        "{}",
        String::from_utf8_lossy(&filter.body)
    );
    let filter = filter.json_body();
    let filter_id = id_of(&filter);
    assert_eq!(filter["keywords"][0]["keyword"], "Dragon");

    let hit = id_of(
        &s.post_form(
            "/api/v1/statuses",
            Some(&bob),
            &[("status", "@alice a dragon appears")],
        )
        .json_body(),
    );
    s.post_form(
        "/api/v1/statuses",
        Some(&bob),
        &[("status", "snapdragons are flowers")],
    );
    let home = s.get("/api/v1/timelines/home", Some(&alice)).json_body();
    for status in home.as_array().unwrap() {
        let filtered = status["filtered"].as_array().unwrap();
        if status["id"] == hit.as_str() {
            assert_eq!(filtered[0]["filter"]["id"], filter_id.as_str());
            assert_eq!(filtered[0]["filter"]["filter_action"], "hide");
            assert_eq!(filtered[0]["keyword_matches"], json!(["Dragon"]));
        } else {
            assert!(filtered.is_empty(), "whole words only");
        }
    }
    let notes = s.get("/api/v1/notifications", Some(&alice)).json_body();
    assert_eq!(
        notes[0]["status"]["filtered"][0]["filter"]["id"],
        filter_id.as_str()
    );
    // Not in the public context; bob has no filters.
    let public = s.get("/api/v1/timelines/public", Some(&alice)).json_body();
    assert!(public[1]["filtered"].as_array().unwrap().is_empty());

    // A specific post, a keyword via its own endpoint, and v1's view.
    let entry = s
        .post_form(
            &format!("/api/v2/filters/{filter_id}/statuses"),
            Some(&alice),
            &[("status_id", &hit)],
        )
        .json_body();
    assert_eq!(entry["status_id"], hit.as_str());
    let kw = s
        .post_form(
            &format!("/api/v2/filters/{filter_id}/keywords"),
            Some(&alice),
            &[("keyword", "wyrm"), ("whole_word", "false")],
        )
        .json_body();
    let kw_id = id_of(&kw);
    let v1 = s.get("/api/v1/filters", Some(&alice)).json_body();
    assert_eq!(v1.as_array().unwrap().len(), 2);
    assert_eq!(v1[0]["irreversible"], true);
    let r = send(
        &mut s,
        "PUT",
        &format!("/api/v2/filters/keywords/{kw_id}"),
        &alice,
        &[("keyword", "drake")],
    );
    assert_eq!(r.json_body()["keyword"], "drake");
    // Not bob's.
    assert_eq!(
        s.get(&format!("/api/v2/filters/{filter_id}"), Some(&bob))
            .status,
        404
    );
    assert_eq!(
        s.get(&format!("/api/v2/filters/keywords/{kw_id}"), Some(&bob))
            .status,
        404
    );

    // Expired filters stop matching; filters survive a restart.
    let r = send(
        &mut s,
        "PUT",
        &format!("/api/v2/filters/{filter_id}"),
        &alice,
        &[("expires_in", "-1")],
    );
    assert_eq!(r.status, 200);
    let mut s = s.restart();
    let home = s.get("/api/v1/timelines/home", Some(&alice)).json_body();
    assert!(home
        .as_array()
        .unwrap()
        .iter()
        .all(|st| st["filtered"].as_array().unwrap().is_empty()));
    let fetched = s
        .get(&format!("/api/v2/filters/{filter_id}"), Some(&alice))
        .json_body();
    assert_eq!(fetched["statuses"][0]["status_id"], hit.as_str());
    assert_eq!(fetched["keywords"].as_array().unwrap().len(), 2);

    // v1 create and delete.
    let v1 = s
        .post_form(
            "/api/v1/filters",
            Some(&alice),
            &[
                ("phrase", "hand"),
                ("context[]", "home"),
                ("irreversible", "true"),
            ],
        )
        .json_body();
    assert_eq!(v1["phrase"], "hand");
    let v1_id = id_of(&v1);
    assert_eq!(
        send(
            &mut s,
            "DELETE",
            &format!("/api/v1/filters/{v1_id}"),
            &alice,
            &[]
        )
        .status,
        200
    );
    assert_eq!(
        s.get(&format!("/api/v1/filters/{v1_id}"), Some(&alice))
            .status,
        404
    );
    assert_eq!(
        s.get("/api/v2/filters", Some(&alice))
            .json_body()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
