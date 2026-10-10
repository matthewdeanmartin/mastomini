# Deploy mastomini to the ESP32-S3 board

This runbook is written to be followed step by step, by a person or by an
automated agent. Every step says what to run, what output means success, and
when to **stop and ask**. Do not improvise around a failed step.

There are two procedures:

| Procedure | When | What it writes | Risk |
|---|---|---|---|
| **A. Upgrade** (`scripts/deploy.sh`) | The board already runs mastomini | Application only, at `0x10000` | Low: household data is untouched |
| **B. First install** (`scripts/install.sh`) | The board has never run mastomini, and the user asked for it | Backup first, then bootloader, partition table, application; erases `nvs`, `store`, `media`, `coredump` | High: destroys what was on the board |

Step 3 tells you which one applies. You never choose B on your own.

## Rules

- Work in Git Bash, in `/c/github/mastomini/mastomini_rs`.
- Never run `esptool erase_flash`, `write_flash` by hand, `erase_region` by
  hand, or flash a merged image. Only `scripts/deploy.py` and
  `scripts/install.py` (also invoked by the `.sh` wrappers above) write to
  the board.
- Never run procedure B on a board whose partition table is already the
  mastomini layout. `install.py` refuses it; do not work around the refusal.
- Never provision the board, create accounts, register apps or post anything to
  prove a deployment. The household owner does that. The probe is read-only.
- Never print or copy the contents of `.env`, Wi-Fi passwords, tokens, or backup
  files. Backups contain household posts and password hashes.
- If the serial port or the target board is ambiguous, stop and ask.
- Do not use `espflash`, `idf.py monitor` or a terminal program to watch the
  board. Their reset leaves this board stuck in download mode (see Step 5).
  Use `make boot-log`.
- A successful flash is not completion. The boot log and the probe must pass.

## Prerequisites

- ESP-IDF v5.5.3 at `C:\Espressif` and the `esp` Rust toolchain (`rustup
  toolchain list` shows `esp`). Same setup as nanacoin.
- `uv` (for the probe) and `make`.
- Wi-Fi: nothing is required. A board remembers the network it joined (in its
  own `nvs` partition, which no procedure here overwrites). A board with no
  saved network opens an open Wi-Fi network called `mastomini-setup`, and a
  person joins it with a phone to pick the home network (see "First-time setup"
  below).

  Optional, for developers: built-in credentials in the gitignored
  `mastomini_rs/.env` let a freshly installed board join straight away (and are
  then saved on the board). Check without printing values:
  ```bash
  grep -cE '^MASTOMINI_WIFI_(SSID|PASSWORD)' .env    # 2 = present, 0 = none
  ```
- The board connected with a USB **data** cable.
- HTTPS certificates in `certs/`, made by `make certs` from the household CA
  in `.local/ca` (see `docs/security/https.md`). The firmware build runs
  `make certs` itself: it creates them only if missing and checks them either
  way. **Do not regenerate, reissue or rotate working certificates just to
  deploy**: rotation makes every household device trust a new CA. If
  `.local/ca` is missing on a build computer that isn't the usual one, stop
  and ask: a build there would silently create a second household CA.

## Step 1: Record the tree and pass the desktop gates

The safety audit raises storage to schema 7. Password changes commit the
verifier and password-sealed DM key in one account record, and restart repairs
the canonical key. Firmware that only knows schema 6 refuses the upgraded
store; rollback needs a compatible backup. No existing record kind is reused.

```bash
cd /c/github/mastomini/mastomini_rs
git status --short          # record it; a dirty tree is allowed, never discard changes
git rev-parse --short HEAD  # record it
make check
```

Success: `make check` exits 0 and ends with the pytest line `N passed`.
If it fails, stop: do not deploy code that fails its own gates.

### Safety audit and failure tests

The portable library forbids unsafe Rust. Firmware denies unsafe by default,
with exceptions only for NVS, diagnostic SDK queries and the household TLS
callback module shared with `mastomini_bots`. Timer/reset/RNG queries use safe
APIs. NVS writes use svc's own commit; there is no extra raw commit call.
NVS blobs are bounded to 8192 bytes before allocation and read without a second
copy. Streamed profile bodies are bounded to 160 KiB and must match the declared
length; short/erroring streams cannot mutate profiles.

`FaultStore::fail_after` covers failure before a write, persistence followed by
a lost acknowledgement, storage full, reads, namespace enumeration, individual
visits and statistics. Recovery tests exercise password/key consistency,
revocation, deletion, edits, polls, DMs and scheduler publication. Boot repair
is itself interrupted at every read/visit/write boundary and must converge.
These simulate Store failures; they are not real ESP-IDF allocation/flash fault
injection or physical power-cut tests.

FileStore tests cut every append byte and interrupt actual compaction file
operations after creation, writing, sync and replacement. Complete bad-checksum
records, including the final record, now fail closed and preserve the log.
Incomplete tails are still recoverable, but an in-range corrupted length can
be indistinguishable from a torn append: this format has no independently
checksummed length or explicit acknowledgement marker. Do not claim it can
recover all arbitrary corruption. Record decoding rejects trailing bytes and
malformed lengths.

Additional tests warm DM responses before blocking/revoking/deleting/reusing a
slot, tamper with envelopes, exercise scheduler retries/configuration changes,
and send profile streams through the framework adapter. The reproduced
codec/multipart/scope bugs had failing tests before their fixes. ID allocation
returns an error on exhaustion or unrepresentable clocks.

The remaining unsafe operations have these boundaries:

