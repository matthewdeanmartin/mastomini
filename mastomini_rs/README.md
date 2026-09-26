# Mastomini server

Rust server for the household Mastodon instance. See [deployment](DEPLOY.md)
for board builds and flashing, and [project documentation](../docs/index.md).

## Read-only HTTPS capacity

Tiny dataset; public metadata reads, one request per client at a time.

| Concurrent readers | Measured result |
|---|---|
| **8** | **95 reads/s**, 84 ms average, 160 ms p95; **0 failures / 5,725 reads** |
| 10–12 | About 0.7% failed; occasional 4–8 second delays |
| 16 | **24% failed**; maximum delay 12.7 seconds |

Board recovered; two accounts and two posts unchanged. See the
[load-test README](loadtests/README.md) and [full performance findings](../spec/08-performance.md)
for workload, timings, limitations and reproduction commands.
