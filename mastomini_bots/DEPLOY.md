# Deploy mastomini-bots to its ESP32-S3 board

This runbook is written to be followed step by step, by a person or by an
automated agent. Every step says what to run, what output means success, and
when to **stop and ask**. Do not improvise around a failed step.

It follows mastomini's runbook (`../mastomini_rs/DEPLOY.md`). The steps below
cover what is different for the bots board.

There are two procedures:

| Procedure | When | What it writes | Risk |
|---|---|---|---|
| **A. Upgrade** (`scripts/deploy.sh`) | The board already runs mastomini-bots | Application only, at `0x10000` | Low: bot settings, API keys and memory are untouched |
| **B. First install** (`scripts/install.sh`) | The board has never run mastomini-bots, and the user asked for it | Backup first, then bootloader, partition table, application; erases `nvs`, `store`, `coredump` | High: destroys what was on the board |

Step 3 tells you which one applies. You never choose B on your own.

## Two boards: never confuse them

The household may have two ESP32-S3 boards on the same computer:

| Board | Runs | Known MAC | Usual port |
|---|---|---|---|
| mastomini | The household's Mastodon server: everyone's posts and accounts | `ac:a7:04:2c:29:9c` | COM11 |
| mastomini-bots | The bots and their admin site | see "Known boards" below | another COM port |

**This runbook never writes to the mastomini board.** The scripts refuse a
board with mastomini's partition layout, or with a MAC listed in
`MASTOMINI_BOARDS` in `scripts/board.py`. If you see `REFUSED: the board on
this port … runs mastomini`, you picked the wrong port: stop, and do not look
for a way around it.

## Rules

- Work in Git Bash, in `/c/github/mastomini/mastomini_bots`.
- Never run `esptool erase_flash`, `write_flash` by hand, `erase_region` by
  hand, or flash a merged image. Only the two scripts above write to the board.
- Never run procedure B on a board whose partition table is the mastomini-bots
  layout, or the mastomini layout. `install.py` refuses both; do not work around
  a refusal.
- Never set the admin password, save bot settings, or enter API keys
  (Mastodon or OpenRouter) to prove a deployment. Never turn a bot on, and never
  press **Run now** or **Check API key**. The admin does all of that. Bots post
  publicly, and LLM bots spend money. The probe is read-only.
- When the owner explicitly requests forgotten-admin recovery, use the
  one-time procedure below. It clears only the admin verifier; never erase
  the store partition for password recovery.
- Never print or copy the contents of `.env` files, Wi-Fi passwords, API keys,
  the household CA's private key, or backup files.
- If the serial port or the target board is ambiguous, stop and ask.
- Do not use `espflash`, `idf.py monitor` or a terminal program to watch the
  board. Their reset leaves these boards stuck in download mode (see Step 5).
  Use `make boot-log`.
- A successful flash is not completion. The boot log and the probe must pass.

## Prerequisites

- The same toolchain as mastomini:
  - ESP-IDF v5.5.3 at `C:\Espressif`.
  - The `esp` Rust toolchain (`rustup toolchain list` shows `esp`).
  - `uv` and `make`.
  - Node and npm (the admin app is built into the firmware).
- The mastomini checkout next to this one (`../mastomini_rs`), for three things:
  - the household CA in `../mastomini_rs/.local/ca`, which issues this board's
    certificate;
  - the certificate script;
  - the Python test environment (`../mastomini_rs/clienttests`).
- **Wi-Fi credentials in the build.** This board has **no setup network**: it
  joins only the network compiled into it. Check without printing values:
  ```bash
  cat .env ../.env ../mastomini_rs/.env 2>/dev/null | grep -cE '^(export )?(MASTOMINI_)?WIFI_(SSID|PASSWORD)'   # 2 or more = present
  ```
  If this prints 0, stop and ask for them.
- An ESP32-S3 with 16 MB flash and 8 MB octal PSRAM (N16R8, like the mastomini
  board), connected with a USB **data** cable.

## Step 1: Record the tree and pass the desktop gates

```bash
cd /c/github/mastomini/mastomini_bots
git status --short          # record it; a dirty tree is allowed, never discard changes
git rev-parse --short HEAD  # record it
make check
```

`make check` runs formatting, clippy, the board-script syntax check, the Rust
tests, the admin app tests, and the end-to-end tests. The end-to-end tests run
a real mastomini and mastomini-bots on this computer, and check the safety
rails against both partition tables.

Success: `make check` exits 0 and ends with a pytest line such as
`5 passed, 3 skipped`. The skipped ones are the live model tests, which need
an OpenRouter key.

