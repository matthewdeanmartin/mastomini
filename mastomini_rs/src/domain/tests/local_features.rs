use super::*;
use crate::domain::social::AnnouncementRec;

fn announcement() -> AnnouncementRec {
    AnnouncementRec {
        id: 0,
        text: "Dinner at six".into(),
        published: true,
        all_day: false,
        starts_ms: None,
        ends_ms: None,
        published_ms: None,
        updated_ms: T0,
        read_by: vec![],
        reactions: BTreeMap::new(),
    }
}

#[test]
fn local_preferences_commit_atomically_and_purge_after_power_cuts() {
    for budget in 0..20 {
        let mut svc = household(FaultStore::new(MemStore::default()));
        let old = svc.state.account(2).unwrap().rec.id;
        svc.set_tag(2, "rust", true, true, T0).unwrap();
        svc.set_tag(2, "rust", false, true, T0).unwrap();
        svc.set_account_note(0, 2, "private", T0).unwrap();
        svc.set_account_note(2, 0, "also private", T0).unwrap();
        svc.dismiss_suggestion(0, 2, T0).unwrap();
        let ann = svc.save_announcement(0, announcement(), T0).unwrap();
        svc.react_announcement(2, ann, None, T0).unwrap();
        svc.react_announcement(2, ann, Some(("👍", true)), T0)
            .unwrap();
        svc.store().cut_after(budget);
        let result = svc.delete_account(0, 2, T0 + 1);
        let mut svc = Service::open(svc.into_store().inner, config()).unwrap();
        if svc.state.account(2).is_none() {
            assert!(svc.account_note(0, 2).is_empty());
            assert!(svc.account_note(2, 0).is_empty());
            assert!(!svc.social(0).dismissed.contains(&old));
            assert!(svc.state.announcements[&ann].read_by.is_empty());
            assert!(svc.state.announcements[&ann].reactions.is_empty());
            svc.create_member(0, member("replacement"), T0 + 2).unwrap();
            assert!(svc.social(2).followed.is_empty() && svc.social(2).featured.is_empty());
        } else {
            assert!(result.is_err());
        }
    }
    for budget in 0..2 {
        let mut svc = household(FaultStore::new(MemStore::default()));
        svc.store().cut_after(budget);
        let result = svc.set_account_note(0, 1, "new note", T0);
        let svc = Service::open(svc.into_store().inner, config()).unwrap();
        assert_eq!(
            svc.account_note(0, 1),
            if result.is_ok() { "new note" } else { "" }
        );
    }
}

#[test]
fn derived_counts_follow_mutations_eviction_and_reboot() {
    let mut svc = household(MemStore::default());
    assert_eq!(svc.statuses_count(0), 0);
    let a = svc.post_status(0, post("one"), T0).unwrap();
    assert_eq!(svc.statuses_count(0), 1);
    assert_eq!(svc.last_status_ms(0), Some(ids::millis(a)));
    svc.set_follow(2, 0, true, None, None, T0).unwrap();
    assert_eq!(svc.follower_counts(0), (2, 1));
    svc.set_block(0, 2, true, T0).unwrap();
    assert_eq!(svc.follower_counts(0), (1, 1));
    svc.remove_status(a).unwrap();
    assert_eq!(svc.statuses_count(0), 0);
    assert_eq!(svc.last_status_ms(0), None);
    for i in 0..MAX_STATUSES + 1 {
        svc.post_status(1, post("bounded"), T0 + (i as u64 + 1) * MINUTE)
            .unwrap();
        if i % 500 == 0 {
            assert_eq!(svc.statuses_count(1), i + 1);
        }
    }
    assert_eq!(svc.statuses_count(1), MAX_STATUSES);
    let mut svc = reopen(svc);
    assert_eq!(svc.statuses_count(1), MAX_STATUSES);
    svc.delete_account(0, 1, T0 + 10_000 * MINUTE).unwrap();
    assert_eq!(svc.statuses_count(1), 0);
    svc.create_member(0, member("newbob"), T0 + 10_001 * MINUTE)
        .unwrap();
    assert_eq!(svc.last_status_ms(1), None);
}

#[test]
fn poll_deadline_resets_when_a_poll_changes() {
    let mut svc = household(MemStore::default());
    svc.tick(T0);
    assert_eq!(svc.state.next_poll_check, Some(u64::MAX));
    let id = svc
        .post_status(
            0,
            NewStatus {
                poll: Some(NewPoll {
                    options: vec!["a".into(), "b".into()],
                    expires_in_s: 300,
                    multiple: false,
                    hide_totals: false,
                }),
                ..post("vote")
            },
            T0,
        )
        .unwrap();
    svc.tick(T0 + 1);
    assert_eq!(svc.state.next_poll_check, Some(T0 + 300_000));
    svc.tick(T0 + 299_999);
    assert!(!svc.state.polls_announced.contains(&id));
    svc.tick(T0 + 300_000);
    assert!(svc.state.polls_announced.contains(&id));
    let before = svc.state.notifications.len();
    svc.tick(T0 + 400_000);
    assert_eq!(svc.state.notifications.len(), before);
    svc.remove_status(id).unwrap();
    svc.tick(T0 + 400_001);
    assert_eq!(svc.state.next_poll_check, Some(u64::MAX));
}