| Boundary | Why a safe replacement is not currently equivalent |
|---|---|
| NVS enumeration/release/statistics | svc 0.52.1's safe iterator ignores advance errors. Store must propagate failures, so a small fallible iterator owns and releases the SDK handle with RAII. Names are decoded within the fixed 16-byte array. Partition statistics have no safe svc API. |
| Heap and RSSI queries | Heap capability queries have no safe equivalent in the pinned svc. Its safe Wi-Fi query needs the radio owner; the independent incident sampler (now miniframework's `mf-incidents` task) must continue without application or radio locks. Local records use Default and SDK failures yield absent RSSI. |
| Household TLS parsing/configuration/SAN callbacks | svc exposes no equivalent callback API for this constrained household CA. mbedTLS supplies live parsed structures and bounded extension slices. The CA is owned in a Box, freed on failed parsing, then retained in OnceLock. Its private Send/Sync implementations rely on SDK verification only reading the shared CA. Callbacks add verification failures and never clear existing flags. Raw operations are explicit under unsafe_op_in_unsafe_fn. |

The password transition retains one additional sealed key in each changed
account record (at most 16 accounts); plaintext keys/passwords are not stored.
Heap/RSSI callbacks and the SDK's actual thread/allocation behavior still need
board validation. Desktop Miri cannot execute these ESP-IDF calls.

Validation on 2026-10-05: full `make check` passed (211 Rust tests, 52 UI,
27 client and 138 conformance; 58 skipped, 14 expected failures). The two final
recovery assertions and schema-marker regression passed in the final 212-test
Rust suite; the feature-minimal suite passed 205 tests. All 85 bots library
tests passed, as did the S3 release build with HTTP/2 and three targeted Miri
checks. The S3 compiler could not emit `AtomicU32::fetch_max` even in an
isolated reproduction; the handshake maximum now uses an atomic CAS loop,
which compiled on that target and prevents concurrent samples lowering it.

Ten mutations in an isolated source copy were detected by their targeted
tests: accepting codec suffixes, granting excess app scopes, reusing exhausted
IDs, accepting short profile streams, skipping password-key boot repair,
dropping the post-commit recovery latch, advancing schema before validation,
ignoring a complete final-record CRC failure, removing DM block checks, and
recognizing partial multipart delimiters. Live repository sources were not
modified during mutation runs. No board was accessed or flashed.

The next tests with the highest remaining value are SDK fault tests: force
NVS iterator find/info/next failures and verify one release on every exit;
force blob-length changes and allocation failures; fail CA parsing partway
through and verify cleanup; invoke TLS SAN callbacks with existing failure
flags and verify none are cleared. A C shim or controlled board harness is
needed to exercise these actual SDK paths. Follow that with power interruption
during the schema-7 password transition and compaction sync/rename on the
intended filesystem. Those tests are planned, not claimed as completed.

Targeted portable checks from `mastomini_rs`:

```bash
cargo test --locked --no-default-features --lib
cargo +nightly miri test --target x86_64-unknown-linux-gnu --no-default-features --lib records_require_exact_consumption
cargo +nightly miri test --target x86_64-unknown-linux-gnu --no-default-features --lib multipart_delimiters
cargo +nightly miri test --target x86_64-unknown-linux-gnu --no-default-features --lib exhaustion_and_extreme_clocks
```

## Step 2: Identify the serial port

In PowerShell (read-only, does not open ports):

```powershell
Get-CimInstance Win32_PnPEntity |
  Where-Object { $_.Name -match 'COM\d+' -or $_.PNPDeviceID -match 'VID_303A' } |
  Select-Object Name, PNPDeviceID | Format-Table -AutoSize
```

The board is the `USB Serial Device (COMn)` whose `PNPDeviceID` starts with
`USB\VID_303A&PID_1001`. The serial number at the end of the composite device
ID is the board's MAC (e.g. `AC:A7:04:2C:29:9C`).

- Exactly one such port: set `PORT=COMn` in Git Bash.
- None: follow the two-socket check below before concluding the board is
  disconnected. Do not open or flash an unidentified port.
- More than one: stop and ask which board is intended.

Close any serial monitor that holds the port.

### If the board has two USB-C sockets

The known Mastomini board has two USB-C sockets. One exposes a WCH CH343
USB-to-UART adapter (`VID_1A86&PID_55D3`); the other exposes the ESP32-S3's
native USB interface (`VID_303A&PID_1001`) used by this runbook. Both can
supply power. A CH343 port may therefore be this same board through the
other socket; its different USB ID does **not** prove it is a different board.

1. List currently attached devices without opening any serial port:
   ```powershell
   Get-PnpDevice -PresentOnly |
     Where-Object { $_.InstanceId -match 'VID_303A|VID_1A86' -or $_.Class -eq 'Ports' } |
     Select-Object Status, FriendlyName, InstanceId | Format-Table -AutoSize
   ```
   Use `-PresentOnly`: remembered COM ports from unplugged devices are not
   evidence that a board is connected.
2. If only a CH343 port appears, ask the user to unplug the intended board
   and repeat the listing. If that port disappears, it belongs to the
   unplugged board's adapter. This identifies the connection, not the chip
   MAC or partition layout. Do not guess from a COM number alone.
3. Ask the user to reconnect the same data cable to the board's **other
   USB-C socket**, then repeat the listing. Continue only when exactly one
   native `VID_303A&PID_1001` serial port appears and the composite device's
   MAC identifies the intended board. If none appears, check the data cable
   and computer USB connection; if still missing or ambiguous, stop and ask.
4. If the status light is dark or solid after the swap, run
   `make boot-log PORT="$PORT"` once the native port is identified. This
   performs the documented watchdog restart without flashing. Follow Step 5's
   summary and retry/stop rules. Record the store counts as the baseline
   before an upgrade, then resume Step 3.

Power lights, blinking, and a dark RGB light do not identify the USB interface
or establish firmware health. Use the device listing and boot-log summary.
Do not switch deployment tools or use CH343 as a fallback around these checks.

## Step 3: Decide the procedure (read-only)

```bash
make firmware
source scripts/esp-python.sh
"$esp_python" scripts/install.py --port "$PORT" --images "$images" \
  --backup-dir ../.local/board-backups --dry-run
```

This builds the firmware **once** (first build: 10–20 minutes), then reads the board's
MAC and partition table **without writing anything**, and prints them.

Keep this Git Bash session open for Step 4: `esp_python` and `images` identify
the interpreter and artifacts used here. Do not change firmware inputs,
certificates, build settings or image files between this build and flashing.
If they change, rebuild and repeat the dry run. The `.sh` wrappers rebuild on
every invocation; use the underlying Python scripts below to reuse this
freshly built image through the remaining steps, with the same safety checks.

| Output contains | Meaning | Next |
|---|---|---|
| `REFUSED: this board already runs mastomini` | Board has household data | **Procedure A** (Step 4A) |
| A partition table that is not mastomini, and `Dry run. To install, rerun with --confirm-mac …` | Board has never run mastomini | **Procedure B** only if the user explicitly asked to install mastomini on this board. Otherwise stop and ask, showing them the partition table |
| A build error | | Stop; report the error |
| `Could not read the board MAC` or a serial error | | Check Step 2; retry once; then stop |

Record the MAC and the printed partition table in your report.

## Step 4A: Upgrade (board already runs mastomini)

```bash
"$esp_python" scripts/deploy.py --port "$PORT" \
  --image "$images/mastomini-esp32.bin" --dry-run  # plan only; no build or port access
"$esp_python" scripts/deploy.py --port "$PORT" \
  --image "$images/mastomini-esp32.bin"
```

Success: the last line is
`Application updated. The board restarts; household data was not touched.`

`REFUSED: the board does not have the mastomini partition layout` means you
are on the wrong board or Step 3 was misread: stop.

## Step 4B: First install (only with the user's go-ahead)

Use the MAC printed in Step 3:

```bash
"$esp_python" scripts/install.py --port "$PORT" --images "$images" \
  --backup-dir ../.local/board-backups --confirm-mac aa:bb:cc:dd:ee:ff
```

It backs up the entire 16 MiB flash to `../.local/board-backups/` (about 4
minutes) and prints the backup's sha256 before it writes anything. Success: the
last line is `Installed. The board restarts unprovisioned.`

Record the backup file name and sha256 in your report. If the script stops
before `Backup sha256`, nothing was written.

## Step 5: Boot log and address

```bash
make boot-log PORT="$PORT"
```

This restarts the board (RTC watchdog reset, no flash writes) and captures its
log for up to 60 seconds. Success: the last line is
`SUMMARY: ready, address 192.168.x.y`. Record the address.

A normal boot looks like this (times in ms since reset):

```
I (982)  mastomini_esp32: mastomini 0.1.0 starting
I (3152) mastomini_esp32: store: provisioned=false accounts=0 statuses=0 repairs=0
I (6842) mastomini_esp32: Wi-Fi up: 192.168.1.161
I (7096) esp_https_server: Server listening on port 443
I (7206) esp_https_server: Server listening on port 80
I (7306) mastomini_esp32: Ready at https://mastomini.local (https://192.168.1.161/ and http://192.168.1.161/)
```

`store:` takes about 2 seconds on an empty store and grows with the number of
posts.

Useful lines in the log:

- `store: provisioned=… accounts=… statuses=… repairs=…`: after an upgrade,
  `accounts` and `statuses` must match what the household had. `repairs` above
  0 means boot finished an interrupted operation; note it in the report.
- `Wi-Fi up: …` then `Ready at …`.

| Log shows | Action |
|---|---|
| `store:` error, or a panic before `Wi-Fi up` | Stop. Do not erase anything. Report the log. |
| Stops after `mastomini … starting` with Wi-Fi errors | Wi-Fi credentials or signal. Stop and ask. |
| `SUMMARY: setup mode, network mastomini-setup` (exit code 2) | The board has no working Wi-Fi. After a **first install** without built-in credentials this is expected: continue with "First-time setup". After an **upgrade** of a board that was on Wi-Fi, it means it can no longer join its network: stop and report (router changed, out of range) |
| `SUMMARY: the chip is in download mode` (log shows `boot:0x2 (DOWNLOAD(USB/UART0))` and `waiting for download`) | Something reset the board the wrong way. Run `make boot-log PORT="$PORT"` once more: its watchdog reset clears this. If it repeats, ask the user to unplug and replug the USB cable, then run it again |
| `SUMMARY: … nothing more within the time limit` | Run `make boot-log PORT="$PORT"` once more, then stop and report |

## Step 6: Probe the running board (read-only)

```bash
make probe-board ADDRESS=192.168.x.y
```

Use the IP address for anything scripted: on Windows each request to
`mastomini.local` can take 10+ seconds to resolve. Also try the name once,
which needs mDNS on this computer:

```bash
make probe-board ADDRESS=mastomini.local
```

Success: the output ends with `Board probe passed`, and the `firmware version`
line shows the version in `Cargo.toml`. The HTTPS checks verify strictly
against `certs/household-ca.crt` for `mastomini.local` (also when probing by
IP), and require the board to present exactly `certs/mastomini.crt` and serve
exactly `certs/household-ca.der` at `/ca`: the files of this build. Never
substitute `curl -k` or a browser warning bypass for them. The `info provisioned=…` line reports
the household state; after an upgrade it must match before the upgrade.

If only `mastomini.local` fails, report the name failure separately from
firmware/IP health. Run `make probe-mdns ADDRESS=192.168.x.y` to distinguish
direct mDNS replies, multicast-query delivery, and the OS resolver. A name
failure alone does not prove mDNS is blocked on this computer. See
`../docs/installation.md` for the diagnostic interpretation and limitations.

Optional status light: default off; configure `MASTOMINI_STATUS_LED_PIN=48`
or `38` only after confirming the board's WS2812 GRB LED pin. Put it in this
crate's gitignored `.env` or prefix every build/deploy command. `off` disables
output. Verify the boot log's `Status LED` line; the firmware source
fingerprint does not include environment-based pin selection. See the light
patterns and limitations in `../docs/installation.md`.

Then confirm the board runs exactly this working tree (read-only):

```bash
make board-version ADDRESS=192.168.x.y
```

Success: `Board firmware matches this working tree.` (exit 0). It compares the
build fingerprint from `GET /api/mastomini/v1/version` with the same hash of
the working tree's firmware inputs. Before a deploy, the same command tells you
whether one is needed at all: exit 1 means the board differs.

## First-time setup (done by the household, not by the operator)

After a first install the board is unprovisioned. Do not create the household
yourself unless the user gave you the owner's username and password for that
purpose. The household does it like this:

1. **Wi-Fi** (only if the boot log said setup mode): on a phone, join the Wi-Fi
   network `mastomini-setup`. The setup page opens by itself (or open
   `http://192.168.4.1/`). Pick the home network, type its password, and tap
   **Join network**. The page then shows the board's address on the home
   network, e.g. `http://192.168.1.161/`.
2. **Household**: tap **Create your household** (or visit the board's address
   from any browser on the home network). Enter a household name, a username
   and a password. That account is the owner.
3. **Family**: **Invite your family** opens the household app at `/app/`;
   sign in as the owner and create invite links.

The `mastomini-setup` network closes about two minutes after the household is
created; the phone then goes back to the home network.

### Why "download mode" happens

esptool's usual USB reset (`Hard resetting via RTS pin`) leaves this board's
ESP32-S3 latched in download mode, so the new firmware never starts even
though flashing succeeded. The scripts therefore pass `--after watchdog_reset`
to every esptool call, and `make boot-log` restarts the board the same way.
If you ever run esptool by hand (you normally should not), add
`--after watchdog_reset`.

## Stop conditions

| Symptom | Action |
|---|---|
| Ambiguous serial port, or native port still missing after Step 2's socket/cable checks | Stop; ask |
| `make check` fails | Stop; fix the code first |
| Install refused because the board runs mastomini | Use Procedure A; never bypass |
| Deploy refused because the layout is not mastomini | Stop; wrong board or Step 3 misread |
| Backup incomplete | Nothing was written; retry once, then stop |
| Firmware image too large | Stop; never change partition sizes to make it fit |
| Boot log shows a store error | Stop; never erase to "fix" it; the data may be recoverable |
| Probe fails after a good boot log | Report as unverified; include the failing checks |
| Probe's HTTPS checks fail (certificate not trusted, not this build's) | Stop. Inspect `certs/` and `make certs-check`; never rotate certificates to make it pass |
| After an upgrade, `accounts`/`statuses` dropped to 0 | Stop immediately; do not provision; report |
| Chip stays in download mode after two `make boot-log` runs and a replug | Stop; report the log |

