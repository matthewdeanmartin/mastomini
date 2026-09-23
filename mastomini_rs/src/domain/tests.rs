//! Domain tests: reboot round-trips, power cuts at every write, eviction and
//! visibility.

use super::query::{AccountStatusesQuery, Entry, PageQuery};
use super::*;
use crate::store::mem::{FaultStore, MemStore};

const T0: u64 = 1_750_000_000_000;
const MINUTE: u64 = 60_000;

fn config() -> Config {
    Config {
        host: "mastomini.local".into(),
        password_rounds: 1,
    }
}

fn member(name: &str) -> NewMember {
    NewMember {
        username: name.into(),
        password: "pass".into(),
        display_name: String::new(),
        role: Role::Member,
    }
}

fn post(text: &str) -> NewStatus {
    NewStatus {
        text: text.into(),
        ..NewStatus::default()
    }
}

/// Owner "alice" (slot 0), members "bob" (1) and "carol" (2). Alice and bob
/// follow each other; carol follows nobody.
fn household<S: Store>(store: S) -> Service<S> {
    let mut svc = Service::open(store, config()).unwrap();
    svc.provision(member("alice"), Some("Home".into()), T0)
        .unwrap();
    svc.create_member(0, member("bob"), T0).unwrap();
    svc.create_member(0, member("carol"), T0).unwrap();
    svc.set_follow(0, 1, true, None, None, T0).unwrap();
    svc.set_follow(1, 0, true, None, None, T0).unwrap();
    svc
}

fn reopen<S: Store>(svc: Service<S>) -> Service<S> {
    Service::open(svc.into_store(), config()).unwrap()
}

#[test]
fn unprovisioned_store_boots_empty() {
    let svc = Service::open(MemStore::default(), config()).unwrap();
    assert!(!svc.state.provisioned);
    assert_eq!(svc.state.active_accounts().count(), 0);
}

#[test]
fn provisioning_only_once_and_admin_rules() {
    let mut svc = household(MemStore::default());
    assert!(matches!(
        svc.provision(member("eve"), None, T0),
        Err(Error::Forbidden(_))
    ));
    assert!(matches!(
        svc.create_member(1, member("eve"), T0),
        Err(Error::Forbidden(_))
    ));
    assert!(matches!(
        svc.create_member(0, member("Eve"), T0),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        svc.create_member(0, member("bob"), T0),
        Err(Error::Invalid(_))
    ));
    let svc = reopen(svc);
    assert_eq!(svc.state.server.title, "Home");
    assert_eq!(svc.state.account(0).unwrap().rec.role, Role::Owner);
}

#[test]
fn interrupted_provisioning_rolls_back() {
    for writes in 0..3 {
        let mut fault = FaultStore::new(MemStore::default());
        fault.cut_after(writes);
        let mut svc = Service::open(fault, config()).unwrap();
        assert!(svc
            .provision(member("alice"), Some("Home".into()), T0)
            .is_err());
        let inner = svc.into_store().inner;
        let mut svc = Service::open(inner, config()).unwrap();
        assert!(!svc.state.provisioned, "cut after {writes} writes");
        svc.provision(member("alice"), None, T0).unwrap();
    }
}

#[test]
fn state_survives_reboot() {
    let mut svc = household(MemStore::default());
    let a = svc.post_status(0, post("hello #Tea @bob"), T0 + 1).unwrap();
    let reply = NewStatus {
        in_reply_to_id: Some(a),
        ..post("@alice hi")
    };
    let b = svc.post_status(1, reply, T0 + 2).unwrap();
    svc.set_reaction(1, ReactionKind::Favourite, a, true, T0 + 3)
        .unwrap();
    svc.set_reaction(0, ReactionKind::Pin, a, true, T0 + 3)
        .unwrap();
    let boost = svc.boost(1, a, T0 + 4).unwrap();
    // follow x2, mention x2, favourite, reblog
    assert_eq!(svc.state.notifications.len(), 6);

    let svc = reopen(svc);
    let s = &svc.state.statuses[&a];
    assert_eq!(s.favourited, bit(1));
    assert_eq!(s.pinned, bit(0));
    assert_eq!(s.boosted, bit(1));
    assert_eq!(s.replies, 1);
    assert_eq!(s.mentions, bit(1));
    assert_eq!(s.tags, vec!["tea"]);
    assert_eq!(svc.state.statuses[&b].rec.in_reply_to_id, Some(a));
    assert!(svc.state.boosts.contains_key(&boost));
    assert!(svc.state.follows(0, 1) && svc.state.follows(1, 0));
    assert!(
        svc.state.notifications.is_empty(),
        "notifications are RAM only"
    );
    assert!(svc.last_id() >= boost);
}

