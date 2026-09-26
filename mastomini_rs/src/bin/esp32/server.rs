//! Bounded asynchronous TLS establishment on core 0; established HTTP(S)
//! connections are multiplexed on core 1. No socket I/O holds the service lock.
use crate::{net, now_ms, Net, Shared};
use esp_idf_svc::{
    hal::{cpu::Core, task::thread::ThreadSpawnConfiguration},
    sys,
};
use mastomini::incidents::{Kind, LOG};
use mastomini::{
    api::{self, Ctx},
    http::{Request, Response},
    http_transport::{self as http, Outgoing, Parsed},
};
use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    os::fd::AsRawFd,
    ptr::NonNull,
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

const TLS_CLIENTS: usize = 8;
const HTTP_CLIENTS: usize = 4;
const HANDSHAKES: usize = 2;
const IDLE: Duration = Duration::from_secs(60);
const IO_DEADLINE: Duration = Duration::from_secs(10);
// Pause dispatch at 512 KiB. One additional bounded response can cross the
// threshold; queued responses retain their existing buffers, not full copies.
const RESPONSE_BUDGET: usize = 512 * 1024;
const INPUT_BUDGET: usize = 512 * 1024;

pub struct Context {
    pub shared: Shared,
    pub ctx: Arc<Ctx>,
    pub net: Arc<Mutex<Net>>,
}

/// The socket owns its fd. IDF 5.5.3 server_session_delete frees only the TLS
/// context (unlike conn_destroy); TcpStream closes the descriptor exactly once.
struct Socket {
    tcp: TcpStream,
    tls: Option<NonNull<sys::esp_tls_t>>,
}

// SAFETY: a Socket has a single owner, never shared access. The handshake task
// transfers the completed TLS context through a channel and never touches it
// again. All subsequent TLS calls and destruction happen on the receiving task.
unsafe impl Send for Socket {}

impl Drop for Socket {
    fn drop(&mut self) {
        if let Some(tls) = self.tls {
            // SAFETY: exclusively owned, live context; TCP closes afterward.
            unsafe { sys::esp_tls_server_session_delete(tls.as_ptr()) };
        }
    }
}

fn tls_result(n: isize) -> io::Result<usize> {
    match n as i32 {
        sys::ESP_TLS_ERR_SSL_WANT_READ | sys::ESP_TLS_ERR_SSL_WANT_WRITE => {
            Err(io::ErrorKind::WouldBlock.into())
        }
        n if n < 0 => {
            incident(Kind::SocketError, n, 0);
            Err(io::ErrorKind::ConnectionAborted.into())
        }
        _ => Ok(n as usize),
    }
}

impl Read for Socket {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        match self.tls {
            // SAFETY: the context is exclusively owned and out is writable.
            Some(tls) => tls_result(unsafe {
                sys::esp_tls_conn_read(tls.as_ptr(), out.as_mut_ptr().cast(), out.len())
            }),
            None => self.tcp.read(out),
        }
    }
}