## Restoring a backup (only when the user asks)

A backup from Step 4B restores the board exactly as it was:

```bash
/c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe -m esptool \
  --chip esp32s3 --port "$PORT" --after watchdog_reset   write_flash 0x0 ../.local/board-backups/<file>.bin
```

## Completion report

Report, without credentials or household content:

- revision and whether the tree was dirty; `make check` result;
- port and how it was identified; board MAC;
- procedure used (A or B) and why; partition table seen in Step 3;
- for B: backup file name and sha256;
- flash result line;
- boot log summary: `store:` line, `Ready at` address, any `repairs`;
- probe result by name and by IP;
- anything not done, and why.

## Known boards

| MAC | Installed | Previous contents | Backup | Last known address |
|---|---|---|---|---|
| `ac:a7:04:2c:29:9c` (ESP32-S3 N16R8, COM11 on the build PC). Provisioned 2026-09-23 through the setup page with the owner from the repo-root `.env`; Wi-Fi saved on the board | 2026-09-23, procedure B | MicroPython 1.19.1 with an LED demo `main.py`; no household data | `.local/board-backups/aca7042c299c-20260923-081302-full16MB.bin`, sha256 `d73b6dd5…6fb15` | 192.168.1.161 (DHCP; may change) |

| `e8:f6:0a:e3:6f:a2` (Meshnology/Waveshare ESP32-P4-WIFI6, ESP32-C6 Wi-Fi over SDIO, CH343 COM19). Test board, `MASTOMINI_BOARD=p4`, `mastomini-p4.local`, certificates in `certs-p4/`; seeded with `loadtests/seed_board.py` | 2026-10-09, manual esptool install (owner declined the backup) | nanacoin P4 test deploy (owner: not needed; nanacoin can be redeployed) | none | 192.168.1.164 (DHCP) |

Add a row after every first install. Update the address when it changes.

## Deployment comments