If it fails, stop: do not deploy code that fails its own gates.

Optional, and only if the user asked: `make e2e-llm
OPENROUTER_ENV_FILE=path/to/.env` runs the LLM bots against a real model, for
a few cents at most.

## Step 1b: The certificate

```bash
make certs
```

This creates `certs/` if it is missing: a certificate for
`mastomini-bots.local`, issued by **mastomini's household CA**, so devices
that trust mastomini trust this board as well. It never replaces an existing
certificate.

Success: `Certificate checks passed: mastomini-bots.local` and a CA
fingerprint equal to mastomini's (compare with `../mastomini_rs/DEPLOY.md`).

- `No household CA in …`: stop and ask. The script will not create a second
  CA, and neither should you.
- Once the board's address is known (Step 5), the admin may also want the
  certificate valid by IP:
  `MASTOMINI_BOTS_CERT_IPS=192.168.x.y make reissue-cert`, then procedure A
  again. Never rotate the CA for this board.

## Step 2: Identify the serial port

In PowerShell (read-only, does not open ports):

```powershell
Get-CimInstance Win32_PnPEntity |
  Where-Object { $_.Name -match 'COM\d+' -or $_.PNPDeviceID -match 'VID_303A' } |
  Select-Object Name, PNPDeviceID | Format-Table -AutoSize
```

Each board is a `USB Serial Device (COMn)` whose `PNPDeviceID` starts with
`USB\VID_303A&PID_1001`. The composite device's ID ends with the board's MAC
(e.g. `AC:A7:04:2C:29:9C`).

- One such board, and its MAC is **not** mastomini's (`AC:A7:04:2C:29:9C`):
  set `PORT=COMn` in Git Bash.
- Two boards: take the one whose MAC is not mastomini's. If neither or both
  match the known bots board, stop and ask.
- Only the mastomini board: stop. The bots board is not connected.
- None: stop; check the cable (charge-only cables are common).

Close any serial monitor that holds the port.

Other USB interfaces can appear: `USB-Enhanced-SERIAL CH343` with
`VID_1A86&PID_55D3`, or Espressif USB CDC with `VID_303A&PID_4001`.
Their Windows device IDs may contain adapter serial numbers rather than the
chip MAC. Do not infer the board identity from those numbers or assume only
`PID_1001` can be a candidate. If multiple candidates are present, identify
the new board's connection with the user before opening a port. Step 3 must
still verify its chip MAC and partition table before any write.

## Step 3: Decide the procedure (read-only)

```bash
bash scripts/install.sh "$PORT" --dry-run
```

