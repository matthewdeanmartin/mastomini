# API performance: evidence and tuning plan

Source audit: 2026-09-25 working tree, including the HTTPS work. The reported
1.5 s, 3 s and 15 s client timings are symptoms, **not yet reproduced or
attributed in this audit**. Do not choose a five-minute cache TTL before
separating connection, queue, handler and transfer time.

## What the code tells us

| Finding | Evidence | Consequence |
|---|---|---|
| Normal social reads use RAM/PSRAM | `domain/mod.rs` boot reconstruction; `domain/query.rs`; `domain/oauth.rs::principal` | There is no per-request SQL lookup to cache. Authentication scans in-memory token records; it does not run password PBKDF2 for normal bearer requests. |
| Shared service lock | `bin/desktop.rs::serve`, `bin/esp32.rs::register` | HTTP and HTTPS serialize service work. A slow write/sign-in can hold up unrelated reads. Within each listener, handlers are synchronous too. More sockets do not create more handler CPU capacity. |
| Connection setup is material | Comments and prior board measurements in `bin/esp32.rs` | Prior measurements: approximately 1 s fresh TLS handshake versus 0.1 s on an open connection. These are previous measurements, not new results from this audit; see "Board transport" for the rebuilt listeners. |
| Burst capacity is small | `bin/esp32.rs`: HTTPS 5 sessions, HTTP 4, LRU purge; `sdkconfig.defaults` | Excess active/idle connections can cause purging and expensive reconnects. Previous HTTP-only 6-versus-10-socket tests in spec/04 did not fix backlog stalls. They do not establish current dual-listener HTTPS capacity. |
| Former account JSON repeated scans | `api/entities.rs::account`; `domain/query.rs::statuses_count`, `last_status_ms`, `follower_counts` | Each timeline row rebuilds its author. `statuses_count` scans up to 4,096 posts per rendering; 40 rows can repeat 163,840 status inspections, plus boosts/notifications and other scans. This was optimized below; it was not established as the dominant board cost. |
| Common work on every request | `api/mod.rs::dispatch`; `domain/polls.rs::announce_polls`; `domain/dm.rs::open_session` | Previously poll expiry scanned every request and bearer requests opened the device key eagerly. Both are now deferred as described below. |
| Repeated DM/thread work | `entities::status`, `filters::annotate`, `domain/moderation.rs::thread_muted`, `domain/conversations.rs` | Thread walks, filtering and decryption can add work for large pages. DM decryption uses asymmetric crypto; avoid repeating it within the same request. |

