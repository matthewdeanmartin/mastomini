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
though flashing succeeded. The scripts therefore pass `--after watchdog_reset`
to every esptool call, and `make boot-log` restarts the board the same way.
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

## Known boards

| MAC | Installed | Previous contents | Backup | Last known address |
|---|---|---|---|---|
| none yet | | | | |

Add a row after every first install, and add the MAC to nothing else: only
mastomini boards go in `MASTOMINI_BOARDS`. Update the address when it changes.

## Deployment comments

None yet. After each deployment, add a dated entry here as in mastomini's
runbook: what was deployed, from which revision, what the logs and probe
showed, and anything surprising.
