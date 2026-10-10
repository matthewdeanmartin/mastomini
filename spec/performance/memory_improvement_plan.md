# Internal RAM improvement plan (ESP32-S3 board)

Status: proposed, October 2026. Nothing below is implemented yet.

## Why

The board has 8 MiB of PSRAM, nearly all free, but only ~390 KiB of internal
RAM. Internal RAM is what TLS's DMA bounce buffers, Wi-Fi and flash-writing
task stacks need. When it ran out (internal low-water 23 bytes, October 4–8)
`esp-aes` allocations failed and HTTPS responses were cut off mid-body;
browsers reported that as CORS errors. Putting Wi-Fi/lwIP buffers in PSRAM
(`CONFIG_SPIRAM_TRY_ALLOCATE_WIFI_LWIP`) stopped the failures, but the
low-water under load is still only ~2.8 KiB (`DEPLOY.md`, 2026-10-09). The
goal is headroom: no `heap_low` under the benchmarks, not more throughput.

## What holds internal RAM now

| Holder | Size | Source |
|---|---|---|
| Main task stack | 64 KiB, ~22 KiB never used | `CONFIG_ESP_MAIN_TASK_STACK_SIZE`; health line `stack free main` |
| Serving task (`mf-serve`) stack | 32 KiB, use unknown | miniframework `BoardConfig::serve_stack`; writes flash, so internal |
| TLS handshake task stack | 24 KiB, ~20.5 KiB never used | health line `stack free tls` (measured with RSA; re-measure with ECDSA) |
| Incident sampler + LED stacks | 6 + 4 KiB | internal because the S3 profile sets `psram_stacks: false`; neither writes flash |
| Wi-Fi static RX + TX buffers | 16 + 16 × ~1.6 KiB | forced by `TRY_ALLOCATE_WIFI_LWIP` (RX was 10, TX dynamic) |
| Every allocation under 16 KiB | varies with load | `CONFIG_SPIRAM_MALLOC_ALWAYSINTERNAL=16384`: headers, JSON, most responses go internal first |

## Steps, in order

Each step is one firmware change, flashed by Procedure A and measured before
the next. Keep a step only if nothing regresses.

1. **Measure first.** Report the serving task's stack high-water and internal
   RAM by capability (DMA-capable, 8-bit) in `/api/v1/sys` and the health
   line (miniframework `esp/mod.rs::log_health`, `sys.rs`).
2. **Small allocations to PSRAM.** Lower `CONFIG_SPIRAM_MALLOC_ALWAYSINTERNAL`
   from 16384 to 4096, then 1024 if step 2 helps. One config line; likely the
   largest dynamic gain.
3. **Right-size stacks, with margin.** Main 64 → ~40 KiB, TLS 24 → ~10 KiB
   (after re-measuring with ECDSA), sampler and LED stacks in PSRAM (a
   per-task option in miniframework rather than `psram_stacks` for all).
   About 40 KiB back. A stack overflow resets the board: load-test each.
4. **Wi-Fi static RX buffers 16 → 10**, set explicitly. ~10 KiB.
5. **Spend the headroom only if needed** (see "Not now").

## How to measure (same for every step)

- `make bench-transport` (defaults) and the Locust sweep
  (`loadtests/sweep.py`, levels 1–16), compared with
  `loadtests/compare_reports.py` against
  `loadtests/baselines/transport-ecdsa-tls12-20261009.json`.
- `loadtests/transfer_stalls.py --scheme both --runs 15` on a quiet board.
- `/.well-known/incidents` after each run: no `allocation_failed`,
  `tls_connection_failed` or `h2_goaway`; count `heap_low`.
- Internal low-water from `/api/v1/sys` (`heap.internal_min`) at the end.

Done when, after the benchmarks: internal low-water ≥ 16 KiB, no
`heap_low` incident, and throughput and p95 no worse than the baseline.

## Not now

- **More TLS slots (8 → 12).** A slot is one open TLS connection: a browser on
  HTTP/2 uses one or two however many requests it makes, so 8 slots already
  serve 4–8 tabs or devices. Revisit only for many simultaneous devices or
  HTTP/1.1 clients.
- **Chasing the Sep 26 saturation throughput** (86.6 vs 75 req/s at 8
  Locust clients, ESP-IDF's server vs miniframework). It does not affect a
  household's experience; profile the serving loop separately if it matters.
- **Two boards (front end + app server).** Request time is ~86% transport for
  small requests, but the expensive request, the home timeline, is ~380 ms of
  handler time; a second board would add a link and a serialization hop
  without removing either. (A second board for federation is a separate
  proposal: `../federation_roadmap.md`.)

## Risks

- Stacks too small: a reset. Mitigate with measured high-water + 8 KiB margin.
- PSRAM is slower than internal RAM: watch handler times (`Server-Timing`).
- Fewer Wi-Fi RX buffers: watch the transfer-stall test and p95.
