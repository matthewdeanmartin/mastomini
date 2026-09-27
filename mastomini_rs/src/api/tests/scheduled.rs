use super::*;
use crate::api::time;
use crate::domain::Service;
use crate::store::mem::FaultStore;
use std::sync::{Arc, Mutex};

fn setup() -> (Server, String, String) {
    let mut s = Server::provisioned();
    let alice = s.login("alice", "alicepw", "read write");
    let (_, key) = s
        .svc
        .create_api_key(0, "alicepw", "Existing bots key", "read write", s.now)
        .unwrap();
    let r = s.send_json(
        "PUT",
        "/api/mastomini/v1/admin/scheduler",
        Some(&alice),
        &json!({"bots_url": "https://mastomini-bots.local", "api_key": key}),
    );
    assert_eq!(r.status, 200, "{}", r.json_body());
    assert!(!String::from_utf8_lossy(&r.body).contains(&key));
    (s, alice, key)
}

fn schedule(s: &mut Server, token: &str, at: u64) -> u64 {
    let r = s.send_json(
        "POST",
        "/api/v1/statuses",
        Some(token),
        &json!({"status": "Only publish when due", "scheduled_at": time::iso(at)}),
    );
    assert_eq!(r.status, 200, "{}", r.json_body());
    r.json_body()["id"].as_str().unwrap().parse().unwrap()
}

#[test]
fn scheduled_drafts_are_private_durable_and_manageable() {
    let (mut s, alice, _) = setup();
    let bob = s.add_member(&alice, "bob");
    let at = T0 + 600_000;
    let id = schedule(&mut s, &alice, at);
    assert!(s.svc.state.statuses.is_empty());
    assert!(
        s.svc.scheduling.jobs[&id].post.text.is_empty(),
        "draft bodies must not stay in RAM"
    );
    let path = format!("/api/v1/scheduled_statuses/{id}");
    for method in ["GET", "PUT", "DELETE"] {
        let r = s.send_json(
            method,
            &path,
            Some(&bob),
            &json!({"scheduled_at": time::iso(at + 60_000)}),
        );
        assert_eq!(r.status, 404);
    }
    let mut s = s.restart();
    assert_eq!(
        s.get(&path, Some(&alice)).json_body()["params"]["text"],
        "Only publish when due"
    );
    assert_eq!(
        s.get("/api/v1/scheduled_statuses", Some(&bob)).json_body(),
        json!([])
    );
    assert_eq!(
        s.send_json(
            "PUT",
            &path,
            Some(&alice),
            &json!({"scheduled_at": time::iso(at + 60_000)})
        )
        .status,
        200
    );
    assert_eq!(s.svc.scheduling.jobs[&id].revision, 2);
    assert!(s.svc.publish_scheduled(id, 1, at + 90_000).is_err());
    assert_eq!(
        s.send_json("DELETE", &path, Some(&alice), &json!({}))
            .status,
        200
    );
    assert!(s.svc.publish_scheduled(id, 3, at + 90_000).is_err());
    assert!(s.svc.state.statuses.is_empty());
}

#[test]
fn publish_requires_configured_live_key_and_never_runs_early() {
    let (mut s, alice, key) = setup();
    let at = T0 + 600_000;
    let id = schedule(&mut s, &alice, at);
    let path = format!("/api/mastomini/v1/scheduler/publish/{id}");
    assert_eq!(
        s.send_json("POST", &path, Some(&alice), &json!({"revision": 1}))
            .status,
        403
    );
    assert_eq!(
        s.send_json("POST", &path, Some(&key), &json!({"revision": 1}))
            .status,
        503
    );
    s.now = at + 7 * 86_400_000; // No lateness expiry, even after a week offline.
    let first = s.send_json("POST", &path, Some(&key), &json!({"revision": 1}));
    assert_eq!(first.status, 200);
    assert_eq!(s.svc.state.statuses.len(), 1);
    assert_eq!(
        s.get("/api/v1/scheduled_statuses", Some(&alice))
            .json_body(),
        json!([])
    );
    let mut s = s.restart();
    let retry = s.send_json("POST", &path, Some(&key), &json!({"revision": 1}));
    assert_eq!(retry.json_body(), first.json_body());
    assert_eq!(s.svc.state.statuses.len(), 1);
    s.svc.revoke(&key).unwrap();
    assert_eq!(
        s.send_json("POST", &path, Some(&key), &json!({"revision": 1}))
            .status,
        401
    );
}

