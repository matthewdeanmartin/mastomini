//! `/trust`, `/ca`, `/ca.pem`, and how status and security describe HTTPS.

use super::*;

fn with_tls() -> Server {
    let mut s = Server::provisioned();
    s.ctx.tls = Some(crate::tls::tests::sample());
    s
}

fn text(r: &crate::http::Response) -> String {
    String::from_utf8_lossy(&r.body).into_owned()
}

#[test]
fn without_a_certificate_there_is_nothing_to_trust() {
    let mut s = Server::provisioned();
    for path in ["/ca", "/ca.pem", "/trust"] {
        let r = s.get(path, None);
        assert_eq!(r.status, 404, "{path}");
        assert!(text(&r).contains("plain HTTP only"), "{path}");
    }
    let landing = text(&s.get("/", None));
    assert!(landing.contains("This server uses plain HTTP."));
}

#[test]
fn the_ca_downloads_as_der_and_pem_without_signing_in() {
    let mut s = with_tls();
    let tls = s.ctx.tls.clone().unwrap();
    let r = s.get("/ca", None);
    assert_eq!(r.status, 200);
    assert_eq!(r.body, tls.ca_der);
    assert_eq!(r.header("Content-Type"), Some("application/x-x509-ca-cert"));
    assert!(r
        .header("Content-Disposition")
        .unwrap()
        .contains("mastomini-household-ca.crt"));
    let r = s.get("/ca.pem", None);
    assert_eq!(r.status, 200);
    assert_eq!(text(&r), tls.ca_pem());
    assert_eq!(s.send(Request::new("POST", "/ca")).status, 405);
    // Only these exact paths: no directory under /ca.
    assert_eq!(s.get("/ca/../Cargo.toml", None).status, 404);
}

#[test]
fn the_trust_page_shows_the_fingerprint_and_where_to_go_next() {
    let mut s = with_tls();
    let fingerprint = s.ctx.tls.as_ref().unwrap().ca_fingerprint.clone();
    let r = s.get("/trust?x=1", None);
    assert_eq!(r.status, 200);
    assert!(r
        .header("Content-Security-Policy")
        .unwrap()
        .contains("default-src 'none'"));
    let page = text(&r);
    assert!(page.contains(&fingerprint));
    assert!(page.contains("href=\"/ca\""));
    assert!(page.contains("https://mastomini.local/"));
    assert!(page.contains("never for public websites"));
    assert!(page.contains("until 2028-12-23"));

    let mut secure = Request::new("GET", "/trust");
    secure.secure = true;
    assert!(text(&s.send(secure)).contains("already trusts the board"));

    let landing = text(&s.get("/", None));
    assert!(landing.contains("trust the household certificate first"));
}

#[test]
fn status_and_security_describe_easy_mode() {
    let mut s = with_tls();
    let status = s.get("/api/mastomini/v1/status", None).json_body();
    assert_eq!(status["mode"], "easy");
    assert_eq!(status["https"], true);
    assert_eq!(status["secure"], false);
    assert_eq!(status["https_url"], "https://mastomini.local");
    assert_eq!(
        status["ca_fingerprint"],
        s.ctx.tls.as_ref().unwrap().ca_fingerprint.as_str()
    );

    let alice = s.login("alice", "alicepw", "read write");
    let mut req = Request::new("GET", "/api/mastomini/v1/admin/security")
        .with_header("Authorization", &format!("Bearer {alice}"));
    req.secure = true;
    let sec = s.send(req).json_body();
    assert_eq!(sec["transport"]["mode"], "easy");
    assert_eq!(sec["transport"]["this_connection"], "https");
    assert_eq!(sec["transport"]["secure_mode_available"], false);
    assert_eq!(sec["certificate"]["names"][0], "mastomini.local");
    assert_eq!(
        sec["household_ca"]["fingerprint_sha256"],
        s.ctx.tls.as_ref().unwrap().ca_fingerprint.as_str()
    );
    assert_eq!(sec["household_ca"]["name_constrained"], true);
}

#[test]
fn oauth_discovery_keeps_the_client_on_its_scheme_and_host() {
    let mut s = with_tls();
    let mut req = Request::new("GET", "/.well-known/oauth-authorization-server")
        .with_header("Host", "192.168.1.161");
    req.secure = true;
    let meta = s.send(req).json_body();
    assert_eq!(meta["issuer"], "https://192.168.1.161/");
    assert_eq!(
        meta["authorization_endpoint"],
        "https://192.168.1.161/oauth/authorize"
    );
    // Plain HTTP stays plain; a Host header that isn't a host is ignored.
    let req = Request::new("GET", "/.well-known/oauth-authorization-server")
        .with_header("Host", "evil\"><script>");
    let meta = s.send(req).json_body();
    assert_eq!(meta["token_endpoint"], "http://mastomini.test/oauth/token");
}
