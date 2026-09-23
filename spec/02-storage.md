# 02 — Storage

## The physics

The ESP32-S3 module's SPI NOR flash:

- is **erased in 4 KiB sectors** (bits go to 1) and **programmed in bytes/pages**
  (bits go 1→0). To change a byte back you must erase its whole sector.
- survives roughly **100,000 erase cycles per sector** (typical for this class of
  part; check the module datasheet). After that, bits start to stick.
- can lose power in the middle of any program or erase.

This makes one pattern fatal: a **mutable record at a fixed address** (the
"COBOL file at offset 0x…" approach). Updating it means erasing the same sector
every time. For example, a Mastodon client syncs its read position
(`POST /api/v1/markers`) every time the user stops scrolling, perhaps 200 times a
day per person. For 10 people that is 2,000 erases a day on one sector, and the
sector wears out in about **50 days**. (mastomini keeps markers in RAM anyway, see
below. The example just shows how quickly a fixed address dies.) The same data
spread across an 8 MiB wear-levelled region costs each sector about one erase every
few months.

Write *volume* is not the risk. Write *concentration* is. The whole design follows
from that.

### "What if we just disable edits?"

Making posts immutable doesn't make a fixed-layout file safe. Even with no
edits, the server still has hot mutable state that would sit at fixed addresses:

- per-status favourite/boost/bookmark state,
- OAuth tokens and app registrations (created on every client login),
- preferences, profile edits,
- the "next record goes here" head pointer, and any sequence counter.

An append-only raw ring (write forward, erase the oldest sector when you wrap)
*is* naturally wear-levelling, because each sector is erased once per lap. But once
you add deletes, reactions, tokens and a head pointer that survives power loss, you
have rebuilt NVS's page state machine by hand. ESP-IDF already ships and
power-cut-tests that code.

## Options considered

| Option | Wear levelling | Power-loss safety | Fit | Verdict |
|---|---|---|---|---|
| Fixed-address records on a raw partition (`esp_partition_write`) | None | Hand-rolled | Only for write-once data | **Rejected**: fatal for tokens/reactions/any mutable record |
| Raw append-only ring log, hand-written | Natural (per lap) | Hand-rolled: head recovery, torn writes, compaction of live records | Good for posts, poor for mutable state | Rejected: reimplements NVS |
| FAT on `wear_levelling` layer | Yes | **No**: FAT is not power-loss safe | Files | Rejected |
| SPIFFS | Yes | Weak, deprecated in IDF | Files | Rejected |
| SQLite (over LittleFS/VFS) | Via FS | Journal/WAL writes, heavy RAM and random I/O | Queries we don't need, since everything fits in RAM | Rejected: cost with no benefit at this scale |
| **nanacoin's NVS event journal + checkpoints** | Yes (NVS) | Proven: append-before-apply, two generations | Good for a ledger. Here every checkpoint rewrites *all* retained posts (~2× state in flash), every 2,048 events | Viable alternative, not preferred |
| **NVS entity store** (one key per record) | Yes (NVS) | Each `set`/`erase` is atomic (NVS guarantee) | Posts are mostly immutable: write once, erase on delete/evict. NVS reclaims space. No full-state rewrites | **Chosen** |
| LittleFS | Dynamic wear levelling, copy-on-write | Designed for power loss | Natural for byte blobs | **Chosen for avatars/headers only** |

### Why the entity store over nanacoin's journal

- nanacoin's state is small and highly mutable (balances). A checkpoint of 16
  members and 365 transactions is cheap. mastomini's state is thousands of
  immutable posts, so checkpointing would rewrite megabytes to "retire" a few
  kilobytes of favourites.
- Ring eviction maps directly to `nvs_erase_key` on the oldest post. In a journal
  design, eviction only takes effect at the next checkpoint.
- The main thing the journal gives us is **atomic multi-entity commits**. The
  entity store gives that up. Below, every operation is designed as a single-key
  commit point, and the multi-key operations are listed with a recovery rule.
- Reusable from nanacoin: the NVS wrapper, fail-closed error latching, versioned
  checksummed values, the "never auto-erase on init error" rule, host/board
  adapter split, and fault-injection test style.

## Store design

### Store trait