#[test]
fn publishing_survives_every_write_boundary_without_duplicates() {
    let (mut s, alice, _) = setup();
    let at = T0 + 600_000;
    let id = schedule(&mut s, &alice, at);
    let config = s.svc.config.clone();
    let snapshot = s.svc.into_store();
    for cut in 0..5 {
        let mut svc = Service::open(FaultStore::new(snapshot.clone()), config.clone()).unwrap();
        svc.store().cut_after(cut);
        let _ = svc.publish_scheduled(id, 1, at);
        let mut reboot = Service::open(svc.into_store().inner, config.clone()).unwrap();
        let first = reboot.publish_scheduled(id, 1, at + 1000).unwrap();
        let second = reboot.publish_scheduled(id, 1, at + 2000).unwrap();
        assert_eq!(first, second, "cut {cut}");
        assert_eq!(reboot.state.statuses.len(), 1, "cut {cut}");
    }
}

#[test]
fn scheduling_validates_time_direct_messages_and_capacity() {
    let (mut s, alice, _) = setup();
    for body in [
        json!({"status":"x", "scheduled_at":"not a date"}),
        json!({"status":"x", "scheduled_at":time::iso(T0 + 1000)}),
        json!({"status":"x", "visibility":"direct", "scheduled_at":time::iso(T0 + 600_000)}),
    ] {
        assert_eq!(
            s.send_json("POST", "/api/v1/statuses", Some(&alice), &body)
                .status,
            422
        );
    }
    for _ in 0..16 {
        schedule(&mut s, &alice, T0 + 600_000);
    }
    assert_eq!(
        s.send_json(
            "POST",
            "/api/v1/statuses",
            Some(&alice),
            &json!({"status":"overflow", "scheduled_at":time::iso(T0 + 600_000)})
        )
        .status,
        422
    );
}

#[test]
fn idempotent_submission_and_deleted_author_do_not_duplicate_or_leak() {
    let (mut s, alice, _) = setup();
    let at = T0 + 600_000;
    let request = || {
        Request::new("POST", "/api/v1/statuses")
            .with_header("Authorization", &format!("Bearer {alice}"))
            .with_header("Idempotency-Key", "same-draft")
            .with_body(
                "application/json",
                serde_json::to_vec(&json!({"status":"saved once", "scheduled_at":time::iso(at)}))
                    .unwrap(),
            )
    };
    let first = s.send(request());
    s.now = at; // A late retry must return the original even after the scheduling deadline.
    let again = s.send(request());
    assert_eq!(again.status, 200);
    assert_eq!(first.json_body()["id"], again.json_body()["id"]);
    assert_eq!(s.svc.scheduling.jobs.len(), 1);
    let id: u64 = first.json_body()["id"].as_str().unwrap().parse().unwrap();
    // Changing the account epoch invalidates the queued authorization.
    s.svc.state.accounts[0].as_mut().unwrap().rec.token_epoch += 1;
    assert_eq!(s.svc.publish_scheduled(id, 1, at + 100).unwrap(), None);
    assert!(s.svc.scheduling.jobs[&id].failure.is_some());
    assert!(s.svc.state.statuses.is_empty());
}

#[test]
fn cancelling_during_handoff_does_not_acknowledge_the_old_revision() {
    let (mut s, alice, _) = setup();
    let id = schedule(&mut s, &alice, T0 + 600_000);
    let main = Arc::new(Mutex::new(s.svc));
    crate::scheduler_bridge::transfer(&main, |_| {
        // Also proves there is no service mutex held during network I/O.
        main.lock()
            .unwrap()
            .change_schedule(0, id, None, T0 + 100)
            .unwrap();
        Ok(200)
    });
    assert!(!main
        .lock()
        .unwrap()
        .scheduling
        .acknowledged
        .contains(&(id, 1)));
    crate::scheduler_bridge::transfer(&main, |job| {
        assert_eq!(job.revision, 2);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&job.body).unwrap()["cancelled"],
            true
        );
        Ok(200)
    });
    assert!(main
        .lock()
        .unwrap()
        .scheduling
        .acknowledged
        .contains(&(id, 2)));
}

