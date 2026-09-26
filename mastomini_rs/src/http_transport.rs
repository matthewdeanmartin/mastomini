//! Bounded HTTP/1 framing for the board's nonblocking serving loop.
//! Parsing and partial-write accounting are host tested independently of TLS.
use crate::http::{Request, Response, UPLOAD_BODY_LIMIT};

pub const HEADER_LIMIT: usize = 4096;
pub const INPUT_LIMIT: usize = HEADER_LIMIT + UPLOAD_BODY_LIMIT;
pub const RESPONSE_LIMIT: usize = 512 * 1024;

pub struct Incoming {
    pub request: Request,
    pub consumed: usize,
    pub close: bool,
    pub head: bool,
}

pub enum Parsed {
    /// Send 100 Continue only once, and only after validated complete headers.
    Partial {
        expect_continue: bool,
    },
    Complete(Incoming),
}

pub fn parse(input: &[u8], secure: bool) -> Result<Parsed, u16> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut parsed = httparse::Request::new(&mut headers);
    let start = match parsed.parse(input).map_err(|_| 400u16)? {
        httparse::Status::Partial if input.len() >= HEADER_LIMIT => return Err(431),
        httparse::Status::Partial => {
            return Ok(Parsed::Partial {
                expect_continue: false,
            })
        }
        httparse::Status::Complete(n) if n > HEADER_LIMIT => return Err(431),
        httparse::Status::Complete(n) => n,
    };
    let method = parsed.method.ok_or(400u16)?;
    let uri = parsed.path.ok_or(400u16)?;
    if uri.len() > 1024 {
        return Err(414);
    }
    if !uri.starts_with('/') || uri.starts_with("//") {
        return Err(400);
    }
    let mut length = 0;
    let mut host = false;
    let mut close = parsed.version != Some(1);
    let mut expect_continue = false;
    for (index, header) in parsed.headers.iter().enumerate() {
        // Reject ambiguous framing/authentication rather than letting the
        // parser and API choose different copies of a header.
        if parsed.headers[..index]
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case(header.name))
        {
            return Err(400);
        }
        let value = std::str::from_utf8(header.value)
            .map_err(|_| 400u16)?
            .trim();
        if value.bytes().any(|b| b < 32 && b != b'\t') || value.contains('\x7f') {
            return Err(400);
        }
        if header.name.eq_ignore_ascii_case("Transfer-Encoding") {
            return Err(400);
        }
        if header.name.eq_ignore_ascii_case("Expect") {
            if !value.eq_ignore_ascii_case("100-continue") {
                return Err(417);
            }
            expect_continue = true;
        }
        if header.name.eq_ignore_ascii_case("Host") {
            host = !value.is_empty();
        }
        if header.name.eq_ignore_ascii_case("Connection")
            && value
                .split(',')
                .any(|v| v.trim().eq_ignore_ascii_case("close"))
        {
            close = true;
        }
        if header.name.eq_ignore_ascii_case("Content-Length") {
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(400);
            }
            length = value.parse::<usize>().map_err(|_| 413u16)?;
            if length > UPLOAD_BODY_LIMIT {
                return Err(413);
            }
        }
    }
    if parsed.version == Some(1) && !host {
        return Err(400);
    }
    let end = start + length;
    if input.len() < end {
        return Ok(Parsed::Partial { expect_continue });
    }
    let head = method == "HEAD";
    // Keep the former board behavior: route HEAD as GET but suppress its body
    // on the wire, retaining the selected representation's Content-Length.
    let mut request = Request::new(if head { "GET" } else { method }, uri);
    request.secure = secure;
    request.headers = parsed
        .headers
        .iter()
        .map(|h| {
            (
                h.name.to_owned(),
                std::str::from_utf8(h.value).unwrap().trim().to_owned(),
            )
        })
        .collect();
    request.body = input[start..end].to_vec();
    Ok(Parsed::Complete(Incoming {
        request,
        consumed: end,
        close,
        head,
    }))
}

/// Coalesce headers and small replies; retain large bodies without a second
/// full copy. A TLS WANT_WRITE retry must use the same slice and length.
pub struct Outgoing {
    prefix: Vec<u8>,
    body: Vec<u8>,
    skip: usize,
    sent: usize,
    pub close: bool,
    pub interim: bool,
}

impl Outgoing {
    pub fn continue_100() -> Self {
        Self {
            prefix: b"HTTP/1.1 100 Continue\r\n\r\n".to_vec(),
            body: Vec::new(),
            skip: 0,
            sent: 0,
            close: false,
            interim: true,
        }
    }

