//! Follow requests, lists and filters through the full request path.

use super::*;

fn id_of(v: &serde_json::Value) -> String {
    v["id"].as_str().unwrap().to_string()
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
