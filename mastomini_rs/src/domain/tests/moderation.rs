//! Blocks, mutes, conversation mutes, reports, account moderation and
//! account deletion.

use super::*;

fn ids(page: Vec<(u64, Entry)>) -> Vec<u64> {
    page.into_iter().map(|(_, e)| e.status_id()).collect()
}

fn kinds(svc: &Service<impl Store>, to: u8) -> Vec<NotificationKind> {
    svc.notifications(to, &[], &[], None, &PageQuery::default())
        .into_iter()
        .map(|(_, n)| n.kind)
        .collect()
}

#[test]
fn block_ends_follows_hides_posts_and_survives_reboot() {
    let mut svc = household(MemStore::default());
    let secret = svc.post_status(0, post("alice public"), T0 + 1).unwrap();
    svc.set_block(0, 1, true, T0 + 2).unwrap();
    assert!(!svc.state.follows(0, 1) && !svc.state.follows(1, 0));
    // Blocked bob can't see alice's posts or follow her again.
    assert!(svc.visible(Some(1), secret).is_none());
    assert!(svc.visible(Some(2), secret).is_some());
    assert!(matches!(
        svc.set_follow(1, 0, true, None, None, T0 + 3),
        Err(Error::Forbidden(_))
    ));
    // Neither sees the other on the public timeline; bob's mentions of
    // alice don't notify her.
    let bobs = svc.post_status(1, post("hey @alice"), T0 + 4).unwrap();
    assert!(!ids(svc.public(0, &PageQuery::default())).contains(&bobs));
    assert!(kinds(&svc, 0).is_empty());
    assert!(ids(svc.public(2, &PageQuery::default())).contains(&bobs));

    let mut svc = reopen(svc);
    assert!(svc.state.blocks(0, 1));
    assert!(!svc.state.follows(1, 0));
    svc.set_block(0, 1, false, T0 + 5).unwrap();
    assert!(svc.visible(Some(1), secret).is_some());
    assert!(!reopen(svc).state.blocks(0, 1));
}

#[test]
fn power_cut_during_block_never_leaves_a_follow_across_it() {
    power_cut_every_write(|_| 0, |svc, _| svc.set_block(0, 1, true, T0 + 1));
    for writes in 0..4 {
        let mut svc = household(FaultStore::new(MemStore::default()));
        svc.store().cut_after(writes);
        let _ = svc.set_block(0, 1, true, T0 + 1);
        let svc = Service::open(svc.into_store().inner, config()).unwrap();
        if svc.state.blocks(0, 1) {
            assert!(!svc.state.follows(0, 1) && !svc.state.follows(1, 0));
        }
    }
}

#[test]
fn mute_hides_from_timelines_and_notifications_until_it_expires() {
    let mut svc = household(MemStore::default());
    svc.clear_notifications(0);
    svc.tick(T0);
    svc.set_mute(0, 1, true, None, Some(60), T0).unwrap();
    let bobs = svc.post_status(1, post("hello @alice"), T0 + 1).unwrap();
    // Still follows, still can be seen directly, but not on timelines.
    assert!(svc.state.follows(0, 1));
    assert!(svc.visible(Some(0), bobs).is_some());
    assert!(!ids(svc.home(0, &PageQuery::default())).contains(&bobs));
    assert!(!ids(svc.public(0, &PageQuery::default())).contains(&bobs));
    assert!(kinds(&svc, 0).is_empty());
    // Bob still sees alice.
    let alices = svc.post_status(0, post("hi all"), T0 + 2).unwrap();
    assert!(ids(svc.home(1, &PageQuery::default())).contains(&alices));

    svc.tick(T0 + 61_000);
    assert!(svc.state.mute(0, 1).is_none());
    assert!(ids(svc.home(0, &PageQuery::default())).contains(&bobs));

    // Mute without notifications: timelines hide, mentions still notify.
    svc.set_mute(0, 2, true, Some(false), None, T0 + 61_000)
        .unwrap();
    svc.post_status(2, post("@alice ping"), T0 + 61_001)
        .unwrap();
    assert_eq!(kinds(&svc, 0), vec![NotificationKind::Mention]);
    let svc = reopen(svc);
    assert!(svc.state.mutes.contains_key(&(0, 2)));
}

