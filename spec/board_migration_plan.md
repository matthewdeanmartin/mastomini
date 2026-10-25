# Moving a household to another board

Status: path 2 implemented and **used on 2026-10-21 to move the household from
the ESP32-S3 to the ESP32-P4** (runbook: `mastomini_rs/DEPLOY.md`, procedure C;
lessons and wanted features at the end); path 1 proposed; October 2026. Motivating case: the ESP32-P4 test board
outperforms the household's ESP32-S3 (`DEPLOY.md`, 2026-10-09) and could
replace it. Nothing below is implemented except where marked.

## What has to move

| Data | Where | Portable? |
|---|---|---|
| Accounts, password hashes, posts, history, relations, lists, filters, collections, invites, social, bots schedule | `store` partition (NVS, namespaces `mm_*`, [02-storage](02-storage.md)) | Yes: nothing in it is tied to the chip |
| Sessions (token hashes, epochs) and OAuth apps | `store` (`mm_tok`, `mm_app`) | Yes: devices stay signed in if the name and CA stay the same |
| Direct-message keys (per member, sealed per device/token) | `store` (`mm_key`) | Yes: sealed by passwords and tokens, not by the board |
| Avatars and headers | Nothing stored: avatars are generated from the account id and username, headers are a built-in placeholder, media uploads are refused. The `media` partition is unused | Nothing to move |
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
page (sessions, passwords and direct messages survived). Run between two
physical boards on 2026-10-21 (S3 → P4, 19 accounts, 166 posts, 374
records); that run found that the API refused every archive over 4 KiB
(fixed, below). Not yet done: handover mode (item 5), record-level schema
migrations (item 4; today's upgrades are marker-only, so older exports
restore as they are). The archive holds store records only: item 1's
"plus the media files" turned out to be unnecessary (see the table).

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

## The 2026-10-21 move: lessons learned

The S3 → P4 move took one afternoon, most of it rehearsal and one fix; the
cutover itself (export, boot, import) took under a minute. What it taught:

1. **Rehearse on the target with throwaway data.** The P4's seeded test
   household made a free dress rehearsal on the real hardware and the real
   firmware. It found the one blocking bug before the household's data was
   involved.
2. **"Implemented and tested" was not "works".** The restore had unit tests
   and a desktop round trip, but every test household was under 4 KiB, and
   two layers each kept their own body-limit table: the server streamed the
   import in (4 MiB), then the API refused it (4 KiB general limit). Any
   real household got 413. Body limits now come from one table
   (`http::body_limit`), and a test exports a household over the limit.
3. **Specs drift from code.** The plan said media would move; the
   partition is unused. Read the code that does the moving, not the plan.
4. **A fresh board refuses writes until SNTP sets its clock** (503 with
   `Retry-After: 60`), including OAuth app registration. Scripts that talk
   to a just-booted board must wait for `"clock":"synced"`
   (`household_move.py` does).
5. **The identity belongs to the household, not the chip.** Board profiles
   bound name, certificate and chip together, so "the P4 as the household"
   needed an override (`make firmware-takeover`). Because the name, CA and
   certificate stayed the same, no phone, app or bot noticed the move.
6. **Two boards with one name must never both run.** Flashing with
   `--after no_reset` parks the target in its bootloader until the export is
   done; the source still has to be unplugged by hand, and anything written
   to it in between is lost.
7. **The flash baud rate matters.** The default wrote the 2.6 MB app in
   138 s; `-b 921600` through the P4's CH343 took 26.7 s.
8. **Signing in as the owner can evict a real device.** The owner was at 8
   sessions (the cap) right after the move sign-in, so the least recently
   used owner device may have been signed out.
9. **Windows curl needs `--ssl-no-revoke`** against the household CA
   (schannel cannot check revocation for a private CA); Python's `ssl` does
   not care.

## Features that would make the next move easier

Ordered by how much they would have helped on 2026-10-21:

1. **Handover mode on the source** (item 5 above): after a successful
   export, refuse writes (503) and point to the new board. Removes the
   lost-writes window and the "unplug it now" step.
2. **Household identity separate from board profile**: `MASTOMINI_BOARD`
   picks the chip; a separate `MASTOMINI_IDENTITY=household|test` picks name
   and certificate. Then `make deploy` keeps working after a move, and the
   `firmware-takeover`/`deploy-takeover` stopgap goes away.
3. **A `scripts/takeover.py` that does the whole cutover**: verify MAC and
   layout, flash and erase with the target parked, export, boot, import,
   compare counts on both boards, and print the unplug instruction. Today
   the flash/erase/park part is an ad hoc esptool sequence (DEPLOY.md,
   procedure C).
4. **A claim code for the import**: while unprovisioned, `/setup` and
   `/import` accept anyone on the LAN. Printing a one-time code in the boot
   log (and requiring it) closes the window between erase and restore.
5. **Export refused over plain HTTP**: the owner's password is posted with
   the export request. The archive is encrypted, but the password is not
   protected on port 80. (`household_move.py` only uses HTTPS.)
6. **An export credential that is not a new session**: a short-lived
   export grant from an existing owner session, or an operator key, so the
   tooling never evicts a real device.
7. **Faster flashing by default**: put `-b 921600` in `scripts/board.py`'s
   esptool command for boards whose bridge handles it (CH343 does; test
   the S3's native USB before changing it there).
8. **The store schema in `/api/mastomini/v1/version`** (path 1 above): one
   GET on each board shows whether the target can take the source's
   archive, before anything is erased.
9. **Certificates naming the mDNS name only, or both boards' IPs**: the
   household certificate names 192.168.1.161, the S3's address. The P4 is
   at .164, so clients by IP fail verification until the DHCP reservation
   moves or `make reissue-cert` adds the new address.
10. **Performant backup** (an unsolved problem across the home lab): a
    16 MiB `read_flash` of the S3 over its native USB ran for over ten
    minutes without finishing, while writing the same board ran at
    ~1.4 Mbit/s (esptool compresses writes, not reads). So the old board
    was repurposed (for party, the same evening) with no flash backup, and
    the encrypted export is the only pre-move copy. Options: a scheduled
    `admin/export` pulled over Wi-Fi (seconds, and the format already
    restores), reading only the used part of `store`, or esptool's
    `read_flash` with a higher baud on a bridge-attached board.
