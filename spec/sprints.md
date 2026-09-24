# Sprint log

Quality gate for every sprint: `make check` in `mastomini_rs/` (Git Bash).
It runs `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
`cargo test --locked`, `make smoke` and `make client-test`.

## Sprint 1 — Scaffolding and storage engine: done

- Crate `mastomini_rs` (lib + desktop bin), Makefile, release profile.
- `store`: `Store` trait with NVS semantics; `MemStore`, `FaultStore` (power cut
  after N writes) and `FileStore` (CRC'd append log, torn-tail recovery,
  compaction on open, exclusive lock). Usage is counted in NVS 32-byte entries so
  the watermark behaves as it will on the board.
- `codec`: `[version][kind][postcard]` values; unknown versions fail closed.
- `ids`: snowflakes, monotonic across a clock stepping back; 13-char base32 keys.
- `domain`: accounts (16), statuses (4,096), boosts (2,048), reactions
  (16,384), follows, apps, tokens with epochs. Boot rebuild, orphan repair,
  rollback of interrupted provisioning, invariants, ring eviction (pins spared),
  low watermark, write governor. Notifications, markers, auth codes,
  app-only tokens, lockouts and idempotency keys are RAM only.
- `text`: Mastodon character counting (graphemes, URLs = 23, local-part
  mentions, CW counted) and Mastodon HTML rendering.

Evidence: power-cut-after-every-write tests for status delete, password change
(old sessions never survive a committed change) and provisioning; eviction and
watermark tests; governor, visibility, home-timeline, pagination, thread and
idempotency tests.

## Sprint 2 — HTTP, OAuth, accounts: done

- `http`: transport-neutral request/response; query, form, JSON and multipart
  parameters flattened to Rails-style names.
- `api`: instance v1/v2, nodeinfo, OAuth server metadata, `/api/v1/apps`,
  server-rendered `/oauth/authorize` (no JS, no cookies), `/oauth/token`
  (authorization_code with PKCE or client secret, HTTP Basic, client_credentials),
  `/oauth/revoke`; accounts (verify/update credentials, show, lookup, search,
  relationships, follow/unfollow, followers/following); CORS; body limits;
  clock-invalid 503 on writes; default avatar.
- `/api/mastomini/v1`: `status`, `provision`, `admin/members` (list, create).
- Desktop binary on `tiny_http` with `FileStore`.

Evidence: API tests through the full request path (PKCE and secret flows,
single-use codes, redirect validation, lockout, revoke, scope enforcement,
restart persistence) and `make smoke` against the real binary.

## Sprint 3 — Posting and timelines: done

- Statuses: create (form/JSON, `Idempotency-Key`), show (incl. boosts), bulk
  show, delete (returns source text for redraft), context, favourite, bookmark,
  pin, reblog/unreblog, favourited_by, reblogged_by, source, one-version history.
- Timelines: home (Mastodon reply rule, boosts honour `reblogs`), public, tag,
  account statuses with filters; favourites and bookmarks by reaction id;
  `Link` headers.
- Notifications (RAM ring): v1 list/get/clear/dismiss/unread_count, v2 grouped
  (one group per notification), policy. Markers (RAM). `/api/v2/search`.
- Neutral answers for Tier 2/3 endpoints (lists, filters, trends, conversations,
  blocks, mutes, …) and 422 for media, polls and scheduled posts.

Evidence: Rust API tests plus `make client-test`: Mastodon.py clients signed in
through the real OAuth form follow each other, post, reply, favourite, boost,
see each other on home, get notifications, paginate with `fetch_next`, and after
a server restart everything except notifications is still there.

## Sprint 6 (first slice, pulled forward) — Board bring-up over HTTP

Done at the user's request, before Sprints 4–5:

- `src/bin/esp32.rs`: Wi-Fi, SNTP (writes return 503 until the clock is set),
  mDNS `mastomini.local`, plain HTTP on port 80 ("Easy mode"), the same
  `api::handle` as the desktop build.
- `src/bin/esp32/nvs_store.rs`: `NvsStore` on the 8 MiB `store` partition, one
  namespace per table, commit after every write, never erases.
- `partitions.csv` and `sdkconfig.defaults` as in spec/01, with the secondary
  USB console enabled so boot logs can be read over the USB port.
- Scripts: `build-esp32.sh`, `firmware-image.py`, `deploy.sh`/`deploy.py`
  (app only, layout-checked), `install.sh`/`install.py` (first install:
  refuses mastomini boards, requires the MAC, full backup first), `boot-log.py`,
  `probe-board.py` (read-only). Runbook: `mastomini_rs/DEPLOY.md`.

Not in this slice: HTTPS (Sprint 8), LittleFS avatars, fixed PSRAM slabs and
allocation tests, hardware measurements (gate H1), soak and power-cut runs.

## First-time setup for non-administrators (pulled forward from Sprint 4)

- `src/api/setup.rs` (shared, tested on desktop): on an unprovisioned server,
  `/` is a plain HTML form (household name, owner username, password twice).
  Afterwards `/` explains how to connect a Mastodon app, using the address the
  visitor typed (an IP works on every phone), and has an "Add a family member"
  form confirmed with an admin's password. No sessions, no JavaScript.
- Firmware Wi-Fi onboarding (`src/bin/esp32/net.rs`): a board with no saved
  network, or one it cannot join, opens the open network `mastomini-setup`
  with a captive-portal DNS responder (`src/captive.rs`, tested on desktop).
  The page lists nearby networks, joins the chosen one while the setup network
  stays up, shows the board's new address, and saves the credentials in the
  system `nvs` partition. The setup network closes 2 minutes after the
  household is created (or 20 minutes after joining). Built-in developer
  credentials are now optional and, once they work, are saved on the board.
- Verified on the board: provisioning through the setup page; owner sign-in via
  Mastodon.py; setup network broadcast (open, channel 1) with a forced-setup
  build; rejoin from saved credentials with a build that has no built-in Wi-Fi.
  Not verified: joining a phone to `mastomini-setup` and completing the Wi-Fi
  form (the build PC's only network link is Wi-Fi). Needs a human with a phone.

## Moderation, collections and mastodon_mock conformance (pulled forward from Sprint 5)

- **Blocks and mutes** (persistent `B`/`M` edges), **conversation mutes**
  (`m` reactions on the thread root), filtering in home/public/tag timelines,
  threads, search, suggestions, directory and notifications; `Relationship` and
  `Status.muted` are real. `remove_from_followers`.
- **Reports** with `admin.report` notifications to every admin; **account
  moderation** (disable, silence, suspend, sensitive, delete with a power-cut-safe
  purge), roles, admin post deletion; the **Mastodon admin API** for accounts and
  reports, gated on role and `admin:*` scopes; the matching household endpoints.
  Disabled/suspended tokens answer `403` and recover when the action is undone.
- **Collections** (Mastodon 4.6): CRUD, items, revoke, account listings,
  `added_to_collection` notifications.
- **Store schema 2**: new record kinds and namespaces only; a schema-1 store is
  upgraded in place at boot (marker rewrite only).
- Smaller Mastodon gaps found by the conformance suite: bulk
  `GET /api/v1/accounts`, `familiar_followers` shape, `directory` ordering,
  single grouped notification (fetch, accounts, dismiss), `for_bots` in the
  notification policy, `/health`.
- **`make conformance`**: mastodon_mock's contract suite, copied verbatim, run
  against the desktop binary with real OAuth; deviations as data (spec/07).

Evidence: Rust domain tests (block and account-deletion power cuts at every
write, reboot round-trips, mute expiry, thread mutes, report capacity, slot
reuse, schema upgrade, collection consent and limits) and API tests through the
full request path; `make conformance`: 96 passed, 69 not applicable, 45 listed
deviations, no unlisted failures.

## About, terms, rules; encrypted direct messages

- `/about`, `/terms-of-service`, `/privacy-policy` (server-rendered, no sign-in);
  `GET /api/v1/instance/terms_of_service` (+ `/:date`), `/privacy_policy`,
  `/extended_description` with a date; `configuration.urls` in v2. Three default
  rules; generated terms and privacy policy; `GET/PUT /api/mastomini/v1/admin/server`
  edits name, description, rules and terms.
- Direct messages encrypted per participant (05 "Direct messages"): X25519 member
  keys sealed by password and per device by token, ChaCha20-Poly1305 envelopes,
  request-scoped unlocked keys. New records only; existing accounts get keys at
  their next sign-in. Firmware grows by ~60 KiB.

Evidence: crypto unit tests; API tests showing no DM text or content warning
anywhere in the store, admins (even via reports) and the server without a token
unable to read, keys surviving restart and password change, revocation removing a
device seal, lazy keys for old accounts; a power-cut-at-every-write test for
posting a DM.

## Sprint 4 (first slice) — Invites, reset codes, passwords, devices

The account-management half of the household app, as API endpoints plus plain
HTML pages in the style of the setup pages. The Angular app is not started.

- **Invite and reset codes** (`src/domain/codes.rs`, `mm_inv`, store schema 3):
  `POST /admin/invites`, `POST /admin/members/:id/reset`, `GET /admin/codes`,
  `DELETE /admin/codes/:id`, `POST /codes/redeem`. Hashed, single use, 7 days,
  8 live. Redeeming erases the code first, so a power cut can lose a code but
  never allows a second use. A reset gives the member a new key pair (old DMs
  unreadable, as spec/05 says) and signs out every device.
- **Me:** `POST /me/password`, `POST /me/sign_out_everywhere`,
  `GET /me/devices`, `DELETE /me/devices/:id`. A password change always signs
  out every device; the spec's `sign_out_everywhere: false` option was dropped
  because it would keep old sessions alive across a committed change.
- **Pages:** "Invite or reset a password" on `/` (admin password confirms, the
  result is the one-time link), `/setup/<code>` to redeem, `/setup/password` to
  change your own password.

Evidence: domain tests (single use, expiry, the 8-code limit, reboot, reset
rules, reset codes dying with their member and never matching a reused slot,
power cuts at every write while redeeming an invite and a reset, devices and
sign out everywhere) and API tests through the full request path for the JSON
endpoints and the HTML pages; `make check` green (conformance unchanged: 96
passed, 45 listed deviations).

Schema 3 upgrades a schema-2 store in place (marker rewrite only); no wipe
needed.

Deferred from Sprint 4, with the reason:

| Item | Why not now |
|---|---|
| Angular app, first-party OAuth client, `/connect` data-driven guide | Large, and several ways to build it; the plain pages cover the flows a household needs today |
| Avatar/header upload | Needs LittleFS on the board and a new account-record field; worth doing next, with the board at hand |
| `/admin/health` page, `POST /clock` | Board-side diagnostics and clock fallback; fits with Sprint 6/7 board work |
| `/trust`, `/admin/security` | Sprint 8 (HTTPS) |

## Sprint 4 (second slice) — The household app in Angular

- `mastomini_ui/` (Angular 22, modelled on nanacoin_ui), served at `/app/` by
  `src/web.rs` from gzip-only embedded assets (`--features bundled-web`, implied
  by `esp32`; `make web`, `make run-web`). 104 KiB gzipped.
- Pages: **My account** (devices with revoke, password change, sign out
  everywhere), **Members** (invite and reset links with QR code and copy, open
  links with cancel, disable/enable, silence, suspend, roles for the owner,
  delete with typed confirmation), **Server** (name, description, rules,
  terms), **Health** (`GET /status`).
- Sign-in is the real OAuth code flow with PKCE through `/oauth/authorize`; each
  browser registers the app on first sign-in (spec/06 "Authentication").
- The landing page links to the app when the build includes it.
- `make check` now also runs `ui-test` (vitest) and `web-check` (Rust tests with
  the app embedded).

Evidence: `web.rs` unit tests (exact lookups, 304, gzip negotiation, a build
without the app); vitest (PKCE against RFC 7636, SHA-256 vectors); a manual
Playwright run in Edge against `make run-web`: sign in through the OAuth page,
devices, create an invite, redeem it on the HTML page, disable/enable, reset
link, save rules, health, sign out. Not yet on the board.

## Sprint 4 (third slice) — Parity, connect, trust, diagnostics, security, avatars

- **Generated avatars** (`src/avatar.rs`, open question 17 resolved):
  `/avatars/<account id>.png`, the username's first character on a colour from
  the account id, drawn on request as a ~2 KiB PNG. No uploads, nothing stored.
- **Parity:** the landing page's admin forms are gone; it now links to the app
  (*Set up my phone*, *Household app*, *Trust this server*) and keeps short
  no-JavaScript connect instructions. Members gained "add with a password".
  Server-rendered pages kept: sign-in, first-time setup, Wi-Fi setup,
  `/setup/<code>`, `/setup/password`.
- **Connect** (public): the address this browser used with Copy, the device
  detected from the User-Agent, a data file of clients per device from the
  spec/07 matrix (only Mastodon.py marked tested), and a plain-HTTP warning.
- **Trust** (public): reads `/status`; says there is nothing to install while
  the server is HTTP only, and has per-device steps ready for when HTTPS exists.
- **Diagnostics:** `GET /diag` (admin) with platform facts from a new
  `Ctx::platform` hook (the board reports heap, PSRAM, uptime, reset reason and
  RSSI); `/status` gained `clock`, `mode`, `https`, `household_app`.
- **Clock fallback:** `POST /clock` (admin, only while unsynced, only forward),
  kept in RAM against the platform's uptime; Health offers the button.
- **Security** (owner): `GET /admin/security` and the page: transport, members
  without a DM key, devices per member, password policy.

Evidence: Rust tests (PNG structure, CRCs and glyph pixels; avatar URLs; diag
roles; the clock set while unset through the full request path, then a post
using the manual time; security for owner only); vitest (User-Agent detection,
only plain-HTTP clients "work now"); a Playwright run in Edge against
`make run-web` (landing without admin forms, Connect and Trust signed out, a
protected page bouncing through sign-in and back, Health, adding a member,
invite QR, Security, the avatar PNG). `make firmware` builds.

## Sprint 5 (first slice) — DMs, edits, follow requests, polls, lists, filters

- **Conversations** (`src/domain/conversations.rs`): the DM tab works. Threads of
  direct messages, derived at read time; read/hidden state in RAM.
- **Edits and history** (`src/domain/edits.rs`): `PUT /api/v1/statuses/:id`,
  3 previous versions per post (1,024 total) in `mm_hist`, `update`
  notifications to boosters. DMs are re-encrypted and keep no history.
- **Follow requests** (`src/domain/follow_requests.rs`): locked accounts,
  authorize/reject, unlocking admits everyone waiting.
- **Polls** (`src/domain/polls.rs`): bitmask votes, one write per vote, `poll`
  notifications when they end; not in DMs. Open question 13 resolved.
- **Lists** (`src/domain/lists.rs`): CRUD, members by account id, list
  timelines with reply policies, exclusive lists.
- **Filters** (`src/domain/filters.rs`, `src/api/filters.rs`): v2 with keywords
  and statuses, v1 view, `Status.filtered` on home, lists, public, tags,
  notifications, threads and account pages.
- Store schema 4 (new record kinds and namespaces; older stores upgrade in
  place). Board fixes: `Accept-Encoding`/`If-None-Match` now reach the API on the
  board (the household app answered 406 there), guarded by a test that every
  header the code reads is in `http::FORWARDED_HEADERS`. Favicons (the mammoth,
  10 KiB of the 390 KiB set: `favicon.ico` and `apple-touch-icon.png`).

Evidence: domain tests (power cut at every write while editing, while posting a
poll and while accepting a follow request; revision limits; history and poll
removed with their post; a deleted member's votes not passing to the next;
lists and filters removed with their owner; limits) and API tests for each
feature through the full request path; `make conformance`: **115 passed**, 27
listed deviations (was 96 / 45: the 19 edit, list, filter, conversation, poll
and follow-request entries now pass and were removed).

## Known gaps carried forward

| Gap | Planned |
|---|---|
| RAM structures are bounded std collections, not preallocated PSRAM slabs; no allocation tests yet | Sprint 6 (board bring-up) |
| Avatar/header upload (`update_credentials` with files returns 422); generated avatars instead | By decision (open question 17) |
| Household app not yet exercised on the board (heap/RSSI fields, clock button with SNTP unreachable) | Next board session |
| HTTPS, `/ca`, Easy/Secure switch, certificate upload: Trust and Security show the HTTP-only state | Sprint 8 |
| Followed hashtags | Sprint 5 (rest) |
| Conversation read/hidden state and poll-ended announcements are RAM only (a restart marks conversations read) | By design |
| Notifications are never grouped; notification policy is always accept-all | By design / later |
| Collections not advertised via `api_versions` (open question #16) | When a client uses them |
| OpenAPI schema validation (differential tests against `mastodon_mock` are done) | Sprint 5 |
| Streaming (instance advertises no streaming URL) | Later |
| Desktop server handles one request at a time | Fine for development; board design uses a service mutex |
