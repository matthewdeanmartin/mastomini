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

Scheduled posts can use the trusted bots board for timing; see
[scheduled-post setup](../docs/usage/scheduled-posts.md).

## Property tests

`cargo test --locked` runs the `proptest` suite as well as example tests.
The properties generate Unicode and binary parser inputs, clock extremes,
namespace/capacity operations, follow/block sequences, reboots, and password
changes interrupted before persistence or after a lost acknowledgement.
Store and social state are checked against independent reference models.
The desktop FileStore property also exercises real compaction on reopening.
These are host tests; they do not exercise ESP-IDF or physical power cuts.

Run just the properties, or increase the generated cases per property:

```bash
cargo test --locked --test properties
PROPTEST_CASES=1024 cargo test --locked --test properties
cargo test --locked --no-default-features --test properties
```

The default is 256 generated cases per property. Failures shrink to smaller
inputs or shorter command sequences and persist seeds in
`proptest-regressions/properties.txt`; keep that file in version control when
a failure occurs. Turn confirmed bugs into named example regressions too.
For a reproducible generated run, set `PROPTEST_RNG_SEED=123456` (or another
chosen integer) along with the case count. Saved regressions replay automatically.
Property testing explores the
generated input space; passing is not a proof of correctness.

## Troubleshooting

**Firefox: "CORS request did not succeed" from a web client hosted elsewhere**
(for example Mawkingbird). Firefox's Local Network Access checks each request
a public site makes to your home network; while a check is pending it can drop
a shared HTTP/2 connection and every request on it. Exempt only the board:
`about:config` → `network.lna.skip-domains` = `mastomini.local` (comma-separate
several), then restart Firefox. Pages served by the board itself are not
affected.
