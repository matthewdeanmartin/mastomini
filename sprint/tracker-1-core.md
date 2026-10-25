# Tracker sprint 1: core API and SD storage

Status: **built 2026-10-24 (desktop).** `microcontroller/miniframework/apps/issue_tracker`,
default host `ops.local`. `make test-tracker` (6 tests, clippy, fmt) passes,
and a live desktop run was checked with curl (file, dedup, close, list, health).
See the app's README for the API.

## As built

- Same auth as housemetrics: reads are open on the LAN; tokens with scope
  `report` (file and comment) or `triage` (also change issues); the admin
  password for everything, HTTPS-only on a board. 240 writes an hour per token.
- Journal: length + CRC-32 + protobuf issue per change, plus a snapshot.
  Compacts past 256 KiB. A torn tail is cut on load, and a leftover
  `snapshot.tmp` is recovered.
  Found and fixed while testing: truncating an append-mode file is refused on
  Windows (and likely FAT), so the journal is opened read-write and seeks to the end.
- Dedup reopens unless the issue was closed `wontfix` or `duplicate`.
  Occurrence comments are at most hourly, and are dropped first when an issue
  passes 200 comments.
- Not done: the P4 binary (SD mount, Wi-Fi via the C6), the 10k-issue
  latency fixture, and an export importer. Labels and the UI belong to tracker sprint 2.

## Plan (as written before building)
Status: planned. New app `microcontroller/miniframework/apps/issue_tracker`.
Generic: the SRE bot is one client among many. Nothing SRE-specific in the
schema; SRE data goes in labels and fingerprints.

## Goals

1. **App skeleton** on miniframework, like `apps/housemetrics`: a desktop build
   first, then P4. Make targets for build, test, run-desktop, deploy.
2. **Data model**
   - Issue: `id` (sequential), `title`, `body` (markdown), `state`
     (open / closed), `labels[]`, `assignee`, `priority` (p0-p3 or none),
     `reporter` (token name), `created`, `updated`, `closed`.
   - **Dedup**: optional `fingerprint`, plus `count`, `first_seen`, `last_seen`.
     Filing with an existing fingerprint bumps the count and `last_seen`,
     appends a short occurrence comment (rate-limited), and **reopens** the
     issue if it was closed (unless it was closed as `wontfix`).
   - Comments: author, time, body. An event history covering state, label and assignee changes.
3. **REST API** (`/api/v1`)
   - `POST /issues` (with a fingerprint, it upserts), `GET /issues?state=&label=&q=&assignee=`,
     `GET/PATCH /issues/{id}`, `POST /issues/{id}/comments`, `GET /labels`.
   - **Per-reporter tokens**: named, scoped (`report` = file and comment only,
     `triage` = also edit, `admin`), revocable, hashed at rest. Rate limit per token.
4. **SD-card storage**
   - An append-only journal (one record per change) plus a periodic snapshot.
     Recover from the journal after power loss. Survive a pulled card
     (read-only mode with a clear health check, `storage` runbook).
   - `GET /api/v1/export` (JSON lines) for backups.
5. **Ops**: the `ops` feature so the SRE bot watches the tracker too (SD free
   space, journal size, card errors).

## Hardware step (when the P4 arrives)

- Board model and networking per the open question in the overview. Mount
  FAT on the SD card. Flash, then smoke-test via the e2e script against the
  real board.

## Tests

- Desktop: create, dedup, reopen-on-recurrence, token scopes, rate limits.
- Journal replay after a truncated last record (simulated power cut).
- Fixture: 10k issues, then list and search latency on the desktop build. Repeat on the P4.
