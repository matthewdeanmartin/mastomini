//! The bots board's HTTP and HTTPS listeners, on ESP-IDF's httpd directly:
//! the transport mastomini's board used when it was measured
//! (mastomini_rs spec/08 "Board transport"), without the Wi-Fi setup pages.
//!
//! - A response is built in one buffer and sent with one `httpd_send`
//!   (esp_http_server's own calls write every header separately).
//! - `TCP_NODELAY` before the TLS handshake, so small records don't wait for
//!   the client's delayed ACK.
//! - TLS session tickets: a returning browser resumes in tens of
//!   milliseconds instead of repeating a ~1 s handshake.
//! - No event-loop post per read and write; a 4 s handshake timeout, since
//!   one httpd task serves every HTTPS client.
//!
//! The server never stops, so what the listeners borrow is leaked once.

use esp_idf_svc::sys::*;
use mastobots::{
    api::{self, Ctx},
    http::{Request, Response, BODY_LIMIT, FORWARDED_HEADERS},
};
use std::{
    ffi::{c_char, c_int, c_void, CStr, CString},
    sync::{
        atomic::{AtomicPtr, Ordering},
        Arc,
    },
    time::Instant,
};

use crate::{now_ms, uptime_ms, Shared};

/// What one listener's requests need.
pub struct Listener {
    pub secure: bool,
    pub shared: Shared,
    pub ctx: Arc<Ctx>,
}

pub struct Settings {
    pub port: u16,
    /// httpd's internal control socket; distinct per server.
    pub ctrl_port: u16,
    pub sockets: u16,
    /// Connections the kernel queues before httpd accepts them.
    pub backlog: u16,
}

/// The TLS server configuration shared by every HTTPS session.
static TLS: AtomicPtr<esp_tls_cfg_server_t> = AtomicPtr::new(core::ptr::null_mut());

/// Set up HTTPS: certificate and key are NUL-terminated PEM. Session
/// tickets let a client that reconnects skip the key exchange.
pub fn init_tls(cert: &'static str, key: &'static str) -> Result<(), EspError> {
    // SAFETY: a zeroed esp_tls_cfg_server_t is its documented default; the
    // PEM buffers are 'static, and the config is leaked, so every pointer
    // the TLS sessions keep stays valid.
    unsafe {
        let cfg: &'static mut esp_tls_cfg_server_t = Box::leak(Box::new(core::mem::zeroed()));
        cfg.__bindgen_anon_3.servercert_buf = cert.as_ptr();
        cfg.__bindgen_anon_4.servercert_bytes = cert.len() as _;
        cfg.__bindgen_anon_5.serverkey_buf = key.as_ptr();
        cfg.__bindgen_anon_6.serverkey_bytes = key.len() as _;
        // One httpd task serves every HTTPS client, so a client that stalls
        // mid-handshake stalls them all: give up after 4 s (a handshake
        // takes about 1), not esp-tls's default 10 s.
        cfg.tls_handshake_timeout_ms = 4000;
        match esp!(esp_tls_cfg_server_session_tickets_init(cfg)) {
            Ok(()) => log::info!("TLS session tickets on"),
            Err(e) => log::warn!("TLS session tickets unavailable: {e}"),
        }
        TLS.store(cfg, Ordering::Release);
    }
    Ok(())
}

fn no_delay(fd: c_int) {
    let on: c_int = 1;
    // SAFETY: fd is the session's open socket; the option value outlives
    // the call.
    unsafe {
        lwip_setsockopt(
            fd,
            IPPROTO_TCP as _,
            TCP_NODELAY as _,
            (&on as *const c_int).cast(),
            core::mem::size_of::<c_int>() as _,
        );
    }
}

unsafe extern "C" fn open_plain(_hd: httpd_handle_t, fd: c_int) -> esp_err_t {
    no_delay(fd);
    ESP_OK
}

