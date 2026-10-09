//! Generated inputs and command sequences, with independent reference models.
//! Keep proptest-regressions/ seeds: they replay before newly generated cases.
#![cfg(not(target_os = "espidf"))]

use mastomini::{
    codec::{self, Kind},
    domain::{
        records::{Role, Visibility},
        Config, NewMember, NewStatus, Service, OOB,
    },
    http::{self, Params, Request},
    ids::{self, IdGen},
    store::{
        mem::{Failure, FaultStore, MemStore},
        Key, Ns, Store, StoreError, VALUE_MAX,
    },
};
use proptest::prelude::*;
use std::collections::BTreeMap;

fn property_config() -> proptest::test_runner::Config {
    use proptest::test_runner::{Config, FileFailurePersistence};
    Config {
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "proptest-regressions/properties.txt",
        ))),
        ..Config::default()
    }
}

fn text() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..40).prop_map(|chars| chars.into_iter().collect())
}

// Weight cliffs explicitly: uniform random u64s rarely land near them.
fn clock_or_id() -> impl Strategy<Value = u64> {
    prop_oneof![
        any::<u64>(),
        0u64..100_000,
        Just(u64::MAX),
        Just(u64::MAX - 1),
        Just(u64::MAX >> 16),
        Just((u64::MAX >> 16) + 1)
    ]
}

#[derive(Debug, Clone)]
enum Write {
    Set(u8, u8, Vec<u8>),
    Erase(u8, u8),
    Reboot,
}

fn writes() -> impl Strategy<Value = Vec<Write>> {
    prop::collection::vec(
        prop_oneof![
            5 => (0u8..15, 0u8..8, prop_oneof![
                prop::collection::vec(any::<u8>(), 0..96),
                Just(vec![0; VALUE_MAX + 1]),
            ]).prop_map(|(ns,key,value)|Write::Set(ns,key,value)),
            3 => (0u8..15,0u8..8).prop_map(|(ns,key)|Write::Erase(ns,key)),
            2 => Just(Write::Reboot),
        ],
        1..40,
    )
}

type Image = BTreeMap<(u8, u8), Vec<u8>>;

fn store_key(key: u8) -> Key {
    Key::new(&format!("k{key}")).unwrap()
}

fn expected_write(model: &mut Image, command: &Write, capacity: usize) -> Result<(), StoreError> {
    match command {
        Write::Set(ns, key, value) => {
            if value.len() > VALUE_MAX {
                return Err(StoreError::ValueTooLarge);
            }
            // NVS contract: 2 metadata entries plus one per 32-byte data block.
            let entries = |value: &[u8]| 2 + value.len().div_ceil(32);
            let used: usize = model
                .iter()
                .filter(|(k, _)| **k != (*ns, *key))
                .map(|(_, v)| entries(v))
                .sum();
            if used + entries(value) > capacity {
                return Err(StoreError::Full);
            }
            model.insert((*ns, *key), value.clone());
        }
        Write::Erase(ns, key) => {
            model.remove(&(*ns, *key));
        }
        Write::Reboot => {}
    }
    Ok(())
}

fn check_image(
    store: &mut impl Store,
    model: &Image,
) -> Result<(), proptest::test_runner::TestCaseError> {
    for ns in Ns::ALL {
        let mut actual = Vec::new();
        store
            .for_each(ns, &mut |key, value| {
                actual.push((key.as_str().to_string(), value.to_vec()));
                Ok(())
            })
            .unwrap();
        let expected: Vec<_> = model
            .iter()
            .filter(|((n, _), _)| *n == ns as u8)
            .map(|((_, key), value)| (format!("k{key}"), value.clone()))
            .collect();
        prop_assert_eq!(actual, expected);
        for key in 0..8 {
            prop_assert_eq!(
                store.get(ns, &store_key(key)).unwrap(),
                model.get(&(ns as u8, key)).cloned()
            );
        }
    }
    Ok(())
}

const NOW: u64 = 1_790_000_000_000;
fn config() -> Config {
    Config {
        host: "property.test".into(),
        password_rounds: 1,
    }
}
fn member(username: &str) -> NewMember {
    NewMember {
        username: username.into(),
        password: "pass".into(),
        display_name: String::new(),
        role: Role::Member,
    }
}
fn household() -> Service<MemStore> {
    let mut svc = Service::open(MemStore::default(), config()).unwrap();
    svc.provision(member("alice"), None, NOW).unwrap();
    svc.create_member(0, member("bob"), NOW).unwrap();
    svc.create_member(0, member("carol"), NOW).unwrap();
    svc
}