This builds the firmware and the admin app (the first build takes 10–20
minutes; target directory `C:/mmb`, separate from mastomini's `C:/mmr`). Then
it reads the board's MAC and partition table **without writing anything**, and
prints them.

| Output contains | Meaning | Next |
|---|---|---|
| `REFUSED: the board on this port … runs mastomini` | Wrong board: this is the household server | **Stop.** Recheck Step 2 |
| `REFUSED: this board already runs mastomini-bots` | Board has bot settings | **Procedure A** (Step 4A) |
| Another partition table, and `Dry run. To install, rerun with --confirm-mac …` | Board has never run mastomini-bots | **Procedure B** only if the user explicitly asked to install on this board. Otherwise stop and ask, showing them the table |
| A build error | | Stop; report the error |
| `Could not read the board MAC` or a serial error | | Check Step 2; retry once; then stop |

Record the MAC and the printed partition table in your report.

First-install identification leaves the chip in its bootloader, including
after a dry run or refusal. This keeps the port stable when factory firmware
uses a different USB CDC identity. The installer keeps it there through the
backup and writes, then uses a watchdog reset after the final partition read.
To abandon installation after a dry run, press RESET to restart the existing
firmware. If a board was refused, do not continue installing on it.

## Step 4A: Upgrade (board already runs mastomini-bots)

```bash
bash scripts/deploy.sh "$PORT" --dry-run   # builds, prints the plan, opens no port
bash scripts/deploy.sh "$PORT"
```

Success: the last line is
`Application updated. The board restarts; bot settings and keys were not touched.`

`REFUSED: the board does not have the mastomini-bots partition layout`, or the
mastomini refusal, means you are on the wrong board or misread Step 3: stop.

Settings from older firmware keep working. New fields in a bot's stored
record start empty, and a bot's new settings start at their defaults.

## Step 4B: First install (only with the user's go-ahead)

Use the MAC printed in Step 3:

```bash
bash scripts/install.sh "$PORT" --confirm-mac aa:bb:cc:dd:ee:ff
```

It backs up the entire 16 MiB flash to `../.local/board-backups/` (about 4
minutes) and prints the backup's sha256 before it writes anything. Success: the
last line is
`Installed. The board restarts with no admin password: the first visit to /app/ sets it.`

Record the backup file name and sha256 in your report. If the script stops
before `Backup sha256`, nothing was written.

## Step 5: Boot log and address

```bash
make boot-log PORT="$PORT"
```

This restarts the board (RTC watchdog reset, no flash writes) and captures its
log for up to 90 seconds. Success: the last line is
`SUMMARY: ready, address 192.168.x.y`. Record the address.

A normal boot:

```
I (1100) mastomini_bots_esp32: mastomini-bots 0.1.0 starting
I (1900) mastomini_bots_esp32: 3 bots; admin password not set yet (first visit sets it)
I (6600) mastomini_bots_esp32: Wi-Fi up: 192.168.1.170
I (6700) mastomini_bots_esp32::server: TLS session tickets on
I (6700) mastomini_bots_esp32::server: HTTPS listening on port 443 (4 sockets, backlog 5)
I (6710) mastomini_bots_esp32::server: HTTP listening on port 80 (3 sockets, backlog 5)
I (6800) mastomini_bots_esp32: Ready at https://mastomini-bots.local/app/ (https://192.168.1.170/app/)
```

After an upgrade, the second line must still say `admin password set`, and
the bot count is the number of bots in this build.

| Log shows | Action |
|---|---|
| `SUMMARY: no Wi-Fi credentials in this build` (exit 2) | Nothing to join. Stop and ask for Wi-Fi credentials (Prerequisites); after adding them, repeat from Step 3 |
| `SUMMARY: Wi-Fi: could not join … attempts` (exit 3) | The board keeps retrying with growing pauses (up to a minute). The usual causes are weak signal, a router that is still restarting, or wrong credentials. Run `make boot-log` once more; if it repeats, stop and report the reason shown |
| A panic, or a store error before `Wi-Fi up` | Stop. Do not erase anything. Report the log |
| `SUMMARY: the chip is in download mode` (log shows `boot:0x2 (DOWNLOAD(USB/UART0))` and `waiting for download`) | Something reset the board the wrong way. Run `make boot-log PORT="$PORT"` once more: its watchdog reset clears this. If it repeats, ask the user to unplug and replug the USB cable, then run it again |
| `SUMMARY: … nothing more within the time limit` | Run `make boot-log PORT="$PORT"` once more, then stop and report |

## Step 6: Probe the running board (read-only)

```bash
make probe-board ADDRESS=192.168.x.y
```

Use the IP address for anything scripted: on Windows each request to a
`.local` name can take 10+ seconds to resolve. Also try the name once, which
needs mDNS on this computer:

```bash
make probe-board ADDRESS=mastomini-bots.local
```

Success: the output ends with `Board probe passed`. The checks:

- **It's the bots board.** The status page names `mastomini-bots` (mastomini
  has no such field).
- **Right firmware.** The version is the one in `Cargo.toml`, and the board's
  commit is printed next to this tree's.
- **Admin app.** It is served at `/app/`, and `/` redirects there.
- **Sign-in required.** The bot list answers 401 without a session.
- **HTTPS.** It verifies strictly against `certs/household-ca.crt` for
  `mastomini-bots.local`, also when probing by IP. The board presents exactly
  `certs/server.crt` and serves exactly `certs/household-ca.der` at `/ca`.

Never substitute `curl -k` or a browser warning bypass for these checks.

The `info` lines report whether the admin password is set and whether the
clock (SNTP) is set. Bots wait for the clock; a board without internet access
never runs them.

If only the `.local` name fails, mDNS is blocked on this computer: report it,
it is not a deployment failure.

## Step 7: Hand over to the admin (not done by the operator)

After a **first install** the board has no admin password, and **the first
person to open the admin app sets it**. Tell the admin straight away, with the
address from Step 5, so nobody else on the network claims the board first:

1. Open `https://mastomini-bots.local/app/`, or `http://192.168.x.y/app/` on a
   device that does not trust the household CA yet (it can download it at
   `/ca`). Choose the admin password.
2. **Device → OpenRouter**: only for the LLM bots. Add a key made at
   openrouter.ai with a credit limit.
3. On mastomini: make a member for each bot that should have its own name,
   sign in as that member, and make an API key under **My account → API keys**.
4. **Bots → Settings**: the server (`https://mastomini.local`), the key and
   the bot's own settings. Then **Check API key**, then turn the bot on.

