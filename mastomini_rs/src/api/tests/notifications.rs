use super::*;

#[test]
fn notification_api_paging_and_dismissal_survive_restart() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    for _ in 0..4 {
        assert_eq!(
            s.post_form(
                "/api/v1/statuses",
                Some(&bob),
                &[("status", "@alice hello")]
            )
            .status,
            200
        );
    }
    let first = s.get(
        "/api/v1/notifications?types[]=mention&limit=2",
        Some(&alice),
    );
    assert_eq!(first.status, 200);
    let link = first.header("Link").unwrap().to_string();
    assert!(link.contains("rel=\"next\""));
    let rows = first.json_body();
    let id = rows[0]["id"].as_str().unwrap();
    let cursor = rows[1]["id"].as_str().unwrap();
    let url = format!("/api/v1/notifications?types[]=mention&limit=2&max_id={cursor}");
    let expected = s.get(&url, Some(&alice)).json_body();
    let mut s = s.restart();
    assert_eq!(s.get(&url, Some(&alice)).json_body(), expected);
    assert_eq!(
        s.get(&format!("/api/v1/notifications/{id}"), Some(&alice))
            .status,
        200
    );
    assert_eq!(
        s.get(&format!("/api/v1/notifications/{id}"), Some(&bob))
            .status,
        404
    );
    let groups = s.get("/api/v2/notifications", Some(&alice)).json_body();
    assert_eq!(groups["notification_groups"].as_array().unwrap().len(), 4);
    let group = groups["notification_groups"][0]["group_key"]
        .as_str()
        .unwrap();
    assert_eq!(
        s.post_form(
            &format!("/api/v2/notifications/{group}/dismiss"),
            Some(&alice),
            &[]
        )
        .status,
        200
    );
    let mut s = s.restart();
    assert_eq!(
        s.get("/api/v1/notifications", Some(&alice))
            .json_body()
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let rows = s.get("/api/v1/notifications", Some(&alice)).json_body();
    let id = rows[0]["id"].as_str().unwrap();
    assert_eq!(
        s.post_form(
            &format!("/api/v1/notifications/{id}/dismiss"),
            Some(&alice),
            &[]
        )
        .status,
        200
    );
    let mut s = s.restart();
    assert_eq!(
        s.get("/api/v1/notifications", Some(&alice))
            .json_body()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        s.post_form("/api/v1/notifications/clear", Some(&alice), &[])
            .status,
        200
    );
    let mut s = s.restart();
    assert_eq!(
        s.get("/api/v1/notifications", Some(&alice)).json_body(),
        json!([])
    );
    assert_eq!(
        s.get("/api/v2/notifications", Some(&alice)).json_body()["notification_groups"],
        json!([])
    );
}