- **2026-10-09 — mastomini on the ESP32-P4 test board.** New board profile
  (`MASTOMINI_BOARD=p4`: `scripts/board.py`, `boards/p4/sdkconfig.defaults`,
  `components/p4-network`, feature `board-p4`, target
  `riscv32imafc-esp-espidf`, build dir `C:/mmr-p4`). Identified read-only on
  COM19 (ESP32-P4 v1.3, MAC above, nanacoin layout). The owner cancelled the
  32 MiB backup ("a test deploy, it isn't gold"), so instead of
  `install.py` the same steps ran directly with esptool: erase nvs, store,
  media, coredump; write bootloader 0x2000, table 0x8000, app 0x10000; layout
  verified. First seeding failed on the 140-character limit and left accounts
  without saved passwords; store and media erased, then seeded (owner +
  alice, bob, carol, 48 posts). Strict probe passed, version matched. Quick
  suite (`loadtests/quick_perf.sh`, 198 s, RSSI -60): handshake 242/47 ms,
  cold page h1 1557 / h2 557 ms, timeline handler ~195 ms (S3 ~380),
  internal low-water 61 KB; but sweep failures from 4 users and 21
  `admission_rejected`, not yet explained.
- P4 moved downstairs (RSSI -50, no RST needed, same address). Quick suite on
  both boards back to back, identical settings: P4 vs S3 — handshake 211/32
  vs 321/47 ms; cold page h1 1391 vs 2472 ms, h2 437 vs 767 ms; warm @6 h1
  67.5 vs 41.1 req/s, h2 80.4 vs 46.9 req/s (p95 126 vs 573 ms); sweep
  1/4/8 users 45.6/89.0/102.9 vs 23.5/65.5/77.9 req/s, no failures up to 8
  on either; 12 users 35% vs 15% failed (8 TLS slots); 87 KB HTTPS p50 0.09
  vs 0.22 s; internal low-water 296 KB vs 2.8 KB. The desk-side failures were
  placement (RSSI -60). Reports `loadtests/reports/transport-quick-{p4,s3}-downstairs-20261009-*.json`.

- **2026-10-08 — Wi-Fi/lwIP buffers in PSRAM and framework incidents
  deployed; same-day before/after benchmark.** Revision `10863d6f648c`, dirty
  working tree. Native COM11, MAC `ac:a7:04:2c:29:9c`. On arrival the board
  answered ping at `.161` but refused every TCP connection (HTTP and HTTPS);
  `make boot-log` restarted it (reported `power on`, so no retained lines from
  the unresponsive period; cause unknown). Store 7 accounts, 48 statuses,
  repairs 0; RSSI -71 dBm at boot (weaker than October 4 downstairs).
  Before, on `f56621459135` (HTTP2 build), defaults: HTTP/1.1 cold 3/6
  failed, HTTP/2 cold 6/6 failed; warm 10.2/22.8/28.2 (h1) and
  13.4/28.6/33.6 (h2) req/s at 1/3/6 clients with 0/4/15 and 0/4/14
  connection failures. Board log: 48 `esp-aes: Failed to allocate memory`,
  each followed by `write error :-0x0001`; internal low-water 39 bytes. The
  log over HTTPS was itself truncated at 4 KiB (curl 56); over HTTP complete.
  `HTTP2=1 make firmware`: 2,585,408 bytes. Procedure A: install dry run
  refused as expected with the mastomini layout; only the app at `0x10000`
  written, hash verified. Boot 7/48/0, probe passed, `make board-version`
  matched `64d6df178786`. `/.well-known/incidents` answers with CORS,
  `volatile: false`; `previous` stayed empty because the boot-log restart
  reports `power on` (RTC retention not yet verified on hardware).
  After, same defaults: HTTP/1.1 cold 0/6 failed (median 7465 ms), HTTP/2
  cold 0/6 failed (median 3076 ms); warm 9.3/17.4/21.8 (h1) and
  13.5/22.9/25.5 (h2) req/s with no connection failures, but p95 at 6
  clients rose to 1379 ms (h1) and 3147 ms (h2). No `esp-aes` or TLS write
  errors; internal low-water 2,855 bytes; counts 7/48, no reboot. Incidents:
  4 `slow_request` (1.4–5.1 s), 7 `heap_low`, nothing else. Side effect found:
  `CONFIG_SPIRAM_TRY_ALLOCATE_WIFI_LWIP` forces 16 static Wi-Fi TX buffers and
  raises static RX buffers 10 → 16 (ESP-IDF Kconfig), both in internal RAM:
  idle internal free fell from ~33 KB to ~9 KB. The second-scale tail is
  consistent with TX-buffer exhaustion and TCP retransmission, not verified.
  Reports: `loadtests/baselines/transport-{before,after}-incidents-20261008.json`;
  logs in repo-root `.local/` with the `incidents-20261008` suffix. No commits.
- Same firmware moved downstairs (RST needed after the move again; RSSI -50).
  Locust read sweep over HTTPS, no think time, 30 s stages: 1/2/4/6/8 users
  41/62/61/69/72 req/s with 0 failures (p50 20/27/50/78/100 ms); 10 users
  70 req/s, 7/2107 failed; 12 users 31% failed (fast refusals beyond the 8
  TLS slots), sweep stopped. A single 48-user stage: 26.6 successful of 135
  attempted req/s (80% failed), no reboot, counts unchanged, recovered.
  Incidents: `admission_rejected` 14, `tls_failed` -80 (peer reset during
  handshake) 31, `worker_recovered` 7 (handshake task gaps), no allocation
  failures; internal low-water 1,607 bytes. September 26 sweep on the
  pre-miniframework firmware: 86.6 req/s at 8 users, failures from 16.
  Reports: repo-root `.local/locust-capacity-downstairs-20261008.json`,
  `.local/locust-overload48-downstairs-20261008.json`.
- Mawkingbird reported CORS failures on `/instance`, `/tags`,
  `followed_tags`, `/announcements` and then lost its login. Cause: browsers
  send their first HTTP/2 burst before the board's SETTINGS
  (MAX_CONCURRENT_STREAMS 8) arrive; streams past 8 were reset with
  REFUSED_STREAM, which the browser reports as a CORS error. A one-connection
  burst of 16 preflights + 16 GETs got 8/32 answers. miniframework now queues
  up to 32 bodiless streams (headers only) and still answers at most 8 at a
  time. `HTTP2=1 make firmware`, Procedure A on COM11 (app only, hash
  verified), boot 7/48/0, probe passed, `make board-version` matched
  `f5880a33407f`. The same burst: 32/32 answered with CORS in three runs; no
  incidents beyond boot and `heap_low` (low-water 4,275 bytes).
- **2026-10-09 — Firefox/Mawkingbird follow-up.** The owner still saw
  intermittent "CORS" failures in Firefox, long after page load. Playwright
  Firefox, cross-origin, with a temporary read token (revoked afterwards):
  0 failures in 96 + 160 + 40 requests, but bursts after idle took 2–41 s.
  Single 65 KB home-timeline fetches took 1.7–12.7 s after the first byte
  (HTTP alone 1.4–4.5 s). A/B: without `CONFIG_SPIRAM_TRY_ALLOCATE_WIFI_LWIP`
  5 of 6 such HTTPS responses were truncated (5–53 KB of 65,866) with
  internal low-water 39 bytes, so that setting stays; transfers were as slow
  either way. Idle ping to the board: p50 65 ms, p90 258 ms, max 432 ms (router
  2 ms), Wi-Fi power save off, RSSI -60. Also deployed: idle timeout 60 → 300 s
  (fewer reconnect handshake storms) and `Access-Control-Allow-Private-Network`
  for preflights that ask (Chrome only). Each change flashed by Procedure A,
  7/48/0 after every boot; final firmware restored with the PSRAM setting.
