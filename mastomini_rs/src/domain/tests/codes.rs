//! Invite and reset codes: single use, expiry, limits, reboots and power
//! cuts; sign out everywhere and devices.

use super::*;

fn redemption(username: &str, password: &str) -> Redemption {
    Redemption {
        password: password.into(),
        username: username.into(),
        display_name: String::new(),
    }
}

#[test]
fn invite_creates_a_member_once() {
    let mut svc = household(MemStore::default());
    assert!(matches!(svc.issue_invite(1, T0), Err(Error::Forbidden(_))));
    let (_, code) = svc.issue_invite(0, T0).unwrap();
    // A bad username leaves the code usable.
    assert!(svc
        .redeem_code(&code, redemption("Bad Name", "pass"), T0)
        .is_err());
    assert!(svc
        .redeem_code(&code, redemption("bob", "pass"), T0)
        .is_err());
    let slot = svc
        .redeem_code(&code, redemption("dave", "davepw"), T0)
        .unwrap();
    assert_eq!(svc.state.account(slot).unwrap().rec.role, Role::Member);
    assert!(svc.user_key(slot).is_some());
    assert!(svc
        .redeem_code(&code, redemption("erin", "erinpw"), T0)
        .is_err());
    let mut svc = reopen(svc);
    assert!(svc.state.invites.is_empty());
    assert_eq!(svc.check_password("dave", "davepw", T0).unwrap(), slot);
}

#[test]
fn codes_expire_and_survive_reboot() {
    let mut svc = household(MemStore::default());
    let (_, code) = svc.issue_invite(0, T0).unwrap();
    let svc = reopen(svc);
    assert!(svc.find_code(&code, T0 + CODE_LIFETIME_MS - 1).is_some());
    assert!(svc.find_code(&code, T0 + CODE_LIFETIME_MS).is_none());
    assert!(svc.find_code("not-a-code", T0).is_none());
}

#[test]
fn at_most_eight_codes_and_the_oldest_goes() {
    let mut svc = household(MemStore::default());
    let codes: Vec<String> = (0..=MAX_CODES_LIVE)
        .map(|i| svc.issue_invite(0, T0 + i as u64).unwrap().1)
        .collect();
    assert_eq!(svc.state.invites.len(), MAX_CODES_LIVE);
    assert!(svc.find_code(&codes[0], T0 + 10).is_none());
    assert!(svc.find_code(&codes[MAX_CODES_LIVE], T0 + 10).is_some());
}

#[test]
fn reset_sets_a_new_password_and_signs_out() {
    let mut svc = household(MemStore::default());
    let oob = vec![OOB.to_string()];
    let (app, _) = svc.register_app("app", None, &oob, "read", T0).unwrap();
    let token = svc.issue_token(1, app.id, vec!["read".into()], T0).unwrap();
    // Not by a member, not for yourself, not for the owner.
    assert!(svc.issue_reset(2, 1, T0).is_err());
    assert!(svc.issue_reset(0, 0, T0).is_err());
    svc.set_role(0, 2, Role::Admin, T0).unwrap();
    assert!(svc.issue_reset(2, 0, T0).is_err());
    let (_, first) = svc.issue_reset(0, 1, T0).unwrap();
    // A new reset for the same member replaces the old one.
    let (_, code) = svc.issue_reset(2, 1, T0).unwrap();
    assert!(svc.find_code(&first, T0).is_none());
    // The password doesn't change until the code is redeemed.
    assert!(svc.check_password("bob", "pass", T0).is_ok());
    assert!(svc.redeem_code(&code, redemption("", "no"), T0).is_err());
    svc.redeem_code(&code, redemption("", "bobnew"), T0)
        .unwrap();
    assert!(svc.principal(&token, T0).is_none());
    let mut svc = reopen(svc);
    assert!(svc.check_password("bob", "pass", T0).is_err());
    assert_eq!(svc.check_password("bob", "bobnew", T0).unwrap(), 1);
    assert!(svc.unlock(1, "bobnew").is_ok());
    assert!(svc.find_code(&code, T0).is_none());
}

