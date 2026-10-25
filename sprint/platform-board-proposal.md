# Proposal: a household platform board (ops.local)

Status: proposal, 2026-10-24. Nothing built. The owner decides.

## Why

The SRE bot has outgrown its slot on the bots board:

- **One 4000-byte NVS record** holds its settings, keys, baselines, open
  findings, the notification ledger and history. Sprints 1-3 had to cap
  every one of them (5 readings per board, 6 resets, 12 ledger entries) and
  add a guard that forgets memory to make it fit.
- **One daily run.** It shares a worker with 15 other bots, so "down" means
  "down at 07:30". It can't tell how long an outage lasted, or catch a board
  that hung overnight and was RST'd before breakfast.
- **No archive.** A board keeps only its last 16 events across a reset (RTC
  memory) and loses them on power loss. Today's live read found a real
  crash chain on gamematcher (TLS failure → allocation failure → crash), and
  it vanishes at the next reset unless someone saves it.

Meanwhile **housemetrics** runs on an ESP32-S2 with RAM-only storage (3 days of
rollups). It is itself one of the boards that watchdogs (`slow_request` x3,
then a watchdog, in today's log). And the **issue tracker** needs a P4 with an SD card anyway.

## The proposal

One P4 (Wi-Fi via its C6, SD card, 32 MB PSRAM) runs three miniframework
apps in one firmware, each its own crate with its own routes, so each could
still deploy alone:

| App | Path | From |
|---|---|---|
| **Metrics**: collector and dashboard | `/metrics-ui`, `/api/v1/write`, `/api/v1/query` | housemetrics, moved to SD |
| **Watch**: continuous SRE engine, runbooks, notifications | `/sre`, `/api/v1/sre/*` | `sre_findings.rs` + `notify.rs` moved into a shared `household_ops` crate |
| **Tracker**: generic issues | `/tracker`, `/api/v1/issues` | tracker sprints 1-2, unchanged and generic |

The bots board keeps only a thin **SRE digest bot**: it reads
`ops.local/api/v1/sre/digest` and posts it to Mastodon. Mastodon stays where it is.

## What a whole board makes possible

Ordered by value to this household, highest first.

1. **Continuous watch** (every 60 s, not daily): real outage start and end
   times, per-board availability, and "came back by itself" vs "needed RST" with
   a timestamp. **The RST hang becomes provable:** when power returns, many
   boards report power-on within a minute. Any board that stays silent after
   that is the hang bug, named with the minute it happened.
2. **Incident archive on SD.** On every new boot, save that board's
   `/.well-known/incidents` and log tail permanently. Crashes get a history:
   "gamematcher has crashed 4 times since September, always after `tls_failed`."
   That's the evidence an issue needs.
3. **The tracker on the same board**: filing is a function call, not a network
   hop. Issues attach the archived incident snapshots. Dedup and auto-close
   (N days unseen) run on full history, not a 12-entry ledger.
4. **Real baselines.** Months of metrics on SD instead of 5 readings: "usual"
   by hour of day, leak detection over weeks (sprint 1's known limit goes
   away), and RSSI history per board to spot a moved access point.
5. **Synthetic checks**: what a person would notice. HTTPS handshake time,
   **certificate expiry** (the OPS.md runbook with no implementation yet), key
   pages (mastomini public status, NanaCoin diag), DNS and NTP answering.
6. **A household status page** (`ops.local/status`, public, read-only): "is it
   mastomini or my phone?" Current state, last 7 days, open incidents.
7. **Firmware fleet**: an inventory of every board's build, drift against the
   verified receipts, and later **staged OTA** from SD (miniframework has an OTA
   spec): one board at a time, a health gate after each, stop on regression,
   approval through the existing action flow.
8. **Backups with restore drills** (OPS.md's deferred last item): nightly pulls
   of NanaCoin's journal export, mastomini's backup and the tracker's export
   to SD, plus a monthly "restore to the desktop build and compare" check.
9. **Upstream watch**: the monthly DevOps review from OPS.md (ESP-IDF and
   crate advisories against the deployed builds). It files issues; it doesn't upgrade anything.
10. **Later, the SSO seed**: one always-on board that every app trusts is the
    natural home for household sign-in (an OIDC-style issuer), replacing the
    per-board passwords and the pairing sprint. Large; listed so the design
    doesn't block it.

## Risks and answers

- **Who watches the watcher?** The bots board's thin SRE bot already reads
  `ops.local` daily. Add a dead-man check: if `ops.local` misses two runs,
  it notifies directly through ntfy (it keeps that channel).
- **SD wear and corruption**: append-only journals, periodic compaction,
  read-only mode on card errors (the tracker sprint already plans this), and
  the backup job copies the board's own data off it too.
- **One board, three apps**: a crash takes them all down together. Isolate
  them with separate workers and memory budgets, and metrics ingest must
  never block the watch loop. The 32 MB PSRAM makes this practical, which it
  isn't on the S2 or S3.
- **Moving housemetrics**: run both for a week. Boards keep pushing to
  `housemetrics.local`, then point it at `ops.local` (or move the mDNS name),
  then retire the S2.

## What changes in the plan

| Was | Becomes |
|---|---|
| Tracker sprints 1-2 (own P4) | **P1 Platform skeleton + tracker core** on `ops.local` (tracker kept generic) |
| — | **P2 Watch engine**: move `sre_findings` + `notify` into `household_ops`, 60 s loop, SD-backed memory (no 4000-byte cap), incident archive |
| — | **P3 Metrics on SD**: port housemetrics with SD retention; dual-run, then retire the S2 |
| Tracker sprint 2 (UI) | **P4 Tracker UI + status page** |
| SRE sprint 4 (files issues) | **P5 Watch → tracker** in-process, with archived evidence and auto-close |
| SRE sprint 5 (pairing) | **P6 Pairing**, possibly replaced by the SSO seed (decide then) |
| — | Later: synthetic checks and cert expiry, OTA staging, backups, upstream watch |

What's already built (sprints 1-3) isn't wasted: the triage rules,
notifications and `/sre` UI are the parts that move. They were written as
plain Rust with no board I/O inside the logic, so they port as-is.

## Questions for the owner

1. One P4 for all three apps, or the tracker on its own P4 (more isolation, one more board to watch)?
2. Hostname: `ops.local`? Keep `housemetrics.local` as an alias for the boards that push to it?
3. Is the status page public on the LAN (anyone in the house) or admin-only?
4. Is SSO in scope for this board's design, or a separate later project?
