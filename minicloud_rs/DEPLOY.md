# Deploy the Minicloud messageboard

This is a runbook for a future Luna delegation as well as manual deployment.
Run every command in **Git Bash**, from `C:/github/mastomini/minicloud_rs`.
Deploy only the identified Minicloud C6. This task does not deploy NanaCoin,
Mastomini, another MCU, or a Raspberry Pi.

## Delegation brief

Deploy the current Rust/Angular Minicloud firmware to the non-Touch Waveshare
ESP32-C6-LCD-1.47, verify USB identity before writing, capture startup, and run
the live acceptance test. Keep credentials private. Report the board address,
flash verification, runtime memory measurements, tests, and any remaining
physical-display uncertainty. A successful compiler or flasher exit alone is
not a completed deployment. Do not claim that pixels were visibly correct
unless someone actually observed the LCD.

On September 30, 2026, the owner explicitly authorized replacing the existing
board code without preserving it. That authorization applies to this initial
deployment. For a later routine Minicloud update, use `--update`, retaining
the current notification/blob data. Use `--replace` only when the current
task authorizes erasing all board data. Never substitute an erase to fix an
unexplained startup error.

## Target and fixed constraints

Source of identity/pinout: [the board notes](../../microcontroller/BOARD_SKILL_ESP32_C6_LCD_1_47.md).
That relative link crosses out of the mastomini repository; on this machine
the file is `C:/github/microcontroller/BOARD_SKILL_ESP32_C6_LCD_1_47.md`.

| Item | Expected value |
|---|---|
| Model | Waveshare ESP32-C6-LCD-1.47, **non-Touch** |
| MCU | ESP32-C6FH8, revision v0.2 |
| Base MAC | `ac:eb:e6:1e:13:40` |
| USB | VID:PID `303A:1001`; COM17 during initial deployment |
| Actual flash | **8 MiB**, not the manufacturer's nominal 4 MiB listing |
| RAM/storage | No PSRAM; no SD card installed or required |
| Rust target | `riscv32imac-esp-espidf`, `+esp` toolchain |
| ESP-IDF | Installed 5.5.3 |
| Hostname | `minicloud.local` |
| Services | HTTP 80, MQTT 1883; Angular bundled in app flash |
| Screen | ST7789, landscape 320 × 172; 40% backlight, 15,360-byte DMA strip |

Do not use the Touch board's LCD drivers, S2/S3 binaries, or an app binary at
offset zero. Firmware is compiled with a 2 MiB factory app partition:

| Region | Offset | Size |
|---|---|---|
| Bootloader | `0x0` | Must end before `0x8000` |
| Partition table | `0x8000` | `0x1000` |
| NVS | `0x9000` | `0x6000` |
| PHY | `0xf000` | `0x1000` |
| Factory app | `0x10000` | `0x200000` |
| SPIFFS `storage` | `0x210000` | `0x5f0000` |

## 1. Check tools and select the port

```bash
cd /c/github/mastomini/minicloud_rs
ESP_PYTHON='C:/Espressif/python_env/idf5.5_py3.11_env/Scripts/python.exe'
"$ESP_PYTHON" -m serial.tools.list_ports -v
rustup toolchain list
```

Find the native USB device by VID:PID and serial/MAC; do not assume Windows
will keep assigning COM17. Exclude COM3 (Intel AMT), and close serial monitors
before flashing. If the expected board is absent, reseat the USB data cable
and enumerate again. This unit previously had a loose cable. Do not select a
different board simply because its COM port exists.

For the selected port, inspect without writing:

```bash
"$ESP_PYTHON" -m esptool --chip esp32c6 --port COM17 flash_id
```

Require C6, base MAC `ac:eb:e6:1e:13:40`, and `Detected flash size: 8MB`.
The deployment helper repeats this check before any erase/write. A different
physical replacement unit requires updating the intended identity explicitly;
never bypass a mismatch for this unit.

This machine already has Git Bash, make, stable Rust and `+esp`, Node/npm,
Python, uv, ESP-IDF 5.5.3, esptool 4.12 and pyserial. The C6 build script sets
the installed Windows IDF/compiler paths and defaults its Cargo output to
`C:/mcr` to avoid long Windows build paths. Do not switch to PowerShell,
cmd, WSL, or download a different toolchain to solve a routine path problem.

## 2. Load private build settings