- Board moved downstairs again; RST needed after the move. Firmware
  `5492b079f1f3`, RSSI -44, idle ping p50 11 ms / p90 23 ms (router 2 ms).
  Transport benchmark, defaults: cold 0/6 failed both modes (h2 median
  2358 ms); warm 12.4/28.5/39.3 (h1) and 12.4/31.0/42.2 (h2) req/s, no
  connection failures; p95 at 6 clients 121 ms (h1), 831 ms (h2). No
  `esp-aes` failures; internal low-water 3,907 bytes. HTTPS 87 KB transfers
  after the handshake: p50 0.31 s, p90 0.77 s, max 2.66 s (5 stalls over
  300 ms in 15). Report: `loadtests/baselines/transport-downstairs-final-20261009.json`.
- A Firefox HAR from Mawkingbird showed connection-level failures only (new
  TLS connection per request, preflights failing right after the handshake,
  board log `write error -0x50` at the same uptime). Deployed: incidents
  `h2_goaway` / `h2_peer_goaway` / `h2_refused` / `evicted`, least recently
  used session eviction, and `CONFIG_LWIP_TCP_SND_BUF_DEFAULT` 11520 → 32768.
  MAC `ac:a7:04:2c:29:9c` read on COM11 before flashing (bots board
  `…:38:b8` was attached earlier on COM15 and restarted once by mistake with
  `make boot-log`; no writes). Procedure A, boot 7/49/0, probe passed,
  version matched. 87 KB HTTPS transfers, 20 runs at RSSI -68: p50 0.39 s,
  p90 0.47 s, max 0.52 s, no stalls over 300 ms (before: p90 0.77 s, max
  2.66 s, 5 stalls in 15). HTTP/2 burst 32/32; Playwright Firefox 160/160.
- **Root cause of the Mawkingbird "CORS failed" errors: Firefox 157 Local
  Network Access.** Owner's `about:logging` capture: each request from
  mawkingbird.com to the board awaits an LNA permission check (25
  `LNAPermissionRequest granted`, seconds apart). A request activated on a
  shared HTTP/2 connection before its grant fails with `804b005d`, and Firefox
  closes the whole connection; the blocked request is restarted, but the
  other streams on it are aborted (`80004004`) and reach the page as "CORS
  request did not succeed". 17 such activations and 13 killed connections in
  one session; board incidents showed only client-initiated closes. HTTP/1.1
  hid it (one request per connection). Fix on the owner's Firefox:
  `network.lna.skip-domains = mastomini.local`; afterwards requests ran
  50–250 ms instead of 1–2 s, with no failures.
- **TLS 1.3 and an ECDSA P-256 server certificate.** `idle_close.py
  --handshakes` (median of 10–15, downstairs, RSSI -59): RSA + TLS 1.2 full
  1047 ms / resumed 47 ms; RSA + TLS 1.3 953 / 250; ECDSA + TLS 1.2 328 / 47;
  ECDSA + TLS 1.3 297 / 250. TLS 1.3 resumption repeats the key exchange
  (browsers offer only psk_dhe_ke), so it is ~200 ms slower than TLS 1.2
  tickets; internal low-water ~3.5 KB with it. Certificate reissued with
  `MASTOMINI_CERT_KEY=ec MASTOMINI_CERT_IPS=192.168.1.161 make reissue-cert`
  (same household CA, same names; previous certificates in
  `.local/cert-backups/`). MAC read on COM11, Procedure A, boot 7/51/0, strict
  probe passed, version matched; Firefox (OS roots) and Chromium trust it
  (`browser_session.py --trust`; negative control rejected with
  SEC_ERROR_UNKNOWN_ISSUER); h2 burst 32/32, Firefox session 0 failures.
- Final for this round: **ECDSA P-256 + TLS 1.2 only** (TLS 1.3 disabled again
  for its slower resumption). MAC read on COM11, Procedure A, boot 7/51/0,
  strict probe passed, version matched; TLSv1.2 ECDHE-ECDSA-AES256-GCM-SHA384,
  full handshake p50 422 ms (8 runs, fresh boot), resumed p50 70 ms; Firefox
  and Chromium trust it; h2 burst 32/32. Board then moved downstairs.
- Downstairs perf run (`63bcb3c208dc`, RSSI -44..-47, came up without RST).
  Transport defaults: cold page view h1 2481 ms / h2 792 ms (previous best
  7006 / 1403), 0/6 failed; warm 13.1/32.1/48.3 (h1) and 15.2/34.4/50.1 (h2)
  req/s, p95 @6 136 / 552 ms, no connection failures, heap low-water 2819.
  Sweep: clean to 8 users at 75.0 req/s (Oct 8: 71.5; Sep 26 old firmware
  86.6), first failures at 10 users (8/2096). Handshakes: full p50 312 ms,
  resumed 47 ms. 87 KB transfers: HTTP p50 0.14 s, no stalls; HTTPS p50
  0.25 s but p90 2.16 s with 4 stalls in 15 (run straight after the sweep).
  Reports: `loadtests/baselines/transport-ecdsa-tls12-20261009.json`,
  repo-root `.local/perf-*-20261009.*`; compare with
  `loadtests/compare_reports.py`.

- **2026-10-04 — miniframework and HTTP/2 deployed; upstairs baseline.**
  Revision `10863d6f648c`, dirty working tree. Both USB sockets attached:
  native COM11 (`VID_303A&PID_1001`, composite MAC `ac:a7:04:2c:29:9c`)
  and CH343 COM13. The documented watchdog restart restored visible blinking
  and booted the previous firmware at `192.168.1.161`, with
  `provisioned=true accounts=7 statuses=38 repairs=0`. The quiet-start cause
  remains unknown. `make check` passed: 191 Rust tests (also bundled-web),
  52 UI tests, smoke, 27 client tests, 138 conformance passed / 58 skipped /
  14 expected failures. `HTTP2=1 make firmware` built 2,539,552 / 4,194,304
  application bytes using the existing certificates and GPIO48 setting.
  The read-only install check confirmed `nvs` at `0x9000` (24 KiB),
  `phy_init` at `0xf000` (4 KiB), `factory` at `0x10000` (4 MiB),
  `store` at `0x410000` (8 MiB), `media` at `0xc10000` (3 MiB), and
  `coredump` at `0xf10000` (64 KiB); first install was refused as expected.
  Procedure A dry run passed, then only the app at `0x10000` was written.
  Hash verified; `Application updated. The board restarts; household data
  was not touched.` Boot again reported 7 accounts, 38 statuses, zero repairs
  and ready at `.161`. Strict probes passed by IP and `mastomini.local`,
  including this build's certificate and CA. `make board-version` matched
  `f56621459135`, built `2026-10-05T02:02:57.196Z`.
- Anonymous upstairs benchmark, defaults: 20 samples per endpoint, 6 cold
  views, 30 seconds at each of 1/3/6 clients, 0.5-second think time.
  Before: HTTP/1.1 cold median 7182 ms, 0/6 failures; throughput
  9.8/28.1/51.3 req/s, no connection failures. Old firmware offered no h2.
  After: HTTP/1.1 cold median 7857 ms (successful views only), 4/6 failed
  with incomplete response bodies; throughput 11.7/28.6/47.9 req/s,
  connection failures 0/1/7. HTTP/2 negotiated successfully: cold median
  1487 ms (successful views only), 4/6 failed with closed connections;
  throughput 15.1/33.5/44.0 req/s, connection failures 0/0/10.
  Completed requests had no status errors. Both runs preserved 7/38 counts,
  store availability and increasing uptime; no reboot. Internal heap
  low-water was 11,459 bytes before and 23 bytes after. The owner reports
  weak upstairs Wi-Fi and plans to move downstairs for the same benchmark;
  the failure cause is not established. Do not treat this as a clean
  performance acceptance. Raw reports retained in `loadtests/baselines/`;
  boot, build, check, flash and probe logs are in the repo-root `.local/`
  with the `http2-20261004` suffix. No commits made.
