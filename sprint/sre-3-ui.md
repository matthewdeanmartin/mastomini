# SRE sprint 3: the SRE section of the site

Status: **built and deployed 2026-10-24** to the bots board (COM15,
`ac:a7:04:2c:38:b8`, app-only flash). `make check` passed: 190 Rust tests,
19 UI tests, 12 e2e.

## As built

- `/sre` route with nav entry "SRE". Tabs: Fleet, Findings, Runbooks
  (per-rule select, saves only differences into `runbook_rules`; reset to
  defaults), Notifications (channels, test notice, last 5 deliveries),
  History (the existing history and actions components moved here), and Setup
  (the SRE bot's settings via `SettingFields`; the Mastodon server and key stay
  on Bots). The Bots page card now links to `/sre` instead of embedding history and actions.
- One endpoint, `GET /api/v1/bots/sre/overview` (session required,
  `no-store`). It reads bot memory only, never calls a board, and never returns secrets.
- **The 4000-byte guard** (`Service::save`): a record that wouldn't fit forgets
  its largest *state* values (never settings or keys) and logs an `error`
  activity event naming them. A failed NVS write is now logged instead of
  ignored. `record_bytes` appears on the page, with a warning above 85%.
- Not done from the plan: per-setting test buttons (one "send a test notice"
  covers the channels), "mute for N days", and live "check now" per board
  (the connection check reads every board).

## Verified on hardware

- Boot: 16 bots, admin password set, ready at 192.168.1.162; `make probe-board` passed.
  Health ok, 82 KiB internal free, 0 failed allocations.
- `/api/v1/bots/sre/overview` returns 401 without a session.
- Live error logs (read-only curl) match the parser: gamematcher `previous`
  shows `tls_failed`, then `allocation_failed`, then a crash (log tail: mbedTLS
  -0x7780); housemetrics shows a watchdog after `slow_request` x3.
  Response sizes are 7-12.3 KiB for incidents (16 KiB cap) and ≤2.2 KiB for the log (8 KiB cap).
- Not done (admin-only per DEPLOY.md): signing in, saving notification
  settings, a real digest run. The first real digest will start the 3-day heap baseline.

## Plan (as written before building)

## Problem

The SRE bot shares the generic pop-open settings panel with good-morning-bot.
That's fine for a bot with 4 settings, but not for one with 15+ settings, a
fleet, findings, runbooks and secrets.

## Goals

A `/sre` route with its own navigation. The bots list links to it instead of
opening the panel. Tabs:

1. **Fleet.** One card per board with status, build, uptime, last reset (and
   whether that was a fault or a power-on), and heap shown against the board's
   own baseline. Links to the board's health and error log.
2. **Findings.** New, ongoing and resolved, with fingerprint, first and last
   seen, and count. Each opens the incident detail (`previous` events, log
   tail). Actions: mute for N days, change this rule. Later (sprint 6): link to the issue.
3. **Runbooks.** The rule table from sprint 1: for each rule, the trigger, the
   default action, your override, and the "what to do" text. Reset to default.
4. **Notifications.** Channel setup from sprint 2, with test buttons, routing,
   quiet hours and the delivery log.
5. **Setup.** Every setting grouped by purpose: *Boards*, *Digest*, *Trends*,
   *Restart approvals (advanced, off)*, *Model*. Each secret says where it
   comes from, e.g. "Board approval key: on the board, open
   `https://<board>/` → Ops → Keys → Approval", with a test button that calls
   the board and reports 401, 404 (feature off) or ok. Missing pieces show as
   a checklist, not an error.
6. Keep the existing `sre-actions` and `sre-history` pages; move them under `/sre`.

## Backend

- `GET /api/v1/bots/sre/fleet`, `/findings`, `/findings/{fp}`, `/runbooks`
  (GET, PUT override), `/notify/log`, `POST /setup/test/{setting}`.
- Serve from bot state. No new board calls when a page loads; the fleet tab
  shows the last run's data plus a "check now" button.

## Tests

- Angular specs for each tab (`ng test`, not bare vitest).
- API tests in `server_tests.rs` for each endpoint, with secrets never echoed back.
- The old panel still works for the other bots.
