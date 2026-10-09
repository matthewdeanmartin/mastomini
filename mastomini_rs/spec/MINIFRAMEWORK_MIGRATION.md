# mastomini on miniframework

Status, October 4, 2026: **deployed with HTTP/2; boot, strict probes and
firmware fingerprint verified.** Upstairs transport measurements recorded in
`../DEPLOY.md`; downstairs comparison confirms TLS allocation failures during
cold loads despite stronger Wi-Fi signal.
Same approach as NanaCoin and Minicloud
(`../../nanacoin/nanacoin_rs/spec/MINIFRAMEWORK_MIGRATION.md`).

mastomini's HTTP server, connection loop and board startup were a sibling
copy of what became miniframework. Now the framework runs them and
mastomini is the household Mastodon app on top: its router (`api::handle`)
still answers every path itself, unchanged.

## What moved, and where

| Was mastomini | Now |
|---|---|
| `src/http_transport.rs` (HTTP/1.1 framing, pipelining, `100-continue`) | `miniframework::http` + `mux` (deleted here) |
| `src/bin/esp32/server.rs` (TLS task on core 0, connection loop on core 1, budgets, admission) | `miniframework::esp::Board::serve`, `BoardConfig::s3` (deleted here) |
| `src/bin/desktop.rs` on `tiny_http` (HTTP + rustls HTTPS) | `miniframework::desktop::serve_https` (feature `desktop-tls`) |
| Wi-Fi join, setup network `mastomini-setup`, captive DNS, saved network in `mm_wifi` | `BoardConfig::saved_wifi` / `setup_network` / `force_setup`, `esp::WifiSetup`, `miniframework::captive` |
| Setup page `/setup/wifi`, captive redirects | stay here (`bin/esp32/net.rs`, `captive.rs`): the framework has no UI |
| `/metrics` | framework built-in; `server.rs` adds `store_entries_used/total`, tag `target=` |
| Transport incidents recorded inside the server | `miniframework::incidents` (framework-owned recorder, sampler and RTC mirror; `/.well-known/incidents`) |
| SNTP, mDNS (`_https` TXT `path=/api/v1/instance`, `_http` → `/trust`) | the framework (`BoardConfig::mdns_https_path`) |

The adapter is `src/server.rs` (`Household`, `config`), shared by the
desktop and the board. `app_paths = ["/"]` hands every path to mastomini.

### Framework changes this needed (each with tests)

1. **CORS policy per path** (`Service::cors` → `Cors::Allowlist | Public |
   None`). Mastodon clients run on any origin: API paths are `Public`
   (`*`, also on refusals such as 413); OAuth and HTML pages are `None`
   (no CORS headers, no origin refused, the app answers `OPTIONS`).
2. **An app that owns every path**: an `app_paths` entry of `"/"`.
3. **`Reply::owned`**: an app that builds its own body hands it over
   without a copy and without the reply-buffer limit.
4. **App headers replace any site header** of the same name (was: only
   the four defaults), so mastomini's `Access-Control-Expose-Headers`,
   `Cache-Control` and `Server-Timing` are not doubled.
5. **`Service::metrics`**: app fields on the `/metrics` line;
   `heap_psram_min` added for boards with PSRAM.
6. **`Expect: 100-continue`** answered (was refused with 417; curl and some
   apps send it for uploads).
7. **Input buffers grow on demand** (4 KiB, up to header + body limit, given
   back after the request), as mastomini's server did; the framework
   allocated the whole limit per connection.
8. **Wi-Fi chosen at runtime**: saved network, setup network with captive
   DNS, `WifiSetup` for the app's page. New: while the setup network is open
   and nobody is on it, the saved network is retried every minute, so a
   board that booted before its router recovers by itself (mastomini stayed
   in setup mode until someone used it). The setup network also closes 20
   minutes after joining at the latest.
9. **Desktop HTTPS** (rustls, ALPN `h2` with `http2`), so `make run` and
   `clienttests/test_https.py` keep working and HTTP/2 over TLS can be
   tested without a board.
10. `Config::cors_max_age` (mastomini: 86400), `Request::headers()`,
    `status::SIGNALS.setup()`.

## Feature review: nothing lost

