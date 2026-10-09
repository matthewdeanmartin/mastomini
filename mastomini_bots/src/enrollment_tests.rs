use super::*;
use crate::{
    api::{self, Ctx},
    http::Request,
    mastodon::HttpResponse,
    store::MemStore,
};
use nanacoin::{
    auth::PasswordVerifier,
    domain::{Command, Error, MemberId, Name},
    journal::{Journal, FRAME_SIZE},
};

const NOW: u64 = 1_800_000_000_000;

#[derive(Default)]
struct MemoryJournal(Vec<[u8; FRAME_SIZE]>);
impl Journal for MemoryJournal {
    fn read(&mut self, i: usize, out: &mut [u8; FRAME_SIZE]) -> Result<bool, Error> {
        match self.0.get(i) {
            Some(row) => {
                *out = *row;
                Ok(true)
            }
            None => Ok(false),
        }
    }
    fn append(&mut self, i: usize, row: &[u8; FRAME_SIZE]) -> Result<(), Error> {
        assert_eq!(i, self.0.len());
        self.0.push(*row);
        Ok(())
    }
}

// Real routers and domain/storage logic, with only sockets replaced. A lost
// response happens AFTER the remote mutation, as it can on a real connection.
struct Servers {
    nc: nanacoin::journal::Service<MemoryJournal>,
    mm: mastomini::domain::Service<mastomini::store::mem::MemStore>,
    fail_after: Option<String>,
    fail_before: Option<(String, u16)>,
    writes: Vec<String>,
    shared: Arc<Mutex<Service<MemStore>>>,
}

impl Servers {
    fn new(shared: Arc<Mutex<Service<MemStore>>>) -> Self {
        let mut nc =
            nanacoin::journal::Service::open_with_clock(MemoryJournal::default(), || NOW / 1000)
                .unwrap();
        nc.execute(
            MemberId(1),
            1,
            Command::Provision {
                household_name: Name::try_from("Setup test").unwrap(),
                username: Name::try_from("nana").unwrap(),
                display_name: Name::try_from("Nana").unwrap(),
                password: PasswordVerifier::hash("nana-secret").unwrap(),
            },
        )
        .unwrap();
        let mut mm = mastomini::domain::Service::open(
            mastomini::store::mem::MemStore::default(),
            mastomini::domain::Config {
                host: "mm.test".into(),
                password_rounds: 1,
            },
        )
        .unwrap();
        mm.provision(
            mastomini::domain::NewMember {
                username: "owner".into(),
                password: "owner-secret".into(),
                display_name: "Owner".into(),
                role: mastomini::domain::records::Role::Owner,
            },
            None,
            NOW,
        )
        .unwrap();
        Self {
            nc,
            mm,
            fail_after: None,
            fail_before: None,
            writes: Vec::new(),
            shared,
        }
    }
}

impl HttpClient for Servers {
    fn send(&mut self, req: &HttpRequest) -> Result<HttpResponse, String> {
        assert!(
            self.shared.try_lock().is_ok(),
            "Network I/O must not hold the admin lock"
        );
        let is_nc = req.url.starts_with("https://nc.test");
        let path = req
            .url
            .strip_prefix(if is_nc {
                "https://nc.test"
            } else {
                "https://mm.test"
            })
            .unwrap();
        if let Some((failure, status)) = &self.fail_before {
            if path == failure {
                return Ok(HttpResponse {
                    status: *status,
                    body: br#"{"error":"nana-secret owner-secret"}"#.to_vec(),
                });
            }
        }
        let response = if is_nc {
            let mut buffer = vec![0; nanacoin::api::RESPONSE_LIMIT];
            let auth = req
                .headers
                .iter()
                .find(|(k, _)| k == "Authorization")
                .map(|(_, v)| v.as_str())
                .unwrap_or("");
            let (status, size) =
                nanacoin::api::handle(&mut self.nc, req.method, path, auth, &req.body, &mut buffer);
            HttpResponse {
                status,
                body: buffer[..size].into(),
            }
        } else {
            let mut request = mastomini::http::Request::new(req.method, path)
                .with_body("application/json", req.body.clone());
            request.headers = req.headers.clone();
            request.secure = true;
            let response = mastomini::api::handle(
                &mut self.mm,
                &mastomini::api::Ctx::new("https://mm.test"),
                &request,
                Some(NOW),
            );
            HttpResponse {
                status: response.status,
                body: response.body,
            }
        };
        if req.method == "POST" && (200..300).contains(&response.status) {
            self.writes.push(path.into());
        }
        let lose_response = self.fail_after.as_deref().is_some_and(|pattern| {
            pattern == path
                || pattern
                    .strip_prefix('*')
                    .is_some_and(|suffix| path.ends_with(suffix))
        });
        if lose_response && (200..300).contains(&response.status) {
            self.fail_after = None;
            return Err("nana-secret owner-secret ambiguous timeout".into());
        }
        Ok(response)
    }
}

