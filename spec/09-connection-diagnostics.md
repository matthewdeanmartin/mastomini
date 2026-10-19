# First connection failures and a small incident history

## Seven-day uptime investigation, 2026-10-16

### Follow-up: Firefox 158 capture identifies LNA connection failures

The user confirmed Chrome is fast and does not fail. The supplied compressed
Profiler export, `Firefox 2026-10-16 19.16 profile.json.gz`, covers
23:16:43.165–23:17:47.560 UTC, Firefox 158 release build `20261007201254`.
The updated `firefox_log.py` reads this format directly and reports:

- 57 observed HTTP/2 session closes attributed to the board: 45 with
  `804b005d` (`NS_ERROR_LOCAL_NETWORK_ACCESS_DENIED`), 12 clean.
- 53 connection CloseTransaction observations with that LNA code, plus
  one with `804b0002`. These include repeated attempts, not unique failed
  application requests; heap addresses are reused throughout the capture.
- 60 stream-close observations with `804b0065`
  (`NS_ERROR_NET_UNCLEAN_SHUTDOWN`). Firefox can retry this condition, so
  counting only the older `80004004` misses relevant stream errors, while
  equating all 60 with user-visible failures overcounts them.
- Firefox sent GOAWAY code 2 on 45 observed occasions and code 0 on 12;
  no received GOAWAY was matched. There were 32 LNA permission-granted
  callbacks across the capture (that callback count is not host-filtered).
- Native hostname lookup started at 23:16:51.135; a usable record is
  observed around 23:17:04.924 (~13.8 seconds). Later LNA failures occur
  on established TLS/H2 connections, so slow DNS alone cannot explain them.

For example, at 23:17:08.693 a connection to `mastomini.local` is closed
with `804b005d`, its streams receive `804b0065`, and Firefox sends GOAWAY
2. Subsequent attempts repeat at approximately 0.35–0.42-second intervals.
The trace identifies Firefox's LNA rejection as the immediate connection
failure mechanism, **despite the configured exemption**. It does not show
why that exemption is ineffective. Do not simply prescribe setting the
same preference again or claim the setting is absent.