impl Write for Socket {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.tls {
            // SAFETY: exclusive TLS context and valid input slice. A retry after
            // WANT_WRITE uses the same response slice and length until progress.
            Some(tls) => tls_result(unsafe {
                sys::esp_tls_conn_write(tls.as_ptr(), bytes.as_ptr().cast(), bytes.len())
            }),
            None => self.tcp.write(bytes),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn socket(tcp: TcpStream) -> io::Result<Socket> {
    tcp.set_nodelay(true)?;
    tcp.set_nonblocking(true)?;
    Ok(Socket { tcp, tls: None })
}

/// The TLS configuration and ticket keys live for this task's lifetime. Only
/// this task creates sessions; no pointer to the config crosses the channel.
fn handshakes(
    listener: TcpListener,
    ready: mpsc::SyncSender<Socket>,
    cert: &'static str,
    key: &'static str,
) {
    // SAFETY: zero is IDF's documented default; PEM inputs are static and NUL
    // terminated. Config remains alive until after every pending handshake.
    let mut cfg: sys::esp_tls_cfg_server_t = unsafe { core::mem::zeroed() };
    let cert = cert.as_bytes();
    let key = key.as_bytes();
    cfg.__bindgen_anon_3.servercert_buf = cert.as_ptr();
    cfg.__bindgen_anon_4.servercert_bytes = cert.len() as _;
    cfg.__bindgen_anon_5.serverkey_buf = key.as_ptr();
    cfg.__bindgen_anon_6.serverkey_bytes = key.len() as _;
    // SAFETY: valid config owned on this task. Ticket context intentionally lives
    // until reboot, including after a session moves to the serving task.
    let tickets = unsafe { sys::esp_tls_cfg_server_session_tickets_init(&mut cfg) };
    if tickets != sys::ESP_OK {
        incident(Kind::TlsInitFailed, tickets, 0);
        log::error!("TLS ticket initialization failed: {tickets}");
    }
    let mut pending: Vec<(Socket, Instant)> = Vec::with_capacity(HANDSHAKES);
    loop {
        LOG.beat(0, incident_now());
        LOG.connections(0, pending.len());
        if pending.len() < HANDSHAKES {
            if let Ok((tcp, _)) = listener.accept() {
                if let Ok(mut stream) = socket(tcp) {
                    // SAFETY: init allocates an exclusively owned context. On
                    // every failure Socket's Drop releases context and fd.
                    if let Some(tls) = NonNull::new(unsafe { sys::esp_tls_init() }) {
                        stream.tls = Some(tls);
                        let result = unsafe {
                            sys::esp_tls_server_session_init(
                                &mut cfg,
                                stream.tcp.as_raw_fd(),
                                tls.as_ptr(),
                            )
                        };
                        if result == sys::ESP_OK {
                            pending.push((stream, Instant::now()));
                            LOG.connections(0, pending.len());
                        } else {
                            incident(Kind::TlsInitFailed, result, 0);
                        }
                    } else {
                        incident(Kind::AllocationFailed, 1, 0);
                    }
                }
            }
        }
        let mut i = 0;
        while i < pending.len() {
            let (stream, began) = &pending[i];
            if began.elapsed() >= Duration::from_secs(4) {
                incident(Kind::TlsTimeout, 0, began.elapsed().as_millis() as u32);
                pending.swap_remove(i);
                continue;
            }
            // SAFETY: context exclusively owned here; socket is nonblocking.
            let result =
                unsafe { sys::esp_tls_server_session_continue_async(stream.tls.unwrap().as_ptr()) };
            match result {
                0 => {
                    let (stream, began) = pending.swap_remove(i);
                    log::debug!(
                        "TLS established in {} ms on core 0",
                        began.elapsed().as_millis()
                    );
                    // Bounded handoff; a full queue drops the newly established
                    // session rather than retaining unbounded sockets/memory.
                    LOG.handshake(began.elapsed().as_millis() as u32);
                    if ready.try_send(stream).is_err() {
                        incident(Kind::HandoffFull, 0, 0);
                    }
                }
                sys::ESP_TLS_ERR_SSL_WANT_READ | sys::ESP_TLS_ERR_SSL_WANT_WRITE => i += 1,
                _ => {
                    incident(Kind::TlsFailed, result, began.elapsed().as_millis() as u32);
                    pending.swap_remove(i);
                }
            }
        }
        // IDF's libc usleep busy-waits below one tick. Explicit FreeRTOS delay
        // rounds up and lets IDLE0 run/feed its watchdog even with no traffic.
        esp_idf_svc::hal::delay::FreeRtos::delay_ms(1);
    }
}

struct Trace {
    method: &'static str,
    route: String,
    status: u16,
    bytes: usize,
    began: Instant,
    sending: Instant,
}

struct Client {
    socket: Socket,
    input: Vec<u8>,
    used: usize,
    response: Option<Outgoing>,
    active: Instant,
    request_started: Option<Instant>,
    send_started: Option<Instant>,
    continued: bool,
    trace: Option<Trace>,
}

impl Client {
    fn new(socket: Socket) -> Self {
        Self {
            socket,
            input: vec![0; http::HEADER_LIMIT],
            used: 0,
            response: None,
            active: Instant::now(),
            request_started: None,
            send_started: None,
            continued: false,
            trace: None,
        }
    }

