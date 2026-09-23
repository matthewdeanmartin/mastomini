//! Collections: auto-accepted items, consent by revoking, limits, and what
//! survives a reboot.

use super::*;

fn id_of(svc: &Service<impl Store>, slot: u8) -> u64 {
    svc.state.account(slot).unwrap().rec.id
}

fn named(name: &str, account_ids: Vec<u64>) -> NewCollection {
    NewCollection {
        name: name.into(),
        account_ids,
        ..NewCollection::default()
    }
}

#[test]
fn items_are_accepted_notified_and_revocable_for_good() {
    let mut svc = household(MemStore::default());
    let (bob, carol) = (id_of(&svc, 1), id_of(&svc, 2));
    let id = svc
        .create_collection(0, named("Family", vec![bob]), T0)
        .unwrap();
    let kinds: Vec<_> = svc
        .notifications(1, &[], &[], None, &PageQuery::default())
        .into_iter()
        .map(|(_, n)| (n.kind, n.object))
        .collect();
    assert!(kinds.contains(&(NotificationKind::AddedToCollection, Some(id))));

    let item = svc.add_collection_item(0, id, carol, T0 + 1).unwrap();
    assert_eq!(svc.accepted_count(&svc.state.collections[&id]), 2);
    // Only the curator edits; only the featured account revokes.
    assert!(matches!(
        svc.add_collection_item(1, id, carol, T0 + 2),
        Err(Error::Forbidden(_))
    ));
    assert!(matches!(
        svc.revoke_collection_item(1, id, item, T0 + 2),
        Err(Error::Forbidden(_))
    ));
    svc.revoke_collection_item(2, id, item, T0 + 2).unwrap();
    assert_eq!(svc.accepted_count(&svc.state.collections[&id]), 1);
    assert_eq!(svc.collections_featuring(2, 2), Vec::<u64>::new());

    let mut svc = reopen(svc);
    // A revoked account can't be added back, even after "removing" it.
    svc.remove_collection_item(0, id, item, T0 + 3).unwrap();
    assert!(matches!(
        svc.add_collection_item(0, id, carol, T0 + 4),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        svc.add_collection_item(0, id, bob, T0 + 4),
        Err(Error::Invalid(_))
    ));
    assert_eq!(svc.collections_featuring(0, 1), vec![id]);
}

#[test]
fn validation_and_limits() {
    let mut svc = household(MemStore::default());
    let alice = id_of(&svc, 0);
    let too_long = "x".repeat(41);
    for bad in [
        named("", vec![]),
        named(&too_long, vec![]),
        named("Me", vec![alice]),
        named("Nobody", vec![999]),
        NewCollection {
            description: "d".repeat(101),
            ..named("Long", vec![])
        },
        NewCollection {
            tag: Some("#not a tag".into()),
            ..named("Tag", vec![])
        },
    ] {
        assert!(
            matches!(
                svc.create_collection(0, bad.clone(), T0),
                Err(Error::Invalid(_))
            ),
            "{bad:?}"
        );
    }
    let tagged = svc
        .create_collection(
            0,
            NewCollection {
                tag: Some("#Family".into()),
                ..named("Tagged", vec![])
            },
            T0,
        )
        .unwrap();
    assert_eq!(
        svc.state.collections[&tagged].tag.as_deref(),
        Some("Family")
    );

    // Members who opted out of discovery can't be featured.
    let hidden = ProfileUpdate {
        discoverable: Some(false),
        ..ProfileUpdate::default()
    };
    svc.update_profile(2, hidden, T0).unwrap();
    let carol = id_of(&svc, 2);
    assert!(svc
        .create_collection(0, named("Nope", vec![carol]), T0)
        .is_err());

    while svc.collections_of(0, 0).len() < MAX_COLLECTIONS_PER_ACCOUNT {
        svc.create_collection(0, named("More", vec![]), T0).unwrap();
    }
    assert!(matches!(
        svc.create_collection(0, named("One too many", vec![]), T0),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn discoverability_blocks_and_suspension_limit_who_sees_what() {
    let mut svc = household(MemStore::default());
    let carol = id_of(&svc, 2);
    let open = svc
        .create_collection(0, named("Open", vec![carol]), T0)
        .unwrap();
    let quiet = svc
        .create_collection(
            0,
            NewCollection {
                discoverable: Some(false),
                ..named("Quiet", vec![])
            },
            T0,
        )
        .unwrap();
    assert_eq!(svc.collections_of(0, 0), vec![open, quiet]);
    assert_eq!(svc.collections_of(1, 0), vec![open]);

    svc.set_block(0, 1, true, T0).unwrap();
    assert!(svc.collections_of(1, 0).is_empty());
    assert!(!svc.collection_visible(1, &svc.state.collections[&open].clone()));

    // A suspended member drops out of other people's collections.
    svc.moderate(0, 2, AdminAction::Suspend, None, T0).unwrap();
    let rec = svc.state.collections[&open].clone();
    assert_eq!(svc.visible_items(2, &rec).count(), 0);
    let rec_for_curator: Vec<_> = svc.visible_items(0, &rec).collect();
    assert_eq!(rec_for_curator.len(), 1, "the curator still sees the item");
}

#[test]
fn edits_and_deletes_persist() {
    let mut svc = household(MemStore::default());
    let id = svc
        .create_collection(1, named("Before", vec![]), T0)
        .unwrap();
    svc.update_collection(
        1,
        id,
        CollectionUpdate {
            name: Some("After".into()),
            sensitive: Some(true),
            language: Some(Some("en".into())),
            ..CollectionUpdate::default()
        },
        T0 + 1,
    )
    .unwrap();
    assert!(matches!(
        svc.update_collection(0, id, CollectionUpdate::default(), T0 + 1),
        Err(Error::Forbidden(_))
    ));
    let mut svc = reopen(svc);
    let rec = &svc.state.collections[&id];
    assert_eq!((rec.name.as_str(), rec.sensitive), ("After", true));
    assert_eq!(rec.updated_ms, T0 + 1);
    svc.delete_collection(1, id, T0 + 2).unwrap();
    assert!(reopen(svc).state.collections.is_empty());
}
