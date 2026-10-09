//! Resumable account setup. Only generated bot credentials are journaled;
//! admin credentials and setup sessions exist only during one worker pass.
use crate::{
    auth::random_hex,
    mastodon::{normalize_instance, HttpClient, HttpRequest},
    service::{ConfigUpdate, Service},
    store::KvStore,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

const IDS: [&str; 7] = [
    "nana_news",
    "trader_1",
    "trader_2",
    "trader_3",
    "trader_4",
    "trader_5",
    "trader_6",
];

#[cfg(test)]
#[path = "enrollment_tests.rs"]
mod tests;
const STRATEGIES: [&str; 7] = [
    "band",
    "lotto",
    "borrow",
    "lend",
    "dca",
    "rebalance",
    "revert",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub nanacoin: String,
    pub mastomini: String,
    pub nana_username: String,
    pub nana_password: String,
    pub admin_username: String,
    pub admin_password: String,
    #[serde(default)]
    pub replace_news_read_key: bool,
    pub bots: Vec<Selection>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub id: String,
    pub username: String,
    pub display_name: String,
    #[serde(default)]
    pub strategy: String,
    #[serde(default)]
    pub initial_grant: bool,
}

#[derive(Clone, Serialize)]
pub struct Row {
    pub id: String,
    pub username: String,
    pub state: &'static str,
}

#[derive(Default)]
pub struct Enrollment {
    pending: Option<Input>,
    pub running: bool,
    pub rows: Vec<Row>,
    pub error: Option<String>,
}

impl Enrollment {
    pub fn is_busy(&self) -> bool {
        self.running || self.pending.is_some()
    }
}

// Short NVS keys, small JSON records, and an intent durable before any remote write.
#[derive(Clone, Serialize, Deserialize)]
struct Draft {
    nanacoin: String,
    mastomini: String,
    username: String,
    password: String,
    key_name: String,
    nc_id: String,
    mm_id: String,
    nc_key: String,
    mm_key: String,
}

fn draft_key(id: &str) -> String {
    format!("e.{id}")
}
fn lock<S: KvStore>(shared: &Arc<Mutex<Service<S>>>) -> std::sync::MutexGuard<'_, Service<S>> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

fn server(text: &str) -> Result<String, String> {
    let url = normalize_instance(text)?;
    if url.starts_with("http://")
        && !["http://127.0.0.1", "http://localhost", "http://[::1]"]
            .iter()
            .any(|base| {
                url.strip_prefix(base)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
            })
    {
        return Err("Use HTTPS for credentials, or HTTP on localhost for development".into());
    }
    Ok(url)
}

pub fn start<S: KvStore>(svc: &mut Service<S>, mut input: Input) -> Result<(), String> {
    if svc.enrollment.is_busy() {
        return Err("Setup is already running".into());
    }
    input.nanacoin = server(&input.nanacoin)?;
    input.mastomini = server(&input.mastomini)?;
    if [
        &input.nana_username,
        &input.nana_password,
        &input.admin_username,
        &input.admin_password,
    ]
    .iter()
    .any(|s| s.is_empty() || s.len() > 128)
    {
        return Err("Enter both administrators' usernames and passwords".into());
    }
    if input.bots.is_empty() || input.bots.len() > 7 {
        return Err("Choose one to seven bots".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut names = std::collections::BTreeSet::new();
    for row in &input.bots {
        if !IDS.contains(&row.id.as_str()) || !ids.insert(&row.id) || svc.index(&row.id).is_none() {
            return Err("Choose each built-in NanaCoin bot at most once".into());
        }
        if row.username.is_empty()
            || row.username.len() > 24
            || !row
                .username
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            || !names.insert(&row.username)
        {
            return Err("Bot usernames must be distinct, with 1–24 lowercase letters, digits or underscores".into());
        }
        if row.display_name.trim().is_empty()
            || row.display_name.len() > 40
            || row.display_name.chars().any(char::is_control)
        {
            return Err("Give each bot a display name of 1–40 UTF-8 bytes".into());
        }
        if row.id != "nana_news" && !STRATEGIES.contains(&row.strategy.as_str()) {
            return Err("Choose a valid trading strategy".into());
        }
        let i = svc.index(&row.id).unwrap();
        if svc.runtime[i].running {
            return Err("Wait for the selected bots to finish running".into());
        }
        if let Some(text) = svc.store.get(&draft_key(&row.id))? {
            let draft: Draft = serde_json::from_str(&text)
                .map_err(|_| "The saved enrollment could not be read")?;
            if draft.nanacoin != input.nanacoin
                || draft.mastomini != input.mastomini
                || draft.username != row.username
            {
                return Err("Resume setup with the original servers and bot usernames".into());
            }
        }
        let record = &svc.records[i];
        if (!record.config.token.is_empty() && record.config.instance != input.mastomini)
            || record.settings.get("nc_key").is_some_and(|s| !s.is_empty())
                && svc.settings(i).get("nc_server") != input.nanacoin
        {
            return Err("A selected bot is already connected to another server".into());
        }
    }
    svc.store.put(
        "e.plan",
        &json!({"nanacoin": input.nanacoin, "mastomini": input.mastomini, "bots": input.bots})
            .to_string(),
    )?;
    svc.enrollment.rows = input
        .bots
        .iter()
        .map(|row| Row {
            id: row.id.clone(),
            username: row.username.clone(),
            state: "pending",
        })
        .collect();
    svc.enrollment.error = None;
    svc.enrollment.pending = Some(input);
    Ok(())
}

pub fn status<S: KvStore>(svc: &Service<S>) -> Value {
    let plan = svc
        .store
        .get("e.plan")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok());
    json!({"running": svc.enrollment.is_busy(), "rows": svc.enrollment.rows, "error": svc.enrollment.error, "plan": plan})
}

fn send(
    http: &mut dyn HttpClient,
    method: &'static str,
    base: &str,
    path: &str,
    token: &str,
    body: Value,
) -> Result<Value, String> {
    let mut headers = vec![("Content-Type".into(), "application/json".into())];
    if !token.is_empty() {
        headers.push(("Authorization".into(), format!("Bearer {token}")));
    }
    let response = http
        .send(&HttpRequest {
            method,
            url: format!("{base}{path}"),
            headers,
            body: if method == "GET" {
                Vec::new()
            } else {
                serde_json::to_vec(&body).unwrap()
            },
        })
        .map_err(|_| format!("{path}: could not reach the server; resume setup to retry"))?;
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "{path}: server answered {}; no credentials were displayed",
            response.status
        ));
    }
    serde_json::from_slice(&response.body).map_err(|_| format!("{path}: invalid JSON response"))
}

