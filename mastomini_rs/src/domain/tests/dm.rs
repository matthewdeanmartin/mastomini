//! Direct messages across power cuts.

use super::*;

#[test]
fn power_cut_while_posting_a_direct_message() {
    power_cut_every_write(
        |_| 0,
        |svc, _| {
            let dm = NewStatus {
                visibility: Some(Visibility::Direct),
                ..post("@bob psst")
            };
            svc.post_status(0, dm, T0 + 1).map(|_| ())
        },
    );
    for writes in 0..4 {
        let mut svc = household(FaultStore::new(MemStore::default()));
        svc.store().cut_after(writes);
        let dm = NewStatus {
            visibility: Some(Visibility::Direct),
            ..post("@bob psst")
        };
        let _ = svc.post_status(0, dm, T0 + 1);
        let svc = Service::open(svc.into_store().inner, config()).unwrap();
        // Either the whole message committed, or nothing of it is left.
        for (id, s) in &svc.state.statuses {
            if s.rec.visibility == Visibility::Direct {
                assert!(svc.state.dms.contains_key(id), "cut at {writes}");
            }
        }
        for id in svc.state.dms.keys() {
            assert!(
                svc.state.statuses.contains_key(id),
                "orphan envelope at {writes}"
            );
        }
    }
}
