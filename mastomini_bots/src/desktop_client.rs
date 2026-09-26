//! The desktop's Mastodon HTTP client: ureq over rustls, trusting the
//! public web roots plus the household CA (so `https://mastomini.local`
//! verifies), reusing connections, and resolving each host once.
//!
//! Resolving `mastomini.local` through Windows' mDNS takes seconds to tens
//! of seconds, per request if nothing caches it. Here a name is looked up
//! once and kept for ten minutes (dropped at the first connection failure),
//! and `MASTOBOTS_RESOLVE=mastomini.local=192.168.1.161` skips the lookup
//! entirely, like curl's `--resolve`. TLS still verifies the name.

use crate::mastodon::{HttpClient, HttpRequest, HttpResponse};
use std::collections::HashMap;
use std::io::Read;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(600);
const BODY_LIMIT: u64 = 1024 * 1024;

/// netloc -> (addresses, when looked up)
type Cache = Arc<Mutex<HashMap<String, (Vec<SocketAddr>, Instant)>>>;

#[derive(Clone, Default)]
struct Resolver {
    fixed: HashMap<String, Vec<std::net::IpAddr>>,
    cache: Cache,
}

impl Resolver {
    fn forget(&self) {
        self.cache.lock().unwrap().clear();
    }
}

impl ureq::Resolver for Resolver {
    fn resolve(&self, netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
        let (host, port) = netloc.rsplit_once(':').unwrap_or((netloc, "443"));
        let port: u16 = port.parse().unwrap_or(443);
        if let Some(ips) = self.fixed.get(&host.to_ascii_lowercase()) {
            return Ok(ips.iter().map(|ip| SocketAddr::new(*ip, port)).collect());
        }
        if let Some((addrs, at)) = self.cache.lock().unwrap().get(netloc) {
            if at.elapsed() < TTL {
                return Ok(addrs.clone());
            }
        }
        let addrs: Vec<SocketAddr> = netloc.to_socket_addrs()?.collect();
        self.cache
            .lock()
            .unwrap()
            .insert(netloc.to_string(), (addrs.clone(), Instant::now()));
        Ok(addrs)
    }
}

pub struct DesktopClient {
    agent: ureq::Agent,
    resolver: Resolver,
}

impl DesktopClient {
    /// `household_ca`: PEM of the household CA, if there is one.
    /// `resolve`: `name=ip[,name=ip...]`.
    pub fn new(household_ca: Option<&[u8]>, resolve: &str) -> Result<DesktopClient, String> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        if let Some(pem) = household_ca {
            for cert in rustls_pemfile::certs(&mut &pem[..]) {
                let cert = cert.map_err(|e| format!("household CA: {e}"))?;
                roots.add(cert).map_err(|e| format!("household CA: {e}"))?;
            }
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let mut fixed = HashMap::new();
        for pair in resolve.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (name, ip) = pair
                .split_once('=')
                .ok_or_else(|| format!("MASTOBOTS_RESOLVE: expected name=ip, got {pair}"))?;
            let ip = ip
                .parse()
                .map_err(|_| format!("MASTOBOTS_RESOLVE: {ip} is not an IP address"))?;
            fixed
                .entry(name.to_ascii_lowercase())
                .or_insert_with(Vec::new)
                .push(ip);
        }
        let resolver = Resolver {
            fixed,
            ..Resolver::default()
        };
        let agent = ureq::AgentBuilder::new()
            .tls_config(Arc::new(tls))
            .resolver(resolver.clone())
            .timeout_connect(Duration::from_secs(10))
            // Model answers can take tens of seconds.
            .timeout(Duration::from_secs(90))
            .redirects(0)
            .build();
        Ok(DesktopClient { agent, resolver })
    }
}

impl HttpClient for DesktopClient {
    fn send(&mut self, req: &HttpRequest) -> Result<HttpResponse, String> {
        let mut call = self.agent.request(req.method, &req.url);
        for (name, value) in &req.headers {
            call = call.set(name, value);
        }
        let result = if req.body.is_empty() && req.method == "GET" {
            call.call()
        } else {
            call.send_bytes(&req.body)
        };
        let response = match result {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(ureq::Error::Transport(t)) => {
                // The address may have changed (DHCP): look it up again next time.
                self.resolver.forget();
                return Err(t.to_string());
            }
        };
        let status = response.status();
        let mut body = Vec::new();
        response
            .into_reader()
            .take(BODY_LIMIT)
            .read_to_end(&mut body)
            .map_err(|e| e.to_string())?;
        Ok(HttpResponse { status, body })
    }
}
