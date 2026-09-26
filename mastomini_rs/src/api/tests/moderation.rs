//! Blocks, mutes, reports, the admin API, household member management and
//! collections, through the full request path.

use super::*;

fn me(s: &mut Server, token: &str) -> String {
    s.get("/api/v1/accounts/verify_credentials", Some(token))
        .json_body()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Owner alice with admin scopes, bob and carol with ordinary tokens.
fn household() -> (Server, String, String, String) {
    let mut s = Server::provisioned();
    let alice = s.login(
        "alice",
        "alicepw",
        "read write follow admin:read admin:write",
    );
    let bob = s.add_member(&alice, "bob");
    let carol = s.add_member(&alice, "carol");
    (s, alice, bob, carol)
}

#[test]
fn admin_pages_follow_filtered_links_and_enforce_role_and_scopes() {
    let (mut s, alice, bob, carol) = household();
    let target = me(&mut s, &carol);
    let reporter = me(&mut s, &bob);
    for _ in 0..3 {
        assert_eq!(
            s.send_json(
                "POST",
                "/api/v1/reports",
                Some(&bob),
                &json!({"account_id": target})
            )
            .status,
            200
        );
    }
    let readonly = s.login("alice", "alicepw", "admin:read:reports");
    let path = format!("/api/v1/admin/reports?limit=2&resolved=false&account_id={reporter}&target_account_id={target}");
    let first = s.get(&path, Some(&readonly));
    assert_eq!(first.status, 200);
    assert_eq!(first.json_body().as_array().unwrap().len(), 2);
    assert!(first
        .header("Access-Control-Expose-Headers")
        .unwrap()
        .contains("Link"));
    let links = first.header("Link").unwrap();
    let next = links.split('<').nth(1).unwrap().split('>').next().unwrap();
    assert!(next.contains("resolved=false"));
    assert!(next.contains(&format!("account_id={reporter}")));
    assert!(next.contains(&format!("target_account_id={target}")));
    let second = s.get(
        next.strip_prefix("http://mastomini.test").unwrap(),
        Some(&readonly),
    );
    assert_eq!(second.json_body().as_array().unwrap().len(), 1);
    assert_ne!(first.json_body()[0]["id"], second.json_body()[0]["id"]);
    let report = first.json_body()[0]["id"].as_str().unwrap().to_string();
    let resolve = format!("/api/v1/admin/reports/{report}/resolve");
    assert_eq!(s.post_form(&resolve, Some(&readonly), &[]).status, 403);
    let member_admin_scope = s.login("bob", "secret", "admin:read admin:write");
    assert_eq!(s.get(&path, Some(&member_admin_scope)).status, 403);
    assert_eq!(
        s.post_form(&resolve, Some(&member_admin_scope), &[]).status,
        403
    );

    // Write scopes authorize the mutation AND its response; read is not
    // silently required after committing the operation.
    let writeonly = s.login(
        "alice",
        "alicepw",
        "admin:write:reports admin:write:accounts",
    );
    let resolved = s.post_form(&resolve, Some(&writeonly), &[]);
    assert_eq!(resolved.status, 200);
    assert_eq!(resolved.json_body()["action_taken"], true);
    assert_eq!(s.get(&path, Some(&writeonly)).status, 403);
    let enable = format!("/api/v1/admin/accounts/{target}/enable");
    assert_eq!(s.post_form(&enable, Some(&writeonly), &[]).status, 200);
    assert_eq!(
        s.get("/api/v1/admin/accounts", Some(&writeonly)).status,
        403
    );

    // Empty rule selection must clear previously selected rules through JSON.
    assert_eq!(
        s.send_json(
            "PUT",
            "/api/mastomini/v1/admin/server",
            Some(&alice),
            &json!({"rules":["Be kind"]})
        )
        .status,
        200
    );
    let detail = format!("/api/v1/admin/reports/{report}");
    let classified = s.send_json(
        "PUT",
        &detail,
        Some(&writeonly),
        &json!({"category":"violation","rule_ids":["1"]}),
    );
    assert_eq!(classified.status, 200);
    assert_eq!(classified.json_body()["rules"].as_array().unwrap().len(), 1);
    let cleared = s.send_json(
        "PUT",
        &detail,
        Some(&writeonly),
        &json!({"category":"other","rule_ids":[""]}),
    );
    assert_eq!(cleared.status, 200);
    assert_eq!(cleared.json_body()["rules"], json!([]));
}

#[test]
fn block_and_mute_endpoints_and_lists() {
    let (mut s, alice, bob, carol) = household();
    let (bob_id, carol_id) = (me(&mut s, &bob), me(&mut s, &carol));
    let rel = s
        .post_form(
            &format!("/api/v1/accounts/{bob_id}/block"),
            Some(&alice),
            &[],
        )
        .json_body();
    assert_eq!(rel["blocking"], true);
    assert_eq!(rel["following"], false);
    let rel = s
        .post_form(
            &format!("/api/v1/accounts/{carol_id}/mute"),
            Some(&alice),
            &[("notifications", "false"), ("duration", "3600")],
        )
        .json_body();
    assert_eq!(rel["muting"], true);
    assert_eq!(rel["muting_notifications"], false);

    let alice_id = me(&mut s, &alice);
    let seen_by_bob = s
        .get(
            &format!("/api/v1/accounts/relationships?id[]={alice_id}"),
            Some(&bob),
        )
        .json_body();
    assert_eq!(seen_by_bob[0]["blocked_by"], true);
    assert_eq!(
        s.post_form(
            &format!("/api/v1/accounts/{alice_id}/follow"),
            Some(&bob),
            &[]
        )
        .status,
        403
    );

    let blocks = s.get("/api/v1/blocks", Some(&alice)).json_body();
    assert_eq!(blocks[0]["id"], bob_id.as_str());
    let mutes = s.get("/api/v1/mutes", Some(&alice)).json_body();
    assert_eq!(mutes[0]["id"], carol_id.as_str());
    assert!(mutes[0]["mute_expires_at"].is_string());

    s.post_form(
        &format!("/api/v1/accounts/{bob_id}/unblock"),
        Some(&alice),
        &[],
    );
    s.post_form(
        &format!("/api/v1/accounts/{carol_id}/unmute"),
        Some(&alice),
        &[],
    );
    assert_eq!(s.get("/api/v1/blocks", Some(&alice)).json_body(), json!([]));
    assert_eq!(s.get("/api/v1/mutes", Some(&alice)).json_body(), json!([]));
}

#[test]
fn conversation_mute_sets_muted() {
    let (mut s, alice, bob, _) = household();
    let root = id(&post(&mut s, &alice, &[("status", "thread")]));
    let muted = s
        .post_form(&format!("/api/v1/statuses/{root}/mute"), Some(&alice), &[])
        .json_body();
    assert_eq!(muted["muted"], true);
    let seen_by_bob = s
        .get(&format!("/api/v1/statuses/{root}"), Some(&bob))
        .json_body();
    assert_eq!(seen_by_bob["muted"], false);
    let unmuted = s
        .post_form(
            &format!("/api/v1/statuses/{root}/unmute"),
            Some(&alice),
            &[],
        )
        .json_body();
    assert_eq!(unmuted["muted"], false);
}

#[test]
fn reports_reach_admins_through_notifications_and_the_admin_api() {
    let (mut s, alice, bob, carol) = household();
    let carol_id = me(&mut s, &carol);
    let rude = id(&post(&mut s, &carol, &[("status", "rude")]));
    let report = s.send_json(
        "POST",
        "/api/v1/reports",
        Some(&bob),
        &json!({ "account_id": carol_id, "status_ids": [rude], "comment": "please look" }),
    );
    assert_eq!(
        report.status,
        200,
        "{}",
        String::from_utf8_lossy(&report.body)
    );
    let report = report.json_body();
    assert_eq!(report["category"], "other");
    assert_eq!(report["target_account"]["id"], carol_id.as_str());
    let report_id = report["id"].as_str().unwrap().to_string();

    let notes = s
        .get("/api/v1/notifications?types[]=admin.report", Some(&alice))
        .json_body();
    assert_eq!(notes[0]["report"]["id"], report_id.as_str());

    // Members can't use the admin API; admins need an admin scope.
    assert_eq!(s.get("/api/v1/admin/reports", Some(&bob)).status, 403);
    let plain = s.login("alice", "alicepw", "read write");
    assert_eq!(s.get("/api/v1/admin/reports", Some(&plain)).status, 403);

    let open = s.get("/api/v1/admin/reports", Some(&alice)).json_body();
    assert_eq!(open[0]["id"], report_id.as_str());
    assert_eq!(open[0]["statuses"][0]["id"], rude.as_str());
    assert_eq!(open[0]["account"]["account"]["username"], "bob");

    let url = format!("/api/v1/admin/reports/{report_id}");
    let assigned = s
        .post_form(&format!("{url}/assign_to_self"), Some(&alice), &[])
        .json_body();
    assert_eq!(assigned["assigned_account"]["username"], "alice");
    let resolved = s
        .post_form(&format!("{url}/resolve"), Some(&alice), &[])
        .json_body();
    assert_eq!(resolved["action_taken"], true);
    assert_eq!(resolved["action_taken_by_account"]["username"], "alice");
    assert_eq!(
        s.get("/api/v1/admin/reports", Some(&alice)).json_body(),
        json!([])
    );
    let done = s
        .get("/api/v1/admin/reports?resolved=true", Some(&alice))
        .json_body();
    assert_eq!(done[0]["id"], report_id.as_str());
    let reopened = s
        .post_form(&format!("{url}/reopen"), Some(&alice), &[])
        .json_body();
    assert_eq!(reopened["action_taken"], false);
    assert_eq!(
        s.get("/api/v1/reports", Some(&bob)).json_body()[0]["id"],
        report_id.as_str()
    );
}

#[test]
fn admin_account_actions_and_disabled_tokens() {
    let (mut s, alice, bob, carol) = household();
    let (alice_id, bob_id) = (me(&mut s, &alice), me(&mut s, &bob));
    let all = s
        .get("/api/v2/admin/accounts?origin=local", Some(&alice))
        .json_body();
    assert_eq!(all.as_array().unwrap().len(), 3);
    assert_eq!(
        s.get("/api/v2/admin/accounts?origin=remote", Some(&alice))
            .json_body(),
        json!([])
    );
    let found = s
        .get("/api/v1/admin/accounts?username=bo", Some(&alice))
        .json_body();
    assert_eq!(found[0]["username"], "bob");

    let action = |s: &mut Server, id: &str, kind: &str| {
        s.post_form(
            &format!("/api/v1/admin/accounts/{id}/action"),
            Some(&alice),
            &[("type", kind)],
        )
        .status
    };
    assert_eq!(action(&mut s, &alice_id, "suspend"), 403, "not yourself");
    assert_eq!(action(&mut s, &bob_id, "bogus"), 422);

    assert_eq!(action(&mut s, &bob_id, "disable"), 200);
    let r = s.get("/api/v1/timelines/home", Some(&bob));
    assert_eq!(r.status, 403);
    assert_eq!(r.json_body()["error"], "Your login is currently disabled");
    let enabled = s
        .post_form(
            &format!("/api/v1/admin/accounts/{bob_id}/enable"),
            Some(&alice),
            &[],
        )
        .json_body();
    assert_eq!(enabled["disabled"], false);
    assert_eq!(s.get("/api/v1/timelines/home", Some(&bob)).status, 200);

    assert_eq!(action(&mut s, &bob_id, "suspend"), 200);
    let seen = s
        .get(&format!("/api/v1/accounts/{bob_id}"), Some(&carol))
        .json_body();
    assert_eq!(seen["suspended"], true);
    assert_eq!(seen["display_name"], "");
    let admin_view = s
        .get(&format!("/api/v1/admin/accounts/{bob_id}"), Some(&alice))
        .json_body();
    assert_eq!(admin_view["suspended"], true);
    let suspended = s
        .get("/api/v2/admin/accounts?status=suspended", Some(&alice))
        .json_body();
    assert_eq!(suspended[0]["username"], "bob");
    assert_eq!(
        s.post_form(
            &format!("/api/v1/admin/accounts/{bob_id}/approve"),
            Some(&alice),
            &[]
        )
        .status,
        403
    );

    // Deleting needs a suspension first, then the account is gone.
    let carol_id = me(&mut s, &carol);
    let url = format!("/api/v1/admin/accounts/{carol_id}");
    assert_eq!(s.send(authed("DELETE", &url, &alice)).status, 403);
    let url = format!("/api/v1/admin/accounts/{bob_id}");
    assert_eq!(s.send(authed("DELETE", &url, &alice)).status, 200);
    assert_eq!(s.get(&url, Some(&alice)).status, 404);
    assert_eq!(s.get("/api/v1/timelines/home", Some(&bob)).status, 401);
}

#[test]
fn household_admin_endpoints() {
    let (mut s, alice, bob, carol) = household();
    let (bob_id, carol_id) = (me(&mut s, &bob), me(&mut s, &carol));
    let base = "/api/mastomini/v1/admin/members";

    let r = s.post_form(
        &format!("{base}/{carol_id}/role"),
        Some(&alice),
        &[("role", "admin")],
    );
    assert_eq!(r.json_body()["role"], "admin");
    // Carol, now an admin, can silence bob but not change roles.
    let r = s.post_form(&format!("{base}/{bob_id}/silence"), Some(&carol), &[]);
    assert_eq!(r.json_body()["silenced"], true);
    let r = s.post_form(
        &format!("{base}/{bob_id}/role"),
        Some(&carol),
        &[("role", "admin")],
    );
    assert_eq!(r.status, 403);

    let bobs = id(&post(&mut s, &bob, &[("status", "delete me")]));
    let r = s.send(authed(
        "DELETE",
        &format!("/api/mastomini/v1/admin/statuses/{bobs}"),
        &bob,
    ));
    assert_eq!(r.status, 403, "members can't");
    let r = s.send(authed(
        "DELETE",
        &format!("/api/mastomini/v1/admin/statuses/{bobs}"),
        &carol,
    ));
    assert_eq!(r.status, 200);
    assert_eq!(
        s.get(&format!("/api/v1/statuses/{bobs}"), Some(&bob))
            .status,
        404
    );

    let del = |s: &mut Server, confirm: &str| {
        let req = authed("DELETE", &format!("{base}/{bob_id}"), &alice).with_body(
            "application/json",
            json!({ "confirm": confirm }).to_string(),
        );
        s.send(req).status
    };
    assert_eq!(del(&mut s, "bobby"), 422);
    assert_eq!(del(&mut s, "bob"), 200);
    let members = s.get(base, Some(&alice)).json_body();
    assert_eq!(members.as_array().unwrap().len(), 2);
}

#[test]
fn collections_endpoints() {
    let (mut s, alice, bob, carol) = household();
    let (alice_id, bob_id, carol_id) = (me(&mut s, &alice), me(&mut s, &bob), me(&mut s, &carol));
    let r = s.send_json(
        "POST",
        "/api/v1/collections",
        Some(&alice),
        &json!({ "name": "Kids", "description": "Our two", "account_ids": [bob_id], "tag_name": "family" }),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let created = r.json_body()["collection"].clone();
    let cid = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["account_id"], alice_id.as_str());
    assert_eq!(created["item_count"], 1);
    assert_eq!(created["items"][0]["state"], "accepted");
    assert_eq!(created["tag"]["name"], "family");
    assert_eq!(created["local"], true);

    let item = s.send_json(
        "POST",
        &format!("/api/v1/collections/{cid}/items"),
        Some(&alice),
        &json!({ "account_id": carol_id }),
    );
    let item = item.json_body()["collection_item"].clone();
    assert_eq!(item["account_id"], carol_id.as_str());
    let item_id = item["id"].as_str().unwrap().to_string();

    let shown = s
        .get(&format!("/api/v1/collections/{cid}"), Some(&bob))
        .json_body();
    let names: Vec<&str> = shown["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["username"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["alice", "bob", "carol"]);

    let note = s
        .get(
            "/api/v1/notifications?types[]=added_to_collection",
            Some(&carol),
        )
        .json_body();
    assert_eq!(note[0]["collection"]["id"], cid.as_str());

    let listed = s
        .get(
            &format!("/api/v1/accounts/{alice_id}/collections"),
            Some(&bob),
        )
        .json_body();
    assert_eq!(listed["collections"][0]["id"], cid.as_str());
    let featuring = s
        .get(
            &format!("/api/v1/accounts/{carol_id}/in_collections"),
            Some(&carol),
        )
        .json_body();
    assert_eq!(featuring["collections"][0]["id"], cid.as_str());

    // Carol revokes; she is gone for everyone but the curator.
    let revoke = format!("/api/v1/collections/{cid}/items/{item_id}/revoke");
    assert_eq!(s.post_form(&revoke, Some(&bob), &[]).status, 403);
    assert_eq!(s.post_form(&revoke, Some(&carol), &[]).status, 200);
    let shown = s
        .get(&format!("/api/v1/collections/{cid}"), Some(&bob))
        .json_body();
    assert_eq!(shown["collection"]["item_count"], 1);
    assert_eq!(shown["accounts"].as_array().unwrap().len(), 2);
    let curator = s
        .get(&format!("/api/v1/collections/{cid}"), Some(&alice))
        .json_body();
    assert_eq!(curator["collection"]["items"][1]["state"], "revoked");

    let patched = s.send_json(
        "PATCH",
        &format!("/api/v1/collections/{cid}"),
        Some(&alice),
        &json!({ "name": "Kid", "discoverable": false }),
    );
    assert_eq!(patched.json_body()["collection"]["name"], "Kid");
    let listed = s
        .get(
            &format!("/api/v1/accounts/{alice_id}/collections"),
            Some(&bob),
        )
        .json_body();
    assert_eq!(listed["collections"], json!([]));
    let url = format!("/api/v1/collections/{cid}");
    assert_eq!(s.send(authed("DELETE", &url, &bob)).status, 403);
    assert_eq!(s.send(authed("DELETE", &url, &alice)).status, 200);
    assert_eq!(s.get(&url, Some(&alice)).status, 404);

    let long = "x".repeat(41);
    let r = s.send_json(
        "POST",
        "/api/v1/collections",
        Some(&alice),
        &json!({ "name": long }),
    );
    assert_eq!(r.status, 422);
}

#[test]
fn moderation_survives_restart() {
    let (mut s, alice, bob, _) = household();
    let bob_id = me(&mut s, &bob);
    s.post_form(
        &format!("/api/v1/accounts/{bob_id}/block"),
        Some(&alice),
        &[],
    );
    s.post_form(
        &format!("/api/v1/admin/accounts/{bob_id}/action"),
        Some(&alice),
        &[("type", "silence")],
    );
    let mut s = s.restart();
    let rel = s
        .get(
            &format!("/api/v1/accounts/relationships?id[]={bob_id}"),
            Some(&alice),
        )
        .json_body();
    assert_eq!(rel[0]["blocking"], true);
    let admin = s
        .get(&format!("/api/v1/admin/accounts/{bob_id}"), Some(&alice))
        .json_body();
    assert_eq!(admin["silenced"], true);
}