#[test]
fn conversation_mute_silences_the_whole_thread() {
    let mut svc = household(MemStore::default());
    let root = svc.post_status(0, post("dinner?"), T0 + 1).unwrap();
    let reply = svc
        .post_status(
            1,
            NewStatus {
                in_reply_to_id: Some(root),
                ..post("@alice pizza")
            },
            T0 + 2,
        )
        .unwrap();
    svc.clear_notifications(0);
    // Muting from a reply records the mute on the root.
    svc.set_conversation_mute(0, reply, true, T0 + 3).unwrap();
    assert!(svc.thread_muted(0, root));
    svc.post_status(
        2,
        NewStatus {
            in_reply_to_id: Some(reply),
            ..post("@alice tacos")
        },
        T0 + 4,
    )
    .unwrap();
    svc.set_reaction(1, ReactionKind::Favourite, root, true, T0 + 5)
        .unwrap();
    assert!(kinds(&svc, 0).is_empty());
    // Other threads still notify.
    svc.post_status(1, post("@alice elsewhere"), T0 + 6)
        .unwrap();
    assert_eq!(kinds(&svc, 0), vec![NotificationKind::Mention]);

    let mut svc = reopen(svc);
    assert!(svc.thread_muted(0, reply));
    svc.set_conversation_mute(0, reply, false, T0 + 7).unwrap();
    assert!(!svc.thread_muted(0, reply));
}

#[test]
fn reports_notify_admins_and_make_room_by_dropping_resolved_ones() {
    let mut svc = household(MemStore::default());
    svc.clear_notifications(0);
    svc.state.server.rules = vec!["Be kind".into()];
    let rude = svc.post_status(2, post("rude"), T0 + 1).unwrap();
    let id = svc
        .file_report(
            1,
            NewReport {
                target: 2,
                status_ids: vec![rude],
                comment: "not nice".into(),
                rule_ids: vec![1],
                ..NewReport::default()
            },
            T0 + 2,
        )
        .unwrap();
    let report = &svc.state.reports[&id];
    assert_eq!(report.category, ReportCategory::Violation);
    assert_eq!(kinds(&svc, 0), vec![NotificationKind::AdminReport]);
    assert!(kinds(&svc, 2).is_empty(), "the reported member isn't told");

    // A report can only name the target's own posts, and valid rules.
    let alices = svc.post_status(0, post("mine"), T0 + 3).unwrap();
    let bad = NewReport {
        target: 2,
        status_ids: vec![alices],
        ..NewReport::default()
    };
    assert_eq!(svc.file_report(1, bad, T0 + 4), Err(Error::NotFound));
    let bad = NewReport {
        target: 2,
        rule_ids: vec![2],
        ..NewReport::default()
    };
    assert!(matches!(
        svc.file_report(1, bad, T0 + 4),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        svc.resolve_report(1, id, true, T0 + 5),
        Err(Error::Forbidden(_))
    ));

    let mut svc = reopen(svc);
    assert_eq!(svc.state.reports[&id].comment, "not nice");
    let t = T0 + 10 * MINUTE;
    while svc.state.reports.len() < MAX_REPORTS {
        let r = NewReport {
            target: 2,
            ..NewReport::default()
        };
        svc.file_report(1, r, t).unwrap();
    }
    let extra = NewReport {
        target: 2,
        ..NewReport::default()
    };
    assert!(matches!(
        svc.file_report(1, extra.clone(), t),
        Err(Error::Invalid(_))
    ));
    svc.resolve_report(0, id, true, t).unwrap();
    svc.file_report(1, extra, t).unwrap();
    assert!(
        !svc.state.reports.contains_key(&id),
        "oldest resolved dropped"
    );
}

