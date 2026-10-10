//! Moving a household to another board: export, encryption, restore
//! (domain::transfer, spec/board_migration_plan.md path 2).

use super::*;
use crate::http::Request;

const EXPORT: &str = "/api/mastomini/v1/admin/export";
const IMPORT: &str = "/api/mastomini/v1/import";
const PASSPHRASE: &str = "correct horse battery";

fn export(s: &mut Server, token: &str, password: &str, passphrase: &str) -> crate::http::Response {
    s.post_form(
        EXPORT,
        Some(token),
        &[("password", password), ("passphrase", passphrase)],
    )
}

fn import(s: &mut Server, archive: &[u8], passphrase: &str) -> crate::http::Response {
    s.send(
        Request::new("POST", IMPORT)
            .with_header("X-Mastomini-Passphrase", passphrase)
            .with_body("application/octet-stream", archive.to_vec()),
    )
}

/// A household with a member, posts, a follow and a direct message.
fn household() -> (Server, String, String, String) {
    let mut s = Server::provisioned();
    let alice = s.login(
        "alice",
        "alicepw",
        "read write follow admin:read admin:write",
    );
    let bob = s.add_member(&alice, "bob");
    for text in ["first post", "second post #garden"] {
        let r = s.post_form("/api/v1/statuses", Some(&alice), &[("status", text)]);
        assert_eq!(r.status, 200);
    }
    let r = s.post_form(
        "/api/v1/statuses",
        Some(&alice),
        &[("status", "@bob only for you"), ("visibility", "direct")],
    );
    assert_eq!(r.status, 200);
    let dm = r.json_body()["id"].as_str().unwrap().to_string();
    (s, alice, bob, dm)
}

#[test]
fn a_household_moves_to_an_empty_board_with_sessions_and_messages_intact() {
    let (mut old, alice, bob, dm) = household();
    let r = export(&mut old, &alice, "alicepw", PASSPHRASE);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert!(r
        .header("Content-Disposition")
        .unwrap()
        .contains(".mmexport"));
    assert_eq!(r.header("Cache-Control"), Some("no-store"));
    let archive = r.body.clone();
    assert!(!archive
        .windows(b"only for you".len())
        .any(|w| w == b"only for you"));
    assert!(
        !archive.windows(5).any(|w| w == b"alice"),
        "nothing readable"
    );

    let mut new = Server::new();
    let r = import(&mut new, &archive, PASSPHRASE);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json_body()["accounts"], 2);
    let statuses = r.json_body()["statuses"].as_u64().unwrap();
    assert_eq!(statuses as usize, old.svc.state.statuses.len());

    // Same name (the same board identity): devices stay signed in, and the
    // direct message is still readable with the old token.
    let me = new.get("/api/v1/accounts/verify_credentials", Some(&bob));
    assert_eq!(me.status, 200);
    let message = new.get(&format!("/api/v1/statuses/{dm}"), Some(&bob));
    assert!(message.json_body()["content"]
        .as_str()
        .unwrap()
        .contains("only for you"));
    // Passwords moved too: a new device can sign in.
    new.login("alice", "alicepw", "read");
    // And the household survives a reboot of the new board.
    let mut rebooted = new.restart();
    assert_eq!(
        rebooted.get("/api/mastomini/v1/status", None).json_body()["accounts"],
        2
    );
}

#[test]
fn only_the_owner_with_their_password_and_a_real_passphrase_can_export() {
    let (mut s, alice, bob, _) = household();
    assert_eq!(export(&mut s, &bob, "secret", PASSPHRASE).status, 403);
    assert_eq!(export(&mut s, &alice, "wrong", PASSPHRASE).status, 403);
    let short = export(&mut s, &alice, "alicepw", "too short");
    assert_eq!(short.status, 422);
    assert_eq!(
        s.post_form(EXPORT, None, &[("password", "alicepw")]).status,
        401
    );
}

#[test]
fn a_restore_needs_the_passphrase_an_intact_file_and_an_empty_board() {
    let (mut old, alice, _, _) = household();
    let archive = export(&mut old, &alice, "alicepw", PASSPHRASE).body;
    let mut new = Server::new();

    let wrong = import(&mut new, &archive, "a different passphrase");
    assert_eq!(wrong.status, 422);
    assert!(String::from_utf8_lossy(&wrong.body).contains("Wrong passphrase"));
    let mut damaged = archive.clone();
    let middle = damaged.len() / 2;
    damaged[middle] ^= 1;
    assert_eq!(import(&mut new, &damaged, PASSPHRASE).status, 422);
    assert_eq!(
        import(&mut new, b"not an export at all, clearly", PASSPHRASE).status,
        422
    );
    // Still empty and usable after the failures.
    assert_eq!(
        new.get("/api/mastomini/v1/status", None).json_body()["provisioned"],
        false
    );

    // A board that already has a household is never overwritten.
    assert_eq!(import(&mut old, &archive, PASSPHRASE).status, 403);
    assert_eq!(import(&mut new, &archive, PASSPHRASE).status, 200);
    assert_eq!(import(&mut new, &archive, PASSPHRASE).status, 403);
}

#[test]
fn an_export_from_newer_firmware_is_refused_by_older_firmware() {
    let (mut old, alice, _, _) = household();
    let mut archive = export(&mut old, &alice, "alicepw", PASSPHRASE).body;
    // The schema is in the authenticated header (bytes 10..12).
    let newer = (crate::domain::SCHEMA + 1).to_le_bytes();
    archive[10..12].copy_from_slice(&newer);
    let r = import(&mut Server::new(), &archive, PASSPHRASE);
    assert_eq!(r.status, 422);
    assert!(String::from_utf8_lossy(&r.body).contains("newer firmware"));
}

#[test]
fn the_restore_page_exists_only_before_setup() {
    let mut empty = Server::new();
    let page = empty.get("/setup", None);
    assert!(String::from_utf8_lossy(&page.body).contains("/setup/restore"));
    let restore = empty.get("/setup/restore", None);
    assert_eq!(restore.status, 200);
    let html = String::from_utf8_lossy(&restore.body);
    assert!(html.contains("X-Mastomini-Passphrase"));
    assert!(html.contains("signs in again"), "explains a new name");
    let mut provisioned = Server::provisioned();
    assert_eq!(provisioned.get("/setup/restore", None).status, 302);
}
