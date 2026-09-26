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
