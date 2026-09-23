# mastomini specification

mastomini is a single ESP32-S3 board that serves a private, non-federating
Mastodon-compatible server for one household (5–10 people). Family members
point any ordinary Mastodon app at `mastomini.local` (or an optional real
domain), sign in, and post short messages to each other. A small embedded
Angular app handles the parts that stock Mastodon clients cannot: first-boot
setup, creating members, passwords, devices, certificate trust, and guides for
setting up third-party clients.

## Documents

| File | Contents |
|---|---|
| [roadmap.md](roadmap.md) | Sprint plan, quality gates, exit criteria, risks |
| [sprints.md](sprints.md) | What each finished sprint delivered, evidence, known gaps |
| [01-overview.md](01-overview.md) | Goals, non-goals, architecture, hardware, partition table, code reuse |
| [02-storage.md](02-storage.md) | Flash, wear, the storage options considered, the chosen NVS entity store, key layout, crash consistency, eviction |
| [03-limits.md](03-limits.md) | Every capacity limit and memory/flash budget |
| [04-mastodon-api.md](04-mastodon-api.md) | Mastodon REST surface, tiers, semantics, character counting, IDs, pagination |
| [05-auth-network-tls.md](05-auth-network-tls.md) | OAuth, passwords, tokens, HTTPS, household CA vs real domain, mDNS |
| [06-household-app.md](06-household-app.md) | The embedded Angular app and the `/api/mastomini/v1` admin API |
| [07-testing.md](07-testing.md) | Test strategy, including differential testing against `mastodon_mock` |
| [open-questions.md](open-questions.md) | Decisions still to be made |

## Decisions so far

| Topic | Decision | Source |
|---|---|---|
| Board | ESP32-S3 N16R8 (16 MiB flash, 8 MiB octal PSRAM), same as nanacoin | user |
| Language / platform | Rust on ESP-IDF via `esp-idf-svc`, desktop build for development | nanacoin_rs precedent |
| Federation | None. Local accounts only | user |
| API | Mastodon REST, same shapes as a real server; reference: `mimb/mastodon_mock` | user |
| Post length | 140 characters, counted the way Mastodon counts them | user ("classic twitter") |
| Media | Text + avatars/headers only. No post attachments in v1 | user |
| Web UI | Small embedded Angular app: setup, members, passwords, devices, trust, client guides | user |
| TLS / naming | Both: household CA + `mastomini.local` by default; optional uploaded real-domain certificate | user |
| Retention | Ring buffer: oldest non-pinned posts are evicted automatically when full | user |
| Storage engine | NVS "entity store" (one wear-levelled key per record) on a dedicated partition, avatars on LittleFS. No accounting-style journal | user, see [02-storage.md](02-storage.md) |
| Ephemeral state | RAM/PSRAM only (markers, OAuth codes, counters, idempotency keys) | user |
| Follows | Same as Mastodon: nobody follows anyone automatically | user |
| Passwords | Relaxed: minimum 4 characters; the household Wi-Fi is the second factor | user |
| Household CA | Own CA by default; `MASTOMINI_CA_DIR` reuses an existing one (e.g. nanacoin's) | user |
