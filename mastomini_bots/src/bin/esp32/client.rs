//! The board's Mastodon HTTP client: ESP-IDF's `esp_http_client`, used
//! directly because esp-idf-svc's wrapper hides the two options that make
//! repeated requests fast:
//!
//! - `common_name`: connect to an address we resolved ourselves while still
//!   verifying the certificate for the name. `.local` names are resolved
//!   once over mDNS and kept for ten minutes (dropped on the first failure),
//!   not looked up per request.
//! - `save_client_session`: a reconnect resumes the TLS session instead of
//!   a full ~1 s handshake.
//!
//! One client (and so one kept-alive connection) per server. Household
//! names (`.local`, `.lan`, `.home.arpa`, private addresses) are verified
//! against the household CA; everything else against the public CA bundle.

use esp_idf_svc::sys::*;
use mastobots::mastodon::{HttpClient, HttpRequest, HttpResponse};
use std::collections::HashMap;
use std::ffi::{c_char, CString};
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(600);
const BODY_LIMIT: usize = 256 * 1024;
/// Per socket operation; a model can think for tens of seconds.
const TIMEOUT_MS: i32 = 60_000;

/// NUL-terminated PEM of the household CA.
pub struct EspClient {
    household_ca: &'static str,
    clients: HashMap<String, Handle>,
    resolved: HashMap<String, (Ipv4Addr, Instant)>,
}

/// An `esp_http_client` and the strings its config points at.
struct Handle {
    raw: esp_http_client_handle_t,
    _strings: Vec<CString>,
}

// SAFETY: a Handle is only used by the scheduler thread that owns EspClient.
unsafe impl Send for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: created by esp_http_client_init and not yet cleaned up.
        unsafe { esp_http_client_cleanup(self.raw) };
    }
}

fn household(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return ip.is_private() || ip.is_loopback() || ip.is_link_local();
    }
    [".local", ".lan", ".home.arpa", ".internal"]
        .iter()
        .any(|suffix| host.ends_with(suffix))
}

/// scheme, host, port, path+query
fn split(url: &str) -> Option<(&str, &str, Option<u16>, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let (authority, path) = rest.find('/').map_or((rest, "/"), |i| rest.split_at(i));
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.contains(']') || h.ends_with(']') => (h, p.parse().ok()),
        _ => (authority, None),
    };
    Some((scheme, host, port, path))
}

fn method(name: &str) -> esp_http_client_method_t {
    match name {
        "POST" => esp_http_client_method_t_HTTP_METHOD_POST,
        "PUT" => esp_http_client_method_t_HTTP_METHOD_PUT,
        "PATCH" => esp_http_client_method_t_HTTP_METHOD_PATCH,
        "DELETE" => esp_http_client_method_t_HTTP_METHOD_DELETE,
        _ => esp_http_client_method_t_HTTP_METHOD_GET,
    }
}

fn cstr(text: &str) -> Result<CString, String> {
    CString::new(text).map_err(|_| "text with a NUL byte".to_string())
}

impl EspClient {
    pub fn new(household_ca: &'static str) -> EspClient {
        EspClient {
            household_ca,
            clients: HashMap::new(),
            resolved: HashMap::new(),
        }
    }

    /// The address of a `.local` name, over mDNS, cached.
    fn resolve(&mut self, host: &str) -> Result<Ipv4Addr, String> {
        if let Some((ip, at)) = self.resolved.get(host) {
            if at.elapsed() < TTL {
                return Ok(*ip);
            }
        }
        let name = cstr(host.trim_end_matches(".local"))?;
        let mut addr = esp_ip4_addr_t::default();
        // SAFETY: NUL-terminated name, output written by the call.
        esp!(unsafe { mdns_query_a(name.as_ptr(), 2000, &mut addr) })
            .map_err(|e| format!("could not find {host} on the network (mDNS): {e}"))?;
        let ip = Ipv4Addr::from(u32::from_be(addr.addr));
        log::info!("{host} is {ip}");
        self.resolved.insert(host.to_string(), (ip, Instant::now()));
        Ok(ip)
    }