fn text(value: &Value, name: &str) -> Result<String, String> {
    value[name]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("The server response is missing {name}"))
}

fn nana_login(
    http: &mut dyn HttpClient,
    base: &str,
    username: &str,
    password: &str,
) -> Result<(String, Value), String> {
    let verifier = random_hex(32);
    let redirect = "mastomini-bots://setup";
    let code = send(
        http,
        "POST",
        base,
        "/api/v1/auth/authorize",
        "",
        json!({"username": username, "password": password, "code_challenge": URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())), "code_challenge_method": "S256", "redirect_uri": redirect}),
    )?;
    let result = send(
        http,
        "POST",
        base,
        "/api/v1/auth/token",
        "",
        json!({"code": text(&code, "code")?, "code_verifier": verifier, "redirect_uri": redirect}),
    )?;
    Ok((text(&result, "access_token")?, result["user"].clone()))
}

fn save<S: KvStore>(
    shared: &Arc<Mutex<Service<S>>>,
    id: &str,
    draft: &Draft,
) -> Result<(), String> {
    lock(shared)
        .store
        .put(&draft_key(id), &serde_json::to_string(draft).unwrap())
        .map_err(|_| "Could not save setup progress; resume after checking storage".into())
}

fn row_state<S: KvStore>(shared: &Arc<Mutex<Service<S>>>, id: &str, state: &'static str) {
    if let Some(row) = lock(shared).enrollment.rows.iter_mut().find(|r| r.id == id) {
        row.state = state;
    }
}

