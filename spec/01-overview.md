# 01 — Overview and architecture

## Goals

1. A household of 5–10 people can use **unmodified Mastodon clients**
   (iOS: Ivory, Ice Cubes, Mona, Mastodon official; Android: Tusky, Moshidon,
   Mastodon official; web: Phanpy/Elk where reachable) against one ESP32 board.
2. The board is an appliance: it survives power cuts and runs for years
   without wearing out its flash, and it never needs a shell.
3. Every data structure has an explicit, documented bound (see
   [03-limits.md](03-limits.md)). Running out of room is a designed state,
   never a crash or a corruption.
4. The Mastodon REST shapes match a real server closely enough that client
   feature detection, pagination and OAuth work. `mastodon_mock` is the
   behavioural reference.

## Non-goals

- Federation, ActivityPub, WebFinger for remote lookups, remote accounts.
- Post media attachments (images/video/audio) in v1.
- Open registration. Only the household admin creates accounts.
- Web Push delivery in v1 (needs internet access, VAPID and payload encryption).
- Moderation beyond "admin can disable an account and delete a post".
- High availability, replication or multi-board setups.
- Being a secure store against someone with physical access. Flash is not encrypted
  (see [05-auth-network-tls.md](05-auth-network-tls.md)).

## Usage profile (design point)

| Quantity | Expected | Designed for |
|---|---|---|
| Accounts | 5–10 | 16 |
| New posts | 100s per year | 4,096 retained, ring-evicted |
| Favourites/bookmarks | a few per post | 16,384 retained |
| Concurrent clients | 2–4 phones open at once | 4 TLS sockets, requests beyond that queue |
| Signed-in devices | 2–4 per person | 8 per person, 64 total |
| API keys (bots) | 0–2 per household | 4 per person, 16 total, never evicted by sign-ins |

## Architecture

```
 phone app / browser
        │ HTTPS (443)  ── HTTP (80): /trust, /ca, redirect only (Secure mode)
        ▼
 ┌──────────────────────── ESP32-S3 (core 1) ─────────────────────────┐
 │ esp-idf httpd + TLS  →  router  →  handlers                        │
 │                                     │                              │
 │  static assets (Angular, flash)     │  Mastodon API /api/v1,/api/v2│
 │  OAuth pages (server-rendered)      │  mastomini API /api/mastomini│
 │                                     ▼                              │
 │                 Service (one mutex)  ── domain state in PSRAM      │
 │                        │  (bounded slabs, bitmasks, indexes)       │
 │                        ▼                                           │
 │   Store trait ── NVS entity store (`store` partition)              │
 │               └─ LittleFS (`media` partition): avatars/headers     │
 └────────────────────────────────────────────────────────────────────┘
 core 0: Wi-Fi, lwIP, SNTP, mDNS, diagnostics sampler
```

- **State lives in RAM, flash is the durable copy.** Boot enumerates the store
  and rebuilds bounded in-memory state (as nanacoin does). Reads never touch flash
  except avatars. Writes go to flash first, then RAM.
- **One service mutex** guards domain state. Handlers serialize their reply into a
  per-worker buffer while holding the lock only as long as they need a consistent
  view, then write to the socket outside the lock.
- **Desktop build** runs the same library with a host store adapter and plain HTTP
  (or mkcert TLS), for fast development and for running the test suites.

## Hardware and partition table

Same board as nanacoin: ESP32-S3 N16R8. Proposed `partitions.csv`:

```
# Name,    Type, SubType,  Offset,    Size
nvs,       data, nvs,      0x9000,    0x6000     # Wi-Fi/PHY/device config (ESP-IDF default use)
phy_init,  data, phy,      0xf000,    0x1000
factory,   app,  factory,  0x10000,   0x400000   # 4 MiB: firmware + embedded Angular app
store,     data, nvs,      0x410000,  0x800000   # 8 MiB: NVS entity store (all household data)
media,     data, littlefs, 0xC10000,  0x300000   # 3 MiB: avatars and headers
coredump,  data, coredump, 0xF10000,  0x10000    # 64 KiB: post-mortem crash dumps
# 0xF20000–0xFFFFFF (896 KiB) deliberately unallocated: room for a future layout change
```

No OTA partitions in v1. Like nanacoin, upgrades are done over USB and write only
the `factory` partition. OTA over Wi-Fi is in the roadmap's "later" bucket because it
needs two app slots (8 MiB), which would take space from `store`.

