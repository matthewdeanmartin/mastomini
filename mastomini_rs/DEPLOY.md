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
  hand, or flash a merged image. Only the two scripts above write to the board.
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

```bash
cd /c/github/mastomini/mastomini_rs
git status --short          # record it; a dirty tree is allowed, never discard changes
git rev-parse --short HEAD  # record it
make check
```

Success: `make check` exits 0 and ends with the pytest line `N passed`.
If it fails, stop: do not deploy code that fails its own gates.

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
- None: stop; check the cable (charge-only cables are common).
- More than one: stop and ask which board is intended.

Close any serial monitor that holds the port.

## Step 3: Decide the procedure (read-only)

```bash
bash scripts/install.sh "$PORT" --dry-run
```

This builds the firmware (first build: 10–20 minutes), then reads the board's
MAC and partition table **without writing anything**, and prints them.

| Output contains | Meaning | Next |
|---|---|---|
| `REFUSED: this board already runs mastomini` | Board has household data | **Procedure A** (Step 4A) |
| A partition table that is not mastomini, and `Dry run. To install, rerun with --confirm-mac …` | Board has never run mastomini | **Procedure B** only if the user explicitly asked to install mastomini on this board. Otherwise stop and ask, showing them the partition table |
| A build error | | Stop; report the error |
| `Could not read the board MAC` or a serial error | | Check Step 2; retry once; then stop |

Record the MAC and the printed partition table in your report.

## Step 4A: Upgrade (board already runs mastomini)

```bash
bash scripts/deploy.sh "$PORT" --dry-run   # builds, prints the plan, opens no port
bash scripts/deploy.sh "$PORT"
```

Success: the last line is
`Application updated. The board restarts; household data was not touched.`

`REFUSED: the board does not have the mastomini partition layout` means you
are on the wrong board or Step 3 was misread: stop.

## Step 4B: First install (only with the user's go-ahead)

Use the MAC printed in Step 3:

```bash
bash scripts/install.sh "$PORT" --confirm-mac aa:bb:cc:dd:ee:ff
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
| Ambiguous or missing serial port | Stop; ask |
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

Add a row after every first install. Update the address when it changes.

## Deployment comments

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