The symbolic codes are confirmed by Mozilla's
[error definitions](https://github.com/mozilla-firefox/firefox/blob/main/xpcom/base/ErrorList.py).
Current upstream transaction code checks the target against the skip list
before LNA permission state; this source inspection does not establish
which code path or cached preference state the user's binary used.

Next controlled checks: fully exit and relaunch Firefox with the existing
exemption, then retest. If failures persist, test granting **mawkingbird.com**
Local network devices permission, or adding the initiating domain to the
existing comma-separated skip list (`mastomini.local,mawkingbird.com`).
Mozilla [documents source and target exemptions](https://support.mozilla.org/en-US/kb/control-personal-device-local-network-permissions-firefox).
These are proposed tests, not verified fixes. Capture again to confirm
the LNA closes disappear. Chrome is the verified working option meanwhile.
Do not reboot or change firmware to address this demonstrated browser path.

Analyzer improvements: processed JSON/gzip ingestion, shared string tables,
time ordering across threads, event-time host attribution to avoid pointer
reuse corrupting old events, numeric GOAWAY preservation, and separate
unclean-stream counts. Generic URL redaction must not erase `code=2` before
parsing. The parser emits structured fields, not raw headers or cookies.
The original capture is ignored by Git. Summarized output is local at
`.local/profile-analysis-v2.txt`; 20 diagnostics/read-only tests pass.

### Initial live-board measurements

User reports roughly 50% failures / "CORS Failed" in mawkingbird.com on
Firefox on a laptop. The saved default-release Firefox profile on the test
machine already contains `network.lna.skip-domains = mastomini.local`; the
user also confirmed that setting. Do not assume the old LNA workaround is
missing or that peer GOAWAY identifies the underlying cause.

The live S3 is still build `63bcb3c208dc`, uptime 603,860,804 ms (6.99 days),
without a reboot for this investigation. Before probes, its counters showed:

- No Wi-Fi disconnects, reconnect attempts, allocation failures, worker
  stalls, request timeouts, storage failures, H2 refusals or recorded H2
  protocol-error GOAWAYs from the server.
- 395 peer resets, 200 peer GOAWAYs, 30 handshake failures, 3 established
  TLS failures, 4 full handoffs, 9 admission rejections and 15 evictions.
  These are lifetime totals, **not** the user's current request failure rate.
  The recent ring includes peer GOAWAY code 2 and repeated TLS write errors.
  A peer can terminate a connection because of browser policy or a server
  response it dislikes; this does not exonerate the server implementation.
- Internal free RAM 6,103 bytes, largest block 4,864, lifetime minimum 2,819.
  The October 9 sample of the same build already had minimum 2,819, free
  7,199 and largest block 5,632. Connection occupancy differs, so this is
  not a controlled leak measurement. RAM headroom is poor, but there is no
  newly lowered internal minimum or recorded allocation failure here.

Read-only verification on the still-running board:

- Three HTTP/2 startup bursts: all 96 streams answered with CORS in
  0.50–0.56 seconds per burst. A subsequent verbose burst also answered
  all 32 streams. **The saved diagnostic token is rejected with 401** on
  protected routes: this verifies transport and CORS on error responses,
  not successful authenticated timeline loading. The probes themselves
  incremented the HTTP error counter (13 unauthorized responses per burst).
- Short public-metadata HTTP/1.1 and HTTP/2 tests, one and two concurrent
  clients: no request or connection failures; request counter increased
  by 1,335 over the whole transport run. Account/post counts stayed 9/86.
  H1 warm p95 82–84 ms; H2 warm batch p95 196–261 ms. Cold household-page
  loads were about 2.7 seconds H1 / 0.85 seconds H2. This is not a soak test
  or a reproduction in the user's Firefox profile.
- Verified HTTPS with IP pinned and hostname/SNI preserved returned HTTP
  200 in 430 ms, while the same hostname URL exceeded an 8-second DNS
  deadline. Three later captures returned verified HTTPS in 390–422 ms,
  no incident deltas, and each OS lookup exceeded four seconds. A lookup
  allowed 20 seconds finally returned the correct IP after **13.859 seconds**.
  This establishes very slow OS name resolution; Firefox's resolver path
  still needs a fresh trace. It does not establish that every CORS failure
  is caused by DNS, particularly on already-established H2 connections.

Local raw evidence is in ignored `.local/uptime-{sys,incidents,log}-before.*`,
`.local/uptime-watch-20261016.jsonl` and
`mastomini_rs/loadtests/reports/transport-uptime-seven-days-20261016-190209.json`.
No firmware, browser settings or router configuration were changed.

### Diagnostics and improvements in priority order

1. Use the new `loadtests/watch_health.py` while reproducing with a fresh
   Firefox `about:logging` Networking capture. It appends UTC, boot ID,
   uptime, heap, worker samples, counter deltas, verified direct-IP HTTPS
   timings and bounded OS resolution results. Its DNS child process can
   actually be terminated on timeout. This extends the board's ~160-second
   sample history without adding board RAM or flash writes. See the
   load-test README for the command. The observer itself adds connections.
2. Analyze the Firefox trace with `firefox_log.py`; correlate failed requests
   and connection-close reasons with the capture. Inspect DNS and socket
   activity too, rather than treating code 2 as proof of LNA. Browser
   captures can contain credentials and should remain local.
3. If Firefox confirms lookup delay, compare the **same HTTPS hostname**
   using a temporary hosts mapping to the reserved DHCP address. This
   preserves origin, OAuth identity and certificate checks. If that fixes
   the issue, provide stable LAN name resolution and investigate multicast
   discovery; changing the login server URL to an IP confounds the test.
   This mapping was proposed, not applied.
4. Split firmware `net.errors` into HTTP 4xx/5xx, parse failures and transport
   failures. Add bounded H2 failure context: active stream count, last stream
   ID, connection age, last frame type and peer GOAWAY code, plus peer
   RST_STREAM counters. Avoid raw headers, URLs and GOAWAY debug strings.
   Current lifetime totals and peer GOAWAY alone cannot localize a failure.
5. Add coarse long-duration heap/occupancy trend buckets or retain host-side
   samples over days. Compare free/largest internal blocks at equivalent
   idle occupancy before changing TLS slot counts. Raising limits with only
   a few KiB of internal RAM is not supported by these measurements.
6. At the Angular client layer, bounded read deadlines and at most one
   delayed retry for idempotent network failures can reduce transient pain.
   Preserve the original failure in diagnostics; never retry uncertain writes
   blindly or treat status 0 as an invalid token. This does not fix DNS or
   explain Firefox's connection teardown and was not implemented here.

The collector was exercised against the board, and all 17 diagnostics/read-only
tests pass locally. No claim of a
fixed client failure is made without a fresh Firefox reproduction.

## Earlier investigation

Investigation and implementation, 2026-09-26. The RAM recorder and Health page
are implemented and built locally, but have not been flashed or validated on
hardware. The earlier phone incident has no saved trace, so its cause is not yet
established. Startup fixes discussed below remain separate work.

## What the current code and measurements show

- Last captured board boot reached `Ready` at 6,638 ms after boot. That does not
  establish what happened during today's phone attempts, or how long discovery,
  certificate verification and first-page loading took on the phone.
- First connection is a different workload from established Locust sessions.
  There are two pending TLS slots, a four-second handshake deadline and eight
  established HTTPS slots. Silent peers can occupy both pending slots; excess
  connections wait in the TCP backlog. Completed handoffs and established-client
  admission can also drop connections. The deadline starts after acceptance,
  not when a browser first tries connecting. More connection attempts can make
  admission pressure worse even when subsequent requests are fast.
- Before this change, `server.rs` discarded handshake failures, deadlines, full
  handoffs and admission rejections without retained reasons. These paths now
  record numeric events and counters.
  Slow-request tracing starts after request bytes arrive: it cannot explain DNS,
  TCP backlog or certificate failures, nor time before request receipt.
- There is a concrete freeze risk: the maintenance loop holds `net` while
  `keep_connected()` calls blocking Wi-Fi connect and IP acquisition. Request
  handling takes the same mutex before dispatch. The pinned esp-idf-svc 0.52.1
  connection wait alone permits 15 seconds. A reconnect can therefore stall the
  single HTTP worker, including diagnostics. Reconnect is checked every 30
  seconds. This is a code finding, not evidence that this phone incident was a
  Wi-Fi reconnect. The fix should separate a cheap connection-state snapshot
  from ownership of the Wi-Fi driver and drive reconnection without holding a
  request-path lock across waits.
- Browser differences make trust/discovery worth isolating. If this was Android,
  DuckDuckGo has an [open report about rejecting an installed local CA while Chrome accepts it](https://github.com/duckduckgo/Android/issues/5497).
  This is a matching symptom, not proof about the user's OS/browser version.
  Compare the same exact URL, hostname versus IP, actual error text, and whether
  requests reach the board. Preserve certificate verification during testing.
- The Angular API wrapper has no explicit fetch deadline. Startup verifies a
  stored login; a network error is represented as status 0, which does not clear
  that login. A prolonged network wait can therefore look like a stuck startup.
  Add bounded waits and explicit retry UI; do not automatically retry writes
  whose commit outcome may be unknown.

## Implemented: under 4 KiB retained RAM

The recorder moved into miniframework (`miniframework::incidents`, re-exported
as `mastomini::incidents`; NanaCoin still has its own copy). Every framework
transport and Wi-Fi event is recorded before any app observer sees it, and the
history is served unauthenticated at `/.well-known/incidents`, like
`/api/v1/sys` and `/api/v1/log`. The admin-only `/api/mastomini/v1/diag` still
includes `incidents`; Health displays the events, samples, counters, the
previous boot's retained entries and a local JSON download. A random boot ID and
uptime identify each history. No raw request data, credentials or client
addresses are recorded.

The framework added what the October 2026 CORS investigation lacked: TLS
connection failures after the handshake (with the mbedTLS code; peer resets are
counted only), ESP-IDF's failed-allocation hook (size and heap capabilities, so
an esp-aes DMA bounce-buffer failure is visible), a `heap_low` incident when
internal RAM reaches a new low under 16 KiB (with the open connections), and
`Ready` when serving starts.

Retained recorder state is compile-time checked below 4 KiB. The independent
sampler (the network core, priority 3) adds a 6 KiB task stack, in PSRAM when
the board config allows PSRAM stacks, plus RTOS overhead. The native recorder
mutex is initialized once at boot before workers start. Snapshot vectors and
JSON serialization also use temporary memory during retrieval. The 4 KiB bound
is the retained recorder, not the entire diagnostics feature.

Matching kind/code within five seconds coalesces with first/latest uptime and
maximum duration; new entries overwrite the oldest inserted slot. The ring holds
48 events and 32 five-second health samples. Counters include lost events;
`dropped` counts missed event/sample updates and `overwritten` counts replaced
event slots. Idle expiry only updates its counter. Worker gaps of at least two
seconds emit recovery events on resumption; the independent sampler also records
observed stalls. Sampling may itself be delayed; it is not a watchdog.

TLS/socket errors retain numeric codes. Worker code 0 is TLS and 1 is HTTP;
request timeout code 0 is receive and 1 is send. The Wi-Fi event subscription
retains disconnect reason codes; connection transitions also come from sampling.
Reconnect attempts/errors, boot/readiness, invalid HTTP, slow replies, allocation
failures and newly latched storage failures are captured.

Host tests cover flooding, coalescing, overwrite order, contention, heartbeat
wrap/races, recovery and worst-case JSON size. NanaCoin's allocation-counting
test checks the identical writer/sampler makes zero allocations. Both UI suites
test rendering and export. Hardware fault injection and load comparison remain
pending an authorized deployment.

### Design constraints

Use fixed-size records with numeric event kinds and arguments, not formatted
strings, heap allocation or flash writes. For example, 48 event records capped
at 40 bytes (1,920 bytes), plus 32 compact health samples capped at 32 bytes
(1,024 bytes), leaving room for counters and metadata. Assert the actual total
size at compile time. Use uptime timestamps and a boot identifier, so this works
before the clock is synchronized.

Record state transitions and exceptional events:

- Startup milestones; Wi-Fi disconnect reason, reconnect start/result and IP
  acquisition; retain elapsed time and signal strength when available.
- TLS initialization/handshake failure with the original numeric error code,
  timeout, full handoff queue, admission rejection and abnormal socket errors.
- Slow dispatch/send, incomplete request deadlines, memory allocation failures,
  storage failure transitions and HTTP-worker heartbeat gaps followed by recovery.
- Per-event counters and high-water marks for pending/established connections,
  plus last/max handshake duration. Ordinary idle expiry is a counter, not an
  error event. Coalesce repeated events with first/last time and repeat count;
  expose overwritten-record and dropped-event counts.

Sample heap free/largest block, RSSI, connection counts and independent TLS/HTTP
worker heartbeats every five seconds: roughly 160 seconds of recent context.
The sampler must not acquire the service or network mutex; otherwise the same
freeze could stop both serving and observation. Emit a recovery event when a
worker resumes, including its observed gap. A gap shows stalled progress, not
automatically a crash or its cause.

Writers should use short nonblocking critical sections, counting dropped events
on contention, with no network calls or formatting inside. `/diag` copies the
bounded snapshot and serializes afterward; recording must never acquire the
service lock or introduce a reversed lock order. Diagnostics remains admin-only.
The Health page shows recent incidents, age, repeat count, numeric reason and
small context, with a JSON download. Include startup progress and counters even
when the ring contains no errors.

Never store tokens, cookies, passwords, bodies, raw URLs/query strings, client
IP addresses or user-agent strings. Use a finite route/category ID where needed.

## Browsers: Firefox Local Network Access (October 2026)

Firefox 157 checks Local Network Access (LNA) for every request a public site
(a web client such as Mawkingbird) makes to a household board. While a
request's permission is pending, Firefox can still place it on a shared
HTTP/2 connection; the check then fails (`804b005d`) and Firefox closes the
whole connection. The blocked request is retried, but the other requests on
that connection are aborted and reach the page as "CORS request did not
succeed" (status 0), with nothing wrong on the board: its incidents show only
`h2_peer_goaway` and peer resets. HTTP/1.1 hides the effect because each
request owns its connection.

Workaround per browser: `about:config` → `network.lna.skip-domains` =
`mastomini.local` (measured: requests 50–250 ms instead of 1–2 s, no
failures). Clients should still retry an idempotent request once on status 0,
and must never treat status 0 as a rejected token.

## Recovery, reboot and the client's view

A RAM ring survives a temporary freeze and is available after recovery. The
counters and the 16 newest entries are also mirrored to RTC memory (about 1 KiB,
each entry checksummed), so the next boot serves them as `previous` after a
crash, watchdog or software reset. Nothing survives loss of power; the UI says
which applies.
The existing reset reason tells us the broad restart category, not the preceding
events. Firmware already enables flash core dumps and has a coredump partition;
verify dump extraction and decoding in a separate controlled crash test before
relying on it. A dump may help after a supported panic, not power loss or every
kind of hang. Do not add per-request flash logging to solve this.

The checksummed RTC-retained summary described above is now implemented in
miniframework; it is not a power-loss guarantee.

Mawkingbird's ongoing API trace complements the board log: it can record request
start, elapsed time, outcome and loss of contact while the board cannot answer.
A similarly bounded browser trace can record sanitized route, method, status,
duration and generic network failure, starting before each fetch and updating on
completion. It cannot record events before its JavaScript loads, nor reliably
distinguish DNS from TCP/TLS failures using fetch errors alone. Correlate traces
using board boot ID/uptime and client timestamps; do not claim a missing board
event proves DNS failed.

After deployment, validate with a silent ClientHello, bad TLS handshake, full admission
queue, truncated request, forced worker delay and controlled Wi-Fi reconnect.
Verify useful events, bounded memory, flood coalescing, no secret leakage and
successful history retrieval after recovery, then repeat the read-load baseline.