pub fn tick<S: KvStore>(
    shared: &Arc<Mutex<Service<S>>>,
    http: &mut dyn HttpClient,
    now: Option<u64>,
) -> bool {
    let input = {
        let mut svc = lock(shared);
        let Some(input) = svc.enrollment.pending.take() else {
            return false;
        };
        svc.enrollment.running = true;
        input
    };
    let mut nc_token = String::new();
    let mut mm_token = String::new();
    let result = execute(shared, http, &input, now, &mut nc_token, &mut mm_token);
    // Best effort: failed cleanup never undoes completed accounts or grants.
    if !nc_token.is_empty() {
        let _ = send(
            http,
            "POST",
            &input.nanacoin,
            "/api/v1/auth/logout",
            &nc_token,
            json!({}),
        );
    }
    if !mm_token.is_empty() {
        let _ = send(
            http,
            "POST",
            &input.mastomini,
            "/oauth/revoke",
            "",
            json!({"token": mm_token}),
        );
    }
    let mut svc = lock(shared);
    svc.enrollment.running = false;
    if let Err(error) = result {
        for row in &mut svc.enrollment.rows {
            if row.state == "connecting" {
                row.state = "failed";
            }
        }
        svc.enrollment.error = Some(error);
    }
    true
}

fn execute<S: KvStore>(
    shared: &Arc<Mutex<Service<S>>>,
    http: &mut dyn HttpClient,
    input: &Input,
    now: Option<u64>,
    nc_token: &mut String,
    mm_token: &mut String,
) -> Result<(), String> {
    let (token, user) = nana_login(
        http,
        &input.nanacoin,
        &input.nana_username,
        &input.nana_password,
    )?;
    *nc_token = token;
    if user["role"] != "nana" {
        return Err("The NanaCoin login must belong to Nana".into());
    }
    *mm_token = text(
        &send(
            http,
            "POST",
            &input.mastomini,
            "/api/mastomini/v1/admin/bots/login",
            "",
            json!({"username": input.admin_username, "password": input.admin_password}),
        )?,
        "access_token",
    )?;
    let nc_status = send(
        http,
        "GET",
        &input.nanacoin,
        "/api/v1/status",
        nc_token,
        Value::Null,
    )?;
    let nc_users = send(
        http,
        "GET",
        &input.nanacoin,
        "/api/v1/users",
        nc_token,
        Value::Null,
    )?;
    let mm_users = send(
        http,
        "GET",
        &input.mastomini,
        "/api/mastomini/v1/admin/members",
        mm_token,
        Value::Null,
    )?;
    let capacity = send(
        http,
        "GET",
        &input.mastomini,
        "/api/mastomini/v1/admin/bots/capacity",
        mm_token,
        Value::Null,
    )?;
    let nc_rows = nc_users["users"]
        .as_array()
        .ok_or("NanaCoin did not return its member list")?;
    let mm_rows = mm_users
        .as_array()
        .ok_or("Mastomini did not return its member list")?;
    let mut needed_nc = 0;
    let mut needed_mm = 0;
    let mut needed_keys = 0;
    for row in &input.bots {
        let svc = lock(shared);
        let i = svc.index(&row.id).unwrap();
        let record = &svc.records[i];
        let drafted = svc.store.get(&draft_key(&row.id))?.is_some();
        for (exists, need, required) in [
            (
                nc_rows.iter().find(|u| u["username"] == row.username),
                &mut needed_nc,
                row.id != "nana_news" && record.settings.get("nc_key").is_none_or(|k| k.is_empty()),
            ),
            (
                mm_rows.iter().find(|u| u["username"] == row.username),
                &mut needed_mm,
                record.config.token.is_empty(),
            ),
        ] {
            if required {
                if exists.is_some() && !drafted {
                    return Err(format!(
                        "{} already exists; choose another username",
                        row.username
                    ));
                }
                if exists.is_none() {
                    *need += 1;
                }
            }
        }
        if record.config.token.is_empty() && !mm_rows.iter().any(|u| u["username"] == row.username)
        {
            needed_keys += 1;
        }
    }
    let nc_capacity = nc_status["member_capacity"]
        .as_u64()
        .ok_or("Update NanaCoin to expose member_capacity before setup")?;
    if nc_rows.len() as u64 + needed_nc > nc_capacity
        || needed_mm > capacity["accounts_available"].as_u64().unwrap_or(0)
        || needed_keys > capacity["keys_available"].as_u64().unwrap_or(0)
    {
        return Err(
            "Not enough account or API-key slots; free capacity before creating bots".into(),
        );
    }
    let news_needs_key = input.bots.iter().any(|r| r.id == "nana_news") && {
        let svc = lock(shared);
        let i = svc.index("nana_news").unwrap();
        svc.records[i]
            .settings
            .get("nc_key")
            .is_none_or(|s| s.is_empty())
            && svc
                .store
                .get(&draft_key("nana_news"))?
                .map(|s| serde_json::from_str::<Draft>(&s).map(|d| d.nc_key.is_empty()))
                .transpose()
                .map_err(|_| "Invalid saved setup")?
                .unwrap_or(true)
    };
    if news_needs_key {
        let keys = send(
            http,
            "GET",
            &input.nanacoin,
            "/api/v1/me/api-key",
            nc_token,
            Value::Null,
        )?;
        if keys["read"]["active"] == true && !input.replace_news_read_key {
            return Err("Nana already has a read key. Allow replacement explicitly or leave the news bot unselected".into());
        }
    }
    for row in &input.bots {
        row_state(shared, &row.id, "connecting");
        connect(shared, http, input, row, nc_token, mm_token, now)?;
        row_state(shared, &row.id, "connected");
    }
    Ok(())
}