```rust
pub trait Store {
    fn get(&mut self, ns: Ns, key: &Key, buf: &mut [u8]) -> Result<Option<usize>, StoreError>;
    fn set(&mut self, ns: Ns, key: &Key, value: &[u8]) -> Result<(), StoreError>; // atomic per key
    fn erase(&mut self, ns: Ns, key: &Key) -> Result<(), StoreError>;              // idempotent
    fn for_each(&mut self, ns: Ns, f: &mut dyn FnMut(&Key, &[u8]) -> Result<(), StoreError>) -> Result<(), StoreError>;
    fn stats(&mut self) -> Result<StoreStats, StoreError>; // used/free entries, for the watermark
}
```

Adapters:

- `NvsStore` (board): ESP-IDF NVS on the `store` partition. Each `set` is followed by
  `nvs_commit`, and a request is acknowledged only after the commit returns.
- `FileStore` (desktop): an append-only operation log file replayed on start and
  compacted on start, with `fsync` before acknowledgement. The same trait semantics,
  single process, exclusive file lock (as nanacoin does with `fs2`).
- `MemStore` and `FaultStore` (tests): in-memory, with injectable failures before
  or after any `set`/`erase`, torn values, and "power cut after N operations".

Errors latch the service into a read-only unavailable state until restart, as in
nanacoin. `nvs_flash_init` errors never trigger an automatic erase. Recovering a
corrupted store is an explicit, documented USB procedure.

### Value encoding

Every value is `[version: u8][kind: u8][postcard payload]`. NVS already CRCs each
entry, so no extra checksum is needed on the board. `FileStore` adds a CRC32 per log
record. Decoding an unknown version fails closed at boot with a clear diagnostic.
postcard (serde, `no_std`, compact) replaces nanacoin's JSON payloads to save flash
and RAM. Unknown kinds are an error rather than silently skipped.

### IDs

