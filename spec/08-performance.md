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
| Connection setup is material | Comments and prior board measurements in `bin/esp32.rs` | Prior measurements: approximately 1 s fresh TLS handshake versus 0.1 s on an open connection. HTTPS keeps connections alive; HTTP explicitly closes them. These are previous measurements, not new results from this audit. |
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

Transport tuning remains **measurement-gated**: HTTPS reuse was already enabled;
HTTP closes connections. No socket/backlog increase was made without board heap
and burst evidence. The pinned `esp-idf-svc 0.52.1` sets `backlog_conn=5` internally
and exposes no configuration field for it; changing it requires a dependency
change or reviewed wrapper patch. Five TLS and four HTTP client slots are kept.
Server timings start after acceptance and cannot see DNS, TCP, TLS or backlog
waiting. Near-zero `lock` time does not rule out waiting in a synchronous listener.

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
