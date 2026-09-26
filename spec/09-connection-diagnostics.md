# First connection failures and a small incident history

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

`mastomini_rs/src/incidents.rs` contains the recorder, with the same implementation
in NanaCoin. The existing admin-only `/api/mastomini/v1/diag` now includes
`incidents`; Health displays the events, samples, counters and a local JSON
download. A random boot ID and uptime identify each history. No raw request data,
credentials or client addresses are recorded.

Retained recorder state is compile-time checked below 4 KiB. The independent
core-0 sampler adds an 8 KiB task stack plus RTOS overhead. The native recorder
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

## Recovery, reboot and the client's view

A RAM ring survives a temporary freeze and is available after recovery. It does
not survive a normal reboot or loss of power; say that explicitly in the UI.
The existing reset reason tells us the broad restart category, not the preceding
events. Firmware already enables flash core dumps and has a coredump partition;
verify dump extraction and decoding in a separate controlled crash test before
relying on it. A dump may help after a supported panic, not power loss or every
kind of hang. Do not add per-request flash logging to solve this.

If later needed, evaluate a tiny checksummed RTC-retained incident summary for
supported reset types; it is not a power-loss guarantee. Keep that separate from
the first RAM-only implementation.

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
