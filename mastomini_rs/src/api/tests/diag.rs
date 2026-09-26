//! `/diag`, `/clock`, `/admin/security` and the new `/status` fields.

use super::*;

fn member_id(s: &mut Server, admin: &str, username: &str) -> String {
    let members = s
        .get("/api/mastomini/v1/admin/members", Some(admin))
        .json_body();
    members
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["username"] == username)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Send with no synced clock, as the board does before SNTP.
fn unsynced(s: &mut Server, req: Request, token: &str) -> crate::http::Response {
    let req = req.with_header("Authorization", &format!("Bearer {token}"));
    super::super::handle(&mut s.svc, &s.ctx, &req, None)
}

#[test]
fn diag_is_for_admins() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    let bob = s.add_member(&alice, "bob");
    assert_eq!(s.get("/api/mastomini/v1/diag", Some(&bob)).status, 403);
    assert_eq!(s.get("/api/mastomini/v1/diag", None).status, 401);
    let d = s.get("/api/mastomini/v1/diag", Some(&alice)).json_body();
    assert_eq!(d["platform"]["target"], "desktop");
    assert_eq!(d["incidents"]["volatile"], true);
    assert!(d["incidents"]["retained_bytes"].as_u64().unwrap() < 4096);
    assert!(d["incidents"]["events"].is_array());
    assert!(d["incidents"]["samples"].is_array());
    assert_eq!(d["clock"]["source"], "synced");
    assert_eq!(d["records"]["accounts"], json!([2, 16]));
    assert_eq!(d["store"]["available"], true);
    assert!(d["store"]["entries_used"].as_u64().unwrap() > 0);

    let status = s.get("/api/mastomini/v1/status", None).json_body();
    assert_eq!(status["clock"], "synced");
    assert_eq!(status["mode"], "http");
    assert_eq!(status["https"], false);
}

#[test]
fn an_admin_sets_the_clock_while_it_is_unset() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    let bob = s.add_member(&alice, "bob");
    let later = s.now + 60_000;
    let form = |ms: u64| {
        Request::new("POST", "/api/mastomini/v1/clock")
            .with_body("application/x-www-form-urlencoded", format!("ms={ms}"))
    };

    // Synced: refused.
    let r = s.post_form(
        "/api/mastomini/v1/clock",
        Some(&alice),
        &[("ms", &later.to_string())],
    );
    assert_eq!(r.status, 409);

    // Unset: writes wait for the clock, members can't set it, the past is refused.
    let post = Request::new("POST", "/api/v1/statuses")
        .with_body("application/x-www-form-urlencoded", "status=hello");
    assert_eq!(unsynced(&mut s, post.clone(), &bob).status, 503);
    assert_eq!(unsynced(&mut s, form(later), &bob).status, 403);
    assert_eq!(unsynced(&mut s, form(1_000), &alice).status, 422);
    let r = unsynced(&mut s, form(later), &alice);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json_body()["source"], "manual");

    // Writes work again, with the manual time.
    let r = unsynced(&mut s, post, &bob);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let created = r.json_body()["created_at"].as_str().unwrap().to_string();
    assert_eq!(created, super::super::time::iso(later)[..created.len()]);
    let status = unsynced(
        &mut s,
        Request::new("GET", "/api/mastomini/v1/status"),
        &alice,
    );
    assert_eq!(status.json_body()["clock"], "manual");
}

#[test]
fn security_summary_is_for_the_owner() {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    let bob = s.add_member(&alice, "bob");
    let bob_id = member_id(&mut s, &alice, "bob");
    s.post_form(
        &format!("/api/mastomini/v1/admin/members/{bob_id}/role"),
        Some(&alice),
        &[("role", "admin")],
    );
    let r = s.get("/api/mastomini/v1/admin/security", Some(&bob));
    assert_eq!(r.status, 403);
    let sec = s
        .get("/api/mastomini/v1/admin/security", Some(&alice))
        .json_body();
    assert_eq!(sec["transport"]["mode"], "http");
    assert_eq!(sec["passwords"]["min_length"], 4);
    let members = sec["members"].as_array().unwrap();
    let bob_row = members.iter().find(|m| m["username"] == "bob").unwrap();
    assert_eq!(bob_row["role"], "admin");
    assert_eq!(bob_row["devices"], 1);
    assert_eq!(bob_row["message_key"], true);
}

#[test]
fn version_is_public_and_identifies_the_build() {
    let mut s = Server::provisioned();
    let r = s.get("/api/mastomini/v1/version", None);
    assert_eq!(r.status, 200);
    assert!(r
        .headers
        .iter()
        .any(|(k, v)| k == "Cache-Control" && v == "no-store"));
    let v = r.json_body();
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    let fingerprint = v["fingerprint"].as_str().unwrap();
    assert_eq!(fingerprint.len(), 12);
    assert!(fingerprint.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(v["target"], "desktop");
    assert!(v["uptime_ms"].is_u64());
    assert!(v["built_at"].as_str().unwrap().ends_with('Z'));
    // Nothing about the household.
    for key in ["accounts", "records", "store", "platform"] {
        assert!(v.get(key).is_none(), "{key}");
    }
}
