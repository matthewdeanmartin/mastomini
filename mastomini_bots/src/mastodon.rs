//! The part of the Mastodon API bots use, over any [`HttpClient`]: the
//! board's (esp_http_client), the desktop's (ureq) or a test double.
//!
//! Every post carries an `Idempotency-Key` derived from the bot and its
//! slot, so a retry after a timeout never posts twice: mastomini and
//! Mastodon both return the first post for a repeated key.

use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Sends one request. Implementations keep connections (and TLS sessions)
/// open between calls to the same server, and resolve names once.
pub trait HttpClient: Send {
    fn send(&mut self, req: &HttpRequest) -> Result<HttpResponse, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MastodonError {
    /// `None`: the request never got an HTTP answer (network, TLS, timeout).
    pub status: Option<u16>,
    pub message: String,
}

impl MastodonError {
    /// Worth trying again later: no answer, rate limited, or a server error.
    /// A rejected token or post is not.
    pub fn retryable(&self) -> bool {
        match self.status {
            None => true,
            Some(s) => s == 429 || s >= 500,
        }
    }
}

impl std::fmt::Display for MastodonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(s) => write!(f, "HTTP {s}: {}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

/// `https://mastomini.local` from what an admin types: a bare host gets
/// https, a trailing slash goes. Only http and https.
pub fn normalize_instance(text: &str) -> Result<String, String> {
    let text = text.trim().trim_end_matches('/');
    if text.is_empty() {
        return Err("Enter the server's address, e.g. mastomini.local".into());
    }
    let url = if text.contains("://") {
        text.to_string()
    } else {
        format!("https://{text}")
    };
    let (scheme, rest) = url.split_once("://").unwrap_or(("", ""));
    let host = rest.split('/').next().unwrap_or("");
    // name[:port] or [v6][:port]
    let (name, port) = match host.strip_prefix('[') {
        Some(v6) => match v6.split_once(']') {
            Some((addr, tail)) => (addr, tail.strip_prefix(':')),
            None => ("", None),
        },
        None => match host.split_once(':') {
            Some((name, port)) => (name, Some(port)),
            None => (host, None),
        },
    };
    let name_ok = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b))
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric());
    let port_ok =
        port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()));
    if !matches!(scheme, "http" | "https") || rest.contains('/') || !name_ok || !port_ok {
        return Err(format!("Not a server address: {text}"));
    }
    Ok(format!("{scheme}://{}", host.to_ascii_lowercase()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Unlisted,
    Private,
}

impl Visibility {
    fn as_str(self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Unlisted => "unlisted",
            Visibility::Private => "private",
        }
    }
}

impl Visibility {
    pub fn parse(text: &str) -> Visibility {
        match text {
            "unlisted" => Visibility::Unlisted,
            "private" => Visibility::Private,
            _ => Visibility::Public,
        }
    }

