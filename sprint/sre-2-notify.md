# SRE sprint 2: notifications that reach you

Status: **built 2026-10-24, not flashed.** Bots board only.
Code: `mastomini_bots/src/notify.rs` (shared: settings, channels, ledger,
quiet hours, delivery log). SRE wiring is in `sre.rs` (`notify`, `check`).
`make test` (188) and `make lint` pass.

## As built (differences from the plan below)

- **Email goes through Brevo's HTTPS API** (`POST api.brevo.com/v3/smtp/email`,
  `api-key` header), the same way gamematcher and meritledger send mail. All
  three channels are HTTPS requests on the board's existing client.
  **Gmail is not supported:** SMTP is a separate protocol over a raw TLS
  socket, and the bots firmware only has `esp_http_client`. Adding it means a
  new esp-tls transport tested on hardware. Deferred until someone needs Gmail.
- ntfy: `POST {server}/{topic}` with Title, Priority (4 for notify, 3 for
  issue) and Tags headers, plus an optional Bearer token. The topic is stored
  as a secret because it works as the password on ntfy.sh.
- DM: a `direct` post mentioning `notify_dm_to` (added `Visibility::Direct`).
- **One message per run**, covering every finding that is new, worse, or due a
  reminder. It's sent *before* the digest is posted, so a Mastodon outage
  doesn't hide it. The ledger is saved right away, so a retried digest never
  notifies twice (tested).
- Fingerprints: `host|rule` for conditions, `host|rule|boot` for resets, so
  each reset is notified once.
- Quiet hours hold notices until the first run after them. There's no
  "page" level that breaks through quiet hours yet. Add one if needed.
- **The test button is the existing connection check**: it sends
  "[SRE] Test notice" to every configured channel and reports each result
  (for example "email: 401 from Brevo: check the API key (not the SMTP key)").
  The delivery log (last 5) is in state `notify_log`, for sprint 3's UI.
- **NVS budget is now the top risk.** The settings gained up to ~300 bytes of
  keys, and the state gained the ledger (≤1 KiB) and the log (~300 B). The
  whole bot record must fit in 4000 bytes, and a failed save is ignored
  silently. If that happens, the ledger stops saving and notices repeat (up to
  the daily cap). **Sprint 3 should start with a guard and a size readout.**

## Plan (as written before building)

## Problem

You've had no email because no email code exists. Findings that matter sit in
a followers-only Mastodon post you may not read.

## Goals

1. **A `Notifier` interface in mastomini_bots** (shared, not SRE-only, so other
   bots can use it later) with three channels:
   - **Email over SMTP**: host, port, implicit TLS (465) or STARTTLS (587),
     username, app password, from, to. Plain text with a short subject:
     `[SRE] gamematcher.local crashed twice`.
   - **Mastodon DM**: a direct-visibility status to a configured account on
     the bot's own server (reuses the existing posting key).
   - **ntfy**: POST to `{server}/{topic}` with Title, Priority and Tags
     headers, and an optional access token.
2. **Routing by runbook action.** Only `notify` and `issue` findings (sprint 1)
   notify. Choose channels per severity: e.g. page = all three, notify = DM + ntfy.
3. **Don't nag.** Notify once per fingerprint when it's new or gets worse, not
   on every run. Optional re-notify after N days. Quiet hours (no notifications
   overnight except pages). Cap notifications per day.
4. **A test button per channel** (`POST /api/v1/bots/sre/notify/test`) that
   returns the real error ("535 auth failed: use an app password, not your
   Gmail password").
5. **A delivery log**: the last 20 attempts with channel, ok or error, and time. Shown in sprint 3's UI.

## Risks

- One more TLS client on the bots board: measure heap during an SMTP send
  alongside a Mastodon post. Send notifications one at a time on the existing worker.
- SMTP from an ESP32: keep it minimal (EHLO, AUTH PLAIN or LOGIN, MAIL, RCPT,
  DATA), bounded replies, and a timeout. Test against a local fake SMTP
  server in the desktop build.

## Tests

- Fake SMTP and ntfy servers (desktop). Check the exact bytes sent, and how
  auth errors come out.
- Dedup: the same fingerprint on three runs → one notification. Worsening → a second one.
- Quiet hours honoured, and pages still get through.
- An e2e notification from a crash fixture all the way to a fake ntfy server.
