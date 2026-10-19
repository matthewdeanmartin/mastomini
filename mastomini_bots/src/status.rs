//! Independent, credential-free mastomini checks. Bounded history for small boards.
use crate::{
    mastodon::{HttpClient, HttpRequest},
    service::Service,
    store::KvStore,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const DAY: u64 = 86_400_000;
pub const INTERVAL_MS: u64 = 30 * 60_000;
const STALE_MS: u64 = 2 * INTERVAL_MS + 60_000;
#[derive(Default, Serialize, Deserialize)]
pub struct Monitor {
    // UTC day, successful probes, total probes. Missing days remain unknown.
    days: Vec<(u32, u16, u16)>,
    events: Vec<(u64, bool)>,
    #[serde(skip)]
    checked: Option<u64>,
    #[serde(skip)]
    healthy: bool,
    #[serde(skip)]
    saved: u64,
}
impl Monitor {
    pub fn load(store: &impl KvStore) -> Self {
        let mut state: Self = store
            .get("status_history")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        state.days.truncate(90);
        state.days.shrink_to_fit();
        state.events.truncate(24);
        state.events.shrink_to_fit();
        state
    }
    fn observe(&mut self, now: u64, healthy: bool) -> bool {
        let changed = self.checked.is_none() || self.healthy != healthy;
        if changed {
            if self.events.len() == 24 {
                self.events.remove(0);
            }
            self.events.reserve_exact(24 - self.events.len());
            self.events.push((now, healthy));
        }
        self.checked = Some(now);
        self.healthy = healthy;
        let day = (now / DAY) as u32;
        self.days.retain(|d| d.0 <= day && day - d.0 < 90);
        if self.days.last().map(|d| d.0) != Some(day) {
            self.days.reserve_exact(90 - self.days.len());
            self.days.push((day, 0, 0));
        }
        let row = self.days.last_mut().unwrap();
        row.1 = row.1.saturating_add(u16::from(healthy));
        row.2 = row.2.saturating_add(1);
        changed || now / DAY != self.saved / DAY
    }
    pub fn public(&self, now: Option<u64>) -> Value {
        let fresh = now
            .zip(self.checked)
            .is_some_and(|(n, c)| n >= c && n - c < STALE_MS);
        json!({"mode":"observer", "target":"https://mastomini.local", "checked_at":self.checked,
            "state":if !fresh {"unknown"} else if self.healthy {"operational"} else {"outage"},
            "days":self.days, "events":self.events})
    }
}
pub fn tick<S: KvStore>(
    shared: &Arc<Mutex<Service<S>>>,
    http: &mut dyn HttpClient,
    now: Option<u64>,
) {
    let Some(now) = now else { return };
    {
        let svc = shared.lock().unwrap_or_else(|e| e.into_inner());
        if svc
            .monitor
            .checked
            .is_some_and(|c| now < c || now - c < INTERVAL_MS)
        {
            return;
        }
        // Reuse the timer worker, but give scheduled posts priority. A probe
        // has a five-second timeout; leave ten seconds before the next job.
        if svc
            .scheduler
            .jobs
            .iter()
            .any(|job| job.at <= now.saturating_add(10_000))
        {
            return;
        }
    }
    let result = http.send(&HttpRequest {
        method: "GET",
        url: "https://mastomini.local/api/mastomini/v1/public-status".into(),
        headers: vec![],
        body: vec![],
    });
    let healthy = result
        .ok()
        .filter(|r| r.status == 200)
        .and_then(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .is_some_and(|v| v["state"] == "operational" && v["mode"] == "local");
    let mut svc = shared.lock().unwrap_or_else(|e| e.into_inner());
    if svc.monitor.observe(now, healthy) {
        if let Ok(text) = serde_json::to_string(&svc.monitor) {
            if svc.store.put("status_history", &text).is_ok() {
                svc.monitor.saved = now;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_probe_records_failure_and_recovery_without_credentials() {
        struct Client(bool);
        impl HttpClient for Client {
            fn send(&mut self, req: &HttpRequest) -> Result<crate::mastodon::HttpResponse, String> {
                assert!(req.headers.is_empty());
                assert_eq!(
                    req.url,
                    "https://mastomini.local/api/mastomini/v1/public-status"
                );
                if !self.0 {
                    return Err("offline".into());
                }
                Ok(crate::mastodon::HttpResponse {
                    status: 200,
                    body: br#"{"mode":"local","state":"operational"}"#.to_vec(),
                })
            }
        }
        let shared = Arc::new(Mutex::new(
            Service::open(crate::store::MemStore::default(), vec![]).unwrap(),
        ));
        tick(&shared, &mut Client(false), Some(DAY));
        assert_eq!(
            shared.lock().unwrap().monitor.public(Some(DAY))["state"],
            "outage"
        );
        // No early retry, including a backwards wall-clock adjustment.
        tick(&shared, &mut Client(true), Some(DAY + INTERVAL_MS - 1));
        tick(&shared, &mut Client(true), Some(DAY - 1));
        assert_eq!(shared.lock().unwrap().monitor.days, vec![(1, 0, 1)]);
        tick(&shared, &mut Client(true), Some(DAY + INTERVAL_MS));
        let svc = shared.lock().unwrap();
        assert_eq!(
            svc.monitor.public(Some(DAY + INTERVAL_MS))["state"],
            "operational"
        );
        assert_eq!(Monitor::load(&svc.store).days, vec![(1, 1, 2)]);
    }
    #[test]
    fn stale_and_restarted_observers_never_claim_health() {
        let mut m = Monitor::default();
        m.observe(DAY, true);
        assert_eq!(m.public(Some(DAY))["state"], "operational");
        assert_eq!(m.public(Some(DAY + STALE_MS))["state"], "unknown");
        let restored: Monitor = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(restored.public(Some(DAY))["state"], "unknown");
        m.observe(DAY + INTERVAL_MS, false);
        assert_eq!(m.public(Some(DAY + INTERVAL_MS))["state"], "outage");
        assert_eq!(m.days, vec![(1, 1, 2)]);
    }
    #[test]
    fn retention_is_bounded_and_gaps_are_not_successes() {
        let mut m = Monitor::default();
        for day in 1..200 {
            m.observe(day * DAY, day % 2 == 0);
        }
        assert_eq!(m.days.len(), 90);
        assert_eq!(m.events.len(), 24);
        m.observe(300 * DAY, true);
        assert_eq!(m.days, vec![(300, 1, 1)]);
    }
    #[test]
    fn full_history_stays_small_and_stable_status_saves_daily() {
        let mut m = Monitor::default();
        for day in 20_000..20_090 {
            for probe in 0..48 {
                let now = day * DAY + probe * INTERVAL_MS;
                let save = m.observe(now, true);
                assert_eq!(save, probe == 0);
                if save {
                    m.saved = now;
                }
            }
        }
        for i in 0..24 {
            m.observe(20_090 * DAY + i * INTERVAL_MS, i % 2 == 0);
        }
        assert_eq!(m.days.capacity(), 90);
        assert_eq!(m.events.capacity(), 24);
        let heap = m.days.capacity() * std::mem::size_of::<(u32, u16, u16)>()
            + m.events.capacity() * std::mem::size_of::<(u64, bool)>();
        let bytes = serde_json::to_string(&m).unwrap().len();
        assert!(heap <= 1104, "history heap: {heap}");
        assert!(bytes <= 2560, "serialized history: {bytes}");
        println!(
            "History: {heap} heap bytes plus {} struct bytes; {bytes} serialized bytes",
            std::mem::size_of::<Monitor>()
        );
    }
}
