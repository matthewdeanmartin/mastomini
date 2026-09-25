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

If only `mastomini.local` fails, mDNS is blocked on this computer: report it,
it is not a deployment failure.

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