Bots start **off** and stay off until the admin turns them on. An upgrade
(procedure A) keeps the password, the settings and the keys. Admin sessions
live in RAM, so the admin signs in again after any restart.

### Why "download mode" happens

esptool's usual USB reset (`Hard resetting via RTS pin`) leaves these boards'
ESP32-S3 latched in download mode, so the new firmware never starts even
though flashing succeeded. Upgrades use `--after watchdog_reset`, and
`make boot-log` restarts the board the same way. First installation uses
`--after no_reset` between operations so factory firmware cannot change the
USB port halfway through; only the final partition-table read uses
`--after watchdog_reset`.
If you ever run esptool by hand (you normally should not), add
`--after watchdog_reset`.

## Stop conditions

| Symptom | Action |
|---|---|
| Any `REFUSED: … runs mastomini` | Stop; wrong board. Never bypass |
| Ambiguous or missing serial port, or only the mastomini board attached | Stop; ask |
| `make check` fails | Stop; fix the code first |
| No household CA for `make certs` | Stop; ask. Never create a second CA |
| No Wi-Fi credentials in the build | Stop; ask |
| Install refused because the board runs mastomini-bots | Use Procedure A; never bypass |
| Deploy refused because the layout is not mastomini-bots | Stop; wrong board or Step 3 misread |
| Backup incomplete | Nothing was written; retry once, then stop |
| Firmware image too large | Stop; never change partition sizes to make it fit |
| Boot log shows a panic or store error | Stop; never erase to "fix" it; the settings may be recoverable |
| After an upgrade, the boot log says the admin password is not set | Stop immediately; do not open the admin app; report |
| Probe fails after a good boot log | Report as unverified; include the failing checks |
| Probe's HTTPS checks fail (certificate not trusted, not this build's) | Stop. Run `make certs` (it checks without replacing) and inspect `certs/`; never rotate the CA to make it pass |
| Chip stays in download mode after two `make boot-log` runs and a replug | Stop; report the log |

## Restoring a backup (only when the user asks)

A backup from Step 4B restores the board exactly as it was. Check the MAC in
the backup's file name against the attached board first:

```bash
/c/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe -m esptool \
  --chip esp32s3 --port "$PORT" --after watchdog_reset write_flash 0x0 ../.local/board-backups/<file>.bin
```

## Completion report

Report, without credentials, API keys or bot settings:

- revision and whether the tree was dirty; `make check` result;
- port, how it was identified, and why it is not the mastomini board; board MAC;
- procedure used (A or B) and why; partition table seen in Step 3;
- for B: backup file name and sha256;
- flash result line;
- boot log summary: bot count and admin-password line, `Ready at` address,
  any Wi-Fi retries;
- probe result by IP and by name; whether the clock was set;
- for B: that the admin was told to set the password;
- anything not done, and why.

## Forgotten admin password (owner-requested USB recovery)

Identify the bots board by MAC and port, pass `make check`, then run:

```bash
bash scripts/recover-admin.sh COM15 --dry-run
bash scripts/recover-admin.sh COM15
make boot-log PORT=COM15
make probe-board ADDRESS=192.168.1.162
```

Use the actual port/address. The wrapper generates a random, non-secret
recovery identifier and uses the normal app-only upgrade with its board and
partition protections. On first boot, before serving HTTP, the firmware
deletes only the `admin` verifier and saves an `admin_reset` marker. Bot
settings, Mastodon/OpenRouter keys and bot memory remain intact. If either
operation fails, startup fails; setup is not exposed until recovery commits.

The owner should promptly open `/app/`, refresh, and choose the new password.
The minimum is four characters, with no character-type rules. Existing
passwords still work after an ordinary upgrade. Login throttling and salted
password hashing remain enabled.

The same recovery image will not clear a replacement password on later
reboots: its identifier already matches the durable marker. Normal builds
have no recovery identifier. Do not put `MASTOMINI_BOTS_RESET_ADMIN_ONCE` in
`.env`; it is intentionally accepted only from the build process environment.
For another recovery, rerun the wrapper to generate a fresh identifier.
There is no unauthenticated network reset endpoint or built-in password.

## Optional RGB diagnostics

`MASTOMINI_BOTS_STATUS_LED_PIN=48` enables a single WS2812 GRB LED on GPIO48;
`38` selects GPIO38. Unset, `off`, or invalid settings leave it disabled.
Use only a pin appropriate for the board; no autodetection or pin scanning
is possible because a WS2812 gives no acknowledgement. A board with no LED
on the selected pin still serves the app and runs its bots. An RMT/thread
failure disables only the light. Generic builds default to off, and the
bots setting is separate from mastomini's setting.

