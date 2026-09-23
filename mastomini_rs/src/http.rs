//! Transport-neutral HTTP request/response types and parameter parsing.
//!
//! Mastodon clients send parameters as query strings, form bodies, JSON
//! bodies or multipart forms, often interchangeably. [`Params`] flattens all
//! of them into Rails-style names (`source[privacy]`, `id[]`,
//! `fields_attributes[0][name]`) so handlers read them one way.

use serde_json::Value;

/// Bodies larger than this are refused before parsing (spec/03).
pub const BODY_LIMIT: usize = 4 * 1024;
/// `update_credentials` may carry avatar/header images.
pub const UPLOAD_BODY_LIMIT: usize = 160 * 1024;

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
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

    pub fn with_body(mut self, content_type: &str, body: impl Into<Vec<u8>>) -> Request {
        self.headers
            .push(("Content-Type".to_string(), content_type.to_string()));
        self.body = body.into();
        self
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn bearer(&self) -> Option<&str> {
        let value = self.header("Authorization")?;
        let (scheme, token) = value.split_once(' ')?;
        scheme
            .eq_ignore_ascii_case("bearer")
            .then(|| token.trim())
            .filter(|t| !t.is_empty())
    }

    /// Path segments, percent-decoding not needed for our routes.
    pub fn segments(&self) -> Vec<&str> {
        self.path
            .trim_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect()
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
    }

    pub fn ok(value: Value) -> Response {
        Response::json(200, &value)
    }

    pub fn html(status: u16, body: String) -> Response {
        Response::new(status, "text/html; charset=utf-8", body)
    }

    pub fn error(status: u16, message: &str) -> Response {
        Response::json(status, &serde_json::json!({ "error": message }))
    }

    pub fn redirect(location: &str) -> Response {
        Response::new(302, "text/html; charset=utf-8", Vec::new()).with_header("Location", location)
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

/// An uploaded file from a multipart body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upload {
    pub name: String,
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Params {
    pairs: Vec<(String, String)>,
    pub files: Vec<Upload>,
}

impl Params {
    pub fn parse(req: &Request) -> Result<Params, String> {
        let mut params = Params::default();
        params.add_urlencoded(&req.query);
        if req.body.is_empty() {
            return Ok(params);
        }
        let content_type = req
            .header("Content-Type")
            .unwrap_or("")
            .to_ascii_lowercase();
        if content_type.starts_with("application/json") {
            let value: Value =
                serde_json::from_slice(&req.body).map_err(|e| format!("Invalid JSON: {e}"))?;
            params.flatten("", &value);
        } else if content_type.starts_with("multipart/form-data") {
            let boundary = req
                .header("Content-Type")
                .and_then(|ct| {
                    ct.split(';')
                        .find_map(|p| p.trim().strip_prefix("boundary="))
                })
                .map(|b| b.trim_matches('"').to_string())
                .ok_or("Missing multipart boundary")?;
            params.add_multipart(&req.body, &boundary)?;
        } else {
            // Forms, and clients that omit the content type.
            params.add_urlencoded(&String::from_utf8_lossy(&req.body));
        }
        Ok(params)
    }

    fn add_urlencoded(&mut self, text: &str) {
        for (k, v) in form_urlencoded::parse(text.as_bytes()) {
            self.pairs.push((k.into_owned(), v.into_owned()));
        }
    }

    fn flatten(&mut self, prefix: &str, value: &Value) {
        match value {
            Value::Object(map) => {
                for (k, v) in map {
                    let name = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}[{k}]")
                    };
                    self.flatten(&name, v);
                }
            }
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    match item {
                        Value::Object(_) | Value::Array(_) => {
                            self.flatten(&format!("{prefix}[{i}]"), item)
                        }
                        _ => self.flatten(&format!("{prefix}[]"), item),
                    }
                }
            }
            Value::Null => {}
            Value::String(s) => self.pairs.push((prefix.to_string(), s.clone())),
            other => self.pairs.push((prefix.to_string(), other.to_string())),
        }
    }

    fn add_multipart(&mut self, body: &[u8], boundary: &str) -> Result<(), String> {
        let delimiter = format!("--{boundary}");
        let delimiter = delimiter.as_bytes();
        let mut rest = body;
        let start = find(rest, delimiter).ok_or("Malformed multipart body")?;
        rest = &rest[start + delimiter.len()..];
        loop {
            if rest.starts_with(b"--") {
                return Ok(());
            }
            rest = rest
                .strip_prefix(b"\r\n")
                .ok_or("Malformed multipart part")?;
            let header_end = find(rest, b"\r\n\r\n").ok_or("Malformed multipart headers")?;
            let headers = String::from_utf8_lossy(&rest[..header_end]).to_string();
            rest = &rest[header_end + 4..];
            let end = find(rest, delimiter).ok_or("Unterminated multipart part")?;
            let data = rest[..end].strip_suffix(b"\r\n").unwrap_or(&rest[..end]);
            rest = &rest[end + delimiter.len()..];

            let mut name = None;
            let mut filename = None;
            let mut content_type = String::from("application/octet-stream");
            for line in headers.lines() {
                let lower = line.to_ascii_lowercase();
                if lower.starts_with("content-disposition:") {
                    name = disposition_value(line, "name");
                    filename = disposition_value(line, "filename");
                } else if let Some(ct) = lower.strip_prefix("content-type:") {
                    content_type = ct.trim().to_string();
                }
            }
            let Some(name) = name else { continue };
            match filename {
                Some(filename) => self.files.push(Upload {
                    name,
                    filename,
                    content_type,
                    data: data.to_vec(),
                }),
                None => self
                    .pairs
                    .push((name, String::from_utf8_lossy(data).into_owned())),
            }
        }
    }

    /// First value for `name`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Non-empty value for `name`.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.get(name).filter(|v| !v.is_empty())
    }

    /// Every value of `name` or `name[]`.
    pub fn all(&self, name: &str) -> Vec<&str> {
        let array = format!("{name}[]");
        self.pairs
            .iter()
            .filter(|(k, _)| *k == name || *k == array)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// Rails-style boolean: `true`, `1`, `on`.
    pub fn bool(&self, name: &str) -> Option<bool> {
        self.get(name)
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "on" | "yes"))
    }

    pub fn flag(&self, name: &str) -> bool {
        self.bool(name).unwrap_or(false)
    }

    pub fn u64(&self, name: &str) -> Option<u64> {
        self.get(name).and_then(|v| v.parse().ok())
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.pairs.iter().map(|(k, _)| k.as_str())
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn disposition_value(line: &str, key: &str) -> Option<String> {
    line.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k.eq_ignore_ascii_case(key)).then(|| v.trim().trim_matches('"').to_string())
    })
}