#[test]
fn polls_start_when_published_and_replay_after_deletion_is_harmless() {
    let (mut s, alice, _) = setup();
    let at = T0 + 600_000;
    let r = s.send_json(
        "POST",
        "/api/v1/statuses",
        Some(&alice),
        &json!({
            "status":"Choose", "scheduled_at":time::iso(at),
            "poll":{"options":["red","blue"], "expires_in":3600}
        }),
    );
    assert_eq!(r.status, 200);
    assert!(s.svc.state.polls.is_empty());
    let id = r.json_body()["id"].as_str().unwrap().parse().unwrap();
    let late = at + 86_400_000;
    let published = s.svc.publish_scheduled(id, 1, late).unwrap().unwrap();
    assert_eq!(s.svc.state.polls[&published].expires_ms, late + 3_600_000);
    s.svc.delete_status(0, published, late + 100).unwrap();
    assert_eq!(
        s.svc.publish_scheduled(id, 1, late + 200).unwrap(),
        Some(published)
    );
    assert!(s.svc.state.statuses.is_empty());
}

#[test]
fn existing_member_api_key_can_be_delegated_by_admin_and_deletion_erases_drafts() {
    let (mut s, alice, _) = setup();
    let bob = s.add_member(&alice, "bob");
    let (_, key) = s
        .svc
        .create_api_key(1, "secret", "Existing bot key", "read write", s.now)
        .unwrap();
    let config = json!({"bots_url":"https://mastomini-bots.local","api_key":key});
    assert_eq!(
        s.send_json(
            "PUT",
            "/api/mastomini/v1/admin/scheduler",
            Some(&bob),
            &config
        )
        .status,
        403
    );
    assert_eq!(
        s.send_json(
            "PUT",
            "/api/mastomini/v1/admin/scheduler",
            Some(&alice),
            &config
        )
        .status,
        200
    );
    let at = T0 + 600_000;
    let alice_job = schedule(&mut s, &alice, at);
    let bob_job = schedule(&mut s, &bob, at);
    s.now = at;
    // Normal traffic does not run the scheduler, even after jobs become due.
    assert_eq!(
        s.get("/api/v1/timelines/home", Some(&alice)).json_body(),
        json!([])
    );
    let callback = format!("/api/mastomini/v1/scheduler/publish/{alice_job}");
    assert_eq!(
        s.send_json("POST", &callback, Some(&key), &json!({"revision":1}))
            .status,
        200
    );
    s.svc.delete_account(0, 1, at + 100).unwrap();
    let deleted = s.svc.scheduled_post(bob_job).unwrap();
    assert!(deleted.cancelled);
    assert!(deleted.post.text.is_empty());
    assert!(s.svc.publish_scheduled(bob_job, 1, at + 200).is_err());
}

#[test]
fn interrupted_before_status_commit_can_still_be_cancelled() {
    let (mut s, alice, _) = setup();
    let at = T0 + 600_000;
    let id = schedule(&mut s, &alice, at);
    let config = s.svc.config.clone();
    let mut svc = Service::open(FaultStore::new(s.svc.into_store()), config.clone()).unwrap();
    svc.store().cut_after(1); // reserve the status ID, then fail the status write
    assert!(svc.publish_scheduled(id, 1, at).is_err());
    let mut reboot = Service::open(svc.into_store().inner, config).unwrap();
    reboot.change_schedule(0, id, None, at + 1000).unwrap();
    assert!(reboot.scheduling.jobs[&id].cancelled);
    assert!(reboot.state.statuses.is_empty());
    assert!(reboot.scheduled_post(id).unwrap().post.text.is_empty());
}
