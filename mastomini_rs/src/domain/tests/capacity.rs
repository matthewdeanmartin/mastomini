use super::*;

fn full_household() -> Service<MemStore> {
    let mut svc = household(MemStore::default());
    for slot in 3..MAX_ACCOUNTS as u8 {
        assert_eq!(
            svc.create_member(0, member(&format!("member{slot}")), T0)
                .unwrap(),
            slot
        );
    }
    svc
}

#[test]
fn slot_keys_preserve_legacy_names_and_never_collide() {
    let mut accounts = std::collections::BTreeSet::new();
    let mut relations = std::collections::BTreeSet::new();
    let mut reactions = std::collections::BTreeSet::new();
    for slot in 0..MAX_ACCOUNTS as u8 {
        assert_eq!(parse_slot(slot_digit(slot)), Some(slot));
        let key = keys::account(slot);
        if slot < 16 {
            assert_eq!(key.as_str(), format!("a{slot:x}"));
        }
        assert!(accounts.insert(key.as_str().to_owned()));
        let reaction = keys::reaction(ReactionKind::Favourite, u64::MAX, slot);
        assert!(reaction.as_str().len() <= 15);
        assert!(reactions.insert(reaction.as_str().to_owned()));
        for dst in 0..MAX_ACCOUNTS as u8 {
            assert!(relations.insert(keys::follow(slot, dst).as_str().to_owned()));
        }
    }
    assert_eq!(keys::token(&[0xab; 32]).as_str(), "tababababababab");
    assert_eq!(parse_slot(b'w'), None);
}

#[test]
fn upper_slots_keep_permissions_reactions_polls_and_social_data_after_reboot() {
    let mut svc = full_household();
    assert!(svc.create_member(0, member("overflow"), T0).is_err());
    let private = svc
        .post_status(
            0,
            NewStatus {
                visibility: Some(Visibility::Private),
                ..post("@member31 high audience")
            },
            T0,
        )
        .unwrap();
    assert!(svc.visible(Some(31), private).is_some());
    assert!(svc.visible(Some(15), private).is_none());
    svc.set_follow(16, 0, true, None, None, T0).unwrap();
    assert!(svc.visible(Some(16), private).is_some());
    let poll = svc
        .post_status(
            2,
            NewStatus {
                poll: Some(NewPoll {
                    options: vec!["yes".into(), "no".into()],
                    expires_in_s: 3600,
                    multiple: false,
                    hide_totals: false,
                }),
                ..post("vote")
            },
            T0,
        )
        .unwrap();
    for slot in [0, 15, 16, 31] {
        svc.vote(slot, poll, &[usize::from(slot >= 16)], T0)
            .unwrap();
        svc.set_reaction(slot, ReactionKind::Favourite, poll, true, T0)
            .unwrap();
        svc.set_tag(slot, "rust", false, true, T0).unwrap();
        svc.set_account_note(slot, 31 - slot, &format!("note{slot}"), T0)
            .unwrap();
    }
    svc.set_block(31, 16, true, T0).unwrap();
    svc.set_mute(16, 15, true, None, None, T0).unwrap();
    let mut svc = reopen(svc);
    assert_eq!(svc.state.accounts.iter().flatten().count(), 32);
    assert!(svc.visible(Some(31), private).is_some());
    assert!(svc.visible(Some(15), private).is_none());
    assert!(svc.visible(Some(16), private).is_some());
    assert!(svc.state.blocks(31, 16));
    assert_eq!(svc.state.hidden_mask(16), bit(15) | bit(31));
    assert_eq!(
        svc.state.polls[&poll].votes,
        [bit(0) | bit(15), bit(16) | bit(31)]
    );
    assert_eq!(
        svc.reactors(svc.state.statuses[&poll].favourited),
        [0, 15, 16, 31]
    );
    for slot in [0, 15, 16, 31] {
        assert_eq!(svc.account_note(slot, 31 - slot), format!("note{slot}"));
        assert!(!svc.social(slot).followed.is_empty());
    }
    svc.delete_account(0, 31, T0 + 1).unwrap();
    let svc = reopen(svc);
    assert_eq!(svc.state.polls[&poll].votes, [bit(0) | bit(15), bit(16)]);
    assert_eq!(
        svc.reactors(svc.state.statuses[&poll].favourited),
        [0, 15, 16]
    );
}

