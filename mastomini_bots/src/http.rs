//! Transport-neutral request and response for the admin site, so the same
//! code serves the desktop build (tiny_http) and the board (esp_http_server).

use serde_json::Value;

/// The admin API takes small JSON bodies only.
pub const BODY_LIMIT: usize = 8 * 1024;

/// Request headers the board passes on (esp_http_server cannot list them).
pub const FORWARDED_HEADERS: [&str; 5] = [
    "Host",
    "Authorization",
    "Content-Type",
    "Accept-Encoding",
    "If-None-Match",
];

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Arrived over HTTPS.
    pub secure: bool,
}

impl Request {
    pub fn new(method: &str, url: &str) -> Request {
        let (path, query) = url.split_once('?').unwrap_or((url, ""));
        Request {
            method: method.to_ascii_uppercase(),
            path: path.to_string(),
            query: query.to_string(),
            ..Request::default()
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Request {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn with_json(mut self, body: &Value) -> Request {
        self.headers
            .push(("Content-Type".into(), "application/json".into()));
        self.body = serde_json::to_vec(body).unwrap_or_default();
        self
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn bearer(&self) -> Option<&str> {
        let (scheme, token) = self.header("Authorization")?.split_once(' ')?;
        scheme
            .eq_ignore_ascii_case("bearer")
            .then(|| token.trim())
            .filter(|t| !t.is_empty())
    }

    pub fn query_param(&self, name: &str) -> Option<String> {
        self.query.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (k == name).then(|| {
                percent_encoding::percent_decode_str(&v.replace('+', " "))
                    .decode_utf8_lossy()
                    .into_owned()
            })
        })
    }

    /// The JSON body, or an empty object when there is none.
    pub fn json(&self) -> Result<Value, Response> {
        if self.body.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Object(Default::default()));
        }
        serde_json::from_slice(&self.body)
            .map_err(|e| Response::error(400, &format!("The body is not JSON: {e}")))
    }

    pub fn segments(&self) -> Vec<&str> {
        self.path.split('/').filter(|s| !s.is_empty()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Response {
        Response {
            status,
            headers: vec![("Content-Type".into(), content_type.into())],
            body: body.into(),
        }
    }

    pub fn json(status: u16, value: &Value) -> Response {
        Response::new(
            status,
            "application/json; charset=utf-8",
            serde_json::to_vec(value).unwrap_or_default(),
        )
        .with_header("Cache-Control", "no-store")
    }

    pub fn ok(value: Value) -> Response {
        Response::json(200, &value)
    }

    pub fn error(status: u16, message: &str) -> Response {
        Response::json(status, &serde_json::json!({ "error": message }))
    }

    pub fn redirect(location: &str) -> Response {
        Response::new(302, "text/plain; charset=utf-8", Vec::new())
            .with_header("Location", location)
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Response {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn public_cache(mut self, req: &Request, max_age: u32) -> Response {
        use sha2::{Digest, Sha256};
        if self.status != 200 || !matches!(req.method.as_str(), "GET" | "HEAD") {
            return self;
        }
        let etag = format!("\"{:x}\"", Sha256::digest(&self.body));
        self.headers
            .retain(|(k, _)| !k.eq_ignore_ascii_case("Cache-Control"));
        self = self
            .with_header(
                "Cache-Control",
                &format!("public, max-age={max_age}, must-revalidate"),
            )
            .with_header("ETag", &etag);
        if etag_matches(req.header("If-None-Match"), &etag) {
            self.status = 304;
            self.body.clear();
            self.headers
                .retain(|(k, _)| !k.eq_ignore_ascii_case("Content-Type"));
        }
        self
    }


    pub fn json_body(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// If-None-Match uses weak comparison, including lists and the wildcard.
/// Commas inside a quoted opaque tag are not list separators.
pub fn etag_matches(header: Option<&str>, etag: &str) -> bool {
    let Some(header) = header else {
        return false;
    };
    if header.trim() == "*" {
        return true;
    }
    let mut quoted = false;
    header
        .split(|ch| {
            if ch == '"' {
                quoted = !quoted;
            }
            ch == ',' && !quoted
        })
        .any(|candidate| {
            let candidate = candidate.trim();
            candidate.strip_prefix("W/").unwrap_or(candidate)
                == etag.strip_prefix("W/").unwrap_or(etag)
        })
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[test]
    fn conditional_tags_use_weak_list_comparison() {
        for value in ["\"abc\"", "W/\"abc\"", " \"other\", W/\"abc\" ", "*"] {
            assert!(etag_matches(Some(value), "\"abc\""), "{value}");
        }
        assert!(etag_matches(Some("\"a,b\", \"abc\""), "\"a,b\""));
        assert!(!etag_matches(Some("\"a,b\""), "\"b\""));
        assert!(!etag_matches(Some("\"other\""), "\"abc\""));
        assert!(!etag_matches(None, "\"abc\""));
    }
}