    /// A reply is no more public than what it answers.
    pub fn at_most(self, other: Visibility) -> Visibility {
        let rank = |v: Visibility| v as u8;
        if rank(other) > rank(self) {
            other
        } else {
            self
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewStatus {
    pub text: String,
    pub visibility: Visibility,
    pub spoiler_text: Option<String>,
    pub in_reply_to_id: Option<String>,
}

/// An account, as bots need it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    /// `name` on its own server, `name@host` elsewhere.
    pub acct: String,
    pub display_name: String,
    /// The bio, as plain text.
    pub note: String,
    pub bot: bool,
}

impl Account {
    fn from(v: &Value) -> Account {
        Account {
            id: v["id"].as_str().unwrap_or("").to_string(),
            acct: v["acct"].as_str().unwrap_or("").to_string(),
            display_name: v["display_name"].as_str().unwrap_or("").to_string(),
            note: crate::text::html_to_text(v["note"].as_str().unwrap_or("")),
            bot: v["bot"].as_bool().unwrap_or(false),
        }
    }

    pub fn name(&self) -> &str {
        if self.display_name.is_empty() {
            &self.acct
        } else {
            &self.display_name
        }
    }
}

/// A post, as plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub id: String,
    pub account: Account,
    pub text: String,
    pub spoiler_text: String,
    /// `public`, `unlisted`, `private` or `direct`.
    pub visibility: String,
    pub in_reply_to_id: Option<String>,
    /// accts mentioned.
    pub mentions: Vec<String>,
}

impl Status {
    fn from(v: &Value) -> Status {
        Status {
            id: v["id"].as_str().unwrap_or("").to_string(),
            account: Account::from(&v["account"]),
            text: crate::text::html_to_text(v["content"].as_str().unwrap_or("")),
            spoiler_text: v["spoiler_text"].as_str().unwrap_or("").to_string(),
            visibility: v["visibility"].as_str().unwrap_or("public").to_string(),
            in_reply_to_id: v["in_reply_to_id"].as_str().map(str::to_string),
            mentions: v["mentions"]
                .as_array()
                .map(|m| {
                    m.iter()
                        .filter_map(|x| x["acct"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// `@alice: text`, the line format prompts use.
    pub fn line(&self) -> String {
        let text = self.text.replace(char::from(10), " ");
        match self.spoiler_text.as_str() {
            "" => format!("@{}: {text}", self.account.acct),
            cw => format!("@{}: [{cw}] {text}", self.account.acct),
        }
    }
}

/// A mention of the bot: the notification id (the cursor) and the post.
#[derive(Debug, Clone)]
pub struct Mention {
    pub id: String,
    pub status: Status,
}

/// Mastodon ids are numeric strings; compare them as numbers.
pub fn id_order(a: &str, b: &str) -> std::cmp::Ordering {
    (a.len(), a).cmp(&(b.len(), b))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posted {
    pub id: String,
    pub url: String,
}

pub struct Mastodon<'a> {
    http: &'a mut dyn HttpClient,
    instance: String,
    token: String,
}

impl<'a> Mastodon<'a> {
    pub fn new(http: &'a mut dyn HttpClient, instance: &str, token: &str) -> Mastodon<'a> {
        Mastodon {
            http,
            instance: instance.trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }

    pub fn instance(&self) -> &str {
        &self.instance
    }

    fn call(
        &mut self,
        method: &'static str,
        path: &str,
        body: Option<&Value>,
        idempotency_key: Option<&str>,
    ) -> Result<Value, MastodonError> {
        let mut headers = vec![
            (
                "Authorization".to_string(),
                format!("Bearer {}", self.token),
            ),
            ("Accept".to_string(), "application/json".to_string()),
            (
                "User-Agent".to_string(),
                concat!("mastomini-bots/", env!("CARGO_PKG_VERSION")).to_string(),
            ),
        ];
        if body.is_some() {
            headers.push(("Content-Type".into(), "application/json".into()));
        }
        if let Some(key) = idempotency_key {
            headers.push(("Idempotency-Key".into(), key.into()));
        }
        let req = HttpRequest {
            method,
            url: format!("{}{path}", self.instance),
            headers,
            body: body
                .map(|b| serde_json::to_vec(b).unwrap_or_default())
                .unwrap_or_default(),
        };
        let res = self.http.send(&req).map_err(|message| MastodonError {
            status: None,
            message,
        })?;
        let value: Value = serde_json::from_slice(&res.body).unwrap_or(Value::Null);
        if (200..300).contains(&res.status) {
            return Ok(value);
        }
        let message = value["error"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                String::from_utf8_lossy(&res.body)
                    .chars()
                    .take(200)
                    .collect()
            });
        Err(MastodonError {
            status: Some(res.status),
            message,
        })
    }

    /// The account the token belongs to: `@name`.
    pub fn verify_credentials(&mut self) -> Result<String, MastodonError> {
        Ok(format!("@{}", self.me()?.acct))
    }

    pub fn me(&mut self) -> Result<Account, MastodonError> {
        let me = self.call("GET", "/api/v1/accounts/verify_credentials", None, None)?;
        Ok(Account::from(&me))
    }

    /// The server's post length limit (mastomini: 140; Mastodon: 500).
    pub fn max_characters(&mut self) -> Result<usize, MastodonError> {
        let v = self.call("GET", "/api/v2/instance", None, None)?;
        Ok(v["configuration"]["statuses"]["max_characters"]
            .as_u64()
            .unwrap_or(500) as usize)
    }

    /// Mentions after the notification `after` (all recent ones if `None`),
    /// oldest first, at most `limit`.
    pub fn mentions(
        &mut self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Mention>, MastodonError> {
        let mut path = format!("/api/v1/notifications?types%5B%5D=mention&limit={limit}");
        if let Some(after) = after {
            path.push_str(&format!("&min_id={after}"));
        }
        let v = self.call("GET", &path, None, None)?;
        let mut out: Vec<Mention> = v
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|n| n["type"] == "mention" && n["status"].is_object())
                    .map(|n| Mention {
                        id: n["id"].as_str().unwrap_or("").to_string(),
                        status: Status::from(&n["status"]),
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| id_order(&a.id, &b.id));
        Ok(out)
    }

    /// The posts above `id` in its thread, oldest first.
    pub fn ancestors(&mut self, id: &str) -> Result<Vec<Status>, MastodonError> {
        let v = self.call("GET", &format!("/api/v1/statuses/{id}/context"), None, None)?;
        Ok(v["ancestors"]
            .as_array()
            .map(|a| a.iter().map(Status::from).collect())
            .unwrap_or_default())
    }

    /// An account's latest posts (not replies, not boosts), newest first.
    pub fn recent_posts(
        &mut self,
        account_id: &str,
        limit: usize,
    ) -> Result<Vec<Status>, MastodonError> {
        let path = format!("/api/v1/accounts/{account_id}/statuses?limit={limit}&exclude_replies=true&exclude_reblogs=true");
        let v = self.call("GET", &path, None, None)?;
        Ok(v.as_array()
            .map(|a| a.iter().map(Status::from).collect())
            .unwrap_or_default())
    }

    pub fn post_status(
        &mut self,
        status: &NewStatus,
        idempotency_key: &str,
    ) -> Result<Posted, MastodonError> {
        let mut body = json!({
            "status": status.text,
            "visibility": status.visibility.as_str(),
        });
        if let Some(cw) = &status.spoiler_text {
            body["spoiler_text"] = json!(cw);
        }
        if let Some(parent) = &status.in_reply_to_id {
            body["in_reply_to_id"] = json!(parent);
        }
        let posted = self.call(
            "POST",
            "/api/v1/statuses",
            Some(&body),
            Some(idempotency_key),
        )?;
        Ok(Posted {
            id: posted["id"].as_str().unwrap_or("").to_string(),
            url: posted["url"].as_str().unwrap_or("").to_string(),
        })
    }
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Records requests and answers from a script.
    #[derive(Clone, Default)]
    pub struct Scripted {
        pub sent: Arc<Mutex<Vec<HttpRequest>>>,
        pub answers: Arc<Mutex<Vec<Result<HttpResponse, String>>>>,
    }

    impl Scripted {
        pub fn answer(&self, status: u16, body: Value) {
            self.answers.lock().unwrap().push(Ok(HttpResponse {
                status,
                body: serde_json::to_vec(&body).unwrap(),
            }));
        }
        pub fn fail(&self, message: &str) {
            self.answers.lock().unwrap().push(Err(message.into()));
        }
    }

    impl HttpClient for Scripted {
        fn send(&mut self, req: &HttpRequest) -> Result<HttpResponse, String> {
            self.sent.lock().unwrap().push(req.clone());
            let mut answers = self.answers.lock().unwrap();
            if answers.is_empty() {
                return Err("no scripted answer".into());
            }
            answers.remove(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Scripted;
    use super::*;

    #[test]
    fn instances_are_normalized() {
        assert_eq!(
            normalize_instance("mastomini.local").unwrap(),
            "https://mastomini.local"
        );
        assert_eq!(
            normalize_instance(" https://Mastodon.Social/ ").unwrap(),
            "https://mastodon.social"
        );
        assert_eq!(
            normalize_instance("http://127.0.0.1:8080").unwrap(),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            normalize_instance("https://[fe80::1]:8443").unwrap(),
            "https://[fe80::1]:8443"
        );
        for bad in [
            "",
            "ftp://x",
            "https://x/path",
            "https://",
            "a b",
            "https://x:",
            "https://x:99a",
            "https://-x",
        ] {
            assert!(normalize_instance(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn posts_json_with_an_idempotency_key() {
        let mut http = Scripted::default();
        http.answer(
            200,
            json!({"id": "7", "url": "https://mastomini.local/@bot/7"}),
        );
        let sent = http.sent.clone();
        let mut m = Mastodon::new(&mut http, "https://mastomini.local", "KEY");
        let status = NewStatus {
            text: "Good morning".into(),
            visibility: Visibility::Public,
            spoiler_text: None,
            in_reply_to_id: None,
        };
        let posted = m.post_status(&status, "good_morning-1").unwrap();
        assert_eq!(posted.url, "https://mastomini.local/@bot/7");
        let req = &sent.lock().unwrap()[0];
        assert_eq!(req.method, "POST");
        assert_eq!(req.url, "https://mastomini.local/api/v1/statuses");
        assert!(req
            .headers
            .contains(&("Authorization".into(), "Bearer KEY".into())));
        assert!(req
            .headers
            .contains(&("Idempotency-Key".into(), "good_morning-1".into())));
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(
            body,
            json!({"status": "Good morning", "visibility": "public"})
        );
    }

    #[test]
    fn errors_say_whether_to_retry() {
        let mut http = Scripted::default();
        http.answer(401, json!({"error": "The access token is invalid"}));
        http.answer(503, json!({"error": "busy"}));
        http.fail("connection refused");
        let mut m = Mastodon::new(&mut http, "https://x", "K");
        let e = m.verify_credentials().unwrap_err();
        assert_eq!(e.to_string(), "HTTP 401: The access token is invalid");
        assert!(!e.retryable());
        assert!(m.verify_credentials().unwrap_err().retryable());
        let e = m.verify_credentials().unwrap_err();
        assert!(e.retryable() && e.status.is_none());
    }
}