- White for 1.5 seconds at boot, then a gap and flashes: reset class,
  1 power-on, 2 software, 3 crash, 4 watchdog,
  5 brownout, 6 other. The serial log names the reset cause too.
- Pulsing blue: no Wi-Fi address yet (including reconnecting).
- Cyan Morse phrase, then three green blinks: both admin server tasks executed queued
  probes within 15 seconds and the scheduler completed a pass within 5 seconds.
  These are real task callbacks, not a timer claiming that everything is alive.
- Amber: a bot job is in progress, the clock is unset, or an outbound/request
  failure occurred in the last 30 seconds. A long-running or stuck bot job
  stays amber; green never promises that credentials or a remote server work.
- Red: startup failed, or a required task stopped reporting progress.
  LED output can also freeze or go dark on severe crashes or power faults.

The light has its own low-priority task, no service lock, and dim output
(at most 12/255). Its hardware writes wait at most 20 ms before disabling
the driver; they never block the scheduler or admin request handlers.

## Known boards

| MAC | Installed | Previous contents | Backup | Last known address |
|---|---|---|---|---|
| `ac:a7:04:2c:38:b8` | 2026-09-26, bots 0.1.0 | factory/vfs layout (details below) | `aca7042c38b8-20260926-170416-full16MB.bin` | `192.168.1.162` (COM15) |

Add a row after every first install, and add the MAC to nothing else: only
mastomini boards go in `MASTOMINI_BOARDS`. Update the address when it changes.

## Deployment comments

After each deployment, add a dated entry here as in mastomini's runbook:
what was deployed, from which revision, what the logs and probe showed, and
anything surprising.

### 2026-09-26: optional LED and outbound connection diagnostics deployed

- Confirmed the reattached bots board on COM15, MAC `ac:a7:04:2c:38:b8`.
  Added optional WS2812 diagnostics using a separate bots-only build setting;
  see the color meanings above. `make check` passed: 43 Rust tests, 5 UI
  tests, 7 end-to-end tests; 3 live-model tests skipped. ESP32 release build
  passed. Procedure A verified the bots partition table and wrote only the
  1,567,360-byte app at `0x10000`, hash verified.
- Built with `MASTOMINI_BOTS_STATUS_LED_PIN=48` as a trial; physical output
  was subsequently confirmed by the owner seeing blue/red/amber. Saved
  `MASTOMINI_BOTS_STATUS_LED_PIN=48` in the bots crate's gitignored `.env`
  so routine builds preserve it; generic builds still default to off.
  Boot: reset power on, GPIO48 dim GRB driver initialized, 3 bots, admin
  password set, ready at `192.168.1.162`. Strict IP/HTTPS probe passed with
  unchanged certificate and CA. Build timestamp `2026-09-26T22:26:57.674Z`,
  revision `7a2d172a522c` plus uncommitted changes. Bot settings and keys were
  not written by the upgrade.
  The hostname probe also passed all checks; name-based HTTPS connection
  took 15,009 ms versus 926 ms by IP, consistent with slow Windows lookups.
- The owner's brief red-at-boot observation exposed a health sequencing
  issue: ready was set before the first queued server probes executed.
  Follow-up code holds startup white until initial server/scheduler progress
  arrives (15-second deadline, then red if still missing). It also adds a
  1.5-second white startup marker before the reset-class flashes. These
  follow-up changes still need deployment to the bots board.
- The mastomini server was unplugged during the USB swap. After the owner
  powered it again, it still did not answer HTTP at `.161`, direct or
  multicast mDNS, or the OS hostname lookup. This is distinct from the
  original outbound TLS failure, whose cause is still unconfirmed. Need
  the server's LED/boot state and a fresh user-triggered key check. No bots
  were enabled or run by the agent.

### 2026-09-26: forgotten-admin recovery deployed

- Owner forgot the bots device admin password and requested both a relaxed
  policy and a recovery path. Chosen behavior: minimum four characters,
  no character-type rules, and one-time USB recovery that reopens setup
  while preserving bot settings, keys and memory. The owner chooses the
  replacement password in the browser; no existing `.env` password is reused.
- Added `scripts/recover-admin.sh` and a durable recovery marker. Tests cover
  preserving unrelated records, keeping the new password on reboot, and
  failing startup if the marker cannot be committed. Full gates passed:
  45 Rust, 5 UI, 7 end-to-end; 3 live-model tests skipped. The API test also
  confirmed setup and login with four characters. ESP32 recovery build and
  deployment dry-run passed (1,568,448-byte app).
