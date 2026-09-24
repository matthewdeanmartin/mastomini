# 03 — Limits and budgets

Every collection is bounded. RAM structures use `heapless` or preallocated slabs
sized once at startup. They never grow per request. Exceeding a limit returns a
specific error. It never overwrites live data, except where
[ring eviction](02-storage.md#eviction-ring-buffer) is the designed behaviour.

Byte limits are **UTF-8 bytes**. Character limits use Mastodon's counting rules
(below). Both are checked: characters for the user-facing rule, bytes for storage.

## Text limits

| Field | Mastodon default | mastomini | Storage bound |
|---|---|---|---|
| Status text + content warning | 500 chars | **140 chars** combined (`configuration.statuses.max_characters = 140`) | text ≤ 640 B, CW ≤ 120 B |
| URL in status | counts as 23 | counts as 23 (`characters_reserved_per_url = 23`) | raw URL ≤ 256 B |
| Username | 30, `[a-z0-9_]` | 20, `[a-z0-9_]`, immutable | 20 B |
| Display name | 30 chars | 30 chars | 120 B |
| Bio (`note`) | 500 chars | 160 chars (classic Twitter bio) | 640 B |
| Profile fields | 4 × (255 / 255) | 4 × (name 40 B / value 100 B) | 560 B |
| Password | — | 4–128 bytes. Relaxed on purpose: being on the household Wi-Fi is the second factor | PBKDF2 verifier 48 B |
| List title | — | 40 B | |
| Filter title / keyword | — | 40 B / 40 B | |
| Report comment | 1,000 chars | 500 chars | 1,000 B |
| Collection name / description | 40 / 100 chars | 40 / 100 chars | 160 B / 400 B |
| Hashtag | — | 40 B | |
| Server name / description | — | 40 B / 280 B | |
| Server rules | — | 8 × 140 B | |
| OAuth app name / website | — | 60 B / 120 B (longer is truncated, not rejected: clients don't expect rejection) | |
| Redirect URIs per app | — | 4 × 256 B | |
| `Idempotency-Key` | — | 80 B | |

### Character counting

The rules match Mastodon's `StatusLengthValidator` so that a client's own counter
and the server agree:

- Count Unicode **extended grapheme clusters**, not bytes or code points, as
  Mastodon's validator does. The `unicode-segmentation` crate is enough. Verify
  against the Mastodon source during Phase 2.
- Every URL (http/https) counts as 23 regardless of its length.
- A mention `@user@domain` counts only as `@user`. There are no remote users, but
  clients may still type the long form.
- Content warning text counts toward the same 140.
- If the server's count differs from a client's, the server's wins (`422` with
  `Validation failed: Text character limit of 140 exceeded`). The desktop test suite
  pins this against a table of tricky strings.

## Capacity limits

| Collection | Limit | At limit |
|---|---|---|
| Accounts (incl. admin, disabled, tombstoned) | 16 | `422` "household is full". Fits `u16` bitmasks |
| Statuses retained (posts, replies, DMs) | 4,096 | ring eviction |
| Boosts retained | 2,048 | ring eviction of oldest boost |
| Reactions (favourites + bookmarks + pins) | 16,384 | ring eviction of oldest status |
| Pinned statuses | 5 per account (Mastodon's limit) | `422` |
| Edit history | 3 previous revisions per status, 1,024 total | oldest revision dropped |
| Mentions per status | any household member (bitmask) | — |
| Hashtags per status | 8 | extra tags render as plain text |
| OAuth apps | 32 | evict the oldest app that has no live tokens, else `429` |
| Access tokens | 8 per account, 64 total | issuing a 9th revokes that account's oldest token |
| Pending authorization codes | 16, 60 s, single use | `503` try again |
| Lists | 8 per account | `422` |
| Filters (v2) | 8 per account × 4 keywords and 4 statuses | `422` |
| Followed hashtags | 16 per account | `422` |
| Blocks, mutes | one edge per ordered pair of accounts (≤ 240 each) | — |
| Reports | 32 | oldest resolved dropped; `422` if none is resolved |
| Posts per report | 10 | `422` |
| Collections | 8 per account, items inline (≤ 15 other members) | `422` |
| Notifications (RAM ring, all accounts) | 512 | oldest dropped. Emptied by reboot |
| Invites / reset codes | 8 live, 7-day expiry | oldest expired first |
| Custom emoji | 0 | `[]` |
| Polls | 2–4 options × 50 chars, 1 poll per status, 5 minutes to 7 days, not in direct messages | `422` |

## Request and response limits

| Item | Limit |
|---|---|
| URI length | 1,024 B |
| Request headers | 2,048 B |
| JSON / form body (API) | 4 KiB |
| Multipart body, `PATCH /api/v1/accounts/update_credentials` only | 160 KiB, and file part ≤ 48 KiB avatar / 96 KiB header |
| Media upload endpoints (`/api/v1/media`, `/api/v2/media`) | `422 "media attachments are not supported"` (body not read beyond headers; `413` on `Content-Length`) |
| Page size (`limit`) | default 20, max 40 (Mastodon's defaults) |
| `id[]` array params (relationships, statuses) | 40 |
| Search results | 20 per type |
| Concurrent uploads | 1 |

## Memory budget (targets, verified on hardware)

| Item | Region | Size |
|---|---|---|
| Status slab: 4,096 × ~832 B fixed slots (text, CW, meta, bitmasks) | PSRAM | ~3.4 MiB |
| Boost slab: 2,048 × 32 B | PSRAM | 64 KiB |
| Reaction index: 16,384 × 16 B (id, status slot, account, kind) | PSRAM | 256 KiB |
| Hashtag index (per status: up to 8 × u32 hash) | PSRAM | 128 KiB |
| Accounts 16 × ~1.6 KiB, apps 32, tokens 64 | PSRAM | ~40 KiB |
| Ephemeral: notification ring (512 × 32 B), markers (16 × 2), auth codes, idempotency keys (1 h), login-failure counters, rate/governor counters, token last-used | PSRAM | ~64 KiB |
| Per-worker response buffer: 4 × 128 KiB | PSRAM | 512 KiB |
| NVS page cache (`CONFIG_NVS_ALLOCATE_CACHE_IN_SPIRAM`) | PSRAM | measure (entry count dependent) |
| **PSRAM total target** | | **≤ 5 MiB of 8 MiB** |
| TLS sessions (4 × 4 KiB in + 2 KiB out, one 16 KiB upload record) | internal | ~40 KiB |
| httpd worker stacks 6 × 24 KiB | internal | 144 KiB |
| Reserved internal pool | internal | 64 KiB (as nanacoin) |

Why fixed status slots rather than a variable-length text arena: fixed slots
cannot fragment, eviction and deletion free a slot in O(1), and search is a linear
scan over contiguous memory. Wasted space is about 2 MiB of PSRAM, which we can
spare. If PSRAM measurements come in tight, the fallback is a ring-ordered text
arena. It fits ring eviction naturally, because the oldest text is always at the
tail.

A timeline page of 40 statuses serializes to about 60–120 KiB of JSON, since each
status embeds its author account. The 128 KiB worker buffer is sized for that. If a
reply would overflow it, the handler shortens the page and sets the `Link` header
to match. That is legal Mastodon behaviour: clients follow `Link`, not `limit`.

## Flash budget

| Partition | Size | Worst-case use |
|---|---|---|
| `factory` | 4 MiB | firmware ~2 MiB + Angular ~0.6 MiB gz. A build-time size gate fails above 3.5 MiB |
| `store` (NVS) | 8 MiB | 4,096 × ~800 B worst-case statuses ≈ 3.3 MiB + reactions ~0.5 MiB + the rest ~0.3 MiB. Low watermark eviction at 80% used |
| `media` (LittleFS) | 3 MiB | 2.25 MiB of avatars/headers at full household |