    /// The client for this server, made on first use.
    fn handle(
        &mut self,
        scheme: &str,
        host: &str,
        port: Option<u16>,
    ) -> Result<esp_http_client_handle_t, String> {
        let key = format!("{scheme}://{host}:{port:?}");
        if let Some(h) = self.clients.get(&key) {
            return Ok(h.raw);
        }
        let connect_to = if host.ends_with(".local") {
            self.resolve(host)?.to_string()
        } else {
            host.to_string()
        };
        let port = port.map_or(String::new(), |p| format!(":{p}"));
        let url = cstr(&format!("{scheme}://{connect_to}{port}/"))?;
        let name = cstr(host)?;
        let ca = cstr(self.household_ca.trim_end_matches('\0'))?;
        let mut config = esp_http_client_config_t {
            url: url.as_ptr(),
            timeout_ms: TIMEOUT_MS,
            keep_alive_enable: true,
            disable_auto_redirect: true,
            buffer_size: 2048,
            buffer_size_tx: 2048,
            save_client_session: true,
            ..Default::default()
        };
        if scheme == "https" {
            config.common_name = name.as_ptr();
            if household(host) {
                config.__bindgen_anon_1.cert_pem = ca.as_ptr();
            } else {
                config.crt_bundle_attach = Some(esp_crt_bundle_attach);
            }
        }
        // SAFETY: the config's strings live in the Handle with the client.
        let raw = unsafe { esp_http_client_init(&config) };
        if raw.is_null() {
            return Err("could not create an HTTP client (out of memory?)".into());
        }
        self.clients.insert(
            key,
            Handle {
                raw,
                _strings: vec![url, name, ca],
            },
        );
        Ok(raw)
    }

    fn forget(&mut self, scheme: &str, host: &str, port: Option<u16>) {
        self.clients.remove(&format!("{scheme}://{host}:{port:?}"));
        self.resolved.remove(host);
    }

    fn exchange(&mut self, req: &HttpRequest) -> Result<HttpResponse, String> {
        let (scheme, host, port, path) = split(&req.url).ok_or("not a URL")?;
        let h = self.handle(scheme, host, port)?;
        let connect_to = match self.resolved.get(host) {
            Some((ip, _)) => ip.to_string(),
            None => host.to_string(),
        };
        let port_text = port.map_or(String::new(), |p| format!(":{p}"));
        let url = cstr(&format!("{scheme}://{connect_to}{port_text}{path}"))?;
        // SAFETY: h is a live client; every string outlives the calls that
        // read it (esp_http_client copies URL and headers).
        unsafe {
            esp!(esp_http_client_set_url(h, url.as_ptr())).map_err(|e| e.to_string())?;
            esp!(esp_http_client_set_method(h, method(req.method))).map_err(|e| e.to_string())?;
            // The server sees the name, not the address we connected to.
            let host_header = cstr(&format!("{host}{port_text}"))?;
            esp_http_client_set_header(h, c"Host".as_ptr(), host_header.as_ptr());
            for (name, value) in &req.headers {
                let (name, value) = (cstr(name)?, cstr(value)?);
                esp_http_client_set_header(h, name.as_ptr(), value.as_ptr());
            }
            esp!(esp_http_client_open(h, req.body.len() as i32))
                .map_err(|e| format!("connect: {e}"))?;
            let mut written = 0;
            while written < req.body.len() {
                let n = esp_http_client_write(
                    h,
                    req.body[written..].as_ptr() as *const c_char,
                    (req.body.len() - written) as i32,
                );
                if n <= 0 {
                    return Err("sending the request failed".into());
                }
                written += n as usize;
            }
            if esp_http_client_fetch_headers(h) < 0 {
                return Err("no response".into());
            }
            let status = esp_http_client_get_status_code(h) as u16;
            let mut body = Vec::new();
            let mut chunk = [0u8; 1024];
            loop {
                let n =
                    esp_http_client_read(h, chunk.as_mut_ptr() as *mut c_char, chunk.len() as i32);
                if n < 0 {
                    return Err("reading the response failed".into());
                }
                if n == 0 {
                    break;
                }
                if body.len() + n as usize > BODY_LIMIT {
                    return Err("the response is too large".into());
                }
                body.extend_from_slice(&chunk[..n as usize]);
            }
            // The connection stays open for the next request to this server.
            Ok(HttpResponse { status, body })
        }
    }
}

impl HttpClient for EspClient {
    fn send(&mut self, req: &HttpRequest) -> Result<HttpResponse, String> {
        let started = Instant::now();
        let result = self.exchange(req);
        if result.is_err() {
            // A fresh connection (and name lookup) next time.
            if let Some((scheme, host, port, _)) = split(&req.url) {
                self.forget(scheme, host, port);
            }
        }
        log::info!(
            "http {} {} -> {} in {} ms",
            req.method,
            req.url.split('?').next().unwrap_or(""),
            result
                .as_ref()
                .map_or_else(|e| e.clone(), |r| r.status.to_string()),
            started.elapsed().as_millis()
        );
        result
    }
}