fn house() -> Arc<Mutex<Service<MemStore>>> {
    Arc::new(Mutex::new(
        Service::open(MemStore::default(), crate::bots::all()).unwrap(),
    ))
}
fn input() -> Input {
    Input {
        nanacoin: "https://nc.test".into(),
        mastomini: "https://mm.test".into(),
        nana_username: "nana".into(),
        nana_password: "nana-secret".into(),
        admin_username: "owner".into(),
        admin_password: "owner-secret".into(),
        replace_news_read_key: false,
        bots: IDS
            .iter()
            .enumerate()
            .map(|(i, id)| Selection {
                id: (*id).into(),
                username: (*id).into(),
                display_name: (*id).into(),
                strategy: if i == 0 {
                    String::new()
                } else {
                    STRATEGIES[i - 1].into()
                },
                initial_grant: false,
            })
            .collect(),
    }
}
fn setup(shared: &Arc<Mutex<Service<MemStore>>>, servers: &mut Servers, input: Input) {
    start(&mut lock(shared), input).unwrap();
    assert!(tick(shared, servers, Some(NOW)));
}

#[test]
fn all_seven_connect_through_real_apis_and_never_run_or_expose_secrets() {
    let shared = house();
    let mut servers = Servers::new(shared.clone());
    setup(&shared, &mut servers, input());
    let svc = lock(&shared);
    assert!(svc.enrollment.error.is_none(), "{:?}", svc.enrollment.error);
    assert!(svc.enrollment.pending.is_none());
    assert!(svc.enrollment.rows.iter().all(|r| r.state == "connected"));
    assert_eq!(servers.nc.state().members.len(), 7);
    assert_eq!(servers.mm.state.active_accounts().count(), 8);
    for id in IDS {
        let i = svc.index(id).unwrap();
        assert!(!svc.records[i].config.enabled);
        assert!(!svc.records[i].config.token.is_empty());
        assert!(!svc.settings(i).get("nc_key").is_empty());
        if id != "nana_news" {
            assert!(svc.settings(i).yes("dry_run"));
        }
    }
    let public = status(&svc).to_string();
    assert!(!public.contains("secret") && !public.contains("nc_") && !public.contains("password"));
    assert!(svc
        .store
        .map
        .values()
        .all(|v| !v.contains("nana-secret") && !v.contains("owner-secret")));
    assert!(servers.writes.iter().any(|s| s == "/oauth/revoke"));
    assert!(servers.writes.iter().any(|s| s == "/api/v1/auth/logout"));
}

