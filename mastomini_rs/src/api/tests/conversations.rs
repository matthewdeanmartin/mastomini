//! `/api/v1/conversations`: direct-message threads.

use super::*;

fn dm(s: &mut Server, token: &str, text: &str, reply_to: Option<&str>) -> String {
    let mut form = vec![("status", text), ("visibility", "direct")];
    if let Some(id) = reply_to {
        form.push(("in_reply_to_id", id));
    }
    let r = s.post_form("/api/v1/statuses", Some(token), &form);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json_body()["id"].as_str().unwrap().to_string()
}

fn conversations(s: &mut Server, token: &str) -> Vec<serde_json::Value> {
    let r = s.get("/api/v1/conversations", Some(token));
    assert_eq!(r.status, 200);
    r.json_body().as_array().unwrap().clone()
}

#[test]
fn direct_messages_form_conversations() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write follow");
    let bob = s.add_member(&alice, "bob");
    let carol = s.add_member(&alice, "carol");
    s.post_form(
        "/api/v1/statuses",
        Some(&alice),
        &[("status", "public hello")],
    );

    let first = dm(&mut s, &alice, "@bob psst", None);
    let convs = conversations(&mut s, &bob);
    assert_eq!(convs.len(), 1);
    assert_eq!(convs[0]["id"], first.as_str());
    assert_eq!(convs[0]["unread"], true);
    assert_eq!(convs[0]["accounts"][0]["username"], "alice");
    assert!(convs[0]["last_status"]["content"]
        .as_str()
        .unwrap()
        .contains("psst"));
    // The sender has read their own message; carol isn't in it.
    assert_eq!(conversations(&mut s, &alice)[0]["unread"], false);
    assert!(conversations(&mut s, &carol).is_empty());

    // A reply stays in the same conversation.
    let reply = dm(&mut s, &bob, "@alice what?", Some(&first));
    let convs = conversations(&mut s, &alice);
    assert_eq!(convs.len(), 1);
    assert_eq!(convs[0]["id"], first.as_str());
    assert_eq!(convs[0]["last_status"]["id"], reply.as_str());
    assert_eq!(convs[0]["unread"], true);

    let read = s.post_form(
        &format!("/api/v1/conversations/{first}/read"),
        Some(&alice),
        &[],
    );
    assert_eq!(read.status, 200);
    assert_eq!(read.json_body()["unread"], false);
    assert_eq!(conversations(&mut s, &alice)[0]["unread"], false);

    // A second conversation, then paging one at a time.
    dm(&mut s, &alice, "@carol lunch?", None);
    let page = s.get("/api/v1/conversations?limit=1", Some(&alice));
    assert_eq!(page.json_body().as_array().unwrap().len(), 1);
    assert!(page.header("Link").unwrap().contains("rel=\"next\""));
    assert_eq!(page.json_body()[0]["accounts"][0]["username"], "carol");

    // Deleting hides it until someone writes again.
    let r = s.send(
        Request::new("DELETE", &format!("/api/v1/conversations/{first}"))
            .with_header("Authorization", &format!("Bearer {alice}")),
    );
    assert_eq!(r.status, 200);
    let ids: Vec<String> = conversations(&mut s, &alice)
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect();
    assert!(!ids.contains(&first));
    dm(&mut s, &bob, "@alice hello again", Some(&reply));
    assert_eq!(conversations(&mut s, &alice).len(), 2);

    let missing = s.post_form("/api/v1/conversations/123/read", Some(&alice), &[]);
    assert_eq!(missing.status, 404);
}