    pub fn new(mut reply: Response, head: bool, mut close: bool) -> Self {
        if reply.body.len() > RESPONSE_LIMIT {
            reply = Response::error(503, "Response exceeds the board's transport limit");
            close = true;
        }
        let mut prefix =
            format!("HTTP/1.1 {} {}\r\n", reply.status, reason(reply.status)).into_bytes();
        for (name, value) in &reply.headers {
            if name.eq_ignore_ascii_case("Connection") {
                close |= value
                    .split(',')
                    .any(|s| s.trim().eq_ignore_ascii_case("close"));
                continue;
            }
            if name.eq_ignore_ascii_case("Content-Length")
                || name.eq_ignore_ascii_case("Transfer-Encoding")
                || [name, value].iter().any(|s| s.contains(['\r', '\n']))
            {
                continue;
            }
            prefix.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        if !matches!(reply.status, 204 | 304) {
            prefix
                .extend_from_slice(format!("Content-Length: {}\r\n", reply.body.len()).as_bytes());
        }
        if close {
            prefix.extend_from_slice(b"Connection: close\r\n");
        }
        prefix.extend_from_slice(b"\r\n");
        let body = if head || matches!(reply.status, 204 | 304) {
            Vec::new()
        } else {
            reply.body
        };
        let skip = body.len().min(2048);
        prefix.extend_from_slice(&body[..skip]);
        Self {
            prefix,
            body,
            skip,
            sent: 0,
            close,
            interim: false,
        }
    }

    pub fn next(&self) -> &[u8] {
        if self.sent < self.prefix.len() {
            &self.prefix[self.sent..self.prefix.len().min(self.sent + 8192)]
        } else {
            let rest = &self.body[self.skip + self.sent - self.prefix.len()..];
            &rest[..rest.len().min(8192)]
        }
    }

    pub fn advance(&mut self, count: usize) {
        self.sent += count;
    }

    pub fn retained_bytes(&self) -> usize {
        self.prefix.capacity() + self.body.capacity()
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        413 => "Payload Too Large",
        414 => "URI Too Long",
        417 => "Expectation Failed",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(input: &[u8]) -> Incoming {
        match parse(input, true).unwrap() {
            Parsed::Complete(r) => r,
            Parsed::Partial { .. } => panic!("request incomplete"),
        }
    }

    #[test]
    fn fragmented_upload_and_pipeline_keep_exact_boundaries() {
        let mut data = b"PATCH /api/v1/accounts/update_credentials HTTP/1.1\r\nHost: mastomini.local\r\nContent-Type: multipart/form-data; boundary=X\r\nAuthorization: Bearer test\r\nContent-Length: 163840\r\n\r\n".to_vec();
        let header = data.len();
        data.resize(header + UPLOAD_BODY_LIMIT, b'x');
        for end in [0, 1, header - 1, header, header + 4096, data.len() - 1] {
            assert!(matches!(
                parse(&data[..end], true).unwrap(),
                Parsed::Partial { .. }
            ));
        }
        let consumed = data.len();
        data.extend_from_slice(b"GET /api/v1/statuses?limit=2 HTTP/1.1\r\nHost: mastomini.local\r\nConnection: close\r\n\r\n");
        let a = complete(&data);
        assert_eq!(a.consumed, consumed);
        assert_eq!(a.request.body, vec![b'x'; UPLOAD_BODY_LIMIT]);
        assert_eq!(a.request.bearer(), Some("test"));
        assert!(a.request.secure);
        let b = complete(&data[consumed..]);
        assert_eq!(b.request.query, "limit=2");
        assert!(b.close);
    }

    #[test]
    fn ambiguous_framing_and_oversized_input_fail_before_allocation() {
        for extra in [
            "Content-Length: 0\r\nContent-Length: 1\r\n",
            "Transfer-Encoding: chunked\r\n",
            "Content-Length: +2\r\n",
            "Authorization: a\r\nAuthorization: b\r\n",
            "Content-Length: 163841\r\n",
        ] {
            let input = format!("POST /x HTTP/1.1\r\nHost: x\r\n{extra}\r\n");
            assert!(parse(input.as_bytes(), true).is_err());
        }
        assert!(parse(b"GET / HTTP/1.1\r\n\r\n", true).is_err());
        assert!(matches!(parse(&vec![b'G'; HEADER_LIMIT], true), Err(431)));
        assert!(
            complete(b"POST /api/v1/statuses/1/favourite HTTP/1.1\r\nHost: x\r\n\r\n")
                .request
                .body
                .is_empty()
        );
    }

    #[test]
    fn continue_requires_valid_headers_and_an_outstanding_body() {
        let head =
            b"PUT /x HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\nContent-Length: 2\r\n\r\n";
        assert!(matches!(
            parse(head, true).unwrap(),
            Parsed::Partial {
                expect_continue: true
            }
        ));
        assert!(matches!(
            parse(&head[..head.len() - 1], true).unwrap(),
            Parsed::Partial {
                expect_continue: false
            }
        ));
        let mut full = head.to_vec();
        full.extend_from_slice(b"{}");
        assert_eq!(complete(&full).request.body, b"{}");
        assert_eq!(
            Outgoing::continue_100().next(),
            b"HTTP/1.1 100 Continue\r\n\r\n"
        );
    }

    fn drain(mut response: Outgoing) -> Vec<u8> {
        let mut result = Vec::new();
        while !response.next().is_empty() {
            // Arbitrary partial writes, including across TLS record boundaries.
            let count = response.next().len().min(137);
            result.extend_from_slice(&response.next()[..count]);
            response.advance(count);
        }
        result
    }

    #[test]
    fn partial_writes_head_and_bodyless_statuses_frame_correctly() {
        let body: Vec<_> = (0..100_000).map(|i| (i % 251) as u8).collect();
        let reply = Response::new(200, "application/octet-stream", body.clone());
        let wire = drain(Outgoing::new(reply.clone(), false, false));
        let start = wire.windows(4).position(|x| x == b"\r\n\r\n").unwrap() + 4;
        assert_eq!(&wire[start..], body);
        let head = drain(Outgoing::new(reply, true, false));
        assert!(head.ends_with(b"\r\n\r\n"));
        assert!(String::from_utf8(head)
            .unwrap()
            .contains("Content-Length: 100000\r\n"));
        for status in [204, 304] {
            let wire = String::from_utf8(drain(Outgoing::new(
                Response::new(status, "text/plain", b"ignored".to_vec()),
                false,
                false,
            )))
            .unwrap();
            assert!(!wire.contains("Content-Length:"));
            assert!(wire.ends_with("\r\n\r\n"));
        }
        let r = Outgoing::new(
            Response::new(200, "text/plain", vec![0; RESPONSE_LIMIT + 1]),
            false,
            false,
        );
        assert!(r.close);
        assert!(drain(r).starts_with(b"HTTP/1.1 503 "));
    }
}