Espressif documents substantial initial HTTPS-session setup cost and much faster
reused sessions in its [HTTPS performance guidance](https://docs.espressif.com/projects/esp-idf/en/v5.5/esp32s3/api-reference/protocols/esp_https_server.html#performance).
The [HTTP server configuration](https://docs.espressif.com/projects/esp-idf/en/v5.5/esp32s3/api-reference/protocols/esp_http_server.html)
exposes backlog, socket limits and LRU purging as distinct settings. Check the
actual pinned `esp-idf-svc`/IDF mapping before changing these; reserve internal,
DNS and other stack sockets as well as client sockets.

## Measure client bursts reproducibly

`scripts/bench-api.py` only performs GET requests. It never provisions the board,
posts data or changes settings. Use an existing token with read scopes, supplied
via `MASTOMINI_BENCH_TOKEN`; the tool does not print/save it or response bodies.
Queries and endpoint paths *are* saved, so do not put secrets in them.

From `mastomini_rs/`, with that environment variable set:

```sh
uv run --project clienttests python scripts/bench-api.py https://mastomini.local \
  --ca ../.local/ca/ca.pem --concurrency 1 2 4 --rounds 10 \
  --output ../.local/bench-https.json
```

Use the actual CA file path for this installation. First run small, then include
8 workers to reproduce the observed burst boundary. The default endpoint set is
instance, credentials, home, notifications and featured tags. For a public-only
baseline without a token:

```sh
uv run --project clienttests python scripts/bench-api.py http://127.0.0.1:8080 \
  --path /api/v2/instance --concurrency 1 2 4 --rounds 10
```

Each worker owns its connection pool. Compare `new` versus `reuse` modes,
and first versus subsequent requests. Reuse is attempted, not guaranteed:
HTTP `Connection: close` or LRU purging can force a new connection. There are
no application retries or redirect following. Failures and non-2xx statuses
are counted separately; a fast 401/404 must not look like an optimization.
Any failed request makes the command exit nonzero. Percentiles use nearest rank;
small sample counts are smoke evidence, not stable tail-latency estimates.

Repeat with the same dataset/token at 0, typical, and near-capacity status counts
on a disposable desktop store, then representative data on the board. Include
DM-heavy conversations, notification pages and replies, not just an empty home
timeline. Record firmware commit, release/debug profile, HTTP/HTTPS, Wi-Fi RSSI,
free/largest internal heap, payload bytes, concurrency and sample count.
The script sends repeated bursts to each endpoint separately; also capture a
real client's mixed startup traffic for scheduling/lock contention effects.

For a fresh public request, curl can separate DNS/TCP/TLS from TTFB:

```sh
curl --cacert /path/to/ca.pem -sS -o /dev/null \
  -w 'dns=%{time_namelookup} tcp=%{time_connect} tls=%{time_appconnect} ttfb=%{time_starttransfer} total=%{time_total}\n' \
  https://mastomini.local/api/v2/instance
```

Those are cumulative timestamps. `appconnect - connect` approximates TLS time;
TTFB after TLS includes request transmission, server queue and work. A 15-second
outlier could be a timeout/retry, a blocked listener, network loss or handler work;
the source audit alone cannot choose among them.

## Implemented four-step performance work

1. **Transport visibility and repeatable bursts.** Both listeners now report
   monotonic `Server-Timing: app;dur=..., body;dur=..., lock;dur=...`. Slow requests
   (250 ms after acceptance) log sanitized route, status, bytes, body/lock/send
   and total time. Desktop `MASTOMINI_TRACE_TIMING=1` enables all timing logs;
   for the board set that variable **when building**. No tokens, bodies, IDs or
   hashtag names are logged. The benchmark saves header arrival time, complete
   response time and server stages. A bounded DNS preflight distinguishes a
   resolver failure from an HTTP latency sample.
2. **Request memoization.** Rendered account JSON is reused for repeated authors
   during one API request, at most 16 accounts. DM decryption results are cached
   only in that authenticated request: at most 64 messages / 32 KiB of text.
   Cached strings and the device secret are zeroized on drop; mutations clear
   the plaintext cache. Visibility checks remain on every normal render path.
3. **Exact derived account statistics.** A lazy RAM index computes counts and
   latest-post timestamps for all 16 accounts in one pass, then serves constant
   time lookups across requests. Relevant record writes/deletes invalidate it;
   the next read rebuilds it. Tests cover follow/block/delete, ring eviction,
   restart and slot reuse. Existing count/last-post semantics are preserved.
4. **Maintenance and crypto.** Poll scanning stops until the next expiry
   deadline; poll writes/deletes invalidate that deadline. Device-key unsealing
   is lazy: ordinary non-DM reads never do it. Password hashing strength is
   unchanged. Existing encryption, expiry, edit and revocation tests still run.

Transport was then measured on the board and rebuilt; see "Board transport"
below. Server timings start after acceptance and cannot see DNS, TCP, TLS or
backlog waiting. Near-zero `lock` time does not rule out waiting in a
synchronous listener.

## Board transport (2026-09-25, measured before and after)

Handler time on the board was already small (`app` p95: 1 ms for
`/api/v1/instance/rules`, 7 ms for `/api/v2/instance`). The time went to how
responses and connections were handled, which the source audit above could not
see. Reading esp-idf-svc 0.52.1 and ESP-IDF 5.5.3 found:

| Cause | Evidence | Fix (`src/bin/esp32/server.rs`, `sdkconfig.defaults`) |
|---|---|---|
| Each response was dozens of writes | `httpd_resp_send*` (httpd_txrx.c) sends each header as four writes, then chunked body frames; esp-idf-svc always used chunked | Build status line, headers, `Content-Length` and body in one buffer; one `httpd_send` |
| Nagle's algorithm on every socket | lwIP default; small writes waited for the client's delayed ACK. A tiny response (`/rules`, 299 ms p50) was slower than a large one (`/instance`, 55 ms) | `TCP_NODELAY` in httpd's `open_fn`, before the TLS handshake |
| TLS handshake records also waited | mbedTLS flushes each TLS record separately (packing is DTLS-only) | Same `TCP_NODELAY`, set before `esp_tls_server_session_create` |
| No TLS session resumption | esp-idf-svc hard-codes `session_tickets: false` | esp-tls server session directly, `CONFIG_ESP_TLS_SERVER_SESSION_TICKETS=y` |
| An event-loop post on every TLS read/write | `esp_https_server` posts `HTTPS_SERVER_EVENT_*` per call | Own transport, no events |
| One stalled handshake blocks every HTTPS client for 10 s | esp-tls default server handshake timeout, one httpd task | 4 s handshake timeout |
| Wi-Fi modem sleep | Ping 6–235 ms to an idle board | `esp_wifi_set_ps(WIFI_PS_NONE)`: the board is mains powered |

Also: listen backlog 5 → 8, HTTPS sockets 5 → 7 (a browser keeps six),
`LWIP_MAX_SOCKETS` 16 → 20, TCP send buffer 4 → 8 segments, 16 KiB outgoing
TLS records (PSRAM), precomputed NIST fixed-point tables (flash). HTTP now keeps
connections open like HTTPS; the previous `Connection: close` header never made
httpd close anything, and least-recently-used purging still reclaims sockets.

Public endpoints, same Windows client over Wi-Fi, 5 rounds per worker
(`scripts/bench-api.py`, [before HTTP](performance/board-transport-before-http-2026-09-25.json),
[before HTTPS](performance/board-transport-before-https-2026-09-25.json),
[after HTTP](performance/board-transport-after-http-2026-09-25.json),
[after HTTPS](performance/board-transport-after-https-2026-09-25.json)):

| Request, open connection | Before p50 / p95 | After p50 / p95 |
|---|---:|---:|
| HTTP `/rules`, 1 worker | 299 / 367 ms | 9 / 14 ms |
| HTTP `/instance`, 1 worker | 55 / 74 ms | 18 / 27 ms |
| HTTP `/rules`, 4 workers | 157 / 645 ms | 36 / 102 ms |
| HTTPS `/rules`, 1 worker | 85 / 427 ms | 13 / 24 ms |
| HTTPS `/instance`, 1 worker | 52 / 77 ms | 31 / 95 ms |

| TLS connection setup | Before | After |
|---|---:|---:|
| Full handshake (new client) | 1.25–1.5 s | 0.89–0.96 s |
| Resumed handshake (returning client) | not supported | 13–50 ms |
| 4 new clients at once, p50 | 3.5–4.6 s | 2.8–3.8 s |

Small samples: smoke evidence, not stable tails. `bench-api.py` uses
`requests`, which never resumes TLS sessions, so its "new" rows are the full
handshake. Resumption was timed with Python's `ssl` module and a saved session.
A full handshake is almost all ECDHE: X25519 and P-256 measured the same, and
the S3 has no ECC accelerator. An ECDSA certificate would replace a
hardware-accelerated RSA signature with another curve operation, so the RSA
certificate stays. Ticket keys are generated at boot. A client that retains
and presents a valid ticket can resume; this does not guarantee that every
later connection resumes, especially concurrent initial connections.

Remaining tail: handshakes run on the single HTTPS task, so a new client's
~0.9 s handshake delays requests on other open connections (the 4-worker
open-connection p95 of about 1 s is startup handshakes). Clients should set
`TCP_NODELAY` (browsers and OkHttp do). Without it, the first request after a
resumed handshake waited ~160 ms for the board's delayed ACK. Not yet measured:
authenticated timelines with many posts on the board (needs a read token; the
board holds 2 posts), and internal heap under 7 TLS sessions.

## Follow-up: reproduced 5–7 second calls (2026-09-25)

The current board at `192.168.1.161` matches source fingerprint
`025990d42742`. Its version endpoint reports build time
`2026-09-26T00:43:59.782Z`, based on `5555dbb35fa7` with uncommitted changes;
those inputs now match committed tree `c3e6fac`. No firmware was flashed or
application data modified during this investigation.

Read-only tests against `/api/v2/instance` reproduce the reported delay:

| Experiment | Samples | Mean | Median / p50 | Maximum |
|---|---:|---:|---:|---:|
| Six simultaneous new HTTPS clients, first request | 6 | 4,769 ms | 5,361 ms (nearest-rank p50) | 6,549 ms |
| Four completely established HTTP clients | 40 | 47 ms | 42 ms | 142 ms |
| Four completely established HTTPS clients | 40 | 55 ms | 58 ms | 89 ms |
| Six completely established HTTPS clients | 60 | 158 ms | 84 ms | 1,521 ms |

All measured requests succeeded. During the six-client startup burst, handler
`app` time remained below 8.6 ms and `lock` below 0.01 ms. Even subsequent
requests on those connections could take 2.79 seconds while other clients
were still establishing TLS. The established-only tests first opened and
warmed every socket sequentially, then released a barrier for measured reads;
their results exclude setup without hiding failures. The six-client warm
outlier shows that isolation of TLS setup will not necessarily remove all
network/scheduling tails.

Two direct interference tests identify the blocking mechanism:

- A full new TLS connection plus one request took 1,057 ms. While it was
  being established, a request on an already warm connection took 914 ms.
- A separate TCP connection that sent no TLS ClientHello blocked a warm API
  request for **4,954 ms**. Its next four requests returned promptly.

`src/bin/esp32/server.rs::open_tls` synchronously calls
`esp_tls_server_session_create` on the same core-1 httpd task that serves every
HTTPS request. Its four-second `tls_handshake_timeout_ms` is **not a hard
four-second deadline** with the current blocking socket: IDF 5.5.3
`httpd_accept_conn` first sets `SO_RCVTIMEO` from `recv_wait_timeout = 5`, and
`esp_mbedtls_server_session_create` only checks the handshake deadline after
`esp_mbedtls_server_session_continue_async` returns. One underlying read can
therefore block the whole listener for five seconds before the deadline is
checked. Lowering that timeout alone would still leave all clients waiting
behind every normal one-second full handshake.

Connection persistence and tickets are already working: an explicitly resumed
TLS connection plus a request took 71 ms (`session_reused = true`). They cannot
protect established requests from another client's synchronous handshake.
The bundled Angular API wrapper uses ordinary same-origin `fetch` and has no
five-second retry, delay, or forced connection-close policy. A browser's actual
network waterfall was not accessible, so these tests establish server-side
mechanisms with matching symptoms, not which exact connection pattern caused
each user-observed request. Authenticated timelines and DM-heavy reads were
not measured; no token was supplied.

The next transport change should isolate bounded asynchronous handshakes from
established request serving, as now done in NanaCoin. Moving the existing
single HTTPS task to another CPU would move both the handshake and its waiting
requests together. Established request reads and writes also need bounded,
nonblocking handling: the current body/send helpers retry socket timeouts on
that same task. Retain persistence, tickets, certificate verification, existing
API/authentication behavior and serialized durable writes. This investigation
adds a repeatable probe and evidence; it does not deploy that transport change.

Reproduce the established/interference tests from `mastomini_rs`:

```sh
python scripts/probe-connection-queue.py --address 192.168.1.161 \
  --output ../.local/connection-queue.json
```

The probe verifies the CA and hostname, uses TCP_NODELAY, performs only public
GETs, and deliberately holds one silent TCP peer until its timeout. Run it
separately from other benchmarks. Saved evidence:

- [Six-client startup burst](performance/browser-burst-investigation-2026-09-25.json)
- [HTTP baseline](performance/http-burst-investigation-2026-09-25.json)
- [Established sessions and handshake interference](performance/connection-queue-investigation-2026-09-25.json)

## Isolated transport implementation

For subsequent phone first-connection problems and the proposed bounded RAM
incident history, see [connection diagnostics](09-connection-diagnostics.md).

The subsequent fix replaces the board's synchronous httpd transport with two
bounded tasks. Core 0 accepts TCP connections and advances at most two
nonblocking TLS handshakes, with a four-second elapsed-time deadline. A bounded
channel hands completed sessions to core 1, which multiplexes established HTTP
and HTTPS connections. Each client gets at most one socket read and one socket
write per iteration. Both loops use FreeRTOS delays, which yield instead of
busy-waiting below the scheduler tick. Rust `Builder::stack_size` sets the actual
24 KiB TLS and 32 KiB HTTP stacks; IDF thread configuration alone is overridden
by Rust's pthread attributes.

Limits: eight established TLS clients, four HTTP clients, two pending
handshakes, two completed handoffs, 60-second idle expiry, and ten-second
partial-request/response deadlines. Headers are limited to 4 KiB / 32 fields,
URIs to 1,024 bytes, and bodies to the existing 160 KiB upload maximum. Input
buffers start at 4 KiB, grow only for incoming bodies within a shared 512 KiB
growth budget, and shrink after uploads. Accepted clients always receive their
initial 4 KiB, so at most eleven such initial buffers can exceed that budget
before growth is paused.
API-specific body limits (normally 4 KiB) still apply. Dispatch pauses at 512 KiB
of queued response storage; one additional response up to 512 KiB can cross that
threshold. Large bodies are retained without the previous second full response
copy. Socket writes use bounded chunks and retain the exact unsent buffer on
TLS WANT_WRITE. A single reply over 512 KiB returns 503 instead of being queued.

`src/http_transport.rs` frames persistent requests and responses independently
of the firmware. It consumes request bodies before pipelined requests,
rejects ambiguous framing, supports validated `100 Continue`, and handles HEAD
and bodyless 204/304 replies. The existing API handles OAuth, authentication,
uploads, setup routing and storage under the existing service lock. TLS and
socket I/O never hold that lock. Session tickets, TCP_NODELAY, 240 MHz operation,
PSRAM TLS buffers and disabled modem sleep are retained.

The added host tests cover full-size fragmented uploads, pipelining,
authentication-header forwarding, malformed framing, Continue, partial response
writes, HEAD/204/304 and the response bound. `scripts/probe-transport.py` adds
read-only live checks for those wire behaviors, partial-request isolation and
the exact bundled JavaScript asset; a deliberately oversized GET body exercises
the upload-sized transport path while receiving the API's 413 response. It does
not create accounts, register apps or write household content.

Full cold TLS establishment still needs software key exchange. The change is
intended to prevent that work, and silent/partial clients, from stopping unrelated
established requests. It does not make slow application work or radio loss free.
Flashed on COM11 and verified on 2026-09-25 (build UTC 2026-09-26): firmware
fingerprint `62fba4ca191b`. The strict board probe, household certificate check,
source fingerprint check and all [live framing checks](performance/transport-framing-after-2026-09-25.json)
passed. Existing data remained at two accounts and two statuses.

The [post-flash queue probe](performance/connection-queue-after-2026-09-25.json)
measured hot requests during a silent handshake at mean 95 ms / max 109 ms,
versus the prior 4,954 ms stall. During an active cold handshake hot requests
averaged 113 ms / max 193 ms. TLS resumption remained enabled. The short
four/six-client burst results in this probe were slower (372–662 ms means) than
the subsequent sustained Locust sweep; Wi-Fi/client scheduling variability
means these are not a promise of constant per-request latency.

### Read-only Locust capacity

There is now a separate [Locust project and run instructions](../mastomini_rs/loadtests/README.md).
It enforces allowlisted GETs with no bodies or redirects, performs no login or
token creation, and does not seed or erase data. Public instance/discovery/status
reads work without credentials. An existing read token enables timeline,
notification and account reads; no token was available for this measurement.
Thus the results describe **small public metadata reads on the existing tiny
dataset**, not authenticated timeline or media capacity, and not browser users.

The [HTTPS sweep](performance/locust-read-capacity-2026-09-25.json) ran on
`192.168.1.161`, firmware `62fba4ca191b`, with one outstanding request per client,
no think time, ten seconds of warmup after gradual spawning, and thirty measured
seconds per stage. Each client reused its session when the board allowed it.
Warmup statistics are retained separately in the JSON.

| Concurrent readers | Successful reads/s | Mean ms | p95 ms | Failed / measured requests |
|---|---:|---:|---:|---:|
| 1 | 18.1 | 55 | 130 | 0 / 542 |
| 2 | 51.7 | 38 | 81 | 0 / 1,552 |
| 4 | 58.1 | 69 | 160 | 0 / 1,742 |
| 6 | 80.9 | 74 | 160 | 0 / 2,427 |
| 8 | 86.6 | 89 | 160 | 0 / 2,600 |
| 10 | 94.3 | 106 | 170 | 21 / 2,851 (0.74%) |
| 12 | 91.6 | 127 | 170 | 19 / 2,768 (0.69%) |
| 16 | 85.1 | 140 | 170 | 802 / 3,356 (23.90%) |

Latency statistics include failed attempts, which can fail quickly; a low p95
does not make the overloaded stages healthy. Maximum latency rose to 4.4 s at
ten readers, 7.6 s at twelve and 12.7 s at sixteen. Eight is a provisional
error-free concurrency ceiling for this workload; errors begin beyond the eight
established TLS slots. Sixteen crosses the sweep's 10% failure stop threshold.
All stages recovered afterward, uptime increased without a reboot, and counts
remained two accounts / two statuses. These short stages do not establish a
long-term stability guarantee or the limit for a populated household.

A separate [60-second confirmation at eight readers](performance/locust-read-confirmation-2026-09-25.json)
completed 5,725 measured reads with zero failures: 95.4 successful reads/s,
84 ms mean, 70 ms median, 160 ms p95, 270 ms p99 and 685 ms maximum.
The board again recovered with unchanged counts and uninterrupted uptime.

## Recorded results and commands

From `mastomini_rs/`:

```sh
make bench-read       # release, 4096 posts, home page of 40, 500 iterations
make bench-desktop    # release, temporary FileStore, 40 posts, real OAuth, HTTP
# Equivalent runner, with an explicit saved report:
uv run --project clienttests python scripts/bench-desktop.py \
  --output ../.local/bench-desktop.json
```

The desktop runner provisions **only a new disposable store** and removes it
when done. `bench-api.py` remains read-only for an existing board. The CPU
benchmark (`examples/read_bench.rs`) uses MemStore and excludes network, TLS,
FileStore and authentication; it isolates repeated rendering/count work. These
are permanent workloads, not timings obtained from a manually prepared server.

| Experiment | Before | After | Interpretation |
|---|---:|---:|---|
| CPU timeline p50, 4096 posts / 40 rows | 1.0668 ms | 0.6966 ms | 34.7% lower |
| CPU timeline p95, same 500-iteration workload | 1.3712 ms | 0.9145 ms | 33.3% lower |
| Desktop HTTP burst | No matched baseline | 700/700 successful | Five endpoints, concurrency 1/2/4, new/reuse pools |
| Board HTTP, existing firmware | Weak-signal baseline | 42/42 successful, max 400 ms | Public instance only; no optimized-firmware claim |

[CPU comparison metadata](performance/cpu-read-2026-09-25.json) records the
working-tree provenance and limitations. [Raw HTTP burst results](performance/desktop-burst-2026-09-25.json)
contain all samples and stage timings. This Windows desktop was also compiling
firmware during the burst run; its small samples are smoke evidence, not a
stable tail-latency estimate or a before/after transport comparison.

At four workers, home/20 had p50 **5.80 ms new / 3.47 ms reuse**, p95
**27.74 / 8.23 ms**, while handler p95 was **1.41 / 0.96 ms**. Both the client
pool/connection path and scheduling contribute outside the handler. All 700
responses were 200; empty notifications/featured-tag responses are explicitly
part of this small workload, so their speed proves no heavy-data scalability.
A release near-capacity CPU run covers the account-statistics optimization
separately. No 1.5/3/15-second stall was reproduced on desktop.

The initial board attempt could not resolve `mastomini.local` within five
seconds ([DNS diagnostic](performance/board-dns-2026-09-25.json)). After the user
supplied `192.168.1.161`, the [HTTP baseline](performance/board-http-2026-09-25.json)
completed 42/42 public instance requests on the existing firmware. One-worker
requests were 40–95 ms; four-worker bursts reached 400 ms. HTTP closes connections,
so reuse mode cannot promise actual connection reuse. A simultaneous 10-packet
ping averaged 158 ms (20–441 ms), with no loss; the user's preceding four pings
averaged 492 ms (13–1075 ms). Delays therefore occur outside the REST handler too.
Weak upstairs signal was reported by the user; the samples do not isolate radio
conditions from power saving, scheduling or other network effects.

After relocation, the same address became unreachable: three completed HTTP
attempts timed out connecting, and no ping reply came from the board. Further
bursts were stopped pending its current address. The [availability diagnostic](performance/board-relocation-unreachable-2026-09-25.json)
is not a downstairs latency comparison; HTTPS and relocated-board timing remain
to be collected. No firmware was flashed during these measurements.

Use the reachable IP for HTTP. For HTTPS, retain hostname/SNI verification while
bypassing unreliable `.local` DNS:

```sh
uv run --project clienttests python scripts/bench-api.py https://192.168.1.161 \
  --tls-name mastomini.local --ca certs/household-ca.crt \
  --path /api/v2/instance --concurrency 1 2 4 --rounds 10 \
  --output ../.local/bench-board-https.json
```

This verifies the certificate against the household CA; it never disables TLS
verification. Record the
firmware, Wi-Fi signal, heap and payload sizes alongside it. Add DM-heavy and
mixed real-client traffic before changing socket budgets. A shared five-minute
cache cannot fix DNS or TLS reconnect costs and risks stale blocks/auth/notes.

Only if these measurements implicate lock hold time should the service move to
bounded read snapshots or split critical sections. A blind RwLock conversion
would race request key state, token last-use and poll notifications.

## Where a five-minute microcache fits

| Data | Suggested policy | Required invalidation / key |
|---|---|---|
| Static emoji/trend/unsupported lists | Already trivial; caching adds little | None; do not mistake these stubs for real features |
| Public rules/about/instance configuration | Optional bounded serialized cache, 30–300 s after measuring | Immediate invalidation on settings, rules, terms, contact/profile changes; key by API version and origin. Split live usage counts from static configuration or deliberately define their allowed staleness. |
| Account counts/profile rendering | Request-local first; then exact derived indexes | Per account, reset on all mutations and slot reuse; JSON also depends on base URL and moderation |
| Rendered ordinary post text | Small bounded LRU only if rendering is significant | Post ID + edit revision + origin; remove on edit/delete/eviction; apply visibility independently on every request |
| Timelines, relationships, notifications, auth | No shared five-minute response cache | Follows/blocks/mutes/expiry, read markers, token scopes/revocation, viewer flags and new posts require immediate effects |
| Decrypted messages/device secrets | Request-local only | Do not extend key/plaintext lifetime across requests to save work |

For any long-lived cache, set an explicit entry/byte bound (ESP32), report hits,
misses and bytes, and keep correctness tests for mutation/invalidation. Success
means improved p50/p95 and fewer long-tail failures at the same concurrency,
without new stale data or heap exhaustion. The implemented caches are bounded and invalidated immediately; no five-minute response cache or socket-limit change was introduced.
