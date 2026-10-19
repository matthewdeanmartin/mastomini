use super::*;

#[test]
fn notification_ids_and_pagination_survive_reboot() {
    let mut svc = household(MemStore::default());
    svc.clear_notifications(0).unwrap();
    for i in 0..5 {
        svc.post_status(1, post("@alice hello"), T0 + i + 1)
            .unwrap();
    }
    let q = PageQuery {
        limit: 2,
        ..Default::default()
    };
    let first = svc.notifications(0, &[], &[], None, &q);
    let q = PageQuery {
        max_id: Some(first.last().unwrap().0),
        ..q
    };
    let expected = svc.notifications(0, &[], &[], None, &q);
    let svc = reopen(svc);
    assert_eq!(svc.notifications(0, &[], &[], None, &q), expected);
    assert_eq!(svc.state.notifications.len(), 6); // five mentions plus Bob's follow
    assert!(svc.last_id() >= first[0].0);
}

#[test]
fn notifications_dismiss_clear_and_status_deletion_are_durable() {
    let mut svc = household(MemStore::default());
    let status = svc.post_status(1, post("@alice hello"), T0 + 1).unwrap();
    let n = *svc.state.notifications.back().unwrap();
    assert!(svc.dismiss_notification(1, n.id).is_err());
    svc.dismiss_notification(0, n.id).unwrap();
    let mut svc = reopen(svc);
    assert!(!svc.state.notifications.iter().any(|e| e.id == n.id));
    assert!(svc.state.statuses.contains_key(&status));
    svc.clear_notifications(0).unwrap();
    let mut svc = reopen(svc);
    assert!(svc
        .notifications(0, &[], &[], None, &PageQuery::default())
        .is_empty());
    let status = svc.post_status(1, post("@alice again"), T0 + 2).unwrap();
    svc.remove_status(status).unwrap();
    let svc = reopen(svc);
    assert!(!svc
        .state
        .notifications
        .iter()
        .any(|n| n.status == Some(status)));
}

#[test]
fn notification_retention_is_bounded_without_evicting_posts() {
    let mut svc = household(MemStore::default());
    let status = svc.post_status(1, post("@alice hello"), T0 + 1).unwrap();
    for i in 0..MAX_NOTIFICATIONS + 2 {
        svc.notify(
            NotificationKind::Mention,
            0,
            1,
            Some(status),
            T0 + 2 + i as u64,
        );
    }
    let expected = svc.state.notifications.clone();
    assert_eq!(svc.store().keys(Ns::Notif).len(), MAX_NOTIFICATIONS);
    let svc = reopen(svc);
    assert_eq!(svc.state.notifications, expected);
    assert!(svc.state.statuses.contains_key(&status));
}

#[test]
fn schema_eight_upgrade_preserves_existing_records_without_replaying_events() {
    let mut svc = household(MemStore::default());
    svc.post_status(1, post("@alice historical"), T0 + 1)
        .unwrap();
    let mut store = svc.into_store();
    for key in store.keys(Ns::Notif) {
        store.erase(Ns::Notif, &Key::new(&key).unwrap()).unwrap();
    }
    store
        .set(
            Ns::Cfg,
            &keys::schema(),
            &codec::encode(Kind::Schema, &8u16).unwrap(),
        )
        .unwrap();
    let before: Vec<_> = Ns::ALL
        .iter()
        .filter(|ns| !matches!(ns, Ns::Cfg | Ns::Notif))
        .map(|ns| (*ns, load(&mut store, *ns).unwrap()))
        .collect();
    let mut svc = Service::open(store, config()).unwrap();
    for (ns, records) in before {
        assert_eq!(load(svc.store(), ns).unwrap(), records);
    }
    assert!(svc.state.notifications.is_empty());
    let schema = svc.store().get(Ns::Cfg, &keys::schema()).unwrap().unwrap();
    assert_eq!(codec::decode::<u16>(Kind::Schema, &schema).unwrap(), SCHEMA);
    svc.post_status(1, post("@alice new"), T0 + 2).unwrap();
    assert_eq!(reopen(svc).state.notifications.len(), 1);
}

#[test]
fn notification_failure_keeps_committed_post_and_latches_further_writes() {
    for failure in [Failure::Write, Failure::PersistThenError, Failure::Full] {
        let mut svc = household(FaultStore::new(MemStore::default()));
        svc.store().fail_after(failure, 1); // status commits, notification fails
        let id = svc.post_status(1, post("@alice hello"), T0 + 1).unwrap();
        assert!(svc.latched().is_some());
        assert!(svc.post_status(1, post("retry"), T0 + 2).is_err());
        let svc = Service::open(svc.into_store().inner, config()).unwrap();
        assert!(svc.state.statuses.contains_key(&id));
        let delivered = svc.state.notifications.iter().any(|n| n.status == Some(id));
        assert_eq!(delivered, failure == Failure::PersistThenError);
    }
}