- This recovery build also includes the longer white startup marker and
  first-heartbeat grace fix described above. Deployed via the recovery
  wrapper to confirmed COM15 / `ac:a7:04:2c:38:b8`: 1,568,448-byte app,
  hash verified, build `2026-09-26T22:44:02.699Z`, dirty `7a2d172a522c`.
  Boot showed 3 bots and password setup reopened. The owner was directed
  to choose a replacement password; subsequent public probes reported
  `admin password set=True`. Strict IP and hostname HTTPS probes passed
  with the unchanged household certificate and CA.
- Follow-up deployments must use ordinary `scripts/deploy.sh`, not the
  recovery wrapper, to preserve the replacement password.

### 2026-09-26: outbound TLS root cause and reply-poll clarity

- Fresh key check reported `socket=0, tls=0x8015, mbedtls=9570, verify=0x0`.
  Serial identified `mbedtls_x509_crt_parse of CA cert returned -0x2562`.
  In this exact ESP-IDF 5.5.3 source, 0x8015 is CA parsing failure, and
  -0x2562 is invalid extensions plus unexpected ASN.1 tag. The household
  root has critical nameConstraints, unsupported by this mbedTLS parser.
  This was not a CA mismatch, bad password, bad clock, or mDNS failure:
  the same trace resolved mastomini to `.161` in the fresh attempt.
- The client now parses the unchanged embedded root with the extension
  callback, accepting only the exact supported household nameConstraints
  encoding. It restricts use of that root to the permitted household hosts
  and adds verification failures for any DNS/IP SAN outside those ranges.
  Existing signature, hostname, validity and chain failures remain intact.
  Unknown/changed critical constraints fail closed. Public sites continue
  to use the standard public bundle. No certificates were reissued, no
  CA was rotated, and verification was not disabled.
- Reply-mode scheduling is now labeled as checking mentions. The UI says
  no new mentions means no OpenRouter calls; first check starts from now,
  and failed unanswered mentions may be retried. Tests cover quiet polls
  and repeated old notifications making zero model calls; the client also
  filters notification IDs at/below its saved cursor and removes duplicates.
- Full checks passed: 48 Rust tests, 5 UI tests, 7 end-to-end tests;
  3 live-model tests skipped. ESP32 build passed. Normal procedure A
  (no recovery identifier) on COM15 wrote 1,569,856 bytes at `0x10000`,
  hash verified. Build `2026-09-26T22:53:32.109Z`, dirty `7a2d172a522c`.
  Boot: GPIO48 enabled, 3 bots, replacement admin password still set,
  ready at `.162` by 4.983 seconds. Strict IP/HTTPS probe passed with
  unchanged server certificate and CA. Awaiting the owner's fresh API-key
  check to verify the outbound path on hardware.
- Final constraint-tag hardening built `2026-09-26T22:58:18.661Z`, same
  1,569,856-byte app size, passed full gates and normal procedure A again.
  Strict IP/HTTPS probe passed; the preceding build's hostname probe passed
  at 14,714 ms. The owner confirmed API-key success as `@replyguy`, and the
  serial log showed household HTTPS `verify_credentials -> 200`, public
  OpenRouter HTTPS `chat/completions -> 200`, and mastomini `statuses -> 200`.
  The owner confirmed the good-morning post appeared in the feed. This
  validates outbound household/public trust and end-to-end posting.
- Correction discovered during this verification: the existing LLM bot
  **Check API key** action also made a model completion. The agent had
  incorrectly described it as not calling a model; the owner triggered
  the check and its success message and serial log exposed that behavior.
  Follow-up separates `check` (Mastodon only) from explicit `check-model`
  (**Test OpenRouter (1 model request)**). A regression asserts the former
  makes one GET and zero model calls, and the latter makes exactly one
  model call. No agent-triggered live-model test was run.
- An idle reused connection subsequently failed with `ESP_ERR_HTTP_WRITE_DATA`,
  socket errno 104. The client now drops it and retries once for GET only.
  It never automatically repeats POST/model requests after transport errors.