fn connect<S: KvStore>(
    shared: &Arc<Mutex<Service<S>>>,
    http: &mut dyn HttpClient,
    input: &Input,
    row: &Selection,
    nc_token: &str,
    mm_token: &str,
    now: Option<u64>,
) -> Result<(), String> {
    let (mut draft, existing_nc, existing_mm) = {
        let svc = lock(shared);
        let i = svc.index(&row.id).unwrap();
        let record = &svc.records[i];
        let draft = svc
            .store
            .get(&draft_key(&row.id))?
            .map(|s| serde_json::from_str::<Draft>(&s))
            .transpose()
            .map_err(|_| "Invalid saved setup")?
            .unwrap_or_else(|| Draft {
                nanacoin: input.nanacoin.clone(),
                mastomini: input.mastomini.clone(),
                username: row.username.clone(),
                password: random_hex(24),
                key_name: format!("mmb-{}", random_hex(16)),
                nc_id: String::new(),
                mm_id: String::new(),
                nc_key: String::new(),
                mm_key: String::new(),
            });
        (
            draft,
            record.settings.get("nc_key").cloned().unwrap_or_default(),
            record.config.token.clone(),
        )
    };
    save(shared, &row.id, &draft)?;
    if draft.nc_key.is_empty() && existing_nc.is_empty() {
        if row.id == "nana_news" {
            draft.nc_key = text(
                &send(
                    http,
                    "POST",
                    &input.nanacoin,
                    "/api/v1/me/api-key",
                    nc_token,
                    json!({"password": input.nana_password, "scope": "read"}),
                )?,
                "api_key",
            )?;
        } else {
            if draft.nc_id.is_empty() {
                let users = send(
                    http,
                    "GET",
                    &input.nanacoin,
                    "/api/v1/users",
                    nc_token,
                    Value::Null,
                )?;
                if let Some(user) = users["users"]
                    .as_array()
                    .and_then(|users| users.iter().find(|u| u["username"] == row.username))
                {
                    // Recover a successful create whose response was lost, proving ownership.
                    if user["kind"] != "bot" {
                        return Err("The NanaCoin username belongs to a person".into());
                    }
                    let (token, identity) =
                        nana_login(http, &input.nanacoin, &row.username, &draft.password)?;
                    let _ = send(
                        http,
                        "POST",
                        &input.nanacoin,
                        "/api/v1/auth/logout",
                        &token,
                        json!({}),
                    );
                    draft.nc_id = text(&identity, "id")?;
                } else {
                    draft.nc_id = text(
                        &send(
                            http,
                            "POST",
                            &input.nanacoin,
                            "/api/v1/users",
                            nc_token,
                            json!({"username": row.username, "display_name": row.display_name, "password": draft.password, "kind": "bot", "grant": row.initial_grant}),
                        )?,
                        "id",
                    )?;
                }
                save(shared, &row.id, &draft)?;
            }
            let (proof_token, identity) =
                nana_login(http, &input.nanacoin, &row.username, &draft.password)?;
            let _ = send(
                http,
                "POST",
                &input.nanacoin,
                "/api/v1/auth/logout",
                &proof_token,
                json!({}),
            );
            if identity["id"] != draft.nc_id || identity["kind"] != "bot" {
                return Err(
                    "The saved NanaCoin account no longer belongs to this enrollment".into(),
                );
            }
            draft.nc_key = text(
                &send(
                    http,
                    "POST",
                    &input.nanacoin,
                    &format!("/api/v1/users/{}/api-key", draft.nc_id),
                    nc_token,
                    json!({}),
                )?,
                "api_key",
            )?;
        }
        save(shared, &row.id, &draft)?;
    }
    if draft.mm_key.is_empty() && existing_mm.is_empty() {
        if draft.mm_id.is_empty() {
            draft.mm_id = text(
                &send(
                    http,
                    "POST",
                    &input.mastomini,
                    "/api/mastomini/v1/admin/bots",
                    mm_token,
                    json!({"username": row.username, "display_name": row.display_name, "password": draft.password}),
                )?,
                "id",
            )?;
            save(shared, &row.id, &draft)?;
        }
        draft.mm_key = text(
            &send(
                http,
                "POST",
                &input.mastomini,
                &format!("/api/mastomini/v1/admin/bots/{}/api_keys", draft.mm_id),
                mm_token,
                json!({"name": draft.key_name, "password": draft.password}),
            )?,
            "key",
        )?;
        save(shared, &row.id, &draft)?;
    }
    let nc_key = if existing_nc.is_empty() {
        &draft.nc_key
    } else {
        &existing_nc
    };
    let mm_key = if existing_mm.is_empty() {
        &draft.mm_key
    } else {
        &existing_mm
    };
    let nc_me = send(
        http,
        "GET",
        &input.nanacoin,
        "/api/v1/me",
        nc_key,
        Value::Null,
    )?;
    if row.id != "nana_news" && nc_me["kind"] != "bot" {
        return Err("The trading key must belong to a NanaCoin bot".into());
    }
    let mm_me = send(
        http,
        "GET",
        &input.mastomini,
        "/api/v1/accounts/verify_credentials",
        mm_key,
        Value::Null,
    )?;
    if mm_me["bot"] != true {
        return Err("The posting key must belong to a Mastomini bot".into());
    }
    if !existing_nc.is_empty() && !existing_mm.is_empty() {
        return Ok(());
    }
    let mut changes = BTreeMap::from([
        ("nc_server".into(), input.nanacoin.clone()),
        ("nc_key".into(), nc_key.clone()),
    ]);
    if row.id != "nana_news" {
        changes.insert("strategy".into(), row.strategy.clone());
        changes.insert("dry_run".into(), "yes".into());
    }
    let mut svc = lock(shared);
    let i = svc.index(&row.id).unwrap();
    svc.configure(
        i,
        ConfigUpdate {
            instance: Some(input.mastomini.clone()),
            token: Some(mm_key.clone()),
            enabled: Some(false),
            settings: Some(changes),
        },
        now,
    )
    .map_err(|_| "Could not save bot configuration; resume setup to retry".into())
}
