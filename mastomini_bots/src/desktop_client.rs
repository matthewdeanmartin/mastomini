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
        Self::with_timeout(household_ca, resolve, Duration::from_secs(90))
    }

    pub fn with_timeout(
        household_ca: Option<&[u8]>,
        resolve: &str,
        timeout: Duration,
    ) -> Result<DesktopClient, String> {
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
            .timeout(timeout)
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
            .take(BODY_LIMIT + 1)
            .read_to_end(&mut body)
            .map_err(|e| e.to_string())?;
        if body.len() as u64 > BODY_LIMIT {
            return Err("HTTP response exceeds 1 MiB".into());
        }
        Ok(HttpResponse { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use ureq::Resolver as _;

    #[test]
    fn resolver_honors_overrides_expires_cache_and_forgets_failed_addresses() {
        let client =
            DesktopClient::new(None, " EXAMPLE.test=127.0.0.1,example.test=127.0.0.2 ").unwrap();
        let expected = vec![
            "127.0.0.1:1234".parse().unwrap(),
            "127.0.0.2:1234".parse().unwrap(),
        ];
        assert_eq!(
            client.resolver.resolve("example.test:1234").unwrap(),
            expected
        );
        assert_eq!(
            client.resolver.resolve("EXAMPLE.test:bad").unwrap()[0].port(),
            443
        );
        let resolver = Resolver::default();
        let key = "127.0.0.1:1234";
        let cached = vec!["127.0.0.2:1234".parse().unwrap()];
        resolver
            .cache
            .lock()
            .unwrap()
            .insert(key.into(), (cached.clone(), Instant::now()));
        assert_eq!(resolver.resolve(key).unwrap(), cached);
        resolver.cache.lock().unwrap().get_mut(key).unwrap().1 = Instant::now() - TTL;
        assert_eq!(resolver.resolve(key).unwrap(), vec![key.parse().unwrap()]);
        resolver.forget();
        assert!(resolver.cache.lock().unwrap().is_empty());
        assert!(resolver.resolve("invalid address:123").is_err());
    }

    #[test]
    fn invalid_resolver_and_certificates_are_reported() {
        assert!(DesktopClient::new(None, "missing-ip").is_err());
        assert!(DesktopClient::new(None, "host=bad-ip").is_err());
        assert!(DesktopClient::new(
            Some(b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----"),
            ""
        )
        .is_err());
        assert!(DesktopClient::new(
            Some(b"-----BEGIN CERTIFICATE-----\n!\n-----END CERTIFICATE-----"),
            ""
        )
        .is_err());
    }

    fn serve(status: u16, body: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let headers = String::from_utf8(request.clone()).unwrap();
            let length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|s| s.parse::<usize>().ok())
                })
                .unwrap_or(0);
            let mut payload = vec![0; length];
            socket.read_exact(&mut payload).unwrap();
            request.extend(payload);
            write!(
                socket,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            // An oversize response may cause the client to close the connection early.
            let _ = socket.write_all(&body);
            String::from_utf8(request).unwrap()
        });
        (url, task)
    }

    #[test]
    fn transport_sends_headers_and_body_and_preserves_http_errors() {
        let mut client = DesktopClient::with_timeout(None, "", Duration::from_secs(5)).unwrap();
        for (method, status, payload) in [("GET", 200, ""), ("POST", 503, "{\"test\":true}")] {
            let (url, task) = serve(status, b"answer".to_vec());
            let result = client
                .send(&HttpRequest {
                    method,
                    url,
                    headers: vec![("X-Test".into(), "value".into())],
                    body: payload.as_bytes().to_vec(),
                })
                .unwrap();
            assert_eq!(
                result,
                HttpResponse {
                    status,
                    body: b"answer".to_vec()
                }
            );
            let request = task.join().unwrap();
            assert!(request.starts_with(method));
            assert!(request.to_ascii_lowercase().contains("x-test: value"));
            assert!(request.ends_with(payload));
        }
        let (url, task) = serve(200, vec![b'x'; BODY_LIMIT as usize + 1]);
        assert!(client
            .send(&HttpRequest {
                method: "GET",
                url,
                headers: vec![],
                body: vec![]
            })
            .unwrap_err()
            .contains("1 MiB"));
        task.join().unwrap();
    }

    #[test]
    fn transport_failure_invalidates_dns_cache() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let mut client = DesktopClient::with_timeout(None, "", Duration::from_secs(1)).unwrap();
        client
            .resolver
            .cache
            .lock()
            .unwrap()
            .insert(address.to_string(), (vec![address], Instant::now()));
        assert!(client
            .send(&HttpRequest {
                method: "GET",
                url: format!("http://{address}"),
                headers: vec![],
                body: vec![]
            })
            .is_err());
        assert!(client.resolver.cache.lock().unwrap().is_empty());
    }
}
