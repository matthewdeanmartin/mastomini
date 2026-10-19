//! The scheduler thread, the same on both platforms: once a second, take
//! due jobs under the lock, run them without it, report under it again.

use crate::mastodon::HttpClient;
use crate::service::{execute, Service};
use crate::store::KvStore;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One pass. Returns how many jobs ran.
pub fn tick<S: KvStore>(
    svc: &Arc<Mutex<Service<S>>>,
    http: &mut dyn HttpClient,
    now: fn() -> Option<u64>,
) -> usize {
    if crate::enrollment::tick(svc, http, now()) {
        return 0;
    }
    let (jobs, bots) = {
        let mut svc = svc.lock().unwrap_or_else(|e| e.into_inner());
        (svc.due(now()), Arc::clone(&svc.bots))
    };
    for job in &jobs {
        let started = now().unwrap_or(job.slot_ms);
        let outcome = execute(&bots, job, http, started);
        let finished = now().unwrap_or(started);
        svc.lock()
            .unwrap_or_else(|e| e.into_inner())
            .finish(job, started, finished, outcome);
    }
    crate::profiles::tick(svc, http, now());
    jobs.len()
}

pub fn run_forever<S: KvStore>(
    svc: Arc<Mutex<Service<S>>>,
    mut http: Box<dyn HttpClient>,
    now: fn() -> Option<u64>,
) -> ! {
    loop {
        tick(&svc, http.as_mut(), now);
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bot::{Bot, BotInfo, Run, RunError},
        mastodon::{HttpRequest, HttpResponse},
        schedule::Schedule,
        service::JobKind,
        store::MemStore,
    };

    struct Counter;
    impl Bot for Counter {
        fn info(&self) -> BotInfo {
            BotInfo {
                id: "counter",
                name: "Counter",
                description: "Test",
                schedule: Schedule::Manual,
                grace_minutes: 5,
                default_instance: "",
                uses_llm: false,
                needs_mastodon: false,
            }
        }
        fn run(&self, run: &mut Run<'_>) -> Result<String, RunError> {
            assert!(run.manual);
            let count = run
                .state
                .get("count")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0)
                + 1;
            run.state.insert("count".into(), count.to_string());
            run.http
                .send(&HttpRequest {
                    method: "GET",
                    url: "https://test.invalid".into(),
                    headers: vec![],
                    body: vec![],
                })
                .unwrap();
            Ok("Counted".into())
        }
    }
    struct Unlocked(Arc<Mutex<Service<MemStore>>>);
    impl HttpClient for Unlocked {
        fn send(&mut self, _: &HttpRequest) -> Result<HttpResponse, String> {
            assert!(
                self.0.try_lock().is_ok(),
                "Network I/O must not hold the admin API lock"
            );
            Ok(HttpResponse {
                status: 200,
                body: vec![],
            })
        }
    }
    fn now() -> Option<u64> {
        Some(1_800_000_000_000)
    }

    #[test]
    fn worker_runs_outside_lock_persists_results_and_does_not_repeat_manual_jobs() {
        let shared = Arc::new(Mutex::new(
            Service::open(MemStore::default(), vec![Box::new(Counter)]).unwrap(),
        ));
        let mut http = Unlocked(shared.clone());
        assert_eq!(tick(&shared, &mut http, now), 0);
        shared
            .lock()
            .unwrap()
            .request(0, JobKind::Manual, now())
            .unwrap();
        assert_eq!(tick(&shared, &mut http, now), 1);
        assert_eq!(tick(&shared, &mut http, now), 0);
        let mut svc = shared.lock().unwrap();
        assert!(!svc.runtime[0].running);
        assert_eq!(svc.records[0].runs, 1);
        assert_eq!(svc.records[0].state["count"], "1");
        let reopened =
            Service::open(std::mem::take(&mut svc.store), vec![Box::new(Counter)]).unwrap();
        assert_eq!(reopened.records[0].state["count"], "1");
    }
}
