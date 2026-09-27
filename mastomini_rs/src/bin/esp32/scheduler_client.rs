//! Small, short-lived outbound connection used only for pending handoffs.
use esp_idf_svc::http::client::{Configuration, EspHttpConnection, FollowRedirectsPolicy, Method};
use mastomini::scheduler_bridge::Handoff;
use std::time::Duration;

pub fn send(job: &Handoff) -> Result<u16, String> {
    let host = job
        .url
        .strip_prefix("https://")
        .and_then(|s| s.split('/').next())
        .and_then(|s| s.split(':').next())
        .unwrap_or("");
    if !super::household_trust::household_host(host) {
        return Err("The bots address must be a household HTTPS host".into());
    }
    super::household_tls::prepare(super::CA_DER)?;
    let config = Configuration {
        timeout: Some(Duration::from_secs(5)),
        crt_bundle_attach: Some(super::household_tls::attach),
        follow_redirects_policy: FollowRedirectsPolicy::FollowNone,
        buffer_size: Some(1024),
        buffer_size_tx: Some(1024),
        ..Default::default()
    };
    let run = || -> Result<u16, esp_idf_svc::sys::EspError> {
        let mut conn = EspHttpConnection::new(&config)?;
        let auth = format!("Bearer {}", job.key);
        let length = job.body.len().to_string();
        conn.initiate_request(
            Method::Post,
            &job.url,
            &[
                ("Authorization", &auth),
                ("Content-Type", "application/json"),
                ("Content-Length", &length),
            ],
        )?;
        conn.write_all(&job.body)?;
        conn.initiate_response()?;
        Ok(conn.status())
    };
    // Transport errors must not echo the API key or post contents.
    run().map_err(|_| "Scheduler transport failed".into())
}