#[test]
fn reset_code_dies_with_its_member() {
    let mut svc = household(MemStore::default());
    let (_, code) = svc.issue_reset(0, 2, T0).unwrap();
    svc.delete_account(0, 2, T0).unwrap();
    assert!(svc.find_code(&code, T0).is_none());
    assert!(svc.state.invites.is_empty());
    // The slot is reused by someone else; the old code still doesn't work.
    let (_, stale) = svc.issue_reset(0, 1, T0).unwrap();
    svc.delete_account(0, 1, T0).unwrap();
    svc.create_member(0, member("dave"), T0).unwrap();
    assert!(svc
        .redeem_code(&stale, redemption("", "takeover"), T0)
        .is_err());
}

#[test]
fn power_cut_while_redeeming_never_uses_a_code_twice() {
    for writes in 0.. {
        let mut svc = household(FaultStore::new(MemStore::default()));
        let (_, code) = svc.issue_invite(0, T0).unwrap();
        svc.store().cut_after(writes);
        let result = svc.redeem_code(&code, redemption("dave", "davepw"), T0 + 1);
        let completed = !svc.store().is_cut();
        let mut svc = Service::open(svc.into_store().inner, config()).unwrap();
        svc.check_invariants().unwrap();
        let created = svc.state.account_by_username("dave").is_some();
        let usable = svc.find_code(&code, T0 + 2).is_some();
        assert!(!(created && usable), "cut at {writes}");
        if usable {
            svc.redeem_code(&code, redemption("dave", "davepw"), T0 + 2)
                .unwrap();
        }
        if completed {
            result.unwrap();
            assert!(created);
            break;
        }
        assert!(writes < 20, "operation never completed");
    }
}

#[test]
fn power_cut_while_resetting_never_keeps_old_sessions() {
    for writes in 0.. {
        let mut svc = household(FaultStore::new(MemStore::default()));
        let oob = vec![OOB.to_string()];
        let (app, _) = svc.register_app("app", None, &oob, "read", T0).unwrap();
        let token = svc.issue_token(1, app.id, vec!["read".into()], T0).unwrap();
        let (_, code) = svc.issue_reset(0, 1, T0).unwrap();
        svc.store().cut_after(writes);
        let result = svc.redeem_code(&code, redemption("", "bobnew"), T0 + 1);
        let completed = !svc.store().is_cut();
        let mut svc = Service::open(svc.into_store().inner, config()).unwrap();
        svc.check_invariants().unwrap();
        let changed = svc.check_password("bob", "bobnew", T0 + 2).is_ok();
        assert_eq!(
            svc.principal(&token, T0 + 2).is_some(),
            !changed,
            "cut at {writes}"
        );
        assert!(!(changed && svc.find_code(&code, T0 + 2).is_some()));
        if completed {
            result.unwrap();
            assert!(changed);
            break;
        }
        assert!(writes < 20, "operation never completed");
    }
}

#[test]
fn sign_out_everywhere_and_devices() {
    let mut svc = household(MemStore::default());
    let oob = vec![OOB.to_string()];
    let (app, _) = svc.register_app("app", None, &oob, "read", T0).unwrap();
    let phone = svc.issue_token(1, app.id, vec!["read".into()], T0).unwrap();
    let laptop = svc
        .issue_token(1, app.id, vec!["read".into()], T0 + 1)
        .unwrap();
    svc.issue_token(2, app.id, vec!["read".into()], T0).unwrap();
    let devices: Vec<u64> = svc.devices(1).iter().map(|t| t.rec.id).collect();
    assert_eq!(devices.len(), 2);
    // Newest first; someone else's device is not yours to revoke.
    let carols = svc.devices(2)[0].rec.id;
    assert_eq!(svc.revoke_device(1, carols, T0), Err(Error::NotFound));
    svc.revoke_device(1, devices[0], T0).unwrap();
    assert!(svc.principal(&laptop, T0).is_none());
    assert!(svc.principal(&phone, T0).is_some());
    svc.sign_out_everywhere(1, T0).unwrap();
    let mut svc = reopen(svc);
    assert!(svc.principal(&phone, T0).is_none());
    assert!(svc.devices(1).is_empty());
    assert_eq!(svc.devices(2).len(), 1);
    assert!(svc.check_password("bob", "pass", T0).is_ok());
}
