//! Editing posts (with history) and polls.

use super::*;

fn post(s: &mut Server, token: &str, form: &[(&str, &str)]) -> serde_json::Value {
    let r = s.post_form("/api/v1/statuses", Some(token), form);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json_body()
}

fn put(s: &mut Server, token: &str, id: &str, form: &[(&str, &str)]) -> crate::http::Response {
    let req = Request::new("PUT", &format!("/api/v1/statuses/{id}"))
        .with_body("application/x-www-form-urlencoded", form_body(form))
        .with_header("Authorization", &format!("Bearer {token}"));
    s.send(req)
}

fn form_body(pairs: &[(&str, &str)]) -> String {
    super::super::testkit::form(pairs)
}

#[test]
fn edits_keep_three_previous_versions() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let id = post(&mut s, &alice, &[("status", "tpyo")])["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Bob boosts it, so he hears about edits.
    s.post_form(&format!("/api/v1/statuses/{id}/reblog"), Some(&bob), &[]);

    assert_eq!(
        put(&mut s, &bob, &id, &[("status", "mine now")]).status,
        403
    );
    let r = put(&mut s, &alice, &id, &[("status", "typo @bob")]);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let edited = r.json_body();
    assert!(edited["content"].as_str().unwrap().contains("typo"));
    assert!(edited["edited_at"].is_string());
    assert_eq!(edited["mentions"][0]["username"], "bob");

    let history = s
        .get(&format!("/api/v1/statuses/{id}/history"), Some(&alice))
        .json_body();
    let history = history.as_array().unwrap();
    assert_eq!(history.len(), 2);
    assert!(history[0]["content"].as_str().unwrap().contains("tpyo"));
    assert!(history[1]["content"].as_str().unwrap().contains("typo"));

    let kinds: Vec<String> = s
        .get("/api/v1/notifications", Some(&bob))
        .json_body()
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["type"].as_str().unwrap().to_string())
        .collect();
    assert!(kinds.contains(&"update".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"mention".to_string()), "{kinds:?}");

    for text in ["v3", "v4", "v5"] {
        assert_eq!(put(&mut s, &alice, &id, &[("status", text)]).status, 200);
    }
    let mut s = s.restart();
    let history = s
        .get(&format!("/api/v1/statuses/{id}/history"), Some(&alice))
        .json_body();
    let texts: Vec<String> = history
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["content"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(texts.len(), 4, "{texts:?}");
    assert!(
        texts[0].contains("typo") && texts[3].contains("v5"),
        "{texts:?}"
    );
    let source = s
        .get(&format!("/api/v1/statuses/{id}/source"), Some(&alice))
        .json_body();
    assert_eq!(source["text"], "v5");
}

#[test]
fn direct_messages_can_be_edited_but_keep_no_history() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let id = post(
        &mut s,
        &alice,
        &[("status", "@bob secret one"), ("visibility", "direct")],
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = put(&mut s, &alice, &id, &[("status", "@bob secret two")]);
    assert_eq!(r.status, 200);
    let seen = s
        .get(&format!("/api/v1/statuses/{id}"), Some(&bob))
        .json_body();
    assert!(seen["content"].as_str().unwrap().contains("secret two"));
    let history = s
        .get(&format!("/api/v1/statuses/{id}/history"), Some(&bob))
        .json_body();
    assert_eq!(history.as_array().unwrap().len(), 1);
    // Neither version is readable in flash.
    let mut flash = Vec::new();
    for ns in crate::store::Ns::ALL {
        crate::store::Store::for_each(s.svc.store(), ns, &mut |_, v| {
            flash.extend_from_slice(v);
            Ok(())
        })
        .unwrap();
    }
    assert!(!flash.windows(6).any(|w| w == b"secret"));
}

