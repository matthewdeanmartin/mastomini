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

    pub fn json_body(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}