- The separated model-test action passed `make check` (49 Rust, 5 UI,
  7 end-to-end; 3 live-model tests skipped). The final ESP32 build includes
  the GET reconnect guard. Normal procedure A on COM15 wrote 1,571,328
  bytes, hash verified; build `2026-09-26T23:04:02.734Z`, dirty
  `7a2d172a522c`. Boot reports GPIO48, 3 bots, password set, ready at
  `.162` by 5.044 seconds. Strict IP/HTTPS probe passed with unchanged
  certificate and CA. The reconnect-after-idle path has not yet been
  exercised again on hardware; the observed error and read-only retry
  restriction motivated that change. A final test-only cleanup replaced
  the household CA dependency with its public nameConstraints DER fixture;
  the focused trust-policy tests passed again. No firmware behavior changed
  in that cleanup.

### 2026-09-26: initial outbound connection investigation

- Owner reported `connect: ESP_ERR_HTTP_CONNECT` when checking the saved
  Mastodon key against `https://mastomini.local`. This generic error alone
  does not establish a certificate problem. Both boards' CA files are
  identical; strict live probes verified each board against that CA and
  matched its installed certificate. The bots board at `192.168.1.162`
  has a synchronized clock. No certificate or CA was replaced.
- The client already selects the household CA for `.local` destinations.
  Added socket errno, ESP-TLS error, mbedTLS code and verification flags to
  connection failures, captured before the failed client is destroyed.
  The diagnostic contains no headers, credentials or request body.
- `make check` passed (40 Rust, 5 UI, 7 end-to-end; 3 live-model tests
  skipped). ESP32 release build passed, 1,542,416 bytes. This change has
  not been flashed: only the mastomini server (COM11) was attached to USB.
  Need the bots board on USB and a user-triggered key check to capture the
  underlying failure. Do not run an LLM bot merely to test connectivity.

### 2026-09-26: first install, MAC ac:a7:04:2c:38:b8

- Started from clean revision `7a2d172`. `make check` passed: 40 Rust tests,
  5 UI tests, and 5 end-to-end tests; 3 live-model tests skipped.
- `make certs` passed against the existing household CA.
- ESP32-S3 release build passed; application size 1,542,080 / 4,194,304
  bytes. No board was accessed during preparation.
- Windows showed COM7 (Espressif USB CDC, PID 4001) and COM14 (CH343), on
  separate USB paths. Neither exposed a chip MAC in its Windows device ID;
  asked the user to identify the new board before opening either port.