    /// One read and write at most per client/turn. Partial HTTP and uploads
    /// retain bounded state while unrelated clients continue to run.
    fn poll(&mut self, ctx: &Context, may_dispatch: bool, input_available: usize) -> bool {
        if self.active.elapsed() > IDLE {
            incident(Kind::IdleExpired, 0, 0);
            return false;
        }
        if self
            .request_started
            .is_some_and(|t| t.elapsed() > IO_DEADLINE)
            || self.send_started.is_some_and(|t| t.elapsed() > IO_DEADLINE)
        {
            incident(
                Kind::RequestTimeout,
                if self.send_started.is_some() { 1 } else { 0 },
                IO_DEADLINE.as_millis() as u32,
            );
            return false;
        }
        if self.response.is_none() {
            if !may_dispatch {
                return true;
            }
            // Parse buffered pipelining before reading: the peer may already
            // have half-closed its write side after sending the next request.
            let secure = self.socket.tls.is_some();
            let mut parsed = http::parse(&self.input[..self.used], secure);
            if matches!(parsed, Ok(Parsed::Partial { .. })) {
                if self.used == self.input.len() && self.input.len() < http::INPUT_LIMIT {
                    let next = (self.input.len() * 2).min(http::INPUT_LIMIT);
                    if next.saturating_sub(self.input.capacity()) > input_available {
                        return true;
                    }
                    // reserve_exact avoids Vec's doubling past the upload cap.
                    if self
                        .input
                        .try_reserve_exact(next - self.input.len())
                        .is_err()
                    {
                        incident(Kind::AllocationFailed, 0, 0);
                        return false;
                    }
                    self.input.resize(next, 0);
                }
                let end = (self.used + 4096).min(self.input.len());
                match self.socket.read(&mut self.input[self.used..end]) {
                    Ok(0) => return false,
                    Ok(n) => {
                        self.used += n;
                        self.active = Instant::now();
                        self.request_started.get_or_insert(self.active);
                        parsed = http::parse(&self.input[..self.used], secure);
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        if self.socket.tls.is_none() {
                            incident(Kind::SocketError, e.raw_os_error().unwrap_or(0), 0);
                        }
                        return false;
                    }
                }
            }
            match parsed {
                Ok(Parsed::Complete(incoming)) => {
                    let began = self.request_started.unwrap_or_else(Instant::now);
                    let body_ms = began.elapsed().as_secs_f64() * 1000.0;
                    let reply = respond(ctx, &incoming.request, body_ms);
                    let sending = Instant::now();
                    self.trace = Some(Trace {
                        method: if incoming.head {
                            "HEAD"
                        } else {
                            method_name(&incoming.request.method)
                        },
                        route: mastomini::http::route_label(&incoming.request.path),
                        status: reply.status,
                        bytes: reply.body.len(),
                        began,
                        sending,
                    });
                    self.response = Some(Outgoing::new(reply, incoming.head, incoming.close));
                    self.input.copy_within(incoming.consumed..self.used, 0);
                    self.used -= incoming.consumed;
                    // Release an upload buffer once its body has been consumed.
                    if self.used <= http::HEADER_LIMIT && self.input.len() > http::HEADER_LIMIT {
                        self.input.truncate(http::HEADER_LIMIT);
                        self.input.shrink_to_fit();
                    }
                    self.request_started = if self.used == 0 {
                        None
                    } else {
                        Some(Instant::now())
                    };
                    self.send_started = Some(Instant::now());
                    self.continued = false;
                }
                Ok(Parsed::Partial { expect_continue }) => {
                    if expect_continue && !self.continued {
                        self.response = Some(Outgoing::continue_100());
                        self.send_started = Some(Instant::now());
                        self.continued = true;
                    }
                }
                Err(status) => {
                    incident(Kind::InvalidRequest, status as i32, 0);
                    self.response = Some(Outgoing::new(
                        Response::error(status, "Invalid HTTP request"),
                        false,
                        true,
                    ));
                    self.request_started = None;
                    self.send_started = Some(Instant::now());
                }
            }
        }
        if let Some(response) = &mut self.response {
            if !response.next().is_empty() {
                match self.socket.write(response.next()) {
                    Ok(0) => return false,
                    Ok(n) => {
                        response.advance(n);
                        self.active = Instant::now();
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        if self.socket.tls.is_none() {
                            incident(Kind::SocketError, e.raw_os_error().unwrap_or(0), 0);
                        }
                        return false;
                    }
                }
            }
            if response.next().is_empty() {
                if !response.interim {
                    if let Some(trace) = self.trace.take() {
                        if trace.began.elapsed().as_millis() >= 500 {
                            incident(
                                Kind::SlowRequest,
                                trace.status as i32,
                                trace.began.elapsed().as_millis() as u32,
                            );
                        }
                        if trace.began.elapsed().as_millis() >= 250
                            || option_env!("MASTOMINI_TRACE_TIMING") == Some("1")
                        {
                            log::info!("timing method={} route={} status={} bytes={} send_ms={:.3} accepted_total_ms={:.3}",
                                trace.method, trace.route, trace.status, trace.bytes,
                                trace.sending.elapsed().as_secs_f64() * 1000.0,
                                trace.began.elapsed().as_secs_f64() * 1000.0);
                        }
                    }
                }
                if response.close {
                    return false;
                }
                self.response = None;
                self.send_started = None;
            }
        }
        true
    }
}

fn method_name(method: &str) -> &'static str {
    match method {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "OPTIONS" => "OPTIONS",
        _ => "OTHER",
    }
}

fn add_client(clients: &mut Vec<Client>, stream: Socket) {
    let secure = stream.tls.is_some();
    let limit = if secure { TLS_CLIENTS } else { HTTP_CLIENTS };
    if clients
        .iter()
        .filter(|c| c.socket.tls.is_some() == secure)
        .count()
        >= limit
    {
        // Reclaim only idle sessions of the same transport, never an in-flight
        // response or partial request. Allow active clients to finish.
        if let Some(index) = clients
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.socket.tls.is_some() == secure && c.response.is_none() && c.used == 0
            })
            .max_by_key(|(_, c)| c.active.elapsed())
            .map(|(i, _)| i)
        {
            clients.swap_remove(index);
        } else {
            incident(Kind::AdmissionRejected, i32::from(secure), 0);
            return;
        }
    }
    clients.push(Client::new(stream));
}