`minicloud_rs/.env` is ignored by Git. It contains these three variables:

```dotenv
MINICLOUD_WIFI_SSID="household-2.4GHz-SSID"
MINICLOUD_WIFI_PASSWORD="household-Wi-Fi-password"
MINICLOUD_ADMIN_TOKEN="private-random-management-token-at-least-24-characters"
```

The values above are placeholders, not working credentials. The existing
private file was created from the saved Mastomini household Wi-Fi settings,
with a new random management token. Reuse it for this unit. Never print the
file, put its values in chat/logs, or commit it. If recreating it is necessary,
read already-authorized local settings without displaying them; use a random
token, not the desktop prototype token. The loader supports JSON double-quoted,
single-quoted and unquoted values; it does not evaluate shell substitutions.
Double-quoted values must be valid JSON strings.

Environment variables override `.env`. Do not leave old `prototype` Wi-Fi
settings or a prototype build token exported in the shell. Values are embedded
in firmware at **build time**, not read from the board at runtime. A rebuild
is needed after changing them. The admin UI uses the private management token;
MQTT requires a username and that token as password. Public screen HTTP posting,
reading, dismissal and blob GET URLs deliberately do not require a token.

Verify the ignore rule without displaying secrets:

```bash
git check-ignore .env
```

## 3. Build the actual firmware and website

```bash
mkdir -p .embuild
make firmware > .embuild/deploy-build.log 2>&1
tail -20 .embuild/deploy-build.log
```

Shell redirection happens before make can create its output folder.
`make firmware` installs
locked UI dependencies, builds Angular, gzip-bundles the UI into read-only app
flash, builds the C6 binary with `+esp`, and runs esptool's `elf2image`.
It does **not** flash or format anything.

The build script hashes every file in `components/display` into a comment in
the generated, watched `.embuild/c6.defaults`. This is necessary because
esp-idf-sys does not directly watch extra-component C sources. Without this
dependency, Cargo can rebuild Rust while silently linking an old LCD object.
The LCD object must rebuild after C-source changes; do not infer that from
the Rust application's compilation line alone.

If only Rust changed and the current bundled UI is already built, this faster
command uses the same private settings and target:

```bash
bash scripts/build-c6.sh > .embuild/deploy-build.log 2>&1
```

Require exit zero and an app image under **2,097,152 bytes**. The build writes
`.embuild/deployment-images.json` containing the exact bootloader, partition
table and app paths, offsets, sizes and SHA-256 hashes. Deployment refuses
changed/stale files that no longer match this manifest. The helper also parses
and checks the binary partition table, not just the CSV.

Do not edit a shell script while it is executing. These scripts use LF line
endings. If Git Bash reports a syntax error, use `bash -n scripts/build-c6.sh`
and fix the actual file rather than changing shells or disabling checks.

## 4. Flash exactly one of these modes

Close the serial monitor first. Commands below use the observed COM17; replace
it with the port verified in step 1.

**Initial replacement / authorized clean reset**:

```bash
python scripts/deploy-c6.py --port COM17 --replace > .embuild/deploy-flash.log 2>&1
tail -20 .embuild/deploy-flash.log
```

This verifies the selected unit, creates an empty SPIFFS image matching the
firmware's configuration, erases all 8 MiB, and writes bootloader, partition
table, app, and initialized storage. It removes old code, credentials, files,
notifications and other board data. The firmware itself never auto-formats.
An erased SPIFFS partition alone is not a mounted filesystem.

**Routine update of an already working Minicloud**:

```bash
python scripts/deploy-c6.py --port COM17 --update > .embuild/deploy-flash.log 2>&1
tail -20 .embuild/deploy-flash.log
```

This writes bootloader, the verified unchanged partition layout and app,
retaining NVS and SPIFFS. Do not use it as first-install mode on an unknown
filesystem. The helper requires an explicit mode and has no implicit erase.

Both use esptool 4.12's **underscore** commands, C6, 8MB, DIO, 80MHz and
460800 baud. Require `Hash of data verified` for every written region and
the final reset message. On this native USB unit automatic bootloader entry
and reset have worked; buttons have not been needed. Never leave another
terminal holding the COM port open while flashing.

