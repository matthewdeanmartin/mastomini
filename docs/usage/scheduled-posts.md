# Scheduled posts

Mastomini stores scheduled drafts; **mastomini-bots owns their timers**. If
either board is unavailable at publication time, it retries and publishes at
the first opportunity, with no lateness cutoff.

## Enable

1. On the bots board, keep an existing bot's mastomini server address and API
   key configured. That bot does not need to be enabled. Its address must point
   back to this mastomini board over HTTPS, and the key must allow `write` or
   `write:statuses`. A member's existing API key works; it need not be an admin
   key. The two boards must trust the same household CA.
2. As a mastomini administrator, open **Server → Scheduled posts**. Enter the
   bots board address (normally `https://mastomini-bots.local`) and that same
   existing mastomini API key. Save it. This authorizes the trusted bots board
   to publish any member's queued posts, but only when they become due.
3. Open **Scheduled posts** in mastomini, write a post, select its audience and
   a time at least five minutes ahead, and submit. Times in the form use your
   device's local time zone. The first handoff automatically registers the
   timer service on the bots board; there is no new machine password or token.

Refresh the scheduled-post list to see whether handoff is pending. A pending
handoff is saved durably, but the bots board has not acknowledged it yet.
**Server → Scheduled posts** shows the last handoff error. **Device → Scheduled
posts** on the bots board shows its timer queue; its Activity page reports
callback failures. Check the server address and update the same existing key
on both boards if it is revoked or replaced.

Both boards need the firmware implementing this feature. Building the firmware
does not enable the integration on an already-running board.

## Behaviour and limits

- Up to **16 outstanding jobs per mastomini board**, including failed jobs and
  cancellations still awaiting handoff. Cancel failed jobs to free their slots.
- Public, unlisted and followers-only posts, replies, content warnings and polls
  are supported. Poll duration starts at actual publication time.
- Scheduled direct messages are rejected because their encryption keys are
  available only during a member's authenticated session. Attachments remain
  unsupported, as for ordinary posts. Draft text plus warning is capped at
  2,048 UTF-8 bytes, in addition to normal post length limits.
- Drafts are excluded from ordinary posts, profiles, search and notifications.
  A real status and its notifications are created at publication time.
- Change the time or cancel through the scheduled-post list. Cancellation takes
  effect on mastomini immediately, even while the bots board is offline.
- A removed, disabled, suspended or security-reset author cannot publish the
  queued draft. A missing reply target or other permanent validation failure
  marks the job failed. Network failures, rate limits and storage outages retry.
- No polling scans or scheduled publications run inside ordinary user requests.
  Mastomini's existing maintenance loop delivers one pending handoff every five
  seconds, with network I/O outside the service lock. It keeps only a bounded
  metadata index in RAM; draft bodies live in flash and are read on demand.
- The bots board uses a separate timer worker, checking once a second, independent
  of slow LLM jobs. Failed callbacks retry after 30 seconds. These are best-effort
  times: clock sync, Wi-Fi, lock contention and publication still take time.
  There is no claim of zero resource cost: handoff and ordinary publication
  require work, and the bots worker reserves a 16 KiB stack.
- Publication reserves an ID durably before writing the status. The status write
  is the commit point; boot repairs interrupted acknowledgements. Retried
  callbacks do not duplicate a published or subsequently deleted status.

## API

The ordinary member bearer token authenticates these Mastodon-compatible calls:

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/api/v1/statuses` | Add `scheduled_at` as an RFC3339 timestamp; returns a ScheduledStatus |
| `GET` | `/api/v1/scheduled_statuses` | List your pending/failed jobs |
| `GET` | `/api/v1/scheduled_statuses/:id` | Read your scheduled draft |
| `PUT` | `/api/v1/scheduled_statuses/:id` | Change `scheduled_at` |
| `DELETE` | `/api/v1/scheduled_statuses/:id` | Cancel |

Use an `Idempotency-Key` for submission retries. Mastomini retains that key with
the queued draft and a bounded publication receipt; this is not an unlimited
submission history. The household form reuses its key after uncertain failures
until the draft changes or submission succeeds. API responses expose
`mastomini_delivery` (`pending_handoff`, `scheduled`, `failed`) and
`mastomini_error` alongside the standard fields. These responses are `no-store`.

An admin configures the integration using `PUT
/api/mastomini/v1/admin/scheduler` with `bots_url` and `api_key`; `GET` returns
status without the key. Changing configuration requeues handoffs for outstanding
jobs. The bots board accepts `POST /api/v1/scheduler/jobs` only with a Mastodon
API key already stored in a bot configuration, and derives the callback origin
from that configuration, never from the submitted job. This endpoint stores
only a job ID, revision and due time. Its authenticated `GET /api/v1/scheduler`
admin endpoint lists timer jobs without exposing the key.

Callbacks use `POST /api/mastomini/v1/scheduler/publish/:id` with the same
existing bearer API key and a `revision`. Mastomini checks the key's validity,
scope, configured delegation, current job revision, author and due time. No
general posting credential for the author is copied or minted.

HTTP is permitted only for loopback desktop testing; board handoffs require
HTTPS with certificate verification and no redirects. Desktop handoffs trust
`MASTOMINI_SCHEDULER_CA` (default `certs/household-ca.crt`).

Storage schema 6 adds queue records in `mm_cfg`. Existing stores upgrade in
place; older firmware will refuse a schema-6 store. Back up before a deployment
if you need the option of reverting firmware.

## Verification

Run each crate's `cargo test --lib`, both UIs' `npm test`, and the cross-board
contract test with `cargo test --manifest-path tests/scheduler/Cargo.toml` from
the repository root. The contract test connects the real APIs through a test
transport and simulates a lost publication reply. Main-server tests inject
power cuts at each publication write boundary and check isolation,
rescheduling, cancellation, idempotency and late publication. Board firmware
builds validate both ESP32 transport paths; hardware timing and memory still
need measurement after deployment.