pub fn start(
    ctx: Context,
    cert: &'static str,
    key: &'static str,
) -> Result<(), Box<dyn std::error::Error>> {
    let tls_listener = TcpListener::bind(("0.0.0.0", 443))?;
    let http_listener = TcpListener::bind(("0.0.0.0", 80))?;
    tls_listener.set_nonblocking(true)?;
    http_listener.set_nonblocking(true)?;
    let (ready, completed) = mpsc::sync_channel(2);
    ThreadSpawnConfiguration {
        name: Some(c"mastomini-tls"),
        priority: 4,
        pin_to_core: Some(Core::Core0),
        ..Default::default()
    }
    .set()?;
    // Rust supplies pthread attributes, whose stack size overrides IDF's
    // ThreadSpawnConfiguration. Set it on Builder or this gets only 3 KiB.
    std::thread::Builder::new()
        .stack_size(24 * 1024)
        .spawn(move || handshakes(tls_listener, ready, cert, key))?;
    ThreadSpawnConfiguration {
        name: Some(c"mastomini-http"),
        priority: 5,
        pin_to_core: Some(Core::Core1),
        ..Default::default()
    }
    .set()?;
    std::thread::Builder::new()
        .stack_size(32 * 1024)
        .spawn(move || {
            let mut clients: Vec<Client> = Vec::with_capacity(TLS_CLIENTS + HTTP_CLIENTS);
            loop {
                let mut queued: usize = clients
                    .iter()
                    .filter_map(|c| c.response.as_ref())
                    .map(Outgoing::retained_bytes)
                    .sum();
                let mut input_bytes: usize = clients.iter().map(|c| c.input.capacity()).sum();
                LOG.beat(1, incident_now());
                LOG.connections(1, clients.iter().filter(|c| c.socket.tls.is_some()).count());
                LOG.connections(2, clients.iter().filter(|c| c.socket.tls.is_none()).count());
                clients.retain_mut(|client| {
                    let before = client.response.as_ref().map_or(0, Outgoing::retained_bytes);
                    let input_before = client.input.capacity();
                    let keep = client.poll(
                        &ctx,
                        queued < RESPONSE_BUDGET,
                        INPUT_BUDGET.saturating_sub(input_bytes),
                    );
                    queued -= before;
                    input_bytes -= input_before;
                    if keep {
                        queued += client.response.as_ref().map_or(0, Outgoing::retained_bytes);
                        input_bytes += client.input.capacity();
                    }
                    keep
                });
                // Drain EOF and ready work before admitting peers, then reclaim
                // the least recently used idle slot even if it is <1s old.
                // Otherwise rapid sequential requests can fill the table with
                // recently closed/idle sessions and have their next call reset.
                if let Ok(stream) = completed.try_recv() {
                    add_client(&mut clients, stream);
                }
                if let Ok((tcp, _)) = http_listener.accept() {
                    if let Ok(stream) = socket(tcp) {
                        add_client(&mut clients, stream);
                    }
                }
                esp_idf_svc::hal::delay::FreeRtos::delay_ms(1);
            }
        })?;
    ThreadSpawnConfiguration::default().set()?;
    Ok(())
}