SPIFFS generation uses IDF's `spiffsgen.py`, size `0x5f0000`, page 256, block
4096, object name 32, metadata 4, magic and magic-length enabled. These match
the generated SDK configuration. See [Espressif's SPIFFS image guidance](https://docs.espressif.com/projects/esp-idf/en/v5.2/esp32c6/api-reference/storage/spiffs.html).

## 5. Capture boot and find the actual address

```bash
"$ESP_PYTHON" scripts/monitor-c6.py --port COM17 --reset --seconds 60 \
  > .embuild/deploy-monitor.log 2>&1
```

This restarts the app once, captures boot for a bounded period, and releases
the serial port. Raw bytes are saved in `.embuild/board-serial.log`. To inspect
while capture runs, use another Git Bash terminal:

```bash
rg 'Minicloud|panic|abort|failed|LCD error|HTTP stopped|ip:' .embuild/deploy-monitor.log
```

Require startup through LCD, SPIFFS, Wi-Fi/network stack, HTTP/MQTT/SNTP/mDNS,
then `Minicloud network ready: http://<DHCP-IP>/ http://minicloud.local/`.
There must be no panic/reboot loop, startup failure, stopped HTTP server, or
LCD driver error. A single boot-ROM reset during the requested reset is normal.
Require `Minicloud LCD driver: st7789-landscape-v1 320x172` and the matching
`display` metadata in `/api/status`. These values come from the linked C
driver. `/api/screen` dimensions alone describe Rust's layout and cannot detect
a cached portrait driver.
Do not treat the bootloader's old compile timestamp as the app identity; use
the app's ELF hash and the manifest's image hashes.

Wi-Fi retries indefinitely without blocking screen/HTTP work. An active attempt
can take 60 seconds, followed by five seconds before another attempt. Previous
household joins sometimes took 5–10 attempts. Continue bounded captures or
observe the screen's IP instead of erasing/reflashing during an ordinary slow
join. Check the private SSID/password and 2.4GHz network if repeated failures
continue. Never publish the password when reporting failures.

Once connected:

```bash
curl --fail --max-time 10 http://minicloud.local/api/status
curl --fail --max-time 10 http://minicloud.local/api/screen
```

Use the logged DHCP IP temporarily if the computer cannot resolve `.local`.
That diagnoses host/network multicast behavior; it does not prove mDNS works.
Test the name separately before reporting name discovery as verified. The IP
is not reserved and may change after reconnect/restart.

Notifications initially return HTTP 503 while SNTP has not synchronized the
clock. Allow clock synchronization and retry the same event; do not disable
expiry or hardcode a clock to make the test pass.

## 6. Run the live board acceptance test

```bash
uv run --with paho-mqtt==2.1.0 python scripts/board-smoke.py \
  > .embuild/deploy-acceptance.log 2>&1
cat .embuild/deploy-acceptance.log
```

The default URL is `http://minicloud.local`. For IP-based diagnosis only, append
`--url http://<actual-IP>`. This script targets a **live board**: it creates
uniquely named deployment notifications and a 110,080-byte synthetic RGB565
strip image, then clears only those test notifications and removes that image.
It removes every notification it creates. It never erases storage or dismisses
other producers' notices, and does not add a startup welcome card.

The script resolves the selected name once, records the resulting IP and uses
that address for subsequent probes; this avoids repeated Windows mDNS lookup
delays without bypassing the initial name-discovery check. Expiry assertions
use the board's `/api/status` clock, rather than assuming that the computer and
SNTP-synchronized board have exactly identical clocks.

Require the script's `PASS`, and record its initial/final heap/free-block
measurements. It verifies bundled Angular public/admin pages, unauthenticated
posting and read dismissal, all three text sizes, persisted 24-hour deadlines,
actual short-deadline expiry, MQTT QoS1 delivery, admin-protected blob upload,
public streamed bytes/MIME/ETag, and the RGB565 screen command. The image stays
active for ten seconds, permitting a rotation without touching BOOT.

Keep a serial capture running during acceptance if practical and check for
LCD errors, panic, low-memory failures and reboots. Ask for physical observation
only if visual confirmation is needed; API success cannot establish LCD colors,
legibility, backlight or actual visible pixels. BOOT should advance, not dismiss;
do not repeatedly press the household device's button as a load test.

Verify normal rotation by polling GET `/api/screen` without issuing advance
commands. Repeated POST `/api/screen/next` requests deliberately flip the real
LCD rapidly and cannot verify automatic timing. Observe several page/message
transitions; complete intervals should be about eight seconds. Do not include
the initial partial interval or an explicit dismissal in that comparison.

For restart recovery, using an existing household notice:

1. Read `/api/screen` and save the note ID and `expires_at` without modifying it.
   If no household notice exists, create a uniquely named test notice and remove
   it after verifying restart recovery.
2. Run another bounded `monitor-c6.py --reset` capture and allow Wi-Fi to rejoin.
3. Read `/api/screen` again. Require the same ID and original deadline, not a
   newly generated note or a renewed 24-hour lifetime.
4. Recheck `minicloud.local`, `/api/status`, and the serial log.

Open **http://minicloud.local/** for the public screen/dismissal page and
**http://minicloud.local/admin** for management. Retrieve the admin token only
from the ignored local `.env`; do not paste it into a deployment report.
Normal browser image uploads can prepare a `.rgb565` LCD version. Original
JPEG/PNG files remain directly streamable with their MIME types, but the C6
renderer accepts the prepared RGB565 version rather than decoding originals.

## Troubleshooting and stopping rules

- **Chip/MAC/flash mismatch:** stop before writing. Do not change the guard to
  fit an accidentally selected NanaCoin S2/S3.
- **COM port busy/missing:** close the monitor, enumerate again, reseat the
  cable. Keep automatic reset first; do not add repetitive physical button use.
- **Stale manifest/hash/partition mismatch:** rebuild. Never bypass image
  validation or flash the app at `0x0`.
- **SPIFFS mount failure:** inspect configuration and actual image format.
  A clean initial install needs the explicit `--replace` image; preserving
  current Minicloud data requires diagnosis before any reset.
- **Early startup error:** inspect the named stage. The board adapter does not
  call `mkdir` on SPIFFS's mount root, and mounts through the C API so the
  esp-idf-svc 0.52.1 unregister-label cleanup bug cannot hide the original error.
  TCP sockets bind only after EspWifi initializes esp-netif/lwIP.
- **HTTP 401 for management:** check that firmware and private local settings
  use the same token. Screen notification/read routes should remain public.
- **Notification accepted but absent:** inspect Workers using the admin UI.
  HTTP 202/MQTT PUBACK means queued, not completed. Check failed receipts, clock,
  expiry, stable IDs and prior read tombstones; use a new notice ID for new work.
- **Image fails:** exact 320 × 172 big-endian RGB565, MIME
  `application/x-rgb565`, 110,080 bytes, existing blob reference. No JPEG/PNG
  decoding and no SD dependency in this build.
- **Heap pressure or reboot:** retain logs and reduce workload while diagnosing.
  Do not raise queue/blob limits or move stacks into nonexistent PSRAM.

Keep `.embuild/deploy-build.log`, `deploy-flash.log`, `deploy-monitor.log`,
`board-serial.log`, `deploy-acceptance.log`, and `deployment-images.json` as
ignored local evidence. Firmware/ELF images contain embedded credentials; do
not publish them as generic download artifacts. Report failures accurately
instead of claiming deployment success based only on flashing.

## Completion report

Record date, selected port/MAC, final firmware bytes/hash, hostname and observed
DHCP IP, verified regions, live test result, restart recovery result, runtime
heap/free-block measurements and physical LCD observation status. State any
remaining issue plainly. Future board splitting, Pi/SD storage, SQLite, module
OTA and neighbor-resistant pairing/authentication are roadmap work, not reasons
to expand this deployment.

## Initial deployment result — September 30, 2026

This run used the root coding agent; Luna was not delegated any work.

- COM17, expected C6FH8/MAC, 8 MiB flash verified before each write.
- Initial authorized full replacement installed empty SPIFFS; subsequent
  correction updates preserved that initialized filesystem.
- Final app: **1,398,624 bytes** of the 2,097,152-byte partition.
  SHA-256: `721104622e83ef75fa96d1f883bbce4c2510bca1ae7e190f5241e33a1e91b7c9`.
- Bootloader, partition table and app hash-verified; the initial empty storage
  image was also hash-verified. The board reset into the final app.
- `minicloud.local` resolved to **192.168.1.163**; public HTTP and management
  page reached. HTTP 80, MQTT 1883, `screen` plugin; SQLite excluded.
- Live acceptance passed all checks described above. Initial free heap:
  **240,568 bytes**, largest block **225,280**. At the test's final sample:
  free **224,968**, largest **192,512**, boot minimum **210,080**.
- The RGB565 test blob was removed; logical blob usage returned to zero.
  One large “Minicloud is ready!” notice remained.
- A separate reset/rejoin passed: same welcome ID and unchanged `expires_at`,
  mDNS/HTTP working, synchronized clock. After restart: free **231,980**,
  largest block **212,992**, minimum **229,600** bytes at the sample.
- No startup panic, stopped HTTP server, worker failure or LCD driver error
  appeared in the final boot/acceptance and restart captures. Visible LCD
  pixels/colors still require human observation.
- Desktop format/Clippy/Rust+SQLite tests and real HTTP/MQTT smoke also passed
  after the board-path and clock-status changes.

The first hardware boot found and corrected a SPIFFS `mkdir` mismatch and a
library cleanup panic; the adapter also now initializes lwIP before binding
TCP sockets and prints startup stages. The measured trial is small. It does
not establish maximum-load fit, physical power-cut durability, or every Wi-Fi
failure/reconnect case. Evidence lives in the ignored `.embuild` files listed
above, including `deployment-result.json`, `pre-restart-screen.json`,
`deploy-restart-monitor.log` and `deploy-check.log`.


### Prepared landscape update: September 30, 2026

Build only; deployment deferred by the owner until PC testing in the morning.
The renderer now swaps panel axes and uses a 320 x 172 logical screen with
word wrapping and eight-second pages. The public preview uses those same page
boundaries. Two coalesced NanaCoin economy cards rotate between notifications.
The image converter and board acceptance fixture now emit landscape RGB565;
reconvert old portrait files before testing images.

Follow the existing application-only update procedure after reconnecting USB.
Do not reuse the first-install erase procedure. Run the updated board smoke and
observe a sentence such as "Take me to the playground for 35 NC" in all sizes,
a long message's final page, the two economy cards, image orientation, and BOOT
advance. The smoke now removes every notice it creates and leaves existing
household messages alone. The initial deployment result's retained welcome note
records the previous acceptance run; it is not the new desired idle content.

The wider strip adds 7,104 DMA bytes compared with portrait. Measure free and
largest internal heap blocks while serving blobs and MQTT after the update;
compile success does not establish runtime heap or visible-pixel correctness.
Local evidence is in .embuild/kitchen-{tests,web,c6}.log.


Prepared C6 artifact: 1,405,024 / 2,097,152 bytes, SHA-256
`d2dfe5d4d4d1ca4f19efbdc44a94d00abda5d8b1cdb6a6384a2e33ca46af5d12`.
The existing 8 MiB partition layout is unchanged. Rebuild after PC acceptance
and verify the generated manifest before an app-only flash; this preparation
record does not describe the board's currently running image.

## Initial landscape deployment attempt — October 1, 2026

**Superseded by the display correction below.** The initial update flashed the
new Rust layout but linked a cached portrait LCD driver. Its C object dated
September 30 at 19:36, before the landscape source edit at 22:15. The owner
reported the still-vertical display and broken words. API-only acceptance
missed the mismatch between Rust's 39/19/13 columns and C's portrait widths.
The rapid bursts during the rotation probe were consistent with this agent's
repeated manual advance requests; the subsequent correction uses GET-only
timing observation.

Deployed the Rust update from the root agent, without delegation.

- COM17, C6FH8 revision 0.2, MAC `ac:eb:e6:1e:13:40`, and 8 MiB flash verified.
  The live partition table was read before writing and matched the generated
  table, including the unchanged NVS and SPIFFS offsets.
- Fresh `make firmware` succeeded. App: **1,405,024 / 2,097,152 bytes**;
  SHA-256 `d2dfe5d4d4d1ca4f19efbdc44a94d00abda5d8b1cdb6a6384a2e33ca46af5d12`.
- `deploy-c6.py --update` hash-verified bootloader, partition table and app.
  NVS and SPIFFS were retained; no erase or storage image was written.
- All 24 original notices survived the update with identical content and
  deadlines. Replaying the two most recent economy cards with their original
  deadlines exercised the new coalescing behavior: eight stale economy cards
  became two, and the old startup welcome was removed. All 15 household notices
  retained their content and deadlines. Subsequent NanaCoin economy refreshes
  were observed arriving normally.
- `minicloud.local` resolved to **192.168.1.163**. LCD, SPIFFS, Wi-Fi, HTTP, MQTT,
  SNTP and mDNS started successfully. `/api/screen` reports **320 × 172**.
- Live board acceptance passed public posting/dismissal, three text sizes,
  24-hour deadlines, real short expiry, MQTT QoS1, bundled Angular pages,
  authenticated blob upload and public streamed RGB565 bytes/MIME/ETag.
  All test notices and the 110,080-byte image fixture were removed.
- A separate live layout probe verified the large sentence "Take me to the
  playground for 35 NC" across both eight-second pages, with intact words and
  the final `NC` visible in the API's rendered page text. Traversing a full
  rotation verified the two economy cards alternate between household notices.
  This probe was also removed.
- Acceptance heap: initially **219,492 bytes** free / **176,128** largest block;
  finally **211,552** free / **167,936** largest; minimum **180,504** bytes.
- A separate reset/rejoin preserved all 17 remaining notices and their exact
  deadlines. Blob inventory and storage limits matched the pre-update snapshot
  (no household blobs were present; logical storage usage returned to zero).
  After restart: **219,732** free / **200,704** largest / **216,108** minimum.
- Boot, acceptance and restart serial captures showed no panic, reboot loop,
  startup failure, stopped HTTP server or LCD driver error. Physical screen
  orientation, colors and legibility still need the owner's observation.

Ignored evidence: `.embuild/deploy-{build,flash,monitor,acceptance,
acceptance-monitor,layout,restart-monitor}-2026-10-01.log`,
`pre-update-2026-10-01.json`, `post-update-2026-10-01.json`,
`pre-acceptance-2026-10-01.json`, `pre-restart-2026-10-01.json`,
`pre-update-partition-2026-10-01.bin`, `deploy-layout-2026-10-01.json`,
`deployment-result-2026-10-01.json`, and `deployment-images.json`.

## LCD build-cache correction — October 1, 2026

The owner observed that the initial landscape flash still displayed portrait
text. Investigation confirmed that `display.c.obj` predated the edited C source.
Cargo had rebuilt Rust but reused the old extra-component C library, giving
the renderer different column widths from Rust's word wrapping.

The build now includes a display-component content digest in the generated SDK
defaults watched by esp-idf-sys. A C-source-only edit, with unchanged binding
headers, was tested through a second real C6 build: it regenerated the defaults
and rebuilt the LCD object. The linked driver also exports its own dimensions
and revision to startup logs and `/api/status`; live acceptance now requires
those values as well as the Rust preview dimensions.

- Corrected app: **1,405,968 / 2,097,152 bytes**, SHA-256
  `2ebc1970ba06f6e6e186338a4d8c90e76c08882de81cf716d6e3e91dabb71916`.
- COM17 / expected C6 MAC / 8 MiB verified; `--update` hash-verified all three
  firmware regions while retaining NVS and SPIFFS.
- Reset/rejoin succeeded at `minicloud.local`, **192.168.1.163**. Serial and HTTP
  both identify **`st7789-landscape-v1`, 320 × 172**, from the compiled C driver.
- GET-only automatic-rotation observation recorded **8.062, 7.969, 8.109 and
  7.875 seconds** between transitions. No advance requests were sent during
  this correction's timing check. The previous deliberate rotation traversal
  had sent rapid manual advances to the live household screen.
- Rust format, Clippy and Rust/SQLite tests passed. The updated live board
  acceptance passed, including its compiled-driver check, HTTP/MQTT, expiry,
  text sizes and RGB565 upload/streaming/MIME/ETag. Probe notices and image were
  removed; all 15 household notices retained their content and deadlines.
  NanaCoin's two economy cards continued refreshing normally.
- Acceptance heap: **209,804** free / **172,032** largest initially;
  **203,844** free / **167,936** largest finally; minimum **175,984** bytes.
  The real landscape driver's wider DMA strip now consumes the expected
  additional memory. No panic or LCD driver error appeared in serial captures.
- The owner has been asked to check the corrected physical orientation and
  wrapping. Automated verification does not establish visible LCD pixels.

Ignored evidence uses `.embuild/display-fix-*-2026-10-01.{log,json}`, plus
`display-build-tracking-2026-10-01.log`, `display-build-tracking-pass.json`, and
`pre-display-fix-2026-10-01.json`. This correction supersedes the initial
landscape attempt above.