## Repository layout (proposed)

```
mastomini/
  spec/                 this document set
  mastomini_rs/         Rust crate: lib + bin/desktop.rs + bin/esp32.rs
    src/
      api/              Mastodon handlers, grouped as mastodon_mock's routers are
      admin/            /api/mastomini/v1 handlers
      domain/           accounts, statuses, reactions, relationships, notifications
      render/           status HTML rendering, linkify, char counting
      store/            Store trait, host adapter, NVS adapter, fault injection
      auth.rs           PBKDF2, PKCE, lockout (ported from nanacoin_rs)
      web.rs            embedded static assets (ported from nanacoin_rs)
    scripts/            build-esp32.sh, deploy.sh, dev-certs.sh, probe-board.py …
    tests/
  mastomini_ui/         Angular household app
  Makefile              Git Bash targets, same shape as nanacoin_rs/Makefile
```

## Reuse from nanacoin_rs

| Piece | nanacoin_rs source | Change for mastomini |
|---|---|---|
| Build: toolchain, `sdkconfig.defaults`, `build.rs` Wi-Fi/secret loading, short `CARGO_TARGET_DIR` | `Cargo.toml`, `build.rs`, `scripts/build-esp32.sh` | Rename env vars to `MASTOMINI_*`; raise httpd URI/header limits (below) |
| Deploy that writes only the app partition and refuses unknown layouts | `scripts/deploy.sh`, `deploy.py` | New partition layout check |
| TLS listener pair (HTTPS + HTTP), Easy/Secure modes, `/trust`, `/ca` | `bin/esp32.rs`, `CONNECTION_SECURITY.md` | Add optional uploaded certificate (see 05) |
| Household CA scripts, 100-year RSA certs, `certs-check` | `scripts/dev-certs.sh`, `test-certs.sh` | Own CA by default; `MASTOMINI_CA_DIR` can point at an existing household CA (05) |
| PBKDF2 verifiers, lockout, S256 PKCE | `src/auth.rs` | Tokens become persistent (Mastodon clients expect it) |
| Embedded Angular assets, ETags, immutable caching | `src/web.rs`, `scripts/build-web.sh` | Same |
| Diagnostics sampler and `/diag` | `src/diagnostics.rs` | Add store statistics (entries used/free, eviction count) |
| mDNS, SNTP, clock-validity gating | `bin/esp32.rs` | Add browser clock fallback (see 05) |
| heapless bounds and allocation tests | `src/domain.rs`, `tests/allocation.rs` | Same discipline |

Not reused: the event journal and checkpoint protocol. The data shape is different
(mostly immutable posts, not a ledger), so [02-storage.md](02-storage.md) proposes an
entity store instead.

### sdkconfig changes relative to nanacoin

nanacoin's HTTP limits are too small for Mastodon clients:

| Setting | nanacoin | mastomini | Why |
|---|---|---|---|
| `CONFIG_HTTPD_MAX_URI_LEN` | 96 | 1024 | `/oauth/authorize?client_id=…&redirect_uri=…&scope=…&state=…&code_challenge=…` and `relationships?id[]=…&id[]=…` |
| `CONFIG_HTTPD_MAX_REQ_HDR_LEN` | 768 | 2048 | Bearer token, long User-Agent, Accept-Language and cookies from in-app browsers |
| Request body cap | 1 KiB | 4 KiB (API), 160 KiB (avatar/header upload only) | Form/JSON status posts; multipart image uploads |
| `CONFIG_MBEDTLS_SSL_IN_CONTENT_LEN` | 4096 | 16384 (candidate, must be measured) | Peers may send full 16 KiB TLS records. Small API requests fit in 4 KiB, but an avatar upload will not. |

`IN_CONTENT_LEN` is a single global value. With `CONFIG_MBEDTLS_DYNAMIC_BUFFER=y`
the large buffer is allocated only while a large record is in flight. So the
design allows at most one upload at a time: a second concurrent upload gets
`429`. This keeps worst-case internal RAM at roughly one 16 KiB buffer plus three
4 KiB buffers. It is the main internal-RAM risk and has a hardware measurement gate
in the roadmap. If it doesn't fit: keep 4 KiB, reject avatar uploads from native
apps (`413`), and have the household app upload avatars in ≤3 KiB chunks through
`/api/mastomini/v1`. That is why the measurement comes early.