- After moving the board for the downstairs comparison, the owner reported
  that the working USB socket shows the red power LED on plug-in, but requires
  pressing RST to start the diagnostic light. The other socket does not show
  the red power LED with the same cable and charger. The board is no longer
  attached to the build PC's USB ports. Network-only probes passed after the
  manual reset; 7 accounts and 38 statuses remain. The new boot's public log
  reports `power on`, store repairs 0, ready about 6.1 seconds after app
  startup; it does not capture the failed start before RST. The socket and
  cold-start cause remains unverified. Earlier September 26 notes already
  record a similar power-path-dependent dark start on the previous firmware.
  Saved logs: repo-root `.local/downstairs-startup-http2-20261004.txt` and
  `.local/downstairs-sys-http2-20261004.json`. Boot-time RSSI was -40 dBm;
  the subsequent system snapshot was -50 dBm (upstairs boot: -61 dBm).
- Downstairs, same image and benchmark defaults: HTTP/1.1 throughput at
  1/3/6 clients was 12.9/32.5/59.4 req/s, with no connection failures.
  All 6 HTTP/1.1 cold loads failed. HTTP/2 throughput was 15.6/37.0/46.9
  req/s, with 0/3/6 connection failures; 4/6 cold loads failed, and the
  successful-view median was 1403 ms. `/app/` warm HTTP/1.1 p50 fell from
  19.14 ms upstairs to 14.32 ms downstairs. No status errors on completed
  requests, no reboot, counts still 7/38 and store available. The strict
  IP probe passed again after the run. The mid-benchmark device log explicitly
  shows repeated `esp-aes: Failed to allocate memory` followed by
  `esp-tls-mbedtls: write error :-0x0001`; internal heap low-water again
  reached 23 bytes. This establishes TLS allocation failure during the
  downstairs cold-load phase, despite stronger signal. Socket/RST cold-start
  behavior remains a separate unresolved observation. Raw report:
  `loadtests/baselines/transport-downstairs-http2-20261004.json`; diagnostic
  log: repo-root `.local/downstairs-midbench-http2-20261004.json`.

- **2026-10-02 — latest firmware with public `/metrics` deployed.**
  Procedure A on COM11, MAC `ac:a7:04:2c:29:9c`, revision `10863d6`
  with the runbook edits uncommitted. `make check` passed: 197 Rust tests,
  52 UI tests, smoke, 27 client tests, and 138 conformance tests (58 skipped,
  14 strict expected failures; zero unexpected failures). The read-only
  install check confirmed the existing `nvs`, `phy_init`, 4 MiB `factory`,
  8 MiB `store`, 3 MiB `media`, and `coredump` layout. The 2,391,056-byte
  application was written only at `0x10000`; its hash verified and the script
  reported `Application updated. The board restarts; household data was not
  touched.` Boot and IP probe preserved `provisioned=true accounts=6
  statuses=25 repairs=0`, ready at `192.168.1.161`. Strict HTTPS and the
  existing certificate/CA checks passed. `make board-version` matched
  `73d476749eb6`, built `2026-10-03T03:53:28.970Z`. `/metrics` returned 200
  over HTTPS verified against the household CA, with InfluxDB machine-health
  fields. The hostname probe was not repeated because of the previously
  documented Windows resolver delay; IP health is verified. Boot log:
  `mastomini_rs/.local/boot-log-2026-10-02-metrics.txt`.
- This run followed the old wrapper sequence and compiled the release image
  three times (about 86, 60 and 60 seconds). Steps 3 and 4 now build once and
  reuse the resulting image through the same Python safety checks.

- **2026-10-02 — two USB-C sockets identified; no flash performed.**
  The attached board initially appeared only as CH343 COM13
  (`VID_1A86&PID_55D3`, adapter serial `5B5E002630`). It disappeared when
  the owner unplugged the board, confirming that the adapter belonged to
  that board. Moving the same connection to the other USB-C socket exposed
  native USB COM11 and composite MAC `ac:a7:04:2c:29:9c`, matching the known
  Mastomini board. The owner reported no blinking after the socket swap;
  `make boot-log PORT=COM11` restarted the existing firmware and ended with
  `SUMMARY: ready, address 192.168.1.161`. GPIO48 initialized; the store was
  `provisioned=true accounts=6 statuses=25 repairs=0`. This confirms boot
  after the watchdog restart; it does not establish why the light was quiet
  before it. No firmware was flashed and no board probe was run in this check.

- **2026-09-26 — dark cold-start investigation and clearer boot light.**
  The owner reported power red on but RGB dark for more than 30 seconds;
  HTTP at `.161` and mDNS did not answer. Reattached COM11, confirmed MAC
  `ac:a7:04:2c:29:9c`. Eight seconds of passive serial capture yielded no
  messages. A runbook watchdog restart of the existing firmware restored
  normal boot: GPIO48 initialized at 1.149 seconds, store had six accounts,
  two statuses and zero repairs, Wi-Fi joined `.161` and server ready by
  6.849 seconds. Strict IP/HTTPS probe passed. The original dark boot's
  cause remains unknown; do not infer it was Wi-Fi or download mode.
- The startup white marker now lasts 1.5 seconds, followed by a 0.5-second
  gap and reset-class flashes. LED initialization precedes the incident
  recorder, NVS and Wi-Fi. This still cannot signal a ROM/download-mode
  stop, a failure before application startup, or loss of CPU/LED power.
  An ordinary power-on observation is needed in addition to software-reset
  deployment checks. `make check` passed (180 Rust, 46 UI, smoke, 27 client,
  138 conformance; 58 skipped, 14 xfailed).
- Deployed with procedure A on COM11: 2,287,904 bytes, hash verified.
  Build `2026-09-26T22:35:45.583Z`, fingerprint `3e6d268211f7`, revision
  `7a2d172a522c` plus uncommitted changes. Boot shows the LED initialized
  before the reset-reason log, six accounts, two statuses and zero repairs;
  ready at `.161` by 6.779 seconds. Strict IP/HTTPS probe passed and
  `make board-version` matched. Cold power-on behavior remains unverified.
  The hostname probe subsequently passed all strict HTTP/HTTPS checks too.
- The owner then confirmed that using the board's other USB connector for
  power makes it boot, visibly confirmed by the diagnostic light. This
  identifies the dark-start symptom as dependent on the USB power path;
  the exact connector labeling and electrical cause were not established.
  Use the known-working power connector. A red power LED alone did not
  establish that the CPU had booted. Do not attribute this occurrence to
  Wi-Fi, mDNS, authentication, or the status-light task.

- **2026-09-26 — household app sign-in cache compatibility.** After signing
  out, the browser failed before requesting the OAuth form, at
  `stored.scopes?.split(' ')`. Older UI bundles cached the server's scope
  array; the newer UI expected a string. Optional chaining does not make
  an array support `split`. This was a browser exception, not LED load.
  Normalize and validate cached registrations before use, retain the existing
  server validation and scope checks, and save the normalized record. No
  clearing site data is required. Malformed records are replaced through
  normal app registration. Sign-in failures now show a stage/error code in
  the page and console without logging credentials, tokens or OAuth state.
  Required session storage failures are reported before leaving the page.
  The regression reproduced the exact TypeError before the fix. `make check`
  passed: 180 Rust tests, 46 UI tests, smoke, 27 client tests, and 138
  conformance tests (58 skipped, 14 xfailed). After upgrading, resume any
  paused debugger, hard-refresh the app (Ctrl+Shift+R), then try Sign in;
  an already-open tab may still be executing the old bundle.