#[test]
fn retries_after_ambiguous_creates_and_key_responses_never_duplicate_accounts_or_grants() {
    for path in [
        "/api/v1/users",
        "/api/v1/users/user-2/api-key",
        "/api/mastomini/v1/admin/bots",
        "*/api_keys",
    ] {
        let shared = house();
        let mut servers = Servers::new(shared.clone());
        let mut request = input();
        request.bots = vec![request.bots.remove(1)];
        request.bots[0].initial_grant = true;
        servers.fail_after = Some(path.into());
        setup(&shared, &mut servers, request);
        assert!(lock(&shared).enrollment.error.is_some());
        assert!(!status(&lock(&shared)).to_string().contains("secret"));
        let store = lock(&shared).store.clone();
        *lock(&shared) = Service::open(store, crate::bots::all()).unwrap();
        let mut retry = input();
        retry.bots = vec![retry.bots.remove(1)];
        setup(&shared, &mut servers, retry);
        assert!(
            lock(&shared).enrollment.error.is_none(),
            "{path}: {:?}",
            lock(&shared).enrollment.error
        );
        assert_eq!(servers.nc.state().members.len(), 2, "{path}");
        assert_eq!(servers.mm.state.active_accounts().count(), 2, "{path}");
        assert_eq!(
            servers
                .writes
                .iter()
                .filter(|p| *p == "/api/v1/users")
                .count(),
            1,
            "grant only on first create"
        );
    }
}

#[test]
fn resuming_a_completed_setup_preserves_all_keys_and_settings() {
    let shared = house();
    let mut servers = Servers::new(shared.clone());
    setup(&shared, &mut servers, input());
    let records = lock(&shared).records.clone();
    let before = servers.writes.len();
    setup(&shared, &mut servers, input());
    assert!(
        lock(&shared).enrollment.error.is_none(),
        "{:?}",
        lock(&shared).enrollment.error
    );
    assert_eq!(records, lock(&shared).records);
    assert!(!servers.writes[before..]
        .iter()
        .any(|p| p.ends_with("api-key")
            || p.ends_with("api_keys")
            || p == "/api/v1/users"
            || p == "/api/mastomini/v1/admin/bots"));
}

#[test]
fn preflight_failures_write_no_accounts_and_redact_remote_errors() {
    for (path, code) in [
        ("/api/v1/auth/authorize", 401),
        ("/api/mastomini/v1/admin/bots/login", 403),
        ("/api/mastomini/v1/admin/bots/capacity", 503),
    ] {
        let shared = house();
        let mut servers = Servers::new(shared.clone());
        servers.fail_before = Some((path.into(), code));
        setup(&shared, &mut servers, input());
        assert!(lock(&shared).enrollment.error.is_some());
        assert!(!status(&lock(&shared)).to_string().contains("secret"));
        assert_eq!(servers.nc.state().members.len(), 1);
        assert_eq!(servers.mm.state.active_accounts().count(), 1);
    }
}

#[test]
fn unrelated_username_collision_is_refused_before_any_account_is_created() {
    let shared = house();
    let mut servers = Servers::new(shared.clone());
    let mut request = input();
    request.bots[0].username = "owner".into();
    setup(&shared, &mut servers, request);
    assert!(lock(&shared)
        .enrollment
        .error
        .as_ref()
        .unwrap()
        .contains("already exists"));
    assert_eq!(servers.nc.state().members.len(), 1);
    assert_eq!(servers.mm.state.active_accounts().count(), 1);
}

#[test]
fn account_capacity_is_checked_before_remote_mutations() {
    let shared = house();
    let mut servers = Servers::new(shared.clone());
    for n in 0..10 {
        servers
            .mm
            .create_member(
                0,
                mastomini::domain::NewMember {
                    username: format!("person{n}"),
                    password: "pass".into(),
                    display_name: "Person".into(),
                    role: mastomini::domain::records::Role::Member,
                },
                NOW,
            )
            .unwrap();
    }
    setup(&shared, &mut servers, input());
    assert!(lock(&shared)
        .enrollment
        .error
        .as_ref()
        .unwrap()
        .contains("capacity"));
    assert_eq!(servers.nc.state().members.len(), 1);
    assert_eq!(servers.mm.state.active_accounts().count(), 11);
}

