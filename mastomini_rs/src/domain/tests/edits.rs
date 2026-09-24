//! Edits and polls across power cuts and reboots.

use super::*;

fn poll(options: &[&str]) -> NewPoll {
    NewPoll {
        options: options.iter().map(|o| o.to_string()).collect(),
        expires_in_s: 3600,
        multiple: false,
        hide_totals: false,
    }
}

fn edit(text: &str, p: Option<NewPoll>) -> StatusEdit {
    StatusEdit {
        text: text.into(),
        poll: p,
        ..StatusEdit::default()
    }
}

#[test]
fn power_cut_while_editing_leaves_one_consistent_version() {
    for writes in 0.. {
        let mut svc = household(FaultStore::new(MemStore::default()));
        let new = NewStatus {
            poll: Some(poll(&["a", "b"])),
            ..post("first")
        };
        let id = svc.post_status(0, new, T0).unwrap();
        svc.edit_status(0, id, edit("second", Some(poll(&["a", "b"]))), T0 + 1)
            .unwrap();
        svc.vote(1, id, &[0], T0 + 1).unwrap();
        svc.store().cut_after(writes);
        let result = svc.edit_status(0, id, edit("third", Some(poll(&["x", "y"]))), T0 + 2);
        let completed = !svc.store().is_cut();
        let svc = Service::open(svc.into_store().inner, config()).unwrap();
        svc.check_invariants().unwrap();
        let status = &svc.state.statuses[&id];
        let since = status.rec.edited_at_ms.unwrap();
        let history: Vec<&str> = svc.revisions(id).iter().map(|r| r.text.as_str()).collect();
        // Never a history entry as new as the current version.
        assert!(svc.revisions(id).iter().all(|r| r.created_ms < since));
        match status.rec.text.as_str() {
            "second" => assert_eq!(history, ["first"], "cut at {writes}"),
            "third" => {
                assert_eq!(history, ["first", "second"], "cut at {writes}");
                assert_eq!(svc.state.polls[&id].options, ["x", "y"]);
                assert_eq!(svc.state.polls[&id].voters(), 0);
            }
            other => panic!("cut at {writes}: text {other}"),
        }
        if completed {
            result.unwrap();
            break;
        }
        assert!(writes < 20, "operation never completed");
    }
}

#[test]
fn power_cut_while_posting_a_poll() {
    power_cut_every_write(
        |_| 0,
        |svc, _| {
            let new = NewStatus {
                poll: Some(poll(&["a", "b"])),
                ..post("vote!")
            };
            svc.post_status(0, new, T0 + 1).map(|_| ())
        },
    );
    // No poll without its status.
    for writes in 0..3 {
        let mut svc = household(FaultStore::new(MemStore::default()));
        svc.store().cut_after(writes);
        let new = NewStatus {
            poll: Some(poll(&["a", "b"])),
            ..post("vote!")
        };
        let _ = svc.post_status(0, new, T0 + 1);
        let svc = Service::open(svc.into_store().inner, config()).unwrap();
        assert!(svc
            .state
            .polls
            .keys()
            .all(|id| svc.state.statuses.contains_key(id)));
    }
}

#[test]
fn three_previous_versions_at_most() {
    let mut svc = household(MemStore::default());
    let id = svc.post_status(0, post("v0"), T0).unwrap();
    for i in 1..=5 {
        svc.edit_status(0, id, edit(&format!("v{i}"), None), T0 + i)
            .unwrap();
    }
    let svc = reopen(svc);
    let texts: Vec<&str> = svc.revisions(id).iter().map(|r| r.text.as_str()).collect();
    assert_eq!(texts, ["v2", "v3", "v4"]);
    assert_eq!(svc.state.statuses[&id].rec.text, "v5");
}

#[test]
fn deleting_a_status_takes_its_history_and_poll() {
    let mut svc = household(MemStore::default());
    let new = NewStatus {
        poll: Some(poll(&["a", "b"])),
        ..post("v0")
    };
    let id = svc.post_status(0, new, T0).unwrap();
    svc.edit_status(0, id, edit("v1", Some(poll(&["a", "b"]))), T0 + 1)
        .unwrap();
    svc.delete_status(0, id, T0 + 2).unwrap();
    let svc = reopen(svc);
    assert!(svc.state.history.is_empty());
    assert!(svc.state.polls.is_empty());
}

#[test]
fn a_deleted_members_votes_do_not_pass_to_the_next() {
    let mut svc = household(MemStore::default());
    let new = NewStatus {
        poll: Some(poll(&["a", "b"])),
        ..post("vote!")
    };
    let id = svc.post_status(0, new, T0).unwrap();
    svc.vote(2, id, &[1], T0).unwrap();
    svc.delete_account(0, 2, T0).unwrap();
    let mut svc = reopen(svc);
    assert_eq!(svc.state.polls[&id].voters(), 0);
    let slot = svc.create_member(0, member("dave"), T0).unwrap();
    assert_eq!(slot, 2);
    assert_eq!(svc.state.polls[&id].voters() & bit(slot), 0);
}
