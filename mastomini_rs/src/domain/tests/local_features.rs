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
fn interrupted_local_token_creation_never_leaves_an_orphan_or_loses_an_existing_key() {
    let mut base = household(MemStore::default());
    let (existing_id, existing) = base
        .create_api_key(1, "pass", "Existing", "read", T0)
        .unwrap();
    let image = base.into_store();
    for browser in [false, true] {
        for failure in [Failure::Write, Failure::PersistThenError, Failure::Full] {
            let mut completed = false;
            for at in 0..8 {
                let mut svc = Service::open(FaultStore::new(image.clone()), config()).unwrap();
                svc.store().fail_after(failure, at);
                let result = if browser {
                    svc.browser_session("bob", "pass", T0 + 1)
                } else {
                    svc.create_api_key(1, "pass", "Pending", "read write", T0 + 1)
                        .map(|(_, token)| token)
                };
                let mut reboot = Service::open(svc.into_store().inner, config()).unwrap();
                assert!(
                    reboot.principal(&existing, T0 + 2).is_some(),
                    "{browser} {failure:?} {at}"
                );
                assert!(reboot.state.local_tokens.contains_key(&existing_id));
                for token in &reboot.state.tokens {
                    assert!(
                        reboot.state.local_tokens.contains_key(&token.rec.id),
                        "orphan token after {failure:?} at {at}"
                    );
                }
                assert!(reboot.state.local_tokens.len() <= 2);
                if let Ok(token) = result {
                    let principal = reboot.principal(&token, T0 + 2).unwrap();
                    assert_eq!(principal.slot, Some(1));
                    assert!(!browser || !principal.allows("write"));
                    reboot.open_session(1, &token);
                    assert!(
                        reboot.state.session.is_none(),
                        "local tokens cannot unlock DMs"
                    );
                    completed = true;
                }
                assert_eq!(reopen(reboot).state.repairs, 0, "repair must converge");
                if completed {
                    break;
                }
            }
            assert!(
                completed,
                "{browser} {failure:?}: operation never completed"
            );
        }
    }
}

#[test]
fn household_api_key_capacity_refuses_new_keys_and_reuses_only_a_revoked_slot() {
    let mut svc = household(MemStore::default());
    svc.create_member(0, member("dave"), T0).unwrap();
    svc.create_member(0, member("erin"), T0).unwrap();
    let mut issued = Vec::new();
    for slot in 0..4 {
        for n in 0..MAX_API_KEYS_PER_ACCOUNT {
            issued.push(
                svc.create_api_key(slot, "pass", &format!("Bot {n}"), "read", T0 + 1)
                    .unwrap(),
            );
        }
    }
    assert_eq!(issued.len(), MAX_API_KEYS);
    assert!(matches!(
        svc.create_api_key(4, "pass", "Over capacity", "read", T0 + 2),
        Err(Error::TooMany(_))
    ));
    for (_, token) in &issued {
        assert!(svc.principal(token, T0 + 3).is_some());
    }
    svc.revoke(&issued[0].1).unwrap();
    let (_, replacement) = svc
        .create_api_key(4, "pass", "Replacement", "read", T0 + 4)
        .unwrap();
    let mut svc = reopen(svc);
    assert!(svc.principal(&issued[0].1, T0 + 5).is_none());
    assert!(svc.principal(&replacement, T0 + 5).is_some());
    for (_, token) in &issued[1..] {
        assert!(svc.principal(token, T0 + 5).is_some());
    }
    assert_eq!(svc.state.local_tokens.len(), MAX_API_KEYS);
}

#[test]
fn blob_reads_preserve_errors_and_distinguish_disappearance_from_an_empty_value() {
    use crate::store::read_blob;
    assert_eq!(read_blob(16, |_| Ok(None)).unwrap(), None);
    assert_eq!(
        read_blob(0, |buf| {
            assert!(buf.is_empty());
            Ok(Some(0))
        })
        .unwrap(),
        Some(vec![])
    );
    assert_eq!(
        read_blob(16, |_| Err(StoreError::Io("read failure".into()))),
        Err(StoreError::Io("read failure".into()))
    );
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
