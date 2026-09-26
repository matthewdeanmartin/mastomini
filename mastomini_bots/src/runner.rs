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
