# Moving a household to another board

Status: path 2 implemented (below), path 1 proposed; October 2026. Motivating case: the ESP32-P4 test board
outperforms the household's ESP32-S3 (`DEPLOY.md`, 2026-10-09) and could
replace it. Nothing below is implemented except where marked.

## What has to move

| Data | Where | Portable? |
|---|---|---|
| Accounts, password hashes, posts, history, relations, lists, filters, collections, invites, social, bots schedule | `store` partition (NVS, namespaces `mm_*`, [02-storage](02-storage.md)) | Yes: nothing in it is tied to the chip |
| Sessions (token hashes, epochs) and OAuth apps | `store` (`mm_tok`, `mm_app`) | Yes: devices stay signed in if the name and CA stay the same |
| Direct-message keys (per member, sealed per device/token) | `store` (`mm_key`) | Yes: sealed by passwords and tokens, not by the board |
| Avatars and headers | `media` partition (LittleFS) | Yes |
| Saved Wi-Fi, PHY calibration | `nvs` partition | **No**: per board (the P4's Wi-Fi is on its C6) |
| HTTPS certificate, hostname | the firmware image (`certs*/`, `MASTOMINI_HOSTNAME`) | Rebuilt for the target: the URLs are rendered from the name, never stored |

Safety catch already in place: the store records `SCHEMA` (7). Newer
firmware upgrades an older store in place at boot; older firmware refuses a
newer store rather than misreading it. So the target must run the same or a
newer firmware than the source.

## Path 1: raw partition copy (offline; tooling only, no firmware change)

Works today between the S3 and the P4 because their partition tables are
identical (`store` 0x410000 + 8 MiB, `media` 0xC10000 + 3 MiB) and both run
ESP-IDF v5.5.3. Downtime: about 10 minutes.

1. **Target identity.** Build the target as the household board: hostname
   `mastomini`, a certificate for `mastomini.local` (+ its IP) from the same
   household CA. Devices then see the same name, CA and tokens: nothing to
   re-pair. Flash it, check it boots.
2. **Freeze the source.** Putting it in download mode (esptool) stops the
   app: the flash is then as after a power cut, which the store's crash
   consistency already handles ([02-storage](02-storage.md#crash-consistency)).
3. **Copy.** Read `store` and `media` from the source (MAC and layout
   verified), write them to the target (MAC and layout verified), compare
   SHA-256 of what was read and written.
4. **Swap.** Power the source off (or rebuild it as `mastomini-old`); give
   the target the source's DHCP reservation so bots and clients pinned to the
   IP follow.
5. **Verify.** Boot log shows the same `accounts`/`statuses` and
   `repairs=0`; `make probe-board`; a device that was signed in still works;
   `/.well-known/incidents` clean. **Rollback:** the source was never
   written: power it back on.

Needed:
- `scripts/migrate.py`: read, verify, write, compare; refuses unless both
  layouts match and the target firmware's schema is ≥ the source's;
- the store `SCHEMA` in `/api/mastomini/v1/version` (today it reports the
  commit and build, not the schema), read from both boards before freezing;
- a DEPLOY.md procedure, and a firmware build per identity (the profile
  system already separates name, certificate and target: `MASTOMINI_BOARD`).

Today both boards run commit `6dc858c1b30f`, so their schemas match.

Limits: same layout only (cannot grow `store` on the P4's 32 MiB flash),
same flash geometry, needs USB to both boards.

## Path 2: export and import in the app (online; firmware features)

**Implemented (October 2026):** `domain/transfer.rs` (format, encryption,
restore with rollback to an empty board), `POST /api/mastomini/v1/admin/export`
(owner + password, a `.mmexport` download), `POST /api/mastomini/v1/import`
(unprovisioned boards only; passphrase in `X-Mastomini-Passphrase`, streamed
body up to 4 MiB), the setup page's *Restore a household* page
(`/setup/restore`) and the household app's owner page *Move to another board*
(`/app/#/admin/move`). The export reads the whole store inside one request
under the service lock, so it is a single point in time without a separate
read-only mode. Tests: `api/tests/transfer.rs`, `mastomini_ui` `move.spec.ts`,
and an end-to-end run between two desktop servers through the real restore
page (sessions, passwords and direct messages survived). Not yet done:
handover mode (item 5), record-level schema migrations (item 4; today's
upgrades are marker-only, so older exports restore as they are), and a run
between two physical boards.

For different layouts, a bigger store, no USB, or a household moving its
own data.

Features:

1. **Consistent export** — `GET /api/mastomini/v1/admin/export` (owner,
   password re-entered): a versioned, chunked archive of every store record
   plus the media files, taken in a short **read-only mode** (writes answer
   503 + `Retry-After`) so it is a single point in time. Streamed (the store
   is up to 8 MiB) through the framework's streamed bodies.
2. **Encryption** — the archive contains password hashes, token hashes and
   sealed keys: encrypt it with a passphrase (e.g. XChaCha20-Poly1305 over a
   key from the owner's passphrase) and never log it.
3. **Import on an empty board** — `POST /setup/import`, only while
   unprovisioned (as `/setup` is), verifying version, checksums and that
   `SCHEMA` ≤ the board's; then the same boot-time repair pass.
4. **Schema migrations** — import must accept older schemas; today upgrades
   are marker-only, so record migrations need code once layouts change.
5. **Handover mode** on the old board — after export, refuse writes and
   show where the household moved, so a device that still finds the old
   board does not diverge.
6. **Household app UI** — Settings → Move to another board: export, the
   passphrase, progress; on a new board's setup page: Restore.

Tests: export → import round trip on desktop (every namespace, media, DM
readable after, sessions valid), an older-schema archive, a truncated or
wrong-passphrase archive rejected, and a write during export refused.

## Path 3 (only if federated): moving the public identity

If the household federates under a public domain and the domain changes,
remote servers need Mastodon's account migration (`Move`, `alsoKnownAs`).
Not needed while the board stays on its local name; belongs to
[federation_roadmap.md](federation_roadmap.md).

## Recommendation

Path 1 for the S3 → P4 move now (small tool, no firmware risk, rollback by
powering the old board back on). Path 2 when the store layout changes or the
P4's larger flash should hold a larger store.