proptest! {
    #![proptest_config(property_config())]
    #[test]
    fn base32_roundtrips_and_orders_every_u64(a in any::<u64>(), b in any::<u64>()) {
        prop_assert_eq!(ids::from_b32(&ids::b32_str(a)),Some(a));
        prop_assert_eq!(ids::b32(a).cmp(&ids::b32(b)),a.cmp(&b));
        prop_assert_eq!(ids::parse(&a.to_string()),Some(a));
    }

    #[test]
    fn id_generator_matches_a_wider_integer_model(commands in prop::collection::vec((any::<bool>(),clock_or_id()),1..100)) {
        let mut generator = IdGen::default();
        let mut last = 0u128;
        for (observe,value) in commands {
            if observe { generator.observe(value); last = last.max(u128::from(value)); }
            else {
                let candidate = u128::from(value) * 65_536;
                let next = candidate.max(last + 1);
                let expected = (candidate <= u128::from(u64::MAX) && next <= u128::from(u64::MAX)).then_some(next as u64);
                prop_assert_eq!(generator.next(value),expected);
                if let Some(id) = expected { last = u128::from(id); }
            }
            prop_assert_eq!(u128::from(generator.last()),last);
        }
    }

    #[test]
    fn record_codec_consumes_exactly_one_complete_record(value in (any::<u64>(),text(),prop::collection::vec(any::<u8>(),0..64)),suffix in prop::collection::vec(any::<u8>(),1..32)) {
        type Record = (u64,String,Vec<u8>);
        let bytes = codec::encode(Kind::Follow,&value).unwrap();
        prop_assert_eq!(codec::decode::<Record>(Kind::Follow,&bytes).unwrap(),value);
        for cut in 0..bytes.len() { prop_assert!(codec::decode::<Record>(Kind::Follow,&bytes[..cut]).is_err()); }
        let mut extended = bytes.clone(); extended.extend(suffix);
        prop_assert!(codec::decode::<Record>(Kind::Follow,&extended).is_err());
        prop_assert!(codec::decode::<Record>(Kind::Boost,&bytes).is_err());
        let mut newer = bytes; newer[0] = codec::VERSION + 1;
        prop_assert!(codec::decode::<Record>(Kind::Follow,&newer).is_err());
    }

    #[test]
    fn unicode_scalar_parameters_agree_between_query_form_and_json(fields in prop::collection::btree_map("[a-z]{1,8}",text(),0..8)) {
        let form = fields.iter().map(|(k,v)|format!("{}={}",http::encode(k),http::encode(v))).collect::<Vec<_>>().join("&");
        let query = Params::parse(&Request::new("GET",&format!("/?{form}"))).unwrap();
        let body = Params::parse(&Request::new("POST","/").with_body("application/x-www-form-urlencoded",form)).unwrap();
        let json = Params::parse(&Request::new("POST","/").with_body("application/json",serde_json::to_vec(&fields).unwrap())).unwrap();
        for (key,value) in fields {
            prop_assert_eq!(query.get(&key),Some(value.as_str()));
            prop_assert_eq!(body.get(&key),Some(value.as_str()));
            prop_assert_eq!(json.get(&key),Some(value.as_str()));
        }
    }

    #[test]
    fn multipart_preserves_binary_bytes_and_false_delimiters(random in prop::collection::vec(any::<u8>(),0..256)) {
        const BOUNDARY: &str = "PROP_BOUNDARY";
        // Real delimiters cannot occur unescaped inside a MIME part.
        prop_assume!(!random.windows(BOUNDARY.len()).any(|w|w == BOUNDARY.as_bytes()));
        let mut payload = random;
        payload.extend_from_slice(b"\r\n--PROP_BOUNDARYpartial\r\n--PROP_BOUNDARY--garbage\r\n");
        let mut body = b"--PROP_BOUNDARY\r\nContent-Disposition: form-data; name=\"avatar\"; filename=\"image.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n".to_vec();
        body.extend_from_slice(&payload); body.extend_from_slice(b"\r\n--PROP_BOUNDARY--\r\n");
        let params = Params::parse(&Request::new("POST","/").with_body("multipart/form-data; boundary=PROP_BOUNDARY",body)).unwrap();
        prop_assert_eq!(params.files.len(),1);
        prop_assert_eq!(&params.files[0].data,&payload);
    }

    #[test]
    fn arbitrary_parser_inputs_never_panic(body in prop::collection::vec(any::<u8>(),0..1024),query in text()) {
        for content_type in ["application/json","application/x-www-form-urlencoded","multipart/form-data; boundary=PROP_BOUNDARY"] {
            let _ = Params::parse(&Request::new("POST",&format!("/?{query}")).with_body(content_type,body.clone()));
        }
        let _ = codec::decode::<(u64,String,Vec<u8>)>(Kind::Follow,&body);
        let _ = ids::from_b32(&query);
        let _ = ids::parse(&query);
    }

    #[test]
    fn bounded_store_matches_independent_capacity_and_namespace_model(commands in writes(), capacity in 0usize..64) {
        let mut store = MemStore::with_capacity(capacity);
        let mut model = Image::new();
        for command in commands {
            let expected = expected_write(&mut model,&command,capacity);
            let actual = match command {
                Write::Set(ns,key,value) => store.set(Ns::from_u8(ns).unwrap(),&store_key(key),&value),
                Write::Erase(ns,key) => store.erase(Ns::from_u8(ns).unwrap(),&store_key(key)),
                Write::Reboot => { store = store.clone(); Ok(()) }
            };
            prop_assert_eq!(actual,expected);
            check_image(&mut store,&model)?;
            let stats = store.stats().unwrap();
            let used: usize = model.values().map(|v|2 + v.len().div_ceil(32)).sum();
            prop_assert_eq!(stats.used_entries,used);
            prop_assert_eq!(stats.total_entries,capacity);
        }
    }

    #[test]
    fn follows_blocks_and_private_visibility_match_a_model_across_reboots(commands in prop::collection::vec((0u8..3,0u8..3,0u8..3,any::<bool>()),1..60)) {
        let mut svc = household();
        let mut follows = [[false;3];3];
        let mut blocks = [[false;3];3];
        let posts: Vec<_> = (0..3).map(|slot|svc.post_status(slot,NewStatus { text: "private".into(), visibility: Some(Visibility::Private), ..Default::default() },NOW).unwrap()).collect();
        for (step,(op,src,dst,on)) in commands.into_iter().enumerate() {
            let (a,b) = (src as usize,dst as usize);
            let now = NOW + step as u64 * 3_600_000;
            if op == 2 { svc = Service::open(svc.into_store(),config()).unwrap(); }
            else if op == 0 {
                let result = svc.set_follow(src,dst,on,None,None,now);
                let allowed = a != b && (!on || !(blocks[a][b] || blocks[b][a]));
                prop_assert_eq!(result.is_ok(),allowed);
                if allowed { follows[a][b] = on; }
            } else {
                let result = svc.set_block(src,dst,on,now);
                prop_assert_eq!(result.is_ok(),a != b);
                if a != b {
                    blocks[a][b] = on;
                    if on { follows[a][b] = false; follows[b][a] = false; }
                }
            }
            svc.check_invariants().unwrap();
            for viewer in 0..3 {
                for author in 0..3 {
                    prop_assert_eq!(svc.state.follows(viewer as u8,author as u8),follows[viewer][author]);
                    prop_assert_eq!(svc.state.blocks(author as u8,viewer as u8),blocks[author][viewer]);
                    let visible = viewer == author || (follows[viewer][author] && !blocks[author][viewer]);
                    prop_assert_eq!(svc.visible(Some(viewer as u8),posts[author]).is_some(),visible);
                }
            }
        }
    }

    #[test]
    fn password_fault_sequences_preserve_dm_keys_and_revoke_exactly_on_commit(steps in prop::collection::vec(("[a-zA-Z0-9]{1,16}",0u8..3,0usize..6),1..10)) {
        let mut svc = household();
        let original = svc.unlock(1,"pass").unwrap();
        let (app,_) = svc.register_app("Property client",None,&[OOB.into()],"read",NOW).unwrap();
        let mut password = String::from("pass");
        for (step,(suffix,failure,at)) in steps.into_iter().enumerate() {
            let now = NOW + step as u64 * 3_600_000;
            let token = svc.issue_token(1,app.id,vec!["read".into()],now).unwrap();
            let new = format!("new-{step}-{suffix}");
            let mut faulty = Service::open(FaultStore::new(svc.into_store()),config()).unwrap();
            let failure = [Failure::Write,Failure::PersistThenError,Failure::Full][failure as usize];
            faulty.store().fail_after(failure,at);
            let result = faulty.change_password(1,&password,&new,now+1);
            svc = Service::open(faulty.into_store().inner,config()).unwrap();
            let committed = svc.check_password("bob",&new,now+2).is_ok();
            if committed { password = new; }
            prop_assert_eq!(*svc.unlock(1,&password).unwrap(),*original);
            prop_assert_eq!(svc.principal(&token,now+2).is_some(),!committed);
            prop_assert!(!result.is_ok() || committed);
            svc = Service::open(svc.into_store(),config()).unwrap();
            prop_assert_eq!(svc.state.repairs,0);
        }
    }
}

#[cfg(feature = "desktop")]
proptest! {
    #![proptest_config(property_config())]
    #[test]
    fn file_store_matches_a_model_through_overwrites_deletes_and_compacting_reopens(commands in writes()) {
        use mastomini::store::file::FileStore;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("properties.store");
        let mut store = FileStore::open(&path).unwrap();
        let mut model = Image::new();
        for command in commands {
            let expected = expected_write(&mut model,&command,usize::MAX);
            let actual = match command {
                Write::Set(ns,key,value) => store.set(Ns::from_u8(ns).unwrap(),&store_key(key),&value),
                Write::Erase(ns,key) => store.erase(Ns::from_u8(ns).unwrap(),&store_key(key)),
                Write::Reboot => { drop(store); store = FileStore::open(&path).unwrap(); Ok(()) }
            };
            prop_assert_eq!(actual,expected);
            check_image(&mut store,&model)?;
        }
        drop(store);
        check_image(&mut FileStore::open(&path).unwrap(),&model)?;
    }
}