#[test]
fn interrupted_status_cleanup_is_repaired_on_boot() {
    let mut svc = household(FaultStore::new(MemStore::default()));
    let id = svc.post_status(1, post("@alice hello"), T0 + 1).unwrap();
    svc.store().cut_after(1); // status deletion commits before notification cleanup
    assert!(svc.remove_status(id).is_err());
    let mut svc = Service::open(svc.into_store().inner, config()).unwrap();
    assert!(!svc.state.statuses.contains_key(&id));
    assert!(!svc.state.notifications.iter().any(|n| n.status == Some(id)));
    assert_eq!(load(svc.store(), Ns::Notif).unwrap().len(), 2);
}

#[test]
fn notification_cleanup_never_crosses_reused_account_ids() {
    for slot in [0, 1] {
        let mut svc = household(MemStore::default());
        svc.post_status(1, post("@alice hello"), T0 + 1).unwrap();
        let mut replacement = svc.state.account(slot).unwrap().rec.clone();
        replacement.id = svc.next_id(T0 + 2).unwrap();
        // Simulate an old notification surviving an account-slot replacement.
        svc.put(Ns::Acct, &keys::account(slot), Kind::Account, &replacement)
            .unwrap();
        let mut svc = reopen(svc);
        assert!(svc.state.notifications.is_empty());
        assert!(svc.store().keys(Ns::Notif).is_empty());
    }
}

#[test]
fn interrupted_notification_clear_and_ring_rotation_preserve_posts() {
    let mut base = household(MemStore::default());
    let status = base.post_status(1, post("@alice hello"), T0 + 1).unwrap();
    for i in 0..MAX_NOTIFICATIONS {
        base.notify(
            NotificationKind::Mention,
            0,
            1,
            Some(status),
            T0 + 2 + i as u64,
        );
    }
    let image = base.into_store();
    for budget in [0, 1, 2, MAX_NOTIFICATIONS] {
        let mut svc = Service::open(FaultStore::new(image.clone()), config()).unwrap();
        svc.store().cut_after(budget);
        let result = svc.clear_notifications(0);
        let svc = Service::open(svc.into_store().inner, config()).unwrap();
        assert_eq!(svc.state.notifications.len(), MAX_NOTIFICATIONS - budget);
        assert_eq!(result.is_ok(), budget == MAX_NOTIFICATIONS);
        assert!(svc.state.statuses.contains_key(&status));
    }
    for failure in [Failure::Write, Failure::PersistThenError] {
        for budget in 0..=2 {
            let mut svc = Service::open(FaultStore::new(image.clone()), config()).unwrap();
            svc.store().fail_after(failure, budget);
            svc.notify(NotificationKind::Mention, 0, 1, Some(status), T0 + 1000);
            let svc = Service::open(svc.into_store().inner, config()).unwrap();
            assert!(svc.state.notifications.len() <= MAX_NOTIFICATIONS);
            assert!(svc.state.notifications.len() >= MAX_NOTIFICATIONS - 1);
            assert!(svc.state.statuses.contains_key(&status));
        }
    }
}

#[test]
fn notification_cleanup_of_reactions_and_blocks_survives_restart() {
    let mut svc = household(MemStore::default());
    let id = svc.post_status(0, post("hello"), T0 + 1).unwrap();
    svc.set_reaction(1, ReactionKind::Favourite, id, true, T0 + 2)
        .unwrap();
    svc.boost(1, id, T0 + 3).unwrap();
    let mut svc = reopen(svc);
    assert!(svc
        .state
        .notifications
        .iter()
        .any(|n| n.kind == NotificationKind::Favourite));
    assert!(svc
        .state
        .notifications
        .iter()
        .any(|n| n.kind == NotificationKind::Reblog));
    svc.set_reaction(1, ReactionKind::Favourite, id, false, T0 + 4)
        .unwrap();
    svc.unboost(1, id, T0 + 5).unwrap();
    let mut svc = reopen(svc);
    assert!(!svc.state.notifications.iter().any(|n| n.status == Some(id)));
    svc.set_block(0, 1, true, T0 + 6).unwrap();
    assert!(reopen(svc).state.notifications.is_empty());
}
