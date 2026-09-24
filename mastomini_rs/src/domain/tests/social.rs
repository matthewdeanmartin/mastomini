//! Follow requests, lists and filters across power cuts and reboots.

use super::*;

fn locked(svc: &mut Service<impl Store>, slot: u8) {
    let update = ProfileUpdate {
        locked: Some(true),
        ..ProfileUpdate::default()
    };
    svc.update_profile(slot, update, T0).unwrap();
}

#[test]
fn power_cut_while_accepting_leaves_a_request_or_a_follow() {
    for writes in 0.. {
        let mut svc = household(FaultStore::new(MemStore::default()));
        locked(&mut svc, 2);
        svc.set_follow(1, 2, true, None, None, T0).unwrap();
        assert!(svc.state.follow_requests.contains_key(&(1, 2)));
        svc.store().cut_after(writes);
        let result = svc.authorize_follow(2, 1, T0 + 1);
        let completed = !svc.store().is_cut();
        let svc = Service::open(svc.into_store().inner, config()).unwrap();
        svc.check_invariants().unwrap();
        let follows = svc.state.follows(1, 2);
        let requested = svc.state.follow_requests.contains_key(&(1, 2));
        assert!(follows != requested, "cut at {writes}: exactly one of them");
        if completed {
            result.unwrap();
            assert!(follows);
            break;
        }
        assert!(writes < 10, "operation never completed");
    }
}

#[test]
fn blocking_ends_requests_and_deleting_ends_lists_and_filters() {
    let mut svc = household(MemStore::default());
    locked(&mut svc, 2);
    svc.set_follow(1, 2, true, None, None, T0).unwrap();
    svc.set_block(2, 1, true, T0).unwrap();
    assert!(svc.state.follow_requests.is_empty());

    let list = svc
        .create_list(1, "friends", RepliesPolicy::List, false, T0)
        .unwrap();
    svc.add_to_list(1, list, &[svc.state.account(0).unwrap().rec.id], T0)
        .unwrap();
    let filter = FilterRec {
        id: 0,
        title: "hand".into(),
        context: 1,
        action: FilterAction::Hide,
        expires_ms: None,
        keywords: vec![FilterKeywordRec {
            id: 0,
            keyword: "hand".into(),
            whole_word: true,
        }],
        statuses: Vec::new(),
    };
    svc.save_filter(1, filter, T0).unwrap();
    let mut svc = reopen(svc);
    assert_eq!(svc.lists_of(1).count(), 1);
    assert_eq!(svc.filters_of(1).count(), 1);
    svc.delete_account(0, 1, T0).unwrap();
    let svc = reopen(svc);
    assert!(svc.state.lists.is_empty());
    assert!(svc.state.filters.is_empty());
}

#[test]
fn at_most_eight_lists_and_filters() {
    let mut svc = household(MemStore::default());
    for i in 0..8 {
        svc.create_list(0, &format!("l{i}"), RepliesPolicy::List, false, T0)
            .unwrap();
    }
    assert!(svc
        .create_list(0, "one more", RepliesPolicy::List, false, T0)
        .is_err());
    let filter = |title: &str| FilterRec {
        id: 0,
        title: title.into(),
        context: 1,
        action: FilterAction::Warn,
        expires_ms: None,
        keywords: Vec::new(),
        statuses: Vec::new(),
    };
    for i in 0..8 {
        svc.save_filter(0, filter(&format!("f{i}")), T0).unwrap();
    }
    assert!(svc.save_filter(0, filter("one more"), T0).is_err());
}
