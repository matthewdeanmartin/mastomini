use super::*;

fn names(rows: &serde_json::Value) -> Vec<&str> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|a| a["username"].as_str().unwrap())
        .collect()
}

#[test]
fn directory_orders_pages_and_excludes_hidden_accounts_after_restart() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let carol = s.add_member(&alice, "carol");
    s.post_form("/api/v1/statuses", Some(&bob), &[("status", "older")]);
    s.post_form("/api/v1/statuses", Some(&alice), &[("status", "newer")]);
    assert_eq!(
        names(
            &s.get("/api/v1/directory?order=active&limit=2", Some(&alice))
                .json_body()
        ),
        ["alice", "bob"]
    );
    assert_eq!(
        names(
            &s.get("/api/v1/directory?order=new&offset=1&limit=1", Some(&alice))
                .json_body()
        ),
        ["bob"]
    );
    assert_eq!(
        s.get(
            "/api/v1/directory?offset=18446744073709551615",
            Some(&alice)
        )
        .json_body(),
        json!([])
    );
    assert_eq!(
        s.send_json(
            "PATCH",
            "/api/v1/accounts/update_credentials",
            Some(&carol),
            &json!({"discoverable":false})
        )
        .status,
        200
    );
    let bob_id = s
        .get("/api/v1/accounts/verify_credentials", Some(&bob))
        .json_body()["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.post_form(
        &format!("/api/v1/accounts/{bob_id}/block"),
        Some(&alice),
        &[],
    );
    let mut s = s.restart();
    assert_eq!(
        names(&s.get("/api/v1/directory", Some(&alice)).json_body()),
        ["alice"]
    );
    assert_eq!(s.get("/api/v1/directory", None).status, 401);
}

#[test]
fn suggestion_versions_share_follow_mute_and_persistent_dismissal_rules() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    s.add_member(&alice, "carol");
    let bob_id = s
        .get("/api/v1/accounts/verify_credentials", Some(&bob))
        .json_body()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        names(&s.get("/api/v1/suggestions", Some(&alice)).json_body()),
        ["bob", "carol"]
    );
    assert_eq!(
        s.get("/api/v2/suggestions", Some(&alice)).json_body()[0]["account"]["username"],
        "bob"
    );
    for action in ["follow", "unfollow", "mute", "unmute"] {
        assert_eq!(
            s.post_form(
                &format!("/api/v1/accounts/{bob_id}/{action}"),
                Some(&alice),
                &[]
            )
            .status,
            200
        );
        let expected = if matches!(action, "follow" | "mute") {
            vec!["carol"]
        } else {
            vec!["bob", "carol"]
        };
        assert_eq!(
            names(&s.get("/api/v1/suggestions", Some(&alice)).json_body()),
            expected
        );
    }
    assert_eq!(
        s.send_json(
            "DELETE",
            &format!("/api/v1/suggestions/{bob_id}"),
            Some(&alice),
            &json!({})
        )
        .status,
        200
    );
    let mut s = s.restart();
    assert_eq!(
        names(&s.get("/api/v1/suggestions", Some(&alice)).json_body()),
        ["carol"]
    );
    let v2 = s.get("/api/v2/suggestions", Some(&alice)).json_body();
    assert_eq!(v2.as_array().unwrap().len(), 1);
    assert_eq!(v2[0]["account"]["username"], "carol");
}

#[test]
fn notification_compatibility_routes_require_auth_and_never_invent_records() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    for (method, path, status) in [
        ("GET", "/api/v1/notifications/policy", 200),
        ("PUT", "/api/v1/notifications/policy", 200),
        ("PATCH", "/api/v2/notifications/policy", 200),
        ("GET", "/api/v1/notifications/requests/merged", 200),
        ("POST", "/api/v1/notifications/requests/accept", 200),
        ("POST", "/api/v1/notifications/requests/dismiss", 200),
        ("GET", "/api/v1/notifications/requests/123", 404),
        ("POST", "/api/v1/notifications/requests/123/accept", 404),
        ("POST", "/api/v1/notifications/requests/123/dismiss", 404),
    ] {
        assert_eq!(
            s.send_json(method, path, None, &json!({})).status,
            401,
            "{method} {path}"
        );
        assert_eq!(
            s.send_json(
                method,
                path,
                Some(&alice),
                &json!({"for_not_following":"drop"})
            )
            .status,
            status,
            "{method} {path}"
        );
    }
    let policy = s
        .get("/api/v2/notifications/policy", Some(&alice))
        .json_body();
    assert_eq!(policy["for_not_following"], "accept");
    assert_eq!(policy["summary"]["pending_requests_count"], 0);
    assert!(s.svc.state.notifications.is_empty());
}