#[test]
fn news_read_key_replacement_requires_explicit_consent() {
    let shared = house();
    let mut servers = Servers::new(shared.clone());
    let (token, _) = nana_login(&mut servers, "https://nc.test", "nana", "nana-secret").unwrap();
    send(
        &mut servers,
        "POST",
        "https://nc.test",
        "/api/v1/me/api-key",
        &token,
        json!({"password": "nana-secret", "scope": "read"}),
    )
    .unwrap();
    setup(&shared, &mut servers, input());
    assert!(lock(&shared)
        .enrollment
        .error
        .as_ref()
        .unwrap()
        .contains("read key"));
    assert_eq!(servers.nc.state().members.len(), 1);
    let mut consent = input();
    consent.bots.truncate(1);
    consent.replace_news_read_key = true;
    setup(&shared, &mut servers, consent);
    assert!(
        lock(&shared).enrollment.error.is_none(),
        "{:?}",
        lock(&shared).enrollment.error
    );
    assert_eq!(servers.nc.state().members.len(), 1);
}

#[test]
fn setup_input_validation_and_busy_state_make_no_remote_calls() {
    let shared = house();
    for n in 0..10 {
        let mut bad = input();
        match n {
            0 => bad.nanacoin = "http://evil.test".into(),
            1 => bad.bots.clear(),
            2 => bad.bots.push(bad.bots[0].clone()),
            3 => bad.bots[0].username = "Bad Name".into(),
            4 => bad.bots[1].username = bad.bots[0].username.clone(),
            5 => bad.bots[0].display_name.clear(),
            6 => bad.bots[1].strategy = "unknown".into(),
            7 => bad.admin_password.clear(),
            8 => bad.bots[0].id = "llm_post".into(),
            _ => bad.bots[0].display_name = "\n".into(),
        }
        assert!(start(&mut lock(&shared), bad).is_err(), "{n}");
    }
    start(&mut lock(&shared), input()).unwrap();
    assert!(start(&mut lock(&shared), input())
        .unwrap_err()
        .contains("already running"));
    let mut servers = Servers::new(shared.clone());
    tick(&shared, &mut servers, Some(NOW));
    assert!(!tick(&shared, &mut servers, Some(NOW)));
    let mut renamed = input();
    renamed.bots[0].username = "other".into();
    assert!(start(&mut lock(&shared), renamed)
        .unwrap_err()
        .contains("original"));
}

#[test]
fn setup_api_requires_a_session_and_never_returns_credentials() {
    let mut svc = Service::open(MemStore::default(), crate::bots::all()).unwrap();
    let ctx = Ctx {
        platform: api::Platform::default,
        tls: None,
    };
    let path = "/api/v1/integrations/nanacoin/setup";
    assert_eq!(
        api::handle(&mut svc, &ctx, &Request::new("GET", path), Some(NOW), 0).status,
        401
    );
    let token = svc.sessions.start(0);
    let req = |method| {
        Request::new(method, path).with_header("Authorization", &format!("Bearer {token}"))
    };
    assert_eq!(
        api::handle(
            &mut svc,
            &ctx,
            &req("POST").with_json(&json!({"password":"hidden"})),
            Some(NOW),
            0
        )
        .status,
        422
    );
    assert_eq!(
        api::handle(&mut svc, &ctx, &req("DELETE"), Some(NOW), 0).status,
        405
    );
    let result = api::handle(&mut svc, &ctx, &req("GET"), Some(NOW), 0);
    assert_eq!(result.status, 200);
    assert!(result.header("Cache-Control").unwrap().contains("no-store"));
    start(&mut svc, input()).unwrap();
    for (method, target, body) in [
        ("PATCH", "/api/v1/bots/trader_1", json!({"enabled":true})),
        ("POST", "/api/v1/bots/trader_1/run", json!({})),
    ] {
        let request = Request::new(method, target)
            .with_header("Authorization", &format!("Bearer {token}"))
            .with_json(&body);
        assert_eq!(
            api::handle(&mut svc, &ctx, &request, Some(NOW), 0).status,
            409
        );
    }
}
