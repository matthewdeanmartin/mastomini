//! Trusted mastomini timer service. Reuses the Mastodon key already stored
//! in a bot configuration; it never holds drafts or user login credentials.
use crate::http::{Request, Response};
use crate::mastodon::{HttpClient, HttpRequest};
use crate::service::Service;
use crate::store::KvStore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::{Arc, Mutex};

const CAPACITY: usize = 16;

#[derive(Clone, Serialize, Deserialize)]
pub struct Timer {
    pub id: String,
    pub revision: u64,
    pub at: u64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Scheduler {
    pub instance: String,
    pub token: String,
    pub jobs: Vec<Timer>,
    #[serde(skip)]
    pub retry_at: u64,
}

pub fn load(store: &impl KvStore) -> Result<Scheduler, String> {
    let state: Scheduler = match store.get("scheduler")? {
        Some(text) => serde_json::from_str(&text).map_err(|e| format!("scheduler: {e}"))?,
        None => Scheduler::default(),
    };
    if state.jobs.len() > CAPACITY {
        return Err("Scheduler capacity exceeded".into());
    }
    Ok(state)
}

impl<S: KvStore> Service<S> {
    fn save_scheduler(&mut self, next: Scheduler) -> Result<(), String> {
        self.store.put(
            "scheduler",
            &serde_json::to_string(&next).map_err(|e| e.to_string())?,
        )?;
        self.scheduler = next;
        Ok(())
    }
}

/// Called before admin-session authentication; only the existing configured
/// Mastodon API key is accepted here, not a password or browser session.
pub fn receive<S: KvStore>(svc: &mut Service<S>, req: &Request) -> Response {
    if req.method != "POST" {
        return Response::error(405, "Use POST");
    }
    let Some(token) = req.bearer().filter(|t| !t.is_empty()) else {
        return Response::error(401, "API key required");
    };
    let configured = svc
        .records
        .iter()
        .find(|r| crate::auth::same(&r.config.token, token));
    let instance = match configured {
        Some(r) => r.config.instance.clone(),
        None => {
            return Response::error(401, "Use an existing Mastodon API key configured on a bot")
        }
    };
    let secure = instance.starts_with("https://");
    let loopback =
        instance.starts_with("http://127.0.0.1:") || instance.starts_with("http://localhost:");
    if !secure && !(cfg!(feature = "desktop") && loopback) {
        return Response::error(422, "The scheduler requires an HTTPS Mastodon instance");
    }
    if !svc.scheduler.instance.is_empty() && svc.scheduler.instance != instance {
        return Response::error(
            409,
            "This scheduler is already paired with another instance",
        );
    }
    let body = match req.json() {
        Ok(body) => body,
        Err(r) => return r,
    };
    let id = body["id"].as_str().unwrap_or("");
    let revision = body["revision"].as_u64().unwrap_or(0);
    let at = body["at"].as_u64().unwrap_or(0);
    if id.parse::<u64>().ok().filter(|id| *id > 0).is_none() || revision == 0 || at == 0 {
        return Response::error(422, "id, revision and at are required");
    }
    let mut next = svc.scheduler.clone();
    next.instance = instance;
    next.token = token.into();
    if let Some(old) = next.jobs.iter().find(|j| j.id == id) {
        if old.revision > revision {
            return Response::error(409, "An updated job is already stored");
        }
        if old.revision == revision && old.at != at {
            return Response::error(409, "Revision does not match job");
        }
    }
    let cancelled = body["cancelled"].as_bool().unwrap_or(false);
    next.jobs.retain(|j| j.id != id);
    if !cancelled {
        if next.jobs.len() >= CAPACITY {
            return Response::error(429, "Scheduler queue is full");
        }
        next.jobs.push(Timer {
            id: id.into(),
            revision,
            at,
        });
    }
    // Durable before acknowledging, so mastomini can stop retrying handoff.
    match svc.save_scheduler(next) {
        Ok(()) => Response::ok(json!({"accepted": true})),
        Err(_) => Response::error(503, "Could not persist scheduler job"),
    }
}

/// Separate from LLM/bot execution: a slow model must not delay timed posts.
pub fn tick<S: KvStore>(
    shared: &Arc<Mutex<Service<S>>>,
    http: &mut dyn HttpClient,
    now: Option<u64>,
) -> usize {
    let Some(now) = now else {
        return 0;
    };
    let (instance, token, jobs) = {
        let svc = shared.lock().unwrap_or_else(|e| e.into_inner());
        if now < svc.scheduler.retry_at {
            return 0;
        }
        (
            svc.scheduler.instance.clone(),
            svc.scheduler.token.clone(),
            svc.scheduler
                .jobs
                .iter()
                .filter(|j| j.at <= now)
                .cloned()
                .collect::<Vec<_>>(),
        )
    };
    let count = jobs.len();
    for job in jobs {
        let req = HttpRequest {
            method: "POST",
            url: format!("{instance}/api/mastomini/v1/scheduler/publish/{}", job.id),
            headers: vec![
                ("Authorization".into(), format!("Bearer {token}")),
                ("Content-Type".into(), "application/json".into()),
            ],
            body: serde_json::to_vec(&json!({"revision": job.revision})).unwrap(),
        };
        let result = http.send(&req);
        let mut svc = shared.lock().unwrap_or_else(|e| e.into_inner());
        // Cancellation/rescheduling can arrive while the callback is in flight.
        if !svc
            .scheduler
            .jobs
            .iter()
            .any(|j| j.id == job.id && j.revision == job.revision)
        {
            continue;
        }
        let status = result.as_ref().map(|r| r.status).ok();
        let published = result
            .as_ref()
            .ok()
            .and_then(|r| serde_json::from_slice::<serde_json::Value>(&r.body).ok())
            .is_some_and(|body| body["id"].is_string());
        if matches!(status, Some(200..=299) | Some(404) | Some(410) | Some(422)) {
            let mut next = svc.scheduler.clone();
            next.jobs
                .retain(|j| j.id != job.id || j.revision != job.revision);
            if svc.save_scheduler(next).is_ok() {
                svc.activity.add(
                    Some(now),
                    None,
                    if published { "ok" } else { "info" },
                    format!(
                        "Scheduled post {}: {} (HTTP {})",
                        job.id,
                        if published {
                            "published"
                        } else {
                            "finished without publication; check mastomini"
                        },
                        status.unwrap()
                    ),
                );
            }
        } else {
            // Includes revoked keys: retain the queue until the admin repairs
            // the existing key. No lateness cutoff and no draft text in logs.
            svc.scheduler.retry_at = now.saturating_add(30_000);
            svc.activity.add(
                Some(now),
                None,
                "error",
                format!(
                    "Scheduled post {} awaiting retry (HTTP {:?})",
                    job.id, status
                ),
            );
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::ConfigUpdate;
    use crate::store::MemStore;

    fn service() -> Service<MemStore> {
        let mut svc = Service::open(MemStore::default(), crate::bots::all()).unwrap();
        svc.configure(
            0,
            ConfigUpdate {
                instance: Some("https://mastomini.local".into()),
                token: Some("existing-key".into()),
                ..Default::default()
            },
            Some(1000),
        )
        .unwrap();
        svc
    }

    fn request(id: u64, revision: u64, at: u64, cancelled: bool) -> Request {
        let mut req = Request::new("POST", "/api/v1/scheduler/jobs")
            .with_header("Authorization", "Bearer existing-key");
        req.body = serde_json::to_vec(
            &json!({"id":id.to_string(),"revision":revision,"at":at,"cancelled":cancelled}),
        )
        .unwrap();
        req
    }

    #[test]
    fn reuses_keys_persists_and_ignores_stale_updates() {
        let mut svc = service();
        assert_eq!(
            receive(&mut svc, &Request::new("POST", "/api/v1/scheduler/jobs")).status,
            401
        );
        assert_eq!(receive(&mut svc, &request(1, 1, 1000, false)).status, 200);
        assert_eq!(receive(&mut svc, &request(1, 1, 1000, false)).status, 200);
        assert_eq!(svc.scheduler.jobs.len(), 1);
        let mut svc = Service::open(svc.store, crate::bots::all()).unwrap();
        assert_eq!(svc.scheduler.jobs.len(), 1);
        assert_eq!(receive(&mut svc, &request(1, 2, 2000, false)).status, 200);
        assert_eq!(receive(&mut svc, &request(1, 1, 1000, false)).status, 409);
        assert_eq!(receive(&mut svc, &request(1, 3, 2000, true)).status, 200);
        assert!(svc.scheduler.jobs.is_empty());
    }

    #[test]
    fn bounded_queue_does_not_drop_existing_jobs() {
        let mut svc = service();
        for id in 1..=16 {
            assert_eq!(receive(&mut svc, &request(id, 1, 1000, false)).status, 200);
        }
        assert_eq!(receive(&mut svc, &request(17, 1, 1000, false)).status, 429);
        assert_eq!(svc.scheduler.jobs.len(), 16);
        assert_eq!(receive(&mut svc, &request(1, 2, 1000, true)).status, 200);
        assert_eq!(receive(&mut svc, &request(17, 1, 1000, false)).status, 200);
    }

    struct Offline;
    impl HttpClient for Offline {
        fn send(&mut self, _: &HttpRequest) -> Result<crate::mastodon::HttpResponse, String> {
            Err("offline".into())
        }
    }

    #[test]
    fn retries_after_any_lateness_and_keeps_queue_on_network_failure() {
        let mut svc = service();
        receive(&mut svc, &request(1, 1, 1000, false));
        let shared = Arc::new(Mutex::new(svc));
        assert_eq!(tick(&shared, &mut Offline, None), 0);
        assert_eq!(tick(&shared, &mut Offline, Some(999)), 0);
        let late = 7 * 86400_000;
        assert_eq!(tick(&shared, &mut Offline, Some(late)), 1);
        assert_eq!(tick(&shared, &mut Offline, Some(late + 1)), 0);
        assert_eq!(tick(&shared, &mut Offline, Some(late + 30_000)), 1);
        assert_eq!(shared.lock().unwrap().scheduler.jobs.len(), 1);
    }
}
