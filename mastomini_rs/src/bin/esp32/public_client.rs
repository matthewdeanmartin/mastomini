//! Requests to public servers (server::Fetch): searches, and signing in to
//! the search server. Verified against the public CA bundle, bounded in time
//! and size. The connection loop waits for them, so they must stay short;
//! search answers are cached.
//!
//! The last server's connection is kept open: a TLS handshake costs the P4
//! hundreds of milliseconds, and most searches go to the same search server.
//! A kept connection that fails is replaced once, for a GET only: a GET is
//! safe to repeat, a sign-in POST is not.
use esp_idf_svc::http::client::{Configuration, EspHttpConnection, FollowRedirectsPolicy, Method};
use std::sync::Mutex;
use std::time::Duration;

#[derive(Default)]
pub struct PublicClient {
    kept: Mutex<Option<Kept>>,
}

/// The host it is connected to, and the connection.
struct Kept(String, EspHttpConnection);

// SAFETY: an esp_http_client handle is not tied to the task that made it,
// and the Mutex above lets one caller at a time use it. No event handler
// is installed, so no callback state crosses threads.
unsafe impl Send for Kept {}

fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?']).next().unwrap_or("")
}

fn config() -> Configuration {
    Configuration {
        timeout: Some(Duration::from_secs(4)),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        follow_redirects_policy: FollowRedirectsPolicy::FollowNone,
        buffer_size: Some(2048),
        buffer_size_tx: Some(1024),
        ..Default::default()
    }
}

/// One request; `complete` is false when the body was cut at the limit
/// (the connection then cannot carry another request).
fn exchange(
    conn: &mut EspHttpConnection,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<(u16, Vec<u8>, bool), esp_idf_svc::sys::EspError> {
    let limit = mastomini::api::remote::LOOKUP_LIMIT;
    let length = body.len().to_string();
    let mut all: Vec<(&str, &str)> = vec![("User-Agent", "mastomini (household server)")];
    all.extend(headers.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let method = if method == "POST" {
        all.push(("Content-Length", &length));
        Method::Post
    } else {
        Method::Get
    };
    conn.initiate_request(method, url, &all)?;
    if !body.is_empty() {
        conn.write_all(body)?;
    }
    conn.initiate_response()?;
    let status = conn.status();
    let mut body = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = conn.read(&mut chunk)?;
        if n == 0 {
            return Ok((status, body, true));
        }
        if body.len() + n > limit {
            body.extend_from_slice(&chunk[..limit - body.len()]);
            return Ok((status, body, false));
        }
        body.extend_from_slice(&chunk[..n]);
    }
}

impl mastomini::server::Fetch for PublicClient {
    fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        let host = host(url).to_ascii_lowercase();
        let mut reused = matches!(&*kept, Some(Kept(h, _)) if *h == host);
        loop {
            if !reused {
                *kept = None;
                let conn = EspHttpConnection::new(&config())
                    .map_err(|e| format!("public request failed ({})", e.code()))?;
                *kept = Some(Kept(host.clone(), conn));
            }
            let Kept(_, conn) = kept.as_mut().expect("just set");
            match exchange(conn, method, url, headers, body) {
                Ok((status, body, complete)) => {
                    if !complete {
                        *kept = None;
                    }
                    return Ok((status, body));
                }
                // The server may have closed an idle connection: once more,
                // on a new one. Never a POST: it may have arrived.
                Err(_) if reused && method == "GET" => reused = false,
                // Never echo the server's own words into logs.
                Err(e) => {
                    *kept = None;
                    return Err(format!("public request failed ({})", e.code()));
                }
            }
        }
    }
}