fn respond(listener: &Context, request: &Request, body_ms: f64) -> Response {
    // Never hold network and service locks together; preserve setup routing
    // and the transport-neutral API's authentication and durable writes.
    let (setup, joined) = {
        let net = listener.net.lock().unwrap();
        (net.setup_mode, net.sta_ip.is_some())
    };
    let mut lock_ms = 0.0;
    let reply = if request.path == "/setup/wifi" {
        let provisioned = listener.shared.lock().unwrap().state.provisioned;
        listener.net.lock().unwrap().page(request, provisioned)
    } else if let Some(to) = setup.then(|| net::captive(&request.path, joined)).flatten() {
        Response::redirect(&format!("http://{}{to}", net::AP_IP))
    } else {
        let waiting = Instant::now();
        let mut service = listener.shared.lock().unwrap();
        lock_ms = waiting.elapsed().as_secs_f64() * 1000.0;
        let failed_before = service.latched().is_some();
        let reply = api::handle(&mut service, &listener.ctx, request, now_ms());
        if !failed_before && service.latched().is_some() {
            incident(Kind::StorageFailed, 0, 0);
        }
        reply
    };
    reply.with_timing(&format!("body;dur={body_ms:.3}, lock;dur={lock_ms:.3}"))
}

fn incident_now() -> u64 {
    unsafe { (sys::esp_timer_get_time() / 1000) as u64 }
}
fn incident(kind: Kind, code: i32, ms: u32) {
    LOG.record(incident_now(), kind, code, ms);
}
