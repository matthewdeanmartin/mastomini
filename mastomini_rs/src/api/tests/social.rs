use super::*;

#[test]
fn local_tags_notes_and_dismissals_survive_reboot() {
    let mut s = Server::provisioned();
    let alice = s.login(
        "alice",
        "alicepw",
        "read write follow admin:read admin:write",
    );
    let bob = s.add_member(&alice, "bob");
    let id = s
        .get("/api/v1/accounts/verify_credentials", Some(&bob))
        .json_body()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let note = format!("/api/v1/accounts/{id}/note");
    assert_eq!(
        s.post_form(&note, Some(&alice), &[("comment", "only Alice sees this")])
            .json_body()["note"],
        "only Alice sees this"
    );
    let relationships = format!("/api/v1/accounts/relationships?id[]={id}");
    assert_eq!(s.get(&relationships, Some(&bob)).json_body()[0]["note"], "");
    assert_eq!(
        s.send_json(
            "DELETE",
            &format!("/api/v1/suggestions/{id}"),
            Some(&alice),
            &json!({})
        )
        .status,
        200
    );
    assert_eq!(
        s.post_form("/api/v1/tags/RuSt/follow", Some(&alice), &[])
            .json_body()["following"],
        true
    );
    let feature = s
        .post_form("/api/v1/featured_tags", Some(&bob), &[("name", "Rust")])
        .json_body();
    let again = s
        .post_form("/api/v1/featured_tags", Some(&bob), &[("name", "rust")])
        .json_body();
    assert_eq!(feature["id"], again["id"]);
    let post = s
        .post_form("/api/v1/statuses", Some(&bob), &[("status", "hello #rust")])
        .json_body();
    assert_eq!(
        s.get("/api/v1/timelines/home", Some(&alice)).json_body()[0]["id"],
        post["id"]
    );
    let mut s = s.restart();
    assert_eq!(
        s.get(&relationships, Some(&alice)).json_body()[0]["note"],
        "only Alice sees this"
    );
    assert!(s
        .get("/api/v2/suggestions", Some(&alice))
        .json_body()
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a["account"]["id"] != id));
    assert_eq!(
        s.get("/api/v1/tags/rust", Some(&alice)).json_body()["following"],
        true
    );
    assert_eq!(
        s.get(
            &format!("/api/v1/accounts/{id}/featured_tags"),
            Some(&alice)
        )
        .json_body()[0]["statuses_count"],
        "1"
    );
    assert_eq!(
        s.send_json(
            "DELETE",
            &format!("/api/v1/featured_tags/{}", feature["id"].as_str().unwrap()),
            Some(&alice),
            &json!({})
        )
        .status,
        404
    );
    s.post_form(&format!("/api/v1/accounts/{id}/block"), Some(&alice), &[]);
    assert_eq!(
        s.get("/api/v1/timelines/home", Some(&alice)).json_body(),
        json!([])
    );
    assert_eq!(
        s.post_form(&note, Some(&alice), &[("comment", "")])
            .json_body()["note"],
        ""
    );
}

#[test]
fn tags_are_bounded_and_private_posts_do_not_leak() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    for i in 0..10 {
        assert_eq!(
            s.post_form(
                "/api/v1/featured_tags",
                Some(&alice),
                &[("name", &format!("tag{i}"))]
            )
            .status,
            200
        );
    }
    assert_eq!(
        s.post_form(
            "/api/v1/featured_tags",
            Some(&alice),
            &[("name", "overflow")]
        )
        .status,
        422
    );
    assert_eq!(
        s.post_form("/api/v1/featured_tags", Some(&alice), &[("name", "tag0")])
            .status,
        200
    );
    assert_eq!(
        s.post_form(
            "/api/v1/featured_tags",
            Some(&alice),
            &[("name", "bad tag")]
        )
        .status,
        422
    );
    s.post_form("/api/v1/tags/rust/follow", Some(&bob), &[]);
    s.post_form(
        "/api/v1/statuses",
        Some(&alice),
        &[("status", "private #rust"), ("visibility", "private")],
    );
    assert_eq!(
        s.get("/api/v1/timelines/home", Some(&bob)).json_body(),
        json!([])
    );
    assert_eq!(
        s.get("/api/v1/tags/rust", Some(&bob)).json_body()["history"][0]["uses"],
        "0"
    );
    assert_eq!(
        s.post_form("/api/v1/tags/caf%C3%A9/follow", Some(&bob), &[])
            .json_body()["name"],
        "café"
    );
    let read = s.login("bob", "secret", "read");
    assert_eq!(
        s.post_form("/api/v1/tags/rust/follow", Some(&read), &[])
            .status,
        403
    );
    let account_scope = s.login("bob", "secret", "read:accounts write:accounts");
    assert_eq!(
        s.post_form(
            "/api/v1/featured_tags",
            Some(&account_scope),
            &[("name", "rust")]
        )
        .status,
        200
    );
    assert_eq!(
        s.get("/api/v1/featured_tags", Some(&account_scope)).status,
        200
    );
    assert_eq!(
        s.post_form("/api/v1/tags/rust/follow", Some(&account_scope), &[])
            .status,
        403
    );
}