/// A new HTTPS connection: handshake, then route the session's reads and
/// writes through TLS.
unsafe extern "C" fn open_tls(hd: httpd_handle_t, fd: c_int) -> esp_err_t {
    no_delay(fd);
    let cfg = TLS.load(Ordering::Acquire);
    let tls = esp_tls_init();
    if cfg.is_null() || tls.is_null() {
        return ESP_ERR_NO_MEM;
    }
    let started = Instant::now();
    if esp_tls_server_session_create(cfg, fd, tls) != 0 {
        esp_tls_server_session_delete(tls);
        return ESP_FAIL;
    }
    if option_env!("MASTOMINI_TRACE_TIMING") == Some("1") {
        log::info!(
            "timing tls_handshake_ms={:.3}",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
    httpd_sess_set_transport_ctx(hd, fd, tls.cast(), Some(close_tls));
    httpd_sess_set_send_override(hd, fd, Some(send_tls));
    httpd_sess_set_recv_override(hd, fd, Some(recv_tls));
    httpd_sess_set_pending_override(hd, fd, Some(pending_tls));
    ESP_OK
}

unsafe extern "C" fn close_tls(ctx: *mut c_void) {
    esp_tls_server_session_delete(ctx.cast());
}

fn session(hd: httpd_handle_t, fd: c_int) -> *mut esp_tls_t {
    // SAFETY: called by httpd for a live session that open_tls set up.
    unsafe { httpd_sess_get_transport_ctx(hd, fd).cast() }
}

/// mbedTLS reports a socket timeout as "want read/write"; httpd retries only
/// its own timeout code.
fn sock_result(n: isize) -> c_int {
    match n as i32 {
        ESP_TLS_ERR_SSL_WANT_READ | ESP_TLS_ERR_SSL_WANT_WRITE => HTTPD_SOCK_ERR_TIMEOUT,
        n if n < 0 => HTTPD_SOCK_ERR_FAIL,
        n => n,
    }
}

unsafe extern "C" fn send_tls(
    hd: httpd_handle_t,
    fd: c_int,
    buf: *const c_char,
    len: usize,
    _flags: c_int,
) -> c_int {
    sock_result(esp_tls_conn_write(session(hd, fd), buf.cast(), len))
}

unsafe extern "C" fn recv_tls(
    hd: httpd_handle_t,
    fd: c_int,
    buf: *mut c_char,
    len: usize,
    _flags: c_int,
) -> c_int {
    sock_result(esp_tls_conn_read(session(hd, fd), buf.cast(), len))
}

unsafe extern "C" fn pending_tls(hd: httpd_handle_t, fd: c_int) -> c_int {
    esp_tls_get_bytes_avail(session(hd, fd)) as c_int
}

/// Start one listener (HTTPS when `listener.secure`) routing every path and
/// method to the API.
pub fn start(settings: &Settings, listener: Listener) -> Result<(), EspError> {
    let secure = listener.secure;
    let config = httpd_config_t {
        task_priority: 5,
        // serde_json rendering, plus the TLS handshake's frames.
        stack_size: 32 * 1024,
        core_id: 1,
        task_caps: MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT,
        server_port: settings.port,
        ctrl_port: settings.ctrl_port,
        max_open_sockets: settings.sockets,
        max_uri_handlers: 8,
        max_resp_headers: 8,
        backlog_conn: settings.backlog,
        // Evict the least recently used connection instead of refusing.
        lru_purge_enable: true,
        recv_wait_timeout: 5,
        send_wait_timeout: 5,
        open_fn: Some(if secure { open_tls } else { open_plain }),
        uri_match_fn: Some(httpd_uri_match_wildcard),
        ..Default::default()
    };
    let mut handle: httpd_handle_t = core::ptr::null_mut();
    // SAFETY: config is fully initialized; httpd copies it.
    esp!(unsafe { httpd_start(&mut handle, &config) })?;
    let context: *mut c_void = Box::into_raw(Box::new(listener)).cast();
    let uri: &'static CStr = c"/*";
    for method in [
        http_method_HTTP_GET,
        http_method_HTTP_HEAD,
        http_method_HTTP_POST,
        http_method_HTTP_PUT,
        http_method_HTTP_PATCH,
        http_method_HTTP_DELETE,
        http_method_HTTP_OPTIONS,
    ] {
        let handler = httpd_uri_t {
            uri: uri.as_ptr(),
            method: method as _,
            handler: Some(handle_request),
            user_ctx: context,
            // SAFETY: the remaining fields are optional and zero by default.
            ..unsafe { core::mem::zeroed() }
        };
        // SAFETY: handle is running; uri and context are leaked.
        esp!(unsafe { httpd_register_uri_handler(handle, &handler) })?;
    }
    log::info!(
        "{} listening on port {} ({} sockets, backlog {})",
        if secure { "HTTPS" } else { "HTTP" },
        settings.port,
        settings.sockets,
        settings.backlog
    );
    Ok(())
}

#[allow(non_upper_case_globals)]
fn method_name(method: c_int) -> &'static str {
    match method as u32 {
        http_method_HTTP_GET => "GET",
        http_method_HTTP_HEAD => "HEAD",
        http_method_HTTP_POST => "POST",
        http_method_HTTP_PUT => "PUT",
        http_method_HTTP_PATCH => "PATCH",
        http_method_HTTP_DELETE => "DELETE",
        http_method_HTTP_OPTIONS => "OPTIONS",
        _ => "OTHER",
    }
}

fn header(req: *mut httpd_req_t, name: &str) -> Option<String> {
    let name = CString::new(name).ok()?;
    // SAFETY: req is the live request; the buffer holds len + 1 bytes.
    unsafe {
        let len = httpd_req_get_hdr_value_len(req, name.as_ptr());
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u8; len + 1];
        esp!(httpd_req_get_hdr_value_str(
            req,
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len()
        ))
        .ok()?;
        buf.truncate(len);
        String::from_utf8(buf).ok()
    }
}