- The user identified the Espressif connection (COM7). The install dry run
  failed at `read_mac`; one direct read-only retry reported
  `Failed to connect to ESP32-S3: No serial data received.` No MAC or
  partition table was read, and no backup or flash write occurred. COM14
  was not opened. Requested manual bootloader entry before continuing:
  hold BOOT, press and release EN/RESET, then release BOOT, as described in
  [Espressif's boot-mode guide](https://docs.espressif.com/projects/esptool/en/latest/esp32s3/advanced-topics/boot-mode-selection.html#manual-bootloader).
  Re-enumerate Windows ports afterwards; the bootloader may use a different
  COM port. Repeat Step 3 and verify the MAC and layout before installing.
- Manual bootloader entry exposed COM15 (`VID_303A&PID_1001`), with USB
  serial/MAC `ac:a7:04:2c:38:b8`. The original installer read its MAC, but
  its watchdog reset then removed COM15 before it could read the table.
  Fixed first installation to stay in the bootloader between operations,
  including dry runs, and restart only after final verification. Added
  regression coverage for a disappearing factory USB port and backup-before-
  write ordering. `make check` passed: 40 Rust tests, 5 UI tests, 7 end-to-end
  tests, 3 live-model tests skipped. Another manual bootloader entry is
  needed after the original reset; no flash writes have occurred yet.
- The build reads Wi-Fi credentials from `../mastomini_rs/.env`. The shell
  wrapper's preliminary message checks fewer files and incorrectly mentions
  a setup network when it finds no credentials there. Use the `build.rs`
  credential-source messages and Step 5 boot result as evidence; the bots
  firmware has no setup network. Credential values must remain private.
- With the corrected installer, the dry run succeeded on COM15. esptool
  confirmed ESP32-S3 revision v0.2, embedded 8 MB PSRAM, and 16 MB flash
  (`flash_id`, using `--after no_reset` to preserve bootloader mode).
  The original table was:

  | Name | Type | Subtype | Offset | Size |
  |---|---|---|---|---|
  | nvs | 1 | 0x02 | 0x009000 | 0x006000 |
  | phy_init | 1 | 0x01 | 0x00f000 | 0x001000 |
  | factory | 0 | 0x00 | 0x010000 | 0x1f0000 |
  | vfs | 1 | 0x81 | 0x200000 | 0x600000 |

  This matched neither protected layout, and the MAC differed from the
  mastomini server's. Procedure B was authorized by the user's request to
  install mastomini-bots on this new board. The install build is revision
  `7a2d172a522c`, dirty due to the installer fix, tests, and runbook updates.
- Full 16 MiB backup:
  `../.local/board-backups/aca7042c38b8-20260926-170416-full16MB.bin`.
  SHA-256: `d73b6dd5a15e469a30629798664fb6e9e5c0ee30ef2679d75858d97a72b6fb15`.
  The installer verified all written image hashes and the final bots
  partition table, then reported:
  `Installed. The board restarts with no admin password: the first visit to /app/ sets it.`
- `make boot-log PORT=COM15` passed: 8 MB PSRAM memory test OK, 3 bots,
  `admin password not set yet (first visit sets it)`, no Wi-Fi join failures,
  `SUMMARY: ready, address 192.168.1.162`. COM15 remains the installed
  firmware's port. The admin was told immediately to set the password at
  `https://mastomini-bots.local/app/`.
- IP probe passed, including strict HTTPS verification and exact server/CA
  certificate matches. Firmware identified itself as mastomini-bots 0.1.0,
  commit `7a2d172a522c` (dirty), built `2026-09-26T21:03:55.119Z`.
  Clock was synchronized. Admin password was still unset at the probe.
  No credentials were configured, and no bots were enabled or run.
- Hostname probe also ended with `Board probe passed`. The reported HTTPS
  connect/handshake time was 14,839 ms by name versus 1,132 ms by IP, consistent
  with the slow Windows `.local` lookups noted above. All required deployment
  checks passed; live-model tests were intentionally skipped.

### 2026-09-26: UI polling audit (awaiting board deployment)

Bots and Activity pages previously started a three-second interval after
awaiting the initial load. Navigating away during that load could destroy the
page before a timer existed, then leak a new interval when the response arrived.
Neither interval guarded against overlapping slow requests or hidden tabs.

Both now check destruction before creating the timer, refuse concurrent loads,
skip hidden tabs, and poll at ten seconds. Page destruction clears the timer
and prevents any later request from starting. An already-started fetch may
finish after navigation; it cannot restart polling. This affects admin UI
reads only, not bot schedules or OpenRouter requests.

Nine UI tests and the production Angular build passed. New regressions exercise
both pages, navigation during initial fetch, slow requests, and hidden tabs.
The connected USB board was NanaCoin (COM9), so the bots change is built/tested
but NOT flashed. Use ordinary deployment when the bots board is attached; do
not use password recovery. Refresh existing browser tabs after deployment.

### Configurable healthy Morse message

The admin can set **Device → Healthy light message** (default
`Robots have feelings too!`). The phrase persists under its own `led_phrase`
store key, leaving bot settings, API keys and memory unchanged. Playback uses
200 ms dots, 600 ms dashes and standard Morse gaps in cyan, then three green
blinks. Existing startup, activity and fault indicators interrupt playback.
A change applies on the next free maintenance pass, normally within one second.
The API is session-protected `GET/PUT /api/v1/device/light`; both responses are
`no-store`. Do not change live settings just to verify deployment. Verify the
new firmware using the build timestamp, boot log and strict read-only probe.

### 2026-09-27: configurable healthy Morse message deployed

- Upgraded the bots board on COM15, MAC `ac:a7:04:2c:38:b8`, using
  procedure A. The working tree was dirty at `afb2e2a`; `make check` passed:
  58 Rust tests, 9 UI tests and 7 end-to-end tests passed, with 3 live-model
  tests skipped. Formatting, clippy and board-script checks passed as well.
  Existing household certificate and CA checks passed without rotation.
- The install dry run read the existing bots layout (`factory` 4 MiB at
  `0x10000`, `store` 1 MiB, `coredump` 64 KiB, plus `nvs` and `phy_init`)
  and refused first install as expected. Procedure A wrote the 1,593,360-byte
  application only at `0x10000`; esptool verified the hash. The script ended
  `Application updated. The board restarts; bot settings and keys were not
  touched.`
- Boot log: GPIO48 initialized, 3 bots, admin password still set, Wi-Fi and
  HTTPS/HTTP ready at `192.168.1.162`. Strict IP probe passed, including
  sign-in enforcement and HTTPS/certificate/CA checks. It reports commit
  `afb2e2a49e3b` (dirty), built `2026-09-27T16:12:38.651Z`; this timestamp
  was independently matched against the generated firmware image. The
  `mastomini-bots.local` probe also passed; its HTTPS handshake took 14,875 ms
  on this Windows host. No admin settings, credentials or bot jobs were changed
  or exercised.