- Deployed the sign-in fix with procedure A on COM11, MAC
  `ac:a7:04:2c:29:9c`: 2,287,824 bytes at `0x10000`, hash verified.
  Fingerprint `76b325b635a7`, revision `7a2d172a522c` plus uncommitted
  changes, built `2026-09-26T22:03:40.929Z`. Boot reports GPIO48 enabled,
  `provisioned=true accounts=6 statuses=2 repairs=0`, mDNS registered,
  ready at `192.168.1.161`. Strict IP/HTTPS probe passed with the existing
  household certificate and CA; `make board-version` matched the tree.
  The hostname probe also passed every HTTP/HTTPS check, with slow Windows
  name resolution as previously observed.
  The owner confirmed that sign-in works again with the existing browser data.

- **2026-09-26 — GPIO48 LED trial deployed at the user's request.** The user
  explicitly asked to try the likely pin rather than wait for a schematic.
  Confirmed COM11, MAC `ac:a7:04:2c:29:9c`; pre-flash probe passed. The install
  dry run read the existing mastomini layout (`nvs`, `phy_init`, 4 MiB
  `factory`, 8 MiB `store`, 3 MiB `media`, `coredump`) and correctly refused
  first installation. Used procedure A with `MASTOMINI_STATUS_LED_PIN=48`,
  revision `7a2d172a522c` plus the tested diagnostics changes. The 2,287,296-byte
  app hash verified; `Application updated. The board restarts; household data
  was not touched.`
- Boot log reports `Status LED: WS2812 GRB, GPIO 48, dim output`, reset reason
  `power on` (one white flash), no LED driver error, and
  `provisioned=true accounts=2 statuses=2 repairs=0`. Wi-Fi and mDNS came up
  at `192.168.1.161`. The strict IP probe passed; `make board-version` matched
  fingerprint `5945ba61c182`, built `2026-09-26T21:37:24.450Z`. LED appearance
  is awaiting the user's observation; driver success alone cannot confirm
  a WS2812 is physically connected. Generic firmware still defaults to off.
  The hostname probe subsequently passed every HTTP/HTTPS check as well,
  with the same slow Windows name lookups. No household settings were changed.
- The user visually confirmed the dim green heartbeat: GPIO48 is now verified
  for board `ac:a7:04:2c:29:9c`. Saved `MASTOMINI_STATUS_LED_PIN=48` in this
  checkout's gitignored `mastomini_rs/.env` so routine upgrades retain it.
  This is a local build setting, not a change to the generic default. For
  another board, explicitly select its verified pin or override with `off`.

- **2026-09-26 — mDNS investigation and optional status LED work.** The
  first attached board was COM9, MAC `ac:a7:04:2c:2c:04`; passive serial
  output identified nanacoin. It was not reset or flashed. The user then
  attached mastomini on COM11, MAC `ac:a7:04:2c:29:9c`.
  Before reset, HTTP at the last-known IP `192.168.1.161` timed out and
  Windows name resolution failed. An eight-second passive serial capture
  had no diagnostic output; the pre-reset cause is unknown.
- `make boot-log PORT=COM11` restarted the existing firmware and passed:
  `provisioned=true accounts=2 statuses=2 repairs=0`, Wi-Fi joined and ready
  at `192.168.1.161`. No firmware was written for this diagnosis.
  `scripts/probe-mdns.py` then found HTTP healthy (installed fingerprint
  `e0293f0824a5`), direct and multicast mDNS queries answered with the correct
  IP, and Windows resolved it after about 14 seconds. This establishes a
  slow client lookup after recovery, not the cause of the pre-reset outage.
  A repeat measured first mDNS replies at 125 ms for both query paths versus
  13.89 s for Windows `getaddrinfo`; IPv4-only lookup still took 11.14 s.
  The trailing-dot form `mastomini.local.` failed, so it is not a workaround
  on this PC. The strict IP HTTP/HTTPS board probe passed with both accounts
  and posts preserved. Network and firewall settings were not changed.
- Optional dim WS2812 diagnostics now use completed HTTP/TLS worker turns,
  Wi-Fi IP events, setup state, errors and reset reason. Default output is
  off; no LED is probed or autodetected. An mDNS registration error now logs
  a warning and leaves IP HTTP(S) serving, instead of aborting startup.
  Hardware LED verification remains pending pin confirmation and deployment.
  The GPIO48-enabled ESP32-S3 release build passed at 2,287,296 / 4,194,304
  application bytes. An LED-off build also compiled successfully during
  validation. Neither image was flashed in this investigation.
- Final `make check` passed: formatting, clippy, script syntax, strict docs,
  180 Rust tests (also with bundled web), UI tests, smoke, 27 client tests,
  and 138 conformance tests (58 skipped, 14 expected failures). Regression
  coverage includes missing/invalid LED configuration, disconnected and
  stalled states, both-worker progress including timer wrap, dim patterns,
  and malformed/compressed mDNS packets. An earlier gate run was invalidated
  by editing firmware sources while its fingerprint check was running;
  rerunning against settled sources passed.

- **2026-09-24 — upgrade on COM11** (ESP32-S3, MAC `ac:a7:04:2c:29:9c`),
  checkout revision `780f7e5` with a dirty working tree. `make check` passed:
  117 Rust tests, 14 UI tests, 6 client tests, and 96 conformance tests passed;
  the conformance suite also reported its existing skips and expected xfails.
- The first `bash` available in PowerShell was WSL, where the guide's
  `/c/...` path and `make` were unavailable. Switched to the installed Git Bash
  explicitly; deployment then followed the documented commands.
- The install dry run identified the existing mastomini partition layout, so
  procedure A was used. The app image was 1,813,024 bytes (under the 4 MiB
  limit). The flash hash verified and the script reported `Application
  updated. The board restarts; household data was not touched.`
- Firmware compilation emitted 10 non-fatal uppercase-name warnings for the
  ESP-IDF reset-reason constants in `src/bin/esp32.rs`.
- Boot log: `store: provisioned=true accounts=2 statuses=2 repairs=0`;
  ready at `192.168.1.161`. The read-only probes passed by IP and by
  `mastomini.local`. The hostname probe took about 90 seconds to resolve on
  Windows, then all checks passed.
- **2026-09-24 — redeployed revision `d40d624`** on the same board from a clean
  working tree. `make check` passed: 134 Rust tests, 14 UI tests, 6 client
  tests, and 115 conformance tests (68 skipped, 27 xfailed). Procedure A again
  verified the existing mastomini layout. The 1,962,384-byte application was
  written only at `0x10000`; esptool verified its hash and reported
  `Application updated. The board restarts; household data was not touched.`
  Firmware compilation again emitted 10 non-fatal uppercase-name warnings for
  ESP-IDF reset-reason constants. Boot reported
  `store: provisioned=true accounts=2 statuses=2 repairs=0` and ready at
  `192.168.1.161`. Probes passed by IP and by `mastomini.local`; the Windows
  hostname probe again took about 90 seconds before resolving.