#[test]
fn announcement_lifecycle_permissions_and_reactions() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write admin:read admin:write");
    let bob = s.add_member(&alice, "bob");
    let path = "/api/v1/admin/announcements";
    assert_eq!(
        s.send_json("POST", path, Some(&bob), &json!({"text":"no"}))
            .status,
        403
    );
    let r = s.send_json(
        "POST",
        path,
        Some(&alice),
        &json!({"text":"<script>hi</script> #news","published":false}),
    );
    assert_eq!(r.status, 200);
    let id = r.json_body()["id"].as_str().unwrap().to_string();
    assert_eq!(
        s.get("/api/v1/announcements", Some(&bob)).json_body(),
        json!([])
    );
    assert_eq!(
        s.post_form(&format!("{path}/{id}/publish"), Some(&alice), &[])
            .status,
        200
    );
    let visible = s.get("/api/v1/announcements", Some(&bob)).json_body();
    assert!(visible[0]["content"]
        .as_str()
        .unwrap()
        .contains("&lt;script&gt;"));
    let reaction = format!("/api/v1/announcements/{id}/reactions/%F0%9F%91%8D");
    assert_eq!(
        s.send_json("PUT", &reaction, Some(&bob), &json!({})).status,
        200
    );
    s.send_json("PUT", &reaction, Some(&bob), &json!({}));
    assert_eq!(
        s.get("/api/v1/announcements", Some(&bob)).json_body()[0]["reactions"][0]["count"],
        1
    );
    s.post_form(
        &format!("/api/v1/announcements/{id}/dismiss"),
        Some(&bob),
        &[],
    );
    let mut s = s.restart();
    assert_eq!(
        s.get("/api/v1/announcements", Some(&bob)).json_body(),
        json!([])
    );
    assert_eq!(
        s.get("/api/v1/announcements?with_dismissed=true", Some(&bob))
            .json_body()[0]["read"],
        true
    );
    assert_eq!(
        s.send_json(
            "PATCH",
            &format!("{path}/{id}"),
            Some(&alice),
            &json!({"text":"Updated","starts_at":"2099-01-01T00:00:00Z"})
        )
        .status,
        200
    );
    assert_eq!(
        s.get("/api/v1/announcements", Some(&alice)).json_body(),
        json!([])
    );
    assert_eq!(
        s.send_json(
            "PATCH",
            &format!("{path}/{id}"),
            Some(&alice),
            &json!({"starts_at":"not a date"})
        )
        .status,
        422
    );
    assert_eq!(
        s.send_json("DELETE", &format!("{path}/{id}"), Some(&alice), &json!({}))
            .status,
        200
    );
    assert_eq!(s.get(path, Some(&alice)).json_body(), json!([]));
}