/// Percent-encode a query-string component.
pub fn encode(text: &str) -> String {
    form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_form_and_json_flatten_alike() {
        let req = Request::new("POST", "/x?limit=5&id[]=1&id[]=2").with_body(
            "application/x-www-form-urlencoded",
            "status=hi+there&sensitive=true",
        );
        let p = Params::parse(&req).unwrap();
        assert_eq!(p.get("status"), Some("hi there"));
        assert_eq!(p.all("id"), vec!["1", "2"]);
        assert_eq!(p.u64("limit"), Some(5));
        assert!(p.flag("sensitive"));

        let req = Request::new("PATCH", "/x").with_body(
            "application/json",
            r#"{"source":{"privacy":"private"},"fields_attributes":[{"name":"a","value":"b"}],"media_ids":["1"],"sensitive":true,"x":null}"#,
        );
        let p = Params::parse(&req).unwrap();
        assert_eq!(p.get("source[privacy]"), Some("private"));
        assert_eq!(p.get("fields_attributes[0][name]"), Some("a"));
        assert_eq!(p.all("media_ids"), vec!["1"]);
        assert_eq!(p.bool("sensitive"), Some(true));
        assert_eq!(p.get("x"), None);
    }

    #[test]
    fn multipart_text_and_files() {
        let body: &[u8] = b"--B\r\nContent-Disposition: form-data; name=\"display_name\"\r\n\r\nAlice\r\n--B\r\nContent-Disposition: form-data; name=\"avatar\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n\x89PNG\r\n--B--\r\n";
        let req =
            Request::new("PATCH", "/x").with_body("multipart/form-data; boundary=B", body.to_vec());
        let p = Params::parse(&req).unwrap();
        assert_eq!(p.get("display_name"), Some("Alice"));
        assert_eq!(p.files.len(), 1);
        assert_eq!(p.files[0].data, b"\x89PNG");
        assert_eq!(p.files[0].content_type, "image/png");
    }

    #[test]
    fn bearer_token() {
        let req = Request::new("GET", "/").with_header("authorization", "Bearer abc");
        assert_eq!(req.bearer(), Some("abc"));
        assert_eq!(Request::new("GET", "/").bearer(), None);
    }
}