- **2026-09-24 — HTTPS ("Easy mode") deployed** on the same board (COM11,
  MAC `ac:a7:04:2c:29:9c`), from revision `d40d624` with the uncommitted HTTPS
  work. `make check` passed: 141 Rust tests, 15 UI tests, smoke, 11 client
  tests (5 of them HTTPS end to end), and 115 conformance tests (68 skipped,
  27 xfailed). A new household CA was created with `make certs`
  (`MASTOMINI_CERT_IPS=192.168.1.161` given on the command line, so the board's
  current IP is in the certificate); CA SHA-256 fingerprint
  `C2:8F:EE:1E:39:95:C2:A6:9D:68:BB:99:00:86:0C:A2:65:97:52:4E:7B:AB:2B:71:7A:C7:5E:B2:A0:15:54:FD`,
  server certificate valid until 2028-12-23. The install dry run refused
  (existing mastomini layout), so procedure A. The dry run's reset left the
  board unreachable until one `make boot-log`, as the runbook describes.
  Before the upgrade: `provisioned=true accounts=2 statuses=2 repairs=0`.
- First flash (2,051,904 bytes) booted with both listeners and passed the
  probe, but measuring showed a TLS handshake took about 1.5 s at the default
  160 MHz, every response said `Connection: close` (so clients would redo the
  handshake per request), and 10 simultaneous new HTTPS clients timed out on
  some connects. Fixed and redeployed: 240 MHz CPU, and HTTPS responses keep
  the connection. Second flash 2,051,856 bytes, hash verified.
- Final boot: `store: provisioned=true accounts=2 statuses=2 repairs=0`,
  listeners on 443 and 80, ready at `192.168.1.161`. The probe passed by IP,
  including strict HTTPS for `mastomini.local`, the IP address verified
  against the certificate, this build's certificate and CA, and HTTPS OAuth
  endpoints. The `mastomini.local` probe passed on the first firmware.
  Measured on the final firmware: handshake 1.0–1.2 s; on an open connection
  about 0.05 s per request; 4 clients × 5 requests all succeeded (median
  0.2 s); 10 clients connecting at once got 47 of 50 (handshakes are
  serialized on one server task with 5 HTTPS sockets). Plain HTTP unchanged
  (about 0.2 s, 20 of 20 at 10 at once). Not yet measured: internal heap
  headroom under TLS load (needs an admin token for `/diag`), and real phones.
- **2026-09-24 (evening) — reissued certificate.** The first certificate
  started "now", which showed as Sep 25 (UTC) on devices. `scripts/certs.sh`
  now starts certificates a day early. Reissued the server certificate from
  the same CA (fingerprint unchanged, already trusted in the owner's Windows
  store) with `MASTOMINI_CERT_IPS=192.168.1.161 make reissue-cert`: valid
  from 2026-09-24 00:50 UTC to 2028-12-22. Procedure A, 2,051,872 bytes, hash
  verified. Boot: `accounts=2 statuses=2 repairs=0`, both listeners, ready at
  `192.168.1.161`; probe passed by IP including strict HTTPS and "this build's
  certificate".
- **2026-09-25 — transport rebuild** on COM11 (MAC `ac:a7:04:2c:29:9c`),
  revision `5555dbb` with uncommitted changes. `make check` passed: 151 Rust
  tests, 15 UI tests, smoke, 23 client tests, 138 conformance tests (58
  skipped, 14 xfailed). Before flashing, the board answered HTTP without
  `Server-Timing`: it ran firmware older than the 2026-09-25 performance work,
  which nothing on the board could show. This build adds
  `GET /api/mastomini/v1/version` and `make board-version`. Procedure A,
  2,204,960 bytes, hash verified. Boot: `store: provisioned=true accounts=2
  statuses=2 repairs=0`, `TLS session tickets on`, HTTPS 7 sockets and HTTP 4,
  both backlog 8, ready at `192.168.1.161`. Probe passed by IP (strict HTTPS,
  this build's certificate and CA); `make board-version` matched. Measurements
  are in spec/08 "Board transport".
- **2026-09-25 (later) — API keys and post/profile pages**, same board,
  revision `5555dbb` with uncommitted changes. `make check` passed (164 Rust,
  15 UI, smoke, 25 client, 138 conformance / 58 skipped / 14 xfailed). New
  record kind only (`LocalToken`); no existing layout changed, no wipe.
  Procedure A, hash verified. The first boot after flashing **did not join
  Wi-Fi** within its one attempt and opened `mastomini-setup`; the store was
  intact (`accounts=2 statuses=2 repairs=0`). One more `make boot-log` joined
  normally (RSSI -71, weak) and was ready at `192.168.1.161`; the full log is
  `.local/boot-log-2026-09-25.txt`. Known gap, not fixed: a board that fails
  its boot-time join stays in setup mode and never retries the saved network
  (likely after a power cut, when the router boots slower than the board).
  Probe passed; `make board-version` matched; `/@matt`, `/tags/…` and
  `/web/signin` serve pages over HTTP and HTTPS.
- **2026-09-26 — upgrade to revision `9688221`** on COM11 (MAC
  `ac:a7:04:2c:29:9c`, the only board attached). Clean tree except
  `spec/api-coverage.json`. The coverage gate first failed because
  `api/diag.rs` (an `incidents` field in `/diag`) and `api/moderation.rs`
  (admin undo and report actions answer directly instead of through the
  read endpoint) had changed without review. Both were reviewed: no
  endpoint changed classification. Both were acknowledged, which is the
  ledger change.
- A desktop mastomini (port 18081, not started here) held
  `target/debug/mastomini.exe`, so `make check` ran with
  `CARGO_TARGET_DIR=target-deploycheck` rather than stopping that server. It
  passed: 176 Rust tests, 38 UI tests, smoke, 25 client tests, 138
  conformance tests (58 skipped, 14 xfailed).
- Before: the board was offline because it had been unplugged and plugged
  back in. `make boot-log` found it healthy with
  `provisioned=true accounts=2 statuses=2 repairs=0`, running a build from
  03:12 that day. The install dry run refused (mastomini layout), so
  procedure A: 2,260,944 bytes, hash verified.
- After: boot `store: provisioned=true accounts=2 statuses=2 repairs=0`,
  ready at `192.168.1.161`. Probe passed by IP and by `mastomini.local`; the
  name took about three minutes to resolve on Windows. `make board-version`
  matched.

### 2026-09-26: UI polling audit

Reviewed the Angular UI and Rust-generated web pages while investigating
NanaCoin's global five-second `/status` watch. No recurring network polling
timer or immediate request feedback loop was found in Mastomini's own pages.
Its incident-history timeout only releases a downloaded object URL. No
Mastomini code change or reflash was needed. Third-party Mastodon clients have
their own refresh behavior and were not part of this source audit.

### 2026-09-27 — healthy-light phrase setting and Morse status deployed

Upgraded the mastomini board on COM11 (MAC `ac:a7:04:2c:29:9c`) with
procedure A. The working tree was dirty at revision `afb2e2a`; the root
`make check` passed (196 Rust, 52 UI, 27 client, and 138 conformance tests;
58 skipped and 14 xfailed, plus lint, formatting, API coverage, docs and
smoke checks). The install dry run identified the existing layout: `nvs`,
`phy_init`, 4 MiB `factory`, 8 MiB `store`, 3 MiB `media`, and `coredump`, so
first install was refused as expected. The 2,388,320-byte app image was
written only at `0x10000`; esptool verified its hash and the deploy script
reported `Application updated. The board restarts; household data was not
touched.`

Boot log: GPIO48 initialized, `provisioned=true accounts=6 statuses=10
repairs=0`, Wi-Fi and mDNS started, ready at `192.168.1.161`. The strict
IP probe passed, including HTTPS, the existing household CA and this build's
certificate. `make board-version` matched fingerprint `e1b1f2432f01`,
built `2026-09-27T15:49:42.739Z`. The strict probe also passed by
`mastomini.local`; Windows name resolution took about three minutes.
