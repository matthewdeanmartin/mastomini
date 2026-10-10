# Read-only board capacity tests

Run from `mastomini_rs`. These tests only issue allowlisted GET requests, with
no bodies, redirects, automatic retries, account creation, login or token
creation. They do not clear or seed the board. Normal server maintenance may
still run while handling reads. Do not run other benchmarks simultaneously.

```powershell
uv sync --project loadtests --python 3.12
uv run --project loadtests pytest loadtests/test_readonly.py -q
uv run --project loadtests python loadtests/sweep.py --host https://192.168.1.161 --output ../.local/locust-read-capacity.json
```

The default sweep uses 1, 2, 4, 6, 8, 10, 12 and 16 concurrent clients, one
outstanding request each, without think time. Each stage spawns one user per
second, warms up for another 10 seconds, then measures 30 seconds. Startup
statistics are separate. Results include latency percentiles, throughput,
failures and per-endpoint statistics. It stops after a stage with over 10%
failed requests, failed recovery, changed account/post counts or detected reboot.
It does not reset an overloaded board. Run longer stages (`--seconds 120`) to
confirm a promising capacity range; these short sweeps are not soak tests.

The default profile mixes public instance, rules, emoji, description, OAuth
discovery, nodeinfo, status and version reads. Timelines require authentication
on Mastomini. To include timelines, notifications, featured tags and account
verification, set `MASTOMINI_BENCH_TOKEN` to an **existing** token with appropriate
read scopes, or set `MASTOMINI_BENCH_TOKEN_FILE` to an ignored file containing
only that token. Tokens and response bodies are not written into reports.
Without a token this measures public metadata capacity, not timeline capacity.
Non-200 responses, including permission errors, count as failures.

HTTPS verifies the household CA and uses `mastomini.local` for SNI while
connecting to the given IP. Override `MASTOMINI_CA` / `MASTOMINI_TLS_NAME` for
another household. Use `--host http://192.168.1.161` for a separate HTTP baseline.
Each client reuses connections when possible; beyond the board's eight TLS
client slots, connection churn can dominate. Clients are not browser tabs or
human users, and dataset size and Wi-Fi conditions affect capacity.

Standard Locust CLI/UI also works:

```powershell
uv run --project loadtests locust -f loadtests/locustfile.py --host https://192.168.1.161 --headless -u 4 -r 1 -t 60s --csv ../.local/readonly
```

The standalone CLI does not perform the sweep's recovery/count checks. See
[Locust documentation](https://docs.locust.io/en/stable/running-without-web-ui.html).

# Connection diagnostics

For "CORS request did not succeed", logins that will not stick, or slow
loads in a web client (Mawkingbird, Elk, Phanpy). In October 2026 every such
report was a dropped or refused connection, never a missing CORS header
(`DEPLOY.md`, `spec/09-connection-diagnostics.md`). Work down the list; each
step names the failure it rules in or out. All of them only read.

1. **The board's own view:** `curl http://<ip>/.well-known/incidents`.
   `h2_goaway` (the board ended a connection, with the HTTP/2 error code),
   `h2_refused` (too many streams), `evicted`, `allocation_failed`,
   `heap_low`, `tls_connection_failed`. Only `h2_peer_goaway` and
   `peer_reset` means the browser closed the connections: go to step 5.
2. **`make diag-h2-burst ADDRESS=<ip>`** (`h2_burst.py`): a browser's startup
   burst (a preflight and a GET per request) on one HTTP/2 connection. Every
   stream must be answered with CORS; exit status 1 otherwise.
3. **`make diag-transfer ADDRESS=<ip>`** (`transfer_stalls.py`): large
   responses over HTTP and HTTPS, p50/p90/max after the handshake and stalls
   over 300 ms (TCP retransmission timeouts). HTTPS much slower than HTTP
   points at the TLS write path; both slow at the network. Compare with ping
   to the router.
4. **`make diag-browser ADDRESS=<ip>`** (`browser_session.py`, Firefox or
   `BROWSER=chromium`): a real browser engine, cross-origin, with idle gaps
   (`--gaps`), large downloads (`--big`), slot pressure (`--squat`) and
   Firefox's networking log (`--moz-log`). Install the browsers once:
   `uv run --project loadtests playwright install firefox chromium`.
   `idle_close.py --idle 330` shows how the board closes an idle connection;
   `--handshakes 10 [--tls 1.2|1.3]` times full and resumed TLS handshakes
   (and shows the certificate's key type); `browser_session.py --trust`
   checks that Firefox and Chromium trust the board's certificate.
5. **The user's own browser.** Playwright's browsers do not have the user's
   prefs or permission prompts, so capture from the real one:
   - a HAR (devtools Network → Save All As HAR) → `make diag-har HAR=<file>`;
   - better, Firefox `about:logging` (preset Networking, save to a file) →
     `make diag-firefox-log LOG=<file>`, which names why each connection
     closed and spots Firefox's Local Network Access pattern (fix:
     `about:config` → `network.lna.skip-domains` = `mastomini.local`).

Authenticated runs: `oauth_token.py --address <ip> --username <name>`
(password from `MASTOMINI_PASSWORD` or a prompt) saves a read token to
`<repo>/.local/diag-token`; pass `--token-file ../.local/diag-token`.
A member keeps at most 8 sessions (a ninth evicts the least recently used),
so revoke it afterwards: `oauth_token.py --address <ip> --revoke`. The app
registration is reused, never re-registered. Every tool redacts tokens from
what it prints and saves; HARs and browser logs contain them, so keep those
files out of the repository.