/// The request body, or `None` if the client stopped sending it.
fn read_body(req: *mut httpd_req_t, len: usize) -> Option<Vec<u8>> {
    let mut body = vec![0u8; len];
    let mut done = 0;
    let mut timeouts = 0;
    while done < len {
        // SAFETY: the destination range is within body.
        let n = unsafe { httpd_req_recv(req, body[done..].as_mut_ptr().cast(), len - done) };
        match n {
            HTTPD_SOCK_ERR_TIMEOUT if timeouts < 2 => timeouts += 1,
            n if n > 0 => done += n as usize,
            _ => return None,
        }
    }
    Some(body)
}

fn send_all(req: *mut httpd_req_t, mut data: &[u8]) -> bool {
    let mut timeouts = 0;
    while !data.is_empty() {
        // SAFETY: data is a live slice.
        let n = unsafe { httpd_send(req, data.as_ptr().cast(), data.len()) };
        match n {
            HTTPD_SOCK_ERR_TIMEOUT if timeouts < 2 => timeouts += 1,
            n if n > 0 => data = &data[n as usize..],
            _ => return false,
        }
    }
    true
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
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "",
    }
}

/// Status line, headers and (except for HEAD) body, in one buffer.
fn serialize(reply: &Response, head: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(256 + if head { 0 } else { reply.body.len() });
    out.extend_from_slice(
        format!("HTTP/1.1 {} {}\r\n", reply.status, reason(reply.status)).as_bytes(),
    );
    for (name, value) in &reply.headers {
        // Header values are built by this server, but never let one split
        // the response.
        if name.eq_ignore_ascii_case("Content-Length")
            || [name, value].iter().any(|s| s.contains(['\r', '\n']))
        {
            continue;
        }
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", reply.body.len()).as_bytes());
    if !head {
        out.extend_from_slice(&reply.body);
    }
    out
}

fn respond(listener: &Listener, req: *mut httpd_req_t, name: &'static str) -> esp_err_t {
    let started = Instant::now();
    let mut body_ms = 0.0;
    let mut lock_ms = 0.0;
    let head = name == "HEAD";
    // SAFETY: httpd NUL-terminates the URI inside the live request.
    let uri = unsafe { CStr::from_ptr((*req).uri.as_ptr()) }.to_string_lossy();
    let mut request = Request::new(if head { "GET" } else { name }, &uri);
    request.secure = listener.secure;
    for name in FORWARDED_HEADERS {
        if let Some(value) = header(req, name) {
            request.headers.push((name.to_string(), value));
        }
    }
    // SAFETY: a field of the live request.
    let length = unsafe { (*req).content_len };
    let reply = if length > BODY_LIMIT {
        Response::error(413, "Request body is too large")
    } else if let Some(body) = read_body(req, length) {
        request.body = body;
        body_ms = started.elapsed().as_secs_f64() * 1000.0;
        let waiting = Instant::now();
        let mut service = listener.shared.lock().unwrap_or_else(|e| e.into_inner());
        lock_ms = waiting.elapsed().as_secs_f64() * 1000.0;
        api::handle(&mut service, &listener.ctx, &request, now_ms(), uptime_ms())
    } else {
        Response::error(400, "Request body ended early")
    };
    let reply = reply.with_header(
        "Server-Timing",
        &format!("body;dur={body_ms:.3}, lock;dur={lock_ms:.3}"),
    );
    let sending = Instant::now();
    let sent = send_all(req, &serialize(&reply, head));
    if started.elapsed().as_millis() >= 250 || option_env!("MASTOMINI_TRACE_TIMING") == Some("1") {
        log::info!(
            "timing method={name} route={} status={} bytes={} body_ms={body_ms:.3} lock_ms={lock_ms:.3} send_ms={:.3} accepted_total_ms={:.3}",
            request.path.split('/').take(4).collect::<Vec<_>>().join("/"),
            reply.status,
            reply.body.len(),
            sending.elapsed().as_secs_f64() * 1000.0,
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
    // Anything else makes httpd close the connection.
    if sent {
        ESP_OK
    } else {
        ESP_FAIL
    }
}

unsafe extern "C" fn handle_request(req: *mut httpd_req_t) -> esp_err_t {
    // SAFETY: user_ctx is the leaked Listener registered with this handler.
    let listener = &*((*req).user_ctx as *const Listener);
    respond(listener, req, method_name((*req).method))
}