#[test]
fn announcement_admin_scopes_do_not_replace_roles() {
    let mut s = Server::provisioned();
    let owner = s.login("alice", "alicepw", "read write");
    s.add_member(&owner, "bob");
    let member = s.login("bob", "secret", "admin:read admin:write");
    let read = s.login("alice", "alicepw", "admin:read");
    let write = s.login("alice", "alicepw", "admin:write");
    let path = "/api/v1/admin/announcements";
    for token in [&owner, &member] {
        assert_eq!(s.get(path, Some(token)).status, 403);
        assert_eq!(
            s.send_json(
                "POST",
                path,
                Some(token),
                &json!({"text":"denied","published":false})
            )
            .status,
            403
        );
    }
    assert_eq!(s.get(path, Some(&read)).status, 200);
    assert_eq!(
        s.send_json("POST", path, Some(&read), &json!({"text":"denied"}))
            .status,
        403
    );
    let created = s.send_json(
        "POST",
        path,
        Some(&write),
        &json!({"text":"draft","published":false}),
    );
    assert_eq!(created.status, 200);
    assert_eq!(created.json_body()["published"], false);
    assert_eq!(s.get(path, Some(&write)).status, 403);
    let id = created.json_body()["id"].as_str().unwrap().to_string();
    let detail = format!("{path}/{id}");
    assert_eq!(
        s.post_form(&format!("{detail}/publish"), Some(&member), &[])
            .status,
        403
    );
    assert_eq!(
        s.post_form(&format!("{detail}/publish"), Some(&write), &[])
            .status,
        200
    );
    let updated = s.send_json(
        "PUT",
        &detail,
        Some(&write),
        &json!({"text":"edited","starts_at":"","ends_at":""}),
    );
    assert_eq!(updated.status, 200);
    assert_eq!(updated.json_body()["published"], true);
}

#[test]
fn aliases_and_group_dismissal_are_real() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    let bob = s.add_member(&alice, "bob");
    let carol = s.add_member(&alice, "carol");
    assert_eq!(
        s.get("/api/v1/profile", Some(&alice)).json_body()["username"],
        "alice"
    );
    assert_eq!(
        s.send_json(
            "PATCH",
            "/api/v1/profile",
            Some(&alice),
            &json!({"display_name":"Changed"})
        )
        .json_body()["display_name"],
        "Changed"
    );
    assert_eq!(
        s.get("/oauth/userinfo", Some(&alice)).json_body()["preferred_username"],
        "alice"
    );
    let id = s
        .post_form("/api/v1/statuses", Some(&alice), &[("status", "group")])
        .json_body()["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.post_form(&format!("/api/v1/statuses/{id}/favourite"), Some(&bob), &[]);
    s.post_form(
        &format!("/api/v1/statuses/{id}/favourite"),
        Some(&carol),
        &[],
    );
    let groups = s
        .get("/api/v2/notifications?limit=1", Some(&alice))
        .json_body();
    assert_eq!(groups["notification_groups"][0]["notifications_count"], 2);
    assert_eq!(
        s.get("/api/v2/notifications/unread_count", Some(&alice))
            .json_body()["count"],
        1
    );
    let key = groups["notification_groups"][0]["group_key"]
        .as_str()
        .unwrap();
    let group = format!("/api/v2/notifications/{key}");
    let bob_id = s
        .get("/api/v1/accounts/verify_credentials", Some(&bob))
        .json_body()["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.post_form(
        &format!("/api/v1/accounts/{bob_id}/mute"),
        Some(&alice),
        &[("notifications", "true")],
    );
    assert_eq!(
        s.get("/api/v2/notifications", Some(&alice)).json_body()["notification_groups"][0]
            ["notifications_count"],
        1
    );
    s.post_form(
        &format!("/api/v1/accounts/{bob_id}/unmute"),
        Some(&alice),
        &[],
    );
    assert_eq!(
        s.get(&format!("{group}/accounts"), Some(&alice))
            .json_body()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        s.post_form(&format!("{group}/dismiss"), Some(&bob), &[])
            .status,
        404
    );
    assert_eq!(
        s.post_form(&format!("{group}/dismiss"), Some(&alice), &[])
            .status,
        200
    );
    assert_eq!(
        s.get("/api/v1/notifications", Some(&alice)).json_body(),
        json!([])
    );
}

#[test]
fn announcement_timestamp_parser_rejects_invalid_dates() {
    use crate::api::time::{iso, parse_iso};
    assert_eq!(
        parse_iso("2026-09-25T12:00:00-04:00"),
        parse_iso("2026-09-25T16:00:00Z")
    );
    assert_eq!(
        iso(parse_iso("2024-02-29T01:02:03.45Z").unwrap()),
        "2024-02-29T01:02:03.450Z"
    );
    for bad in [
        "2025-02-29T00:00:00Z",
        "2026-09-25T99:00:00Z",
        "2026-09-25T12:00:00.Z",
    ] {
        assert!(parse_iso(bad).is_none());
    }
}