#[test]
fn delete_removes_boosts_and_reactions_from_store() {
    let mut svc = household(MemStore::default());
    let a = svc.post_status(0, post("bye"), T0).unwrap();
    svc.set_reaction(1, ReactionKind::Favourite, a, true, T0)
        .unwrap();
    svc.set_reaction(1, ReactionKind::Bookmark, a, true, T0)
        .unwrap();
    svc.boost(1, a, T0).unwrap();
    assert!(matches!(
        svc.delete_status(1, a, T0),
        Err(Error::Forbidden(_))
    ));
    svc.delete_status(0, a, T0).unwrap();
    let store = svc.into_store();
    assert!(store.keys(Ns::Stat).is_empty());
    assert!(store.keys(Ns::Rx).is_empty());
}

/// Run `op` with power cut after each possible number of writes, and prove
/// every resulting store boots, passes invariants, and holds no orphans.
fn power_cut_every_write(
    setup: impl Fn(&mut Service<FaultStore<MemStore>>) -> u64,
    op: impl Fn(&mut Service<FaultStore<MemStore>>, u64) -> Result<()>,
) {
    for writes in 0.. {
        let mut svc = household(FaultStore::new(MemStore::default()));
        let target = setup(&mut svc);
        svc.store().cut_after(writes);
        let result = op(&mut svc, target);
        let completed = !svc.store().is_cut();
        let svc = Service::open(svc.into_store().inner, config())
            .unwrap_or_else(|e| panic!("boot after cut at {writes}: {e}"));
        svc.check_invariants().unwrap();
        for boost in svc.state.boosts.values() {
            assert!(svc.state.statuses.contains_key(&boost.target));
        }
        for reaction in svc.state.reactions.values() {
            assert!(svc.state.statuses.contains_key(&reaction.status));
        }
        if completed {
            result.unwrap();
            break;
        }
        assert!(writes < 50, "operation never completed");
    }
}

fn busy_status(svc: &mut Service<FaultStore<MemStore>>) -> u64 {
    let a = svc.post_status(0, post("busy"), T0).unwrap();
    svc.set_reaction(1, ReactionKind::Favourite, a, true, T0)
        .unwrap();
    svc.set_reaction(1, ReactionKind::Bookmark, a, true, T0)
        .unwrap();
    svc.set_reaction(0, ReactionKind::Favourite, a, true, T0)
        .unwrap();
    svc.boost(1, a, T0).unwrap();
    a
}

#[test]
fn power_cut_during_delete() {
    power_cut_every_write(busy_status, |svc, a| {
        svc.delete_status(0, a, T0 + 1).map(|_| ())
    });
}

#[test]
fn power_cut_during_password_change_never_keeps_old_sessions() {
    for writes in 0.. {
        let mut svc = household(FaultStore::new(MemStore::default()));
        let oob = vec![OOB.to_string()];
        let (app, _) = svc.register_app("app", None, &oob, "read", T0).unwrap();
        let old: Vec<String> = (0..2)
            .map(|_| svc.issue_token(1, app.id, vec!["read".into()], T0).unwrap())
            .collect();
        svc.store().cut_after(writes);
        let result = svc.change_password(1, "pass", "new-pass", T0 + 1);
        let completed = !svc.store().is_cut();
        let mut svc = Service::open(svc.into_store().inner, config()).unwrap();
        svc.check_invariants().unwrap();
        let changed = svc.check_password("bob", "new-pass", T0 + 2).is_ok();
        for token in &old {
            // Either the change never committed (old password, old sessions)
            // or it did and every old session is dead.
            assert_eq!(
                svc.principal(token, T0 + 2).is_some(),
                !changed,
                "cut at {writes}"
            );
        }
        if completed {
            result.unwrap();
            assert!(changed);
            break;
        }
        assert!(writes < 20, "operation never completed");
    }
}

#[test]
fn eviction_is_oldest_first_and_spares_pins() {
    let mut svc = household(MemStore::default());
    let pinned = svc.post_status(0, post("keep me"), T0).unwrap();
    svc.set_reaction(0, ReactionKind::Pin, pinned, true, T0)
        .unwrap();
    let first = svc.post_status(0, post("first to go"), T0).unwrap();
    svc.set_reaction(1, ReactionKind::Favourite, first, true, T0)
        .unwrap();
    let mut last = 0;
    for i in 0..MAX_STATUSES as u64 {
        // Spread over time so the hourly write governor stays happy.
        let slot = (i % 3) as u8;
        last = svc
            .post_status(slot, post("filler"), T0 + i * MINUTE)
            .unwrap();
    }
    assert_eq!(svc.state.statuses.len(), MAX_STATUSES);
    assert!(svc.state.statuses.contains_key(&pinned));
    assert!(!svc.state.statuses.contains_key(&first));
    assert!(svc.state.statuses.contains_key(&last));
    assert!(svc.state.reactions.values().all(|r| r.status != first));
    assert_eq!(svc.state.evictions, 2);
    let svc = reopen(svc);
    assert_eq!(svc.state.statuses.len(), MAX_STATUSES);
}

#[test]
fn low_watermark_evicts_before_the_store_fills() {
    // Room for roughly a dozen statuses.
    let store = MemStore::with_capacity(150);
    let mut svc = household(store);
    for i in 0..60 {
        svc.post_status(0, post("a post that takes a few entries"), T0 + i * MINUTE)
            .unwrap();
    }
    let used = svc.store().stats().unwrap().used_fraction();
    assert!(used <= LOW_WATERMARK + 0.1, "used {used}");
    assert!(svc.state.evictions > 0);
}

