# Installation

There are two ways to run mastomini:

- **On a PC** (desktop build): the same server, for trying it out and for
  development. Five minutes.
- **On an ESP32-S3 board**: the real thing, a small box that sits on the home
  network. About an hour the first time, most of it installing the toolchain
  and the first build.

Commands are for **Git Bash** on Windows, which is what the project's scripts
and `Makefile` are written for. Run them from `mastomini_rs/` unless a step
says otherwise.

## Try it on a PC

You need [Rust](https://rustup.rs/) (1.88 or newer), `make`, and for the test
suites [uv](https://docs.astral.sh/uv/).

```bash
cd mastomini_rs
make run
```

The server listens on `http://127.0.0.1:8080` and keeps its data in
`mastomini.store` in the current directory. Open that address in a browser to
create the household (see [First-time setup](#first-time-setup)).

Settings are environment variables:

| Variable | Default | Meaning |
|---|---|---|
| `MASTOMINI_BIND` | `127.0.0.1` | Address to listen on. Use `0.0.0.0` to reach it from a phone |
| `MASTOMINI_PORT` | `8080` | Port |
| `MASTOMINI_STORE` | `mastomini.store` | Data file |
| `MASTOMINI_BASE_URL` | `http://<bind>:<port>` | The address people use, as it appears in links and `@user@host` mentions |
| `MASTOMINI_PASSWORD_ROUNDS` | the board's value | PBKDF2 rounds for new passwords |

The desktop build handles one request at a time. That is fine for a household
trying it out, and for development.

## Install on an ESP32-S3 board

### What you need

- An **ESP32-S3 N16R8** board (16 MiB flash, 8 MiB PSRAM).
- A USB **data** cable (charge-only cables are common and won't work).
- A Windows PC with Git Bash, `make` and [uv](https://docs.astral.sh/uv/).
- **ESP-IDF v5.5.3** installed at `C:\Espressif` (the official Windows
  installer's default).
- The **`esp` Rust toolchain** for the ESP32-S3's Xtensa CPU, installed with
  [espup](https://github.com/esp-rs/espup). `rustup toolchain list` should show
  `esp`.
- A 2.4 GHz Wi-Fi network for the board to join.

### Build the firmware

```bash
cd mastomini_rs
make firmware
```

The first build downloads and compiles ESP-IDF components and takes 10–20
minutes; later builds are much faster. The build never touches a board. It ends
with a line like:

```
Application: C:\mmr\xtensa-esp32s3-espidf\release\mastomini-esp32.bin (1608832 / 4194304 bytes)
```

### Find the board's serial port

Plug the board in. In PowerShell:

```powershell
Get-CimInstance Win32_PnPEntity |
  Where-Object { $_.Name -match 'COM\d+' -or $_.PNPDeviceID -match 'VID_303A' } |
  Select-Object Name, PNPDeviceID | Format-Table -AutoSize
```

The board is the `USB Serial Device (COMn)` whose ID starts with
`USB\VID_303A&PID_1001`. The number at the end of the composite ID is the
board's MAC address. Close any serial monitor that has the port open.

### First install

A first install **erases the board**. The script protects you in three ways: it
refuses a board that already runs mastomini, it only writes to the board whose
MAC address you confirm, and it backs up the entire flash first.

1. Look before writing. This builds, then reads the board's MAC and partition
   table without changing anything:

    ```bash
    bash scripts/install.sh COM11 --dry-run
    ```

    If it says `REFUSED: this board already runs mastomini`, you want
    [Upgrading](#upgrading) instead.

2. Install, confirming the MAC the dry run printed:

    ```bash
    bash scripts/install.sh COM11 --confirm-mac aa:bb:cc:dd:ee:ff
    ```

    The full-flash backup goes to `.local/board-backups/` (about 4 minutes) and
    its sha256 is printed before anything is written. The last line is
    `Installed. The board restarts unprovisioned.`

3. Watch it start:

    ```bash
    make boot-log PORT=COM11
    ```

    This restarts the board and reads its log. It ends with either
    `SUMMARY: ready, address 192.168.x.y` (the board joined Wi-Fi) or
    `SUMMARY: setup mode, network mastomini-setup` (it needs to be told which
    Wi-Fi to join; see below).

Developers can build Wi-Fi credentials into the firmware by putting
`MASTOMINI_WIFI_SSID` and `MASTOMINI_WIFI_PASSWORD` in the gitignored
`mastomini_rs/.env`. A board that joins with them saves them, and saved
networks always take priority.

!!! note "Serial monitors"
    Don't watch the board with `idf.py monitor`, `espflash` or a terminal
    program: their reset leaves this board stuck in download mode. Use
    `make boot-log`, which restarts it the safe way.

### First-time setup

This part is done by the household, from a phone or any browser.

1. **Wi-Fi** (only if the board is in setup mode). Join the open Wi-Fi network
   `mastomini-setup` with a phone. The setup page opens by itself; if not, open
   `http://192.168.4.1/`. Pick the home network, type its password and tap
   **Join network**. The page then shows the board's address on the home
   network, for example `http://192.168.1.161/`.
2. **Create the household.** Open the board's address from any device on the
   home network (or tap **Create your household**). Enter a household name, a
   username and a password. This first account is the **owner**, who is also
   an admin.
3. **Add the family.** Tap **Invite your family** (or open the board's
   address and choose **Household app**), sign in, and create an invite link
   for each person. They open it and choose their own username and password.

The `mastomini-setup` network closes about two minutes after the household is
created.

The board is also reachable as `http://mastomini.local/` on devices that
support mDNS. The IP address works everywhere, and on Windows it is much
faster than the name.

### Check it

A read-only probe of the running board:

```bash
make probe-board ADDRESS=192.168.1.161
```

It ends with `Board probe passed`. It never creates accounts or posts.

## Upgrading

Upgrades write the application only. Household data (accounts, posts, keys)
and the saved Wi-Fi network are not touched.

```bash
bash scripts/deploy.sh COM11 --dry-run   # builds and shows the plan; opens no port
bash scripts/deploy.sh COM11
make boot-log PORT=COM11
```

The deploy ends with `Application updated. The board restarts; household data
was not touched.` In the boot log, check that the `store:` line shows the same
number of accounts and posts as before.

!!! warning "If the boot log shows a store error"
    Stop. Don't erase or reinstall to "fix" it: the data may be recoverable.

## If something goes wrong

| Symptom | What to do |
|---|---|
| No `VID_303A` serial port | Try another cable (it must carry data) or USB port |
| `SUMMARY: the chip is in download mode` | Run `make boot-log PORT=…` once more; it clears this. If it repeats, unplug and replug the board |
| Board was on Wi-Fi, now opens `mastomini-setup` after an upgrade | It can no longer join its network (router changed, out of range). Join `mastomini-setup` and pick the network again |
| `mastomini.local` doesn't resolve | mDNS is blocked on that device. Use the IP address |
| Want the board back exactly as it was before the first install | Restore the backup; see "Restoring a backup" in the runbook |

The step-by-step runbook with every check and stop condition, written so that
an automated agent can follow it too, is
[`mastomini_rs/DEPLOY.md`](https://github.com/matthewdeanmartin/mastomini/blob/main/mastomini_rs/DEPLOY.md).