#[test]
fn polls_vote_and_end() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let carol = s.add_member(&alice, "carol");
    let status = post(
        &mut s,
        &alice,
        &[
            ("status", "what's for dinner?"),
            ("poll[options][]", "tacos"),
            ("poll[options][]", "pizza"),
            ("poll[expires_in]", "3600"),
        ],
    );
    let id = status["id"].as_str().unwrap().to_string();
    assert_eq!(status["poll"]["options"][1]["title"], "pizza");
    assert_eq!(status["poll"]["voted"], true); // the author's own poll
    assert_eq!(status["poll"]["expired"], false);

    let vote = |s: &mut Server, token: &str, choices: &[&str]| {
        let pairs: Vec<(&str, &str)> = choices.iter().map(|c| ("choices[]", *c)).collect();
        s.post_form(&format!("/api/v1/polls/{id}/votes"), Some(token), &pairs)
    };
    assert_eq!(
        vote(&mut s, &alice, &["0"]).status,
        422,
        "no voting on your own poll"
    );
    assert_eq!(
        vote(&mut s, &bob, &["0", "1"]).status,
        422,
        "one choice only"
    );
    assert_eq!(vote(&mut s, &bob, &["7"]).status, 422);
    let r = vote(&mut s, &bob, &["0"]);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let poll = r.json_body();
    assert_eq!(poll["votes_count"], 1);
    assert_eq!(poll["own_votes"], json!([0]));
    assert_eq!(poll["options"][0]["votes_count"], 1);
    assert_eq!(vote(&mut s, &bob, &["1"]).status, 422, "only once");
    assert_eq!(vote(&mut s, &carol, &["1"]).status, 200);

    // Votes survive a restart; nobody is told about a poll that ended
    // before the restart twice.
    let mut s = s.restart();
    let poll = s
        .get(&format!("/api/v1/polls/{id}"), Some(&carol))
        .json_body();
    assert_eq!(poll["votes_count"], 2);
    assert_eq!(poll["own_votes"], json!([1]));

    // The poll ends: author and voters are told once.
    s.now += 3_600_001;
    s.get("/api/v1/instance", None);
    for token in [&alice, &bob, &carol] {
        let polls: Vec<serde_json::Value> = s
            .get("/api/v1/notifications?types[]=poll", Some(token))
            .json_body()
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(polls.len(), 1, "{polls:?}");
        assert_eq!(polls[0]["status"]["poll"]["expired"], true);
    }
    let late = s.post_form(
        &format!("/api/v1/polls/{id}/votes"),
        Some(&alice),
        &[("choices[]", "0")],
    );
    assert_eq!(late.status, 422);

    // No polls in direct messages; bad polls are refused.
    let r = s.post_form(
        "/api/v1/statuses",
        Some(&alice),
        &[
            ("status", "@bob secret poll"),
            ("visibility", "direct"),
            ("poll[options][]", "a"),
            ("poll[options][]", "b"),
            ("poll[expires_in]", "3600"),
        ],
    );
    assert_eq!(r.status, 422);
    let r = s.post_form(
        "/api/v1/statuses",
        Some(&alice),
        &[
            ("status", "one option"),
            ("poll[options][]", "a"),
            ("poll[expires_in]", "3600"),
        ],
    );
    assert_eq!(r.status, 422);

    // Editing the question keeps the votes; changing the options resets them.
    let keep = put(
        &mut s,
        &alice,
        &id,
        &[
            ("status", "dinner, really?"),
            ("poll[options][]", "tacos"),
            ("poll[options][]", "pizza"),
            ("poll[expires_in]", "3600"),
        ],
    );
    assert_eq!(keep.json_body()["poll"]["votes_count"], 2);
    let reset = put(
        &mut s,
        &alice,
        &id,
        &[
            ("status", "dinner?"),
            ("poll[options][]", "soup"),
            ("poll[options][]", "salad"),
            ("poll[expires_in]", "3600"),
        ],
    );
    assert_eq!(reset.json_body()["poll"]["votes_count"], 0);
    let gone = put(&mut s, &alice, &id, &[("status", "never mind")]);
    assert!(gone.json_body()["poll"].is_null());

    let v2 = s.get("/api/v2/instance", None).json_body();
    assert_eq!(v2["configuration"]["polls"]["max_options"], 4);
}