Mastodon-style snowflakes: `(unix_ms << 16) | seq16`, serialized as decimal
strings in the API. No counter is ever persisted (there are no hot keys). At boot,
`last_id = max(all ids found)`. New IDs are `max(now_id, last_id + 1)`, so a clock
that steps backwards still produces increasing IDs. IDs require a valid clock (see
[05](05-auth-network-tls.md#time)).

In NVS keys an ID is written as 13 characters of Crockford base32. That preserves
ordering and fits NVS's 15-character key limit.

### Namespaces and keys

Each NVS namespace acts as a table and is enumerated at boot with
`nvs_entry_find`. `A` = account slot as one hex digit (0–f). `ID` = 13-character
base32 snowflake.

| Namespace | Key | Value | Written when |
|---|---|---|---|
| `mm_cfg` | `schema` | store schema version | format/upgrade |
| `mm_cfg` | `server` | name, description, rules, contact slot | admin edits |
| `mm_cfg` | `https_only` | transport policy byte (as nanacoin) | admin |
| `mm_cfg` | `tls_cert`, `tls_key` | optional uploaded real-domain cert chain/key (PEM, chain ≤ 8 KiB, key ≤ 4 KiB) | admin upload |
| `mm_acct` | `a` + `A` | account: api id, username, display name, note, fields, role, flags, password verifier, **token epoch**, prefs, avatar/header versions | create/edit/password |
| `mm_app` | `p` + `ID` | OAuth app: client id, secret hash, name, website, redirect URIs, scopes | `POST /api/v1/apps` |
| `mm_tok` | `t` + 14-char hash prefix | token: sha256(token), account slot, app id, scopes, epoch at issue, created_at | token issue/revoke |
| `mm_stat` | `s` + `ID` | status: author slot, text, CW, visibility, language, reply-to, mentions bitmask, flags, edited_at | post/edit/delete/evict |
| `mm_stat` | `r` + `ID` | boost: booster slot, target status id | boost/unboost |
| `mm_hist` | `h` + `ID` + rev (0–2) | previous revision of an edited status | edit |
| `mm_rx` | `f`/`b`/`p` + `ID` + `A` | favourite / bookmark / pin: record id (snowflake, used as the notification id) | toggle |
| `mm_rx` | `m` + `ID` + `A` | conversation mute, same shape as a favourite (schema 2) | toggle |
| `mm_rel` | `F`/`B`/`M` + `A` + `A` | follow (record id, `notify`, `reblogs`) / block (record id) / mute (record id, `notifications`, expiry) edge. `B` and `M` since schema 2 | toggle |
| `mm_mod` | `m` + `A` | account moderation: account id (guards a reused slot), silenced, suspended-at, sensitized. Erased when all clear | admin action |
| `mm_mod` | `R` + `ID` | report: reporter and target slots, post ids, comment, category, rule ids, assignee, resolution | report / admin |
| `mm_stat` | `d` + `ID` | encrypted direct message envelope: ephemeral public key, sealed text and content warning, content key sealed per participant. The status record then has empty text (05 "Direct messages") | post/delete/evict |
| `mm_key` | `k` + `A` | member key pair: public key, secret sealed with a password-derived key (salt, rounds) | account created, first sign-in, password change |
| `mm_key` | `w` + token key suffix | the member's secret sealed for one signed-in device (token-derived key) | token issue/revoke |
| `mm_cfg` | `terms` | admin-written terms of service and the date they took effect; absent means generated terms | admin |
| `mm_coll` | `c` + `ID` | collection: curator slot, name, description, flags, language, tag, items inline (item id, account id, revoked) | edit |
| `mm_list` | `l` + `A` + n | list: title, members bitmask, replies policy | edit |
| `mm_filt` | `x` + `A` + n | filter: title, context flags, action, keywords | edit |
| `mm_tag` | `g` + `A` + n | followed hashtag | toggle |
| `mm_inv` | `i` + n | invite / password-reset code hash, target slot, expiry | admin |

**Schema 2** added the `B`/`M`/`m` keys and the `mm_mod` and `mm_coll`
namespaces, as new record kinds. No existing record changed layout (postcard is
not self-describing, so a changed struct would misread every stored record).
Boot rewrites a schema-1 marker to 2, which is the only write needed; firmware
that only knows schema 1 then refuses the store instead of misreading it.
Boot repair also drops follows that cross a block (a block commits before its
unfollows), moderation records whose account id no longer matches the slot,
reports naming a missing account, collections of a missing curator, and (in RAM)
collection items naming a deleted account.

Derived at boot, **not stored**: hashtags and mentions (parsed from text), all
counts, conversations, timelines, and `last_id`.

**Notifications are ephemeral.** They are never written to flash and never
rebuilt at boot. A write that notifies someone (mention, reply, favourite, boost,
follow) pushes an entry into a RAM ring of 512 notifications shared by all accounts.
When the ring is full, the oldest entry is dropped, and a reboot empties it. Clear and
dismiss only touch the ring. Mastodon clients cope with this: they show what
the server returns.

**No feed algorithms.** Timelines are reverse-chronological filters over the
statuses in RAM, about as clever as an RSS feed. Nothing is ranked, scored or
precomputed, and nothing on the read path ever writes. Clients do the clever parts
themselves (filters, catch-up, grouping).

### Ephemeral state stays in RAM

Rule: anything that can be lost on reboot without the household noticing never
touches flash. It lives in internal RAM or PSRAM.

| Data | Rule |
|---|---|
| Markers (`/api/v1/markers`: each client's "last read" position for the home timeline and notifications, synced so a second device resumes where the first stopped) | **RAM only.** A reboot resets them. The only effect is that clients lose cross-device read-position sync once |
| Token "last used", login failure counters, rate limits, sessions of OAuth pages | RAM only |
| Pending OAuth authorization codes (60 s, single use) | RAM only (as nanacoin) |
| Status counts, favourite counts | never stored, always derived |
| Next-ID counter | never stored, derived from data |

### Flash write governor

A misbehaving client stuck in a favourite/unfavourite loop should not be able to
spend the flash's lifetime. The service counts store operations in a sliding hour.
Above 120 per account-hour or 600 per board-hour, writes return
`429 Too Many Requests` with `Retry-After`. Normal household use is well under 50 a
day. The governor counters appear in diagnostics.

Endurance check: a busy year is about 1,000 posts, 5,000 reactions,
and a few hundred token writes, roughly 3 MiB of NVS
entries. Across the 2,048 sectors of an 8 MiB partition that is less than one erase
per sector per year, before counting NVS's own page reclamation, which multiplies it
by a small factor. At the governor ceiling (600/hour, all year) it is still decades.

## Crash consistency

ESP-IDF NVS guarantees each `set`/`erase` is atomic with respect to power loss: a
reader afterwards sees either the old value or the new one. The rules:

1. **One commit point per user action.** Wherever possible, an action is a single
   key write: post, boost, favourite, follow, token issue, app registration, list edit.
2. **Write the durable record first, then update RAM, then respond.** A crash
   before the response means the client retries. Retries are idempotent wherever
   Mastodon makes them so (favourite of a favourited post is a no-op; status creation
   honours `Idempotency-Key`, stored in RAM for 1 hour, same as Mastodon).
3. **Multi-key operations have an explicit order and a boot-time repair:**

| Operation | Order | If power is cut midway | Boot repair |
|---|---|---|---|
| Delete / evict status | erase `s` key → erase its `r`, `mm_rx`, `mm_hist` keys | orphan boosts/reactions/history | orphans (target status missing) are erased at boot |
| Edit status | write `h` (old revision) → overwrite `s` | an extra history entry that duplicates the current text | duplicate newest revision is dropped |
| Revoke all sessions (password change, disable, role change) | bump `token_epoch` in the account record (**single write**) → erase that account's tokens | tokens with old epoch remain on flash | tokens whose epoch ≠ account epoch are rejected, and erased at boot |
| Delete account | set `deleted` flag in account record (single write) → erase its statuses, reactions, relationships, tokens, lists, filters, tags, avatar files → erase account key | account is tombstoned, partial purge | purge resumes at boot. The slot is not reused until the purge has finished |
| Evict to make room | evict (as delete) → then write the new record | post evicted, new one not written | nothing to repair. Client retries |

The token epoch makes "sign out everywhere" a single atomic write, so an
interrupted password change can never leave the old sessions valid.

### Boot sequence

1. Mount `store`. On failure, the service stays unavailable with a diagnostic. It
   never formats.
2. Read `mm_cfg/schema`. If it is missing and the partition is empty, the board is
   unprovisioned and shows the setup wizard. If the schema is newer than the
   firmware, fail closed ("downgrade refused").
3. Enumerate namespaces in dependency order: accounts, apps, tokens, statuses,
   history, reactions, relationships, then the rest. Build RAM state. Collect
   orphans and tombstones.
4. Run the repairs above. Repair writes also count against the governor.
5. Check invariants (bitmask/slot consistency, id monotonicity, capacity bounds). If
   a check fails, fail closed and name the offending key.

Target: serving within 10 s of power-on at full capacity. Measured in the roadmap's
hardware phase.

## Eviction (ring buffer)

Posts are evicted oldest-first automatically. Eviction runs **before** a write
whenever any of these would be exceeded:

- retained statuses (posts + replies + DMs) > 4,096, or boosts > 2,048,
- reactions (favourites + bookmarks + pins) > 16,384,
- NVS free entries fall below the **low watermark** (20% of the partition), which
  covers worst-case long posts, history and NVS page overhead.

Eviction policy:

- The victim is the **oldest status by id** that is not pinned. Pinned posts
  (at most 5 per account, 80 total) are never evicted.
- Its boosts, reactions and history go with it. Replies to it survive, and their
  `in_reply_to_id` points at a post that no longer exists. That matches Mastodon's
  behaviour for deleted parents.
- Boosts can also be evicted on their own (oldest first) when only the boost cap is
  hit.
- Every eviction increments a persistent-in-RAM counter that is shown in
  diagnostics and the household app ("Oldest post now: 12 March 2031"). The app
  warns when an eviction is expected within ~30 days at the current posting rate.
- Export (roadmap, later) lets the household keep an archive before posts roll off.

## Avatars and headers (LittleFS)

- `media` partition, LittleFS via the `joltwallet/littlefs` ESP-IDF component,
  mounted with **no format on failure**.
- Files: `/a/<slot>-<ver>.{png,jpg,webp,gif}` and `/h/<slot>-<ver>.…`. The version
  number is in the account record, so an upload writes the new file and then
  updates the account record (the single commit point), and only then deletes the
  old file. A crash leaves at most one orphan file, which is removed at boot.
- Limits: avatar ≤ 48 KiB, header ≤ 96 KiB. 16 × 144 KiB = 2.25 MiB, which leaves
  headroom in 3 MiB for LittleFS metadata and copy-on-write.
- The server does not decode or resize images. It sniffs the magic bytes (PNG, JPEG,
  WebP, GIF), checks the size, and stores the file. The household app resizes on
  the client (canvas) before uploading. A native app uploading a 3 MB photo gets
  `422` with a message pointing at the household app.
- Accounts with no avatar get one of a few built-in coloured PNGs embedded in the
  firmware (`missing.png` equivalent).