#[test]
fn suspension_hides_everything_and_is_reversible() {
    let mut svc = household(MemStore::default());
    let oob = vec![OOB.to_string()];
    let (app, _) = svc.register_app("app", None, &oob, "read", T0).unwrap();
    let token = svc.issue_token(1, app.id, vec!["read".into()], T0).unwrap();
    let bobs = svc.post_status(1, post("bob's post"), T0 + 1).unwrap();
    // Members can't moderate; nobody moderates the owner or themselves.
    assert!(svc
        .moderate(2, 1, AdminAction::Suspend, None, T0 + 2)
        .is_err());
    assert!(svc
        .moderate(0, 0, AdminAction::Suspend, None, T0 + 2)
        .is_err());

    svc.moderate(0, 1, AdminAction::Suspend, None, T0 + 2)
        .unwrap();
    assert!(svc.visible(Some(0), bobs).is_none());
    assert!(svc.principal(&token, T0 + 3).unwrap().disabled);
    assert!(svc.check_password("bob", "pass", T0 + 3).is_err());

    let mut svc = reopen(svc);
    assert!(svc.state.suspended(1));
    svc.moderate(0, 1, AdminAction::Unsuspend, None, T0 + 4)
        .unwrap();
    assert!(svc.visible(Some(0), bobs).is_some());
    assert!(!svc.principal(&token, T0 + 5).unwrap().disabled);
    assert!(svc
        .store()
        .get(Ns::Mod, &keys::account_mod(1))
        .unwrap()
        .is_none());

    // Silenced: off the public timeline for people who don't follow bob.
    svc.moderate(0, 1, AdminAction::Silence, None, T0 + 6)
        .unwrap();
    assert!(!ids(svc.public(2, &PageQuery::default())).contains(&bobs));
    assert!(ids(svc.public(0, &PageQuery::default())).contains(&bobs));
    svc.post_status(1, post("@carol hi"), T0 + 7).unwrap();
    assert!(kinds(&svc, 2).is_empty());

    // Disabled: signs in no more, until enabled again.
    svc.moderate(0, 2, AdminAction::Disable, None, T0 + 8)
        .unwrap();
    assert!(svc.check_password("carol", "pass", T0 + 8).is_err());
    let svc = reopen(svc);
    assert!(svc.state.account(2).unwrap().rec.disabled);
    assert!(svc.state.silenced(1));
}

#[test]
fn only_the_owner_moderates_admins_and_changes_roles() {
    let mut svc = household(MemStore::default());
    svc.set_role(0, 1, Role::Admin, T0).unwrap();
    assert!(matches!(
        svc.set_role(1, 2, Role::Admin, T0),
        Err(Error::Forbidden(_))
    ));
    // Admin bob can moderate member carol, but not the owner or himself.
    svc.moderate(1, 2, AdminAction::Silence, None, T0).unwrap();
    assert!(svc.moderate(1, 0, AdminAction::Silence, None, T0).is_err());
    svc.set_role(0, 2, Role::Admin, T0).unwrap();
    assert!(svc.moderate(1, 2, AdminAction::Disable, None, T0).is_err());
    svc.moderate(0, 2, AdminAction::Disable, None, T0).unwrap();
}

#[test]
fn moderation_record_is_ignored_when_its_slot_is_reused() {
    let mut svc = household(MemStore::default());
    svc.moderate(0, 2, AdminAction::Suspend, None, T0).unwrap();
    // Simulate a stale record: the slot now belongs to someone else.
    let mut stale = svc.state.moderation(2);
    stale.account_id = 12345;
    svc.state.moderation[2] = Some(stale);
    assert!(!svc.state.suspended(2));
}

fn populated(svc: &mut Service<FaultStore<MemStore>>) -> u64 {
    let carol_post = svc.post_status(2, post("carol"), T0).unwrap();
    let alice_post = svc.post_status(0, post("alice @carol"), T0).unwrap();
    svc.set_reaction(2, ReactionKind::Favourite, alice_post, true, T0)
        .unwrap();
    svc.boost(2, alice_post, T0).unwrap();
    svc.set_follow(2, 0, true, None, None, T0).unwrap();
    svc.set_block(2, 1, true, T0).unwrap();
    svc.set_mute(0, 2, true, None, None, T0).unwrap();
    svc.moderate(0, 2, AdminAction::Silence, None, T0).unwrap();
    svc.file_report(
        0,
        NewReport {
            target: 2,
            status_ids: vec![carol_post],
            ..NewReport::default()
        },
        T0,
    )
    .unwrap();
    let mine = svc
        .create_collection(
            2,
            NewCollection {
                name: "Family".into(),
                account_ids: vec![svc.state.account(0).unwrap().rec.id],
                ..NewCollection::default()
            },
            T0,
        )
        .unwrap();
    let carol_id = svc.state.account(2).unwrap().rec.id;
    let alices = svc
        .create_collection(
            0,
            NewCollection {
                name: "Kids".into(),
                account_ids: vec![carol_id],
                ..NewCollection::default()
            },
            T0,
        )
        .unwrap();
    assert_ne!(mine, alices);
    alices
}