#[test]
fn governor_limits_writes_per_hour() {
    let mut svc = household(MemStore::default());
    let mut refused = None;
    for i in 0..200 {
        if let Err(e) = svc.post_status(2, post("spam"), T0 + i) {
            refused = Some((i, e));
            break;
        }
    }
    let (i, e) = refused.unwrap();
    assert!(matches!(e, Error::TooMany(_)));
    assert_eq!(i, GOVERNOR_PER_ACCOUNT_HOUR as u64);
    assert!(svc.post_status(2, post("later"), T0 + HOUR_MS + 1).is_ok());
}

#[test]
fn visibility_rules() {
    let mut svc = household(MemStore::default());
    let private = svc
        .post_status(
            0,
            NewStatus {
                visibility: Some(Visibility::Private),
                ..post("followers")
            },
            T0,
        )
        .unwrap();
    let direct = svc
        .post_status(
            0,
            NewStatus {
                visibility: Some(Visibility::Direct),
                ..post("@carol psst")
            },
            T0,
        )
        .unwrap();
    assert!(svc.visible(Some(1), private).is_some(), "bob follows alice");
    assert!(svc.visible(Some(2), private).is_none(), "carol does not");
    assert!(svc.visible(Some(2), direct).is_some(), "carol is mentioned");
    assert!(svc.visible(Some(1), direct).is_none());
    assert!(matches!(
        svc.set_reaction(1, ReactionKind::Favourite, direct, true, T0),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        svc.boost(1, private, T0),
        Err(Error::Forbidden(_))
    ));
}

#[test]
fn home_timeline_follows_mastodon_rules() {
    let mut svc = household(MemStore::default());
    let own = svc.post_status(0, post("mine"), T0).unwrap();
    let bobs = svc.post_status(1, post("bob here"), T0).unwrap();
    let carols = svc.post_status(2, post("carol here"), T0).unwrap();
    // Bob replies to carol, whom alice does not follow: hidden from alice.
    let reply = NewStatus {
        in_reply_to_id: Some(carols),
        ..post("@carol yes")
    };
    let bob_to_carol = svc.post_status(1, reply, T0).unwrap();
    let boost = svc.boost(1, carols, T0).unwrap();

    let home: Vec<Entry> = svc
        .home(0, &PageQuery::default())
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    assert_eq!(
        home,
        vec![
            Entry::Boost {
                id: boost,
                target: carols
            },
            Entry::Status(bobs),
            Entry::Status(own)
        ]
    );
    assert!(!home.contains(&Entry::Status(bob_to_carol)));

    let public = svc.public(&PageQuery::default());
    assert_eq!(public.len(), 4);
}

#[test]
fn pagination_cursors() {
    let mut svc = household(MemStore::default());
    let ids: Vec<u64> = (0..10)
        .map(|i| svc.post_status(0, post("n"), T0 + i).unwrap())
        .collect();
    let q = |max_id, since_id, min_id, limit| PageQuery {
        max_id,
        since_id,
        min_id,
        limit,
    };
    let page = |q: PageQuery| -> Vec<u64> {
        svc.account_statuses(Some(0), 0, &AccountStatusesQuery::default(), &q)
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    };
    assert_eq!(page(q(None, None, None, 3)), vec![ids[9], ids[8], ids[7]]);
    assert_eq!(
        page(q(Some(ids[7]), None, None, 3)),
        vec![ids[6], ids[5], ids[4]]
    );
    assert_eq!(
        page(q(None, Some(ids[2]), None, 3)),
        vec![ids[9], ids[8], ids[7]]
    );
    assert_eq!(
        page(q(None, None, Some(ids[2]), 3)),
        vec![ids[5], ids[4], ids[3]]
    );
}

#[test]
fn thread_context() {
    let mut svc = household(MemStore::default());
    let root = svc.post_status(0, post("root"), T0).unwrap();
    let r1 = svc
        .post_status(
            1,
            NewStatus {
                in_reply_to_id: Some(root),
                ..post("r1")
            },
            T0,
        )
        .unwrap();
    let r2 = svc
        .post_status(
            0,
            NewStatus {
                in_reply_to_id: Some(r1),
                ..post("r2")
            },
            T0,
        )
        .unwrap();
    let (anc, desc) = svc.context(Some(0), r1).unwrap();
    assert_eq!(anc, vec![root]);
    assert_eq!(desc, vec![r2]);
}

#[test]
fn idempotency_key_returns_same_status() {
    let mut svc = household(MemStore::default());
    let new = || NewStatus {
        idempotency_key: Some("k1".into()),
        ..post("once")
    };
    let a = svc.post_status(0, new(), T0).unwrap();
    let b = svc.post_status(0, new(), T0 + 1000).unwrap();
    assert_eq!(a, b);
    assert_eq!(svc.state.statuses.len(), 1);
}
