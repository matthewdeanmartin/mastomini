# Roadmap

Work runs in **sprints**. Everything is made to work and proven on the Windows
desktop build first. The board comes after the desktop build behaves like a
Mastodon server for real clients. Certificates and the household CA come last,
because they are nice-to-have.

## Quality gates (every sprint)

A sprint is done only when all of these pass from Git Bash in `mastomini_rs/`:

| Gate | Command |
|---|---|
| Formatting | `cargo fmt --check` |
| Lint | `cargo clippy --locked --all-targets -- -D warnings` |
| Python tooling compiles | `make scripts-check` |
| Unit + integration tests | `cargo test --locked` |
| HTTP smoke against the real desktop binary | `make smoke` (sprint 2+) |
| Mastodon.py client suite against the real desktop binary | `make client-test` (sprint 3+) |

`make check` runs them all. Status for each sprint, and known gaps, are recorded in
[sprints.md](sprints.md).

## Sprint 1 — Scaffolding and storage engine

- `mastomini_rs` crate (lib + desktop bin), Makefile, release profile.
- `Store` trait with `MemStore`, `FileStore` (desktop, CRC'd append log, compaction
  on open, exclusive lock) and `FaultStore` (power cut after N operations) (02).
- Value encoding: version + kind + payload. Key builders: base32 snowflakes.
- Snowflake ID generator, monotonic across a clock that steps backwards.
- Domain core: accounts (16 slots), statuses (4,096), boosts (2,048), reactions
  (16,384), follows. Boot rebuild, orphan repair, ring eviction, invariants.
- Character counting (140, URLs = 23) and status HTML rendering.

**Exit:** gates green. Tests cover reboot round-trip, power-cut-after-every-op for
the multi-key operations, eviction order and the pinned exemption, and the
character-count table.

## Sprint 2 — HTTP, OAuth, accounts

- Transport-neutral `Request`/`Response` and a router. Desktop adapter on
  `tiny_http`.
- Instance v1/v2, OAuth server metadata, nodeinfo, rules, custom emojis `[]`.
- `POST /api/v1/apps`, server-rendered `/oauth/authorize` (GET + POST),
  `/oauth/token` (authorization_code + PKCE, client_credentials), `/oauth/revoke`.
- Passwords (PBKDF2), lockout, persistent hashed tokens with token epochs.
- `verify_credentials`, `update_credentials` (text fields), account fetch, lookup,
  search, relationships.
- Minimal household bootstrap: `POST /api/mastomini/v1/provision` (owner) and
  `POST /api/mastomini/v1/admin/members` (admin creates a member with a password).
  This is enough to run the client tests. Invites and the Angular app come later.

**Exit:** gates green. A smoke script provisions the server, registers an app,
completes the OAuth code flow (form post), and calls `verify_credentials`.

## Sprint 3 — Posting and timelines

- Statuses: create (form + JSON, `Idempotency-Key`), get, delete, context,
  favourite/unfavourite, reblog/unreblog, bookmark/unbookmark, pin/unpin,
  favourited_by, reblogged_by.
- Timelines: home, public, tag, account statuses with filters. Mastodon
  pagination and `Link` headers.
- Follows: follow/unfollow, followers/following.
- Notifications: RAM ring (list, get, clear, dismiss, unread count).
  Markers: RAM only.
- `GET /api/v2/search`. Neutral stubs for filters, lists, followed tags,
  announcements, preferences, and trends (`[]`).
- A Mastodon.py suite run through `uv run pytest` against the desktop binary.

**Exit:** gates green. Two Mastodon.py clients, logged in via real OAuth, follow
each other, post, reply, favourite and boost, see each other's posts on home, get
notifications, and everything except the notifications survives a server restart.

## Sprint 4 — Household app

- Angular app embedded in the binary (nanacoin's `web.rs` pipeline): provisioning
  wizard, invites/reset codes, members admin, profile + avatar/header (LittleFS on
  the board, a directory on desktop), devices/tokens, server settings, health page,
  `/connect` client guide.

## Sprint 5 — Tier 2 API and conformance

- Edits and history, lists, filters v2, conversations, grouped notifications,
  blocks/mutes, tags.
- Differential tests against `mastodon_mock`. OpenAPI schema validation.
- Desktop client tests with Whalebird / Phanpy against the desktop build.

## Sprint 6 — Board bring-up

- Hardware gate H1: internal RAM with TLS, NVS latency and boot time at full
  capacity, PBKDF2 timing. Adjust limits in 03 if needed.
- `bin/esp32.rs`: Wi-Fi, SNTP, mDNS, httpd with raised URI/header limits, `NvsStore`,
  LittleFS, diagnostics. Deploy writes only `factory`.
- Easy mode HTTP first, so browsers can use the board.

## Sprint 7 — Board hardening

- 24 h soak, 200 power cuts, write-governor tuning, clock fallback, boot time.

## Sprint 8 — HTTPS, household CA, real-domain certificates (nice to have)

- Household CA with `MASTOMINI_CA_DIR` bring-your-own, `/trust`, `/ca`, Easy/Secure
  modes, uploaded real-domain certificate, `make renew-cert`.
- Client matrix on real phones (iOS via the CA, Android via the real domain).

## Later / maybe

Export before eviction, polls, streaming, a minimal `/home` timeline in the household
app, announcements, OTA, flash encryption, post images (SD card).

## Risks

| Risk | Mitigation |
|---|---|
| Internal RAM too tight for TLS + large records | H1 in sprint 6. Chunked avatar fallback |
| Clients reject `max_media_attachments: 0` or need streaming | Found by desktop client tests in sprint 5, before hardware |
| Android ignores user CAs and `.local` | Sprint 8 real-domain model |
| Power-cut behaviour on hardware differs from host tests | Sprint 7 power-cut run |
| Desktop-only assumptions (allocation, std) creep into shared code | Keep the domain and store free of OS APIs. Board-specific code stays behind features |