#[test]
fn schema_seven_poll_bytes_upgrade_without_rewriting_existing_votes() {
    #[derive(serde::Serialize)]
    struct LegacyPoll {
        options: Vec<String>,
        expires_ms: u64,
        multiple: bool,
        hide_totals: bool,
        votes: Vec<u16>,
    }
    let mut svc = household(MemStore::default());
    for slot in 3..16 {
        svc.create_member(0, member(&format!("member{slot}")), T0)
            .unwrap();
    }
    let id = svc.post_status(0, post("legacy poll"), T0).unwrap();
    let legacy = codec::encode(
        Kind::Poll,
        &LegacyPoll {
            options: vec!["yes".into(), "no".into()],
            expires_ms: T0 + 3600000,
            multiple: false,
            hide_totals: false,
            votes: vec![0x8001, 0x4000],
        },
    )
    .unwrap();
    let mut image = svc.into_store();
    image.set(Ns::Stat, &keys::poll(id), &legacy).unwrap();
    image
        .set(
            Ns::Cfg,
            &keys::schema(),
            &codec::encode(Kind::Schema, &7u16).unwrap(),
        )
        .unwrap();
    let mut svc = Service::open(image, config()).unwrap();
    assert_eq!(svc.state.polls[&id].votes, [0x8001, 0x4000]);
    assert_eq!(
        svc.store().get(Ns::Stat, &keys::poll(id)).unwrap().unwrap(),
        legacy
    );
    for slot in 16..32 {
        svc.create_member(0, member(&format!("member{slot}")), T0)
            .unwrap();
    }
    svc.vote(31, id, &[1], T0).unwrap();
    let svc = reopen(svc);
    assert_eq!(svc.state.polls[&id].votes, [0x8001, 0x80004000]);
}

#[test]
fn upper_slot_dm_keys_lists_and_encrypted_backup_round_trip() {
    let mut svc = full_household();
    let direct = svc
        .post_status(
            16,
            NewStatus {
                visibility: Some(Visibility::Direct),
                ..post("@member31 secret")
            },
            T0,
        )
        .unwrap();
    svc.set_follow(31, 16, true, None, None, T0).unwrap();
    let list = svc
        .create_list(31, "High slots", RepliesPolicy::List, true, T0)
        .unwrap();
    let member_id = svc.state.account(16).unwrap().rec.id;
    svc.add_to_list(31, list, &[member_id], T0).unwrap();
    let archive = svc
        .export_archive(0, "pass", "backup passphrase", T0 + 1)
        .unwrap();
    let mut restored = Service::open(MemStore::default(), config()).unwrap();
    restored
        .import_archive(&archive, "backup passphrase")
        .unwrap();
    let restored = reopen(restored);
    assert!(restored.visible(Some(31), direct).is_some());
    assert!(restored.visible(Some(15), direct).is_none());
    assert!(restored.visible(Some(0), direct).is_none());
    assert_eq!(
        restored.conversations(31)[0].participants,
        bit(16) | bit(31)
    );
    assert!(restored.conversations(15).is_empty());
    assert_eq!(
        restored.list_members(restored.list(31, list).unwrap()),
        bit(16)
    );
    assert_eq!(restored.exclusive_mask(31), bit(16));
    for slot in [0, 15, 16, 31] {
        assert_eq!(restored.state.user_keys[slot], svc.state.user_keys[slot]);
    }
}

#[test]
fn account_tables_do_not_expand_the_inline_boot_state() {
    // Reload builds a second State while the Service is still on the startup
    // stack. Keep that frame small independently of the account payload size.
    assert!(std::mem::size_of::<State>() < 8192);
    let state = State::default();
    assert_eq!(state.accounts.len(), MAX_ACCOUNTS);
    assert_eq!(state.moderation.len(), MAX_ACCOUNTS);
    assert_eq!(state.user_keys.len(), MAX_ACCOUNTS);
    assert_eq!(state.markers.len(), MAX_ACCOUNTS);
}
