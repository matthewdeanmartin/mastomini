//! Outbox delivery, never called by a user request or while doing network I/O
//! under the service mutex. There is no due-time scan on the mastomini board.
use crate::domain::Service;
use crate::store::Store;
use serde_json::json;
use std::sync::{Arc, Mutex};

pub struct Handoff {
    pub url: String,
    pub key: String,
    pub id: u64,
    pub revision: u64,
    pub body: Vec<u8>,
}

pub fn transfer<S: Store>(
    shared: &Arc<Mutex<Service<S>>>,
    send: impl FnOnce(&Handoff) -> Result<u16, String>,
) {
    let handoff = {
        let mut svc = shared.lock().unwrap_or_else(|e| e.into_inner());
        let state = &mut svc.scheduling;
        if state.bridge.api_key.is_empty() {
            return;
        }
        let pending = |j: &&crate::domain::scheduled::Scheduled| {
            !j.done
                && (j.failure.is_none() || j.cancelled)
                && !state.acknowledged.contains(&(j.id, j.revision))
        };
        let job = state
            .jobs
            .values()
            .filter(pending)
            .find(|j| j.id > state.last_handoff)
            .or_else(|| state.jobs.values().find(pending));
        let Some(job) = job else {
            return;
        };
        let handoff = Handoff {
            url: format!("{}/api/v1/scheduler/jobs", state.bridge.bots_url),
            key: state.bridge.api_key.clone(),
            id: job.id,
            revision: job.revision,
            body: serde_json::to_vec(&json!({"id": job.id.to_string(), "revision": job.revision,
                "at": job.at, "cancelled": job.cancelled}))
            .unwrap(),
        };
        state.last_handoff = job.id;
        handoff
    };
    let result = send(&handoff);
    let mut svc = shared.lock().unwrap_or_else(|e| e.into_inner());
    // A response for an earlier configuration/revision cannot acknowledge a new one.
    if svc.scheduling.bridge.api_key != handoff.key
        || format!("{}/api/v1/scheduler/jobs", svc.scheduling.bridge.bots_url) != handoff.url
    {
        return;
    }
    if svc
        .scheduling
        .jobs
        .get(&handoff.id)
        .is_some_and(|j| j.revision == handoff.revision)
    {
        if matches!(result, Ok(200..=299)) {
            svc.scheduling
                .acknowledged
                .insert((handoff.id, handoff.revision));
            svc.scheduling.last_error = None;
        } else {
            svc.scheduling.last_error = Some(match result {
                Ok(status) => format!("Bots handoff returned HTTP {status}; retrying"),
                Err(_) => "Bots handoff failed; retrying".into(),
            });
        }
    }
}

#[cfg(feature = "desktop")]
pub fn desktop_agent(ca: &[u8]) -> Result<ureq::Agent, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for cert in rustls_pemfile::certs(&mut &ca[..]) {
        roots
            .add(cert.map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(ureq::AgentBuilder::new()
        .tls_config(Arc::new(tls))
        .timeout(std::time::Duration::from_secs(5))
        .redirects(0)
        .build())
}

#[cfg(feature = "desktop")]
pub fn desktop_send(agent: &ureq::Agent, job: &Handoff) -> Result<u16, String> {
    match agent
        .post(&job.url)
        .set("Authorization", &format!("Bearer {}", job.key))
        .set("Content-Type", "application/json")
        .send_bytes(&job.body)
    {
        Ok(r) | Err(ureq::Error::Status(_, r)) => Ok(r.status()),
        Err(_) => Err("Scheduler transport failed".into()),
    }
}
