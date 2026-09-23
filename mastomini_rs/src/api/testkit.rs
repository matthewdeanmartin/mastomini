//! In-process API test harness: requests go through [`super::handle`] exactly
//! as they do from the desktop server, and sign-in uses the real OAuth code
//! flow.

use super::{handle, Ctx};
use crate::auth;
use crate::domain::{Config, Service};
use crate::http::{encode, Request, Response};
use crate::store::mem::MemStore;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::Value;

pub const T0: u64 = 1_790_000_000_000;
pub const REDIRECT: &str = "mastomini-test://oauth";

pub struct Server {
    pub svc: Service<MemStore>,
    pub ctx: Ctx,
    pub now: u64,
}

pub fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

impl Server {
    pub fn new() -> Server {
        let ctx = Ctx::new("http://mastomini.test");
        let config = Config {
            host: ctx.host().to_string(),
            password_rounds: 1,
        };
        Server {
            svc: Service::open(MemStore::default(), config).unwrap(),
            ctx,
            now: T0,
        }
    }

    /// Provisioned with owner `alice` (password `alicepw`).
    pub fn provisioned() -> Server {
        let mut s = Server::new();
        let r = s.post_form(
            "/api/mastomini/v1/provision",
            None,
            &[
                ("username", "alice"),
                ("password", "alicepw"),
                ("title", "Home"),
            ],
        );
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        s
    }

    /// Simulate a reboot: RAM is rebuilt from the store.
    pub fn restart(self) -> Server {
        let config = self.svc.config.clone();
        Server {
            svc: Service::open(self.svc.into_store(), config).unwrap(),
            ctx: self.ctx,
            now: self.now,
        }
    }

    pub fn send(&mut self, req: Request) -> Response {
        self.now += 1;
        handle(&mut self.svc, &self.ctx, &req, Some(self.now))
    }

    fn authed(req: Request, token: Option<&str>) -> Request {
        match token {
            Some(t) => req.with_header("Authorization", &format!("Bearer {t}")),
            None => req,
        }
    }

    pub fn get(&mut self, url: &str, token: Option<&str>) -> Response {
        self.send(Self::authed(Request::new("GET", url), token))
    }

    pub fn post_form(
        &mut self,
        url: &str,
        token: Option<&str>,
        pairs: &[(&str, &str)],
    ) -> Response {
        let req =
            Request::new("POST", url).with_body("application/x-www-form-urlencoded", form(pairs));
        self.send(Self::authed(req, token))
    }

    pub fn send_json(
        &mut self,
        method: &str,
        url: &str,
        token: Option<&str>,
        body: &Value,
    ) -> Response {
        let req = Request::new(method, url).with_body("application/json", body.to_string());
        self.send(Self::authed(req, token))
    }

    pub fn register_app(&mut self, scopes: &str) -> (String, String) {
        let r = self.post_form(
            "/api/v1/apps",
            None,
            &[
                ("client_name", "Test"),
                ("redirect_uris", REDIRECT),
                ("scopes", scopes),
            ],
        );
        assert_eq!(r.status, 200);
        let v = r.json_body();
        (
            v["client_id"].as_str().unwrap().to_string(),
            v["client_secret"].as_str().unwrap().to_string(),
        )
    }

    /// Full code flow with PKCE. Returns the access token.
    pub fn login(&mut self, username: &str, password: &str, scopes: &str) -> String {
        let (client_id, _) = self.register_app(scopes);
        let verifier = auth::random_secret();
        let challenge = URL_SAFE_NO_PAD.encode(auth::sha256(&verifier));
        let r = self.post_form(
            "/oauth/authorize",
            None,
            &[
                ("response_type", "code"),
                ("client_id", &client_id),
                ("redirect_uri", REDIRECT),
                ("scope", scopes),
                ("state", "xyz"),
                ("code_challenge", &challenge),
                ("code_challenge_method", "S256"),
                ("username", username),
                ("password", password),
                ("decision", "approve"),
            ],
        );
        assert_eq!(r.status, 302, "{}", String::from_utf8_lossy(&r.body));
        let location = r.header("Location").unwrap().to_string();
        assert!(location.contains("state=xyz"));
        let code = location
            .split(['?', '&'])
            .find_map(|kv| kv.strip_prefix("code="))
            .unwrap()
            .to_string();
        let r = self.post_form(
            "/oauth/token",
            None,
            &[
                ("grant_type", "authorization_code"),
                ("client_id", &client_id),
                ("code", &code),
                ("redirect_uri", REDIRECT),
                ("code_verifier", &verifier),
            ],
        );
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        r.json_body()["access_token"].as_str().unwrap().to_string()
    }

    /// Owner token plus a member created through the household API.
    pub fn add_member(&mut self, owner_token: &str, username: &str) -> String {
        let r = self.post_form(
            "/api/mastomini/v1/admin/members",
            Some(owner_token),
            &[("username", username), ("password", "secret")],
        );
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        self.login(username, "secret", "read write follow")
    }
}