Checked by the unchanged suites: 190 Rust tests, smoke, 27 client tests
(Mastodon.py, HTTPS), 138 conformance tests (58 skipped, 14 expected
failures: identical to before), plus spot checks of preflight, HEAD,
`100-continue`, oversize bodies and HTTP/2 over TLS.

| Feature | Before | Now |
|---|---|---|
| HTTP :80 and HTTPS :443, both serving everything | own server | framework; same |
| HTTP/2 | no | `HTTP2=1` builds (ALPN `h2`); desktop always |
| Connection slots | 8 TLS + 4 HTTP, 2 handshakes | same (`Limits::large_board`) |
| Unsent-response budget | 512 KiB | 512 KiB (`config.limits.response_budget`) |
| TLS handshake timeout | 4 s | 10 s (framework) |
| Request deadline | 10 s | 5 s for headers/small bodies; uploads stream with a 10 s stall limit |
| Idle keep-alive | 60 s | 60 s |
| Pipelining, keep-alive, HEAD | yes | yes |
| `Expect: 100-continue` | yes | yes (now in the framework) |
| CORS: API, `/oauth/token`, `/oauth/revoke`, `/.well-known/`, `/nodeinfo/` | `*` | `*` (`Cors::Public`); preflight still answered by mastomini (200, max-age 86400) |
| CORS on pages | none | none (`Cors::None`) |
| Body limits | 4 KiB; 160 KiB for `update_credentials` | same (upload route streamed) |
| 413 body | `{"error":"Request body is too large"}` | framework's `{"error":"too_large","message":…}` with CORS `*`: **wording changed** |
| `Server-Timing` | `app`, `body`, `lock` | same |
| Slow-request log line (`timing method=…`) | ≥ 250 ms or `MASTOMINI_TRACE_TIMING`, incl. send time | same trigger; `handler_ms` (sending is the framework's; slow sends are `SlowRequest` incidents) |
| `/metrics` | mastomini's line | framework's line, same field names plus `requests`, `errors`, `kib_out`, TLS counts; keeps `store_entries_*`, `heap_psram_min`, `target` tag |
| Incident history (`/api/mastomini/v1/diag/events`) | own recorder | the framework's (`mastomini::incidents` re-exports it); also public at `/.well-known/incidents` |
| RGB status light, Morse phrase, setup colour | yes | yes (mastomini's own light, fed from framework signals) |
| Saved Wi-Fi in `mm_wifi`, built-in developer Wi-Fi saved on first join | yes | yes (same namespace and keys: existing boards keep their network) |
| Setup network, captive portal, close 120 s after the household exists | yes | yes |
| `MASTOMINI_FORCE_SETUP=1` | yes | yes (`BoardConfig::force_setup`) |
| Modem sleep off | yes | yes (framework) |
| Scheduler bridge to Minicloud every 5 s | main loop | `serve` tick, every 5 s |
| Heap line every 60 s | `heap internal free=…` | framework `health:` line every 60 s |
| Boot log lines `store: …`, `Ready at … (https://IP/ …)`, `Setup network … is up` | yes | yes (`make boot-log` parses them) |
| Startup failure | red light, log, halt | red light, step blinks, log every 5 s, **explanation on http://IP:8080/** from step 5 on (new) |
| New public endpoints | | `/api/v1/sys`, `/api/v1/log`, `/api/v1/schema` (framework built-ins). `/api/v1/log` shows the device log to anyone on the LAN, as on NanaCoin; it holds no tokens or passwords |

## Not done yet

- Resolve TLS memory pressure: identical upstairs/downstairs benchmarks are
  saved in `../loadtests/baselines/`. Downstairs logs report AES allocation
  failures and TLS write errors during cold loads; internal heap low-water
  reaches 23 bytes. Stronger signal improves warm throughput but does not fix
  cold-load failures. Accounts/posts remained 7/38 and no reboot occurred.
- Diagnose the owner's power-on/RST observation separately: one USB socket
  shows no power LED with the same cable/charger; the other requires RST to
  start the diagnostic light. Current network logs capture successful startup
  after RST, not the failed power-on before it. See `../DEPLOY.md`.
- The fingerprint now includes miniframework's sources
  (`build.rs` and `scripts/firmware-version.py`).
- `scripts/deploy.py` and `probe-board.py` keep their own checks; moving
  them onto `miniframework/tools/boardsafe` (as NanaCoin did) is optional.