/// Once the tombstone is written, boot finishes the purge: whatever the cut,
/// either carol is intact or nothing of hers remains.
#[test]
fn deleting_an_account_purges_it_even_across_power_cuts() {
    power_cut_every_write(populated, |svc, _| svc.delete_account(0, 2, T0 + 1));
    for writes in 0..60 {
        let mut svc = household(FaultStore::new(MemStore::default()));
        populated(&mut svc);
        let carol_id = svc.state.account(2).unwrap().rec.id;
        svc.store().cut_after(writes);
        let _ = svc.delete_account(0, 2, T0 + 1);
        let cut = svc.store().is_cut();
        let mut svc = Service::open(svc.into_store().inner, config()).unwrap();
        if svc.state.account_by_username("carol").is_some() {
            assert!(cut);
            continue;
        }
        let s = &svc.state;
        assert!(s.statuses.values().all(|st| st.rec.author != 2));
        assert!(s.boosts.values().all(|b| b.booster != 2));
        assert!(s.reactions.values().all(|r| r.slot != 2));
        assert!(s.follows.keys().all(|(a, b)| *a != 2 && *b != 2));
        assert!(s.blocks.is_empty() && s.mutes.is_empty());
        assert!(s.reports.is_empty() && s.moderation[2].is_none());
        assert!(s.collections.values().all(|c| c.owner != 2));
        assert!(s
            .collections
            .values()
            .all(|c| c.items.iter().all(|i| i.account_id != carol_id)));
        // The slot is free again, and a new member there starts clean.
        svc.create_member(0, member("dave"), T0 + 2).unwrap();
        assert_eq!(svc.state.account_by_username("dave").unwrap().slot, 2);
        assert!(!svc.state.silenced(2));
    }
}

#[test]
fn admin_deletes_posts_but_never_foreign_direct_messages() {
    let mut svc = household(MemStore::default());
    let public = svc.post_status(1, post("oops"), T0).unwrap();
    let dm = svc
        .post_status(
            1,
            NewStatus {
                visibility: Some(Visibility::Direct),
                ..post("@carol secret")
            },
            T0,
        )
        .unwrap();
    assert!(matches!(
        svc.admin_delete_status(2, public, T0),
        Err(Error::Forbidden(_))
    ));
    assert_eq!(
        svc.admin_delete_status(0, dm, T0).err(),
        Some(Error::NotFound)
    );
    svc.admin_delete_status(0, public, T0).unwrap();
    assert!(!svc.state.statuses.contains_key(&public));
}

#[test]
fn schema_one_store_is_upgraded_in_place() {
    let svc = household(MemStore::default());
    let mut store = svc.into_store();
    let old = codec::encode(Kind::Schema, &1u16).unwrap();
    store.set(Ns::Cfg, &keys::schema(), &old).unwrap();
    let mut svc = Service::open(store, config()).unwrap();
    let bytes = svc.store().get(Ns::Cfg, &keys::schema()).unwrap().unwrap();
    assert_eq!(codec::decode::<u16>(Kind::Schema, &bytes).unwrap(), SCHEMA);
    // A store from newer firmware is still refused.
    let newer = codec::encode(Kind::Schema, &(SCHEMA + 1)).unwrap();
    let mut store = svc.into_store();
    store.set(Ns::Cfg, &keys::schema(), &newer).unwrap();
    assert!(Service::open(store, config()).is_err());
}
