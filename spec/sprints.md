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

## Known gaps carried forward

| Gap | Planned |
|---|---|
| RAM structures are bounded std collections, not preallocated PSRAM slabs; no allocation tests yet | Sprint 6 (board bring-up) |
| Avatar/header upload (`update_credentials` with files returns 422) | Sprint 4 |
| Invites, reset codes, password change UI, devices list, household Angular app (first-time setup and adding members exist as plain HTML pages) | Sprint 4 |
| Locked accounts are followed directly (no follow requests) | Sprint 5 |
| Edits/history, lists, filters, conversations, followed tags (blocks/mutes are done) | Sprint 5 |
| Notifications are never grouped; notification policy is always accept-all | By design / later |
| Collections not advertised via `api_versions` (open question #16) | When a client uses them |
| OpenAPI schema validation (differential tests against `mastodon_mock` are done) | Sprint 5 |
| Streaming (instance advertises no streaming URL) | Later |
| Desktop server handles one request at a time | Fine for development; board design uses a service mutex |
