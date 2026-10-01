# Minicloud

A runnable household cloud prototype: Rust HTTP service, a small MQTT broker,
compiled Rust workers, MIME-aware blob URLs, an Angular file browser and a
public kitchen notification screen. The first firmware target is the
**non-Touch Waveshare ESP32-C6-LCD-1.47**, with its actual 8 MiB flash and
no PSRAM or SD card. SQLite is an optional desktop stretch goal.

## Run locally

From **Git Bash**:

```bash
cd /c/github/mastomini/minicloud_rs
make run
```

Open **http://127.0.0.1:8090/** for the public screen and dismissal page.
Open **http://127.0.0.1:8090/admin** for Files, Screen and Workers.
The local default management token is `local-prototype-token`.
MQTT listens on **127.0.0.1:1883**, using that token as its password.
An MQTT username must be supplied; its value is currently informational.

In another Git Bash terminal:

```bash
python scripts/demo.py
```

This sends fictional NanaCoin and Mastomini notifications. Their IDs stay
stable, so rerunning does not duplicate messages. Once dismissed, they stay
dismissed; use a new ID for a new demo. NanaCoin now has a server-side relay
and an optional kitchen screen checkbox. Mastomini can use the same public HTTP
contract.

`make run` installs the UI's locked npm dependencies, builds Angular, compresses
the assets and starts Rust. For later backend-only changes, use
`cargo run --locked`. Runtime data goes in `data/`, which is ignored by Git.
One file lock prevents two desktop writers. Requirements: Rust, Node/npm,
Python and Git Bash `make`; `make check` additionally uses `uv` for Paho.

| Environment variable | Default / meaning |
|---|---|
| `MINICLOUD_DATA` | `data`; dedicated data directory |
| `MINICLOUD_PORT` | `8090`; HTTP port |
| `MINICLOUD_MQTT_PORT` | `1883` |
| `MINICLOUD_ADMIN_TOKEN` | local prototype token; 16+ characters locally |
| `MINICLOUD_BIND` | `127.0.0.1`; LAN binding requires your own 24+ character token |
| `MINICLOUD_PLUGINS` | build-time comma-separated plugin file stems; unset includes all eligible plugins |

HTTP and MQTT are plaintext in this first prototype. Bind to your trusted LAN
only when ready to test other devices. File uploads and worker management require the bearer token. **All blob file URLs and screen contents are public**, as are screen
posting, read updates and advance/dismiss actions. Do not put private messages or private files here.

## Files that work as URLs

```bash
export MINICLOUD_ADMIN_TOKEN='local-prototype-token'
curl -X PUT http://127.0.0.1:8090/api/blobs/photos/kitchen.jpg \
  -H "Authorization: Bearer $MINICLOUD_ADMIN_TOKEN" \
  -H 'Content-Type: image/jpeg' --data-binary @kitchen.jpg

# Public file URL: streams original bytes, with image/jpeg and an ETag.
curl -o kitchen-copy.jpg http://127.0.0.1:8090/blobs/photos/kitchen.jpg
```

There is no separate website-hosting setup. Uploading through Angular preserves
the file's MIME type. `GET /blobs/{bucket}/{key}` streams in 4 KiB chunks,
supports HEAD and conditional ETags, and sends `Content-Disposition: inline`.
HTML/SVG and other active content get a sandbox CSP and `nosniff` to protect the
management origin. No Range requests or multipart/chunked uploads yet; provide
Content-Length, as browsers and the shown curl command do automatically.

Keys may contain folders. Bucket/key segments use ASCII letters, digits,
underscores, dashes and dots, excluding `.` and `..`. The desktop disk and board
SPIFFS both store flat files; folders are a logical file-browser/API concept.

Each blob is capped at **256 KiB**, with **48 objects** and **3 MiB** of logical
blob storage. Content is hashed while streaming; filenames fit SPIFFS's existing
32-byte limit. Uploads write new immutable content before updating metadata.
Collection keeps files referenced by both state generations, allowing rollback.
Some space is reserved for the previous generation, staging and filesystem GC.

## Kitchen screen and images

The public page previews a 320 × 172 display, rotates messages every eight
seconds, and provides a dismissal button for each notice. The admin Screen tab
composes notifications with small/medium/large text and an optional image.
The physical BOOT button advances; it never dismisses.

The C6 advertises **minicloud.local**, HTTP port 80 and MQTT port 1883 via
mDNS. Desktop development stays on loopback port 8090; override the producer
URL for local tests. Every notification expires within **24 hours**. The
absolute Unix-second `expires_at` is saved with the queued job and notice;
restarts and updates cannot renew the lifetime. Later requested deadlines are
capped to acceptance time plus 24 hours, allowing small producer clock offsets. Omitting it uses acceptance
time plus 86,400 seconds. Expired queued jobs are completed without display.
SNTP must synchronize the C6 clock before notifications are accepted.

Posting does not require authentication:

```bash
curl http://minicloud.local/api/screen/notify -H 'Content-Type: application/json' \
  --data '{"event_id":"dinner-1-notify","source":"household","id":"dinner-1","recipient":"Everyone","text":"Dinner is ready","size":"large"}'
curl http://minicloud.local/api/screen/read -H 'Content-Type: application/json' \
  --data '{"event_id":"dinner-1-read","source":"household","id":"dinner-1"}'
```

Use unique event IDs for each action and a stable `(source,id)` for the notice.
Neighbor-resistant posting credentials, pairing, rate limits and TLS are on
[the roadmap](ARCHITECTURE.md#roadmap).

For the C6, leave **Also prepare image for the LCD** checked when uploading an
image. The browser letterboxes it to 320 × 172 and uploads a second file,
`name.rgb565`, with MIME `application/x-rgb565`. Select that version when sending
to the physical screen. It is exactly **110,080 big-endian RGB565 bytes**.
The board streams it into an **8,256-byte DMA strip**, using the existing
ST7789 pinout, X offset 34 and 40% backlight. It does not allocate a full
framebuffer or decode JPEG/PNG. Original images remain independently browsable
through their normal URLs. The desktop preview supports both originals and
RGB565, and displays converted raw images through a canvas.

The physical font scales its 8 × 14 ASCII glyphs by 1, 2 or 3, wraps at the
display edge and clips overflow. Non-ASCII glyphs appear as `?`; the web preview
uses browser fonts. Long text is not scrolled yet. Image notifications include
text overlaid on the image. A referenced image cannot be deleted until its
notices are dismissed. Normal image formats sent directly to C6's screen plugin
produce a failed receipt explaining that an RGB565 version is needed.

## MQTT and workers

MQTT provides publish/subscribe transport. **Workers are application code** that
consume commands; MQTT acknowledgment and successful work are different things.
See [the architecture and protocol limits](ARCHITECTURE.md).

This implementation reuses [mqttbytes](https://docs.rs/mqttbytes/0.6.0/mqttbytes/)
for MQTT framing, parsing and encoding; it supplies a bounded routing layer.
It is a **prototype MQTT 3.1.1 subset**, not a complete off-the-shelf broker.
The commonly suggested [minimq](https://docs.rs/minimq/latest/minimq/) and
[mqttrust](https://docs.rs/mqttrust/latest/mqttrust/) are embedded clients,
which are useful to producers but do not host the broker.

Implemented: four TCP clients, clean sessions, wildcard subscriptions,
QoS 0/1 ingress, QoS 0 subscriber delivery, keepalive, unsubscribe, and eight
RAM-only retained topics. Not implemented: MQTT 5, QoS 2, persistent subscriber
sessions, shared subscriptions, last wills, TLS, WebSockets or full conformance.
Unsupported CONNECT options are rejected. Slow subscribers whose 4 KiB output
budget fills are disconnected. MQTT payloads are capped at 2,048 bytes.

Topics handled by a plugin enter a **persistent 12-job queue**. PUBACK for such a
QoS 1 command is sent after its queue commit. The worker separately publishes a
result on `minicloud/jobs/result`; the most recent 32 results also appear in
Workers. It retries storage errors three times, then saves a failed receipt.
For invalid messages or a full queue, MQTT closes the connection without
acknowledgment; HTTP provides a readable error response. Generic M2M topics are
live publish/subscribe, with no durable work or offline delivery guarantee.

Each plugin command must have a globally unique **`event_id`** (safe identifier,
64 bytes maximum). Use the same event ID when retrying the same operation.
Deduplication applies while the event is queued or its receipt is retained.
Notification `(source,id)` and the most recent **64 dismissal tombstones** provide
additional protection against a delayed notification after reading. Dismissal
history is bounded; after eviction, producers must not replay old notifications.
An accepted request is not a promise of successful processing: inspect receipts.

Examples with a standard MQTT CLI:

```bash
mosquitto_sub -h 127.0.0.1 -p 1883 -u household \
  -P "$MINICLOUD_ADMIN_TOKEN" -t 'minicloud/jobs/result'

mosquitto_pub -h 127.0.0.1 -p 1883 -u nanacoin \
  -P "$MINICLOUD_ADMIN_TOKEN" -q 1 -t 'minicloud/screen/notify' \
  -m '{"event_id":"nc-mail-42-new","source":"nanacoin","id":"mail-42","recipient":"Katie","text":"You have a new message.","size":"large"}'

mosquitto_pub -h 127.0.0.1 -p 1883 -u nanacoin \
  -P "$MINICLOUD_ADMIN_TOKEN" -q 1 -t 'minicloud/screen/read' \
  -m '{"event_id":"nc-mail-42-read","source":"nanacoin","id":"mail-42"}'
```

Commands must not use MQTT retain. `minicloud/image/show` accepts the same notice
shape as `screen/notify`. `minicloud/screen/changed` emits a revision hint; clients
fetch the authoritative screen snapshot so missed hints do not leave stale state.

## Add or remove a plugin

Place a Rust file in `src/plugins/` defining `pub struct Plugin` and implementing
the `minicloud::Plugin` trait (inside this crate, `crate::Plugin`). `build.rs`
discovers and registers it automatically. Remove the file, or set
`MINICLOUD_PLUGINS=screen`, to exclude plugins at build time without editing a
registry. A top-of-file `// minicloud-feature: sqlite` gates an optional plugin
on a Cargo feature. The included screen and SQL insert files are examples.

`accepts(topic)` selects commands; the first matching plugin owns that command.
Use disjoint topics. Explicit HTTP invocation can select a plugin by name.
`run(job,state,store)` executes on one serial worker. State changes and the work
receipt commit together. Plugins are trusted compiled Rust, without a sandbox or
preemption; they must not block indefinitely, panic or make unbounded allocations.
External side effects need their own idempotency, as the SQL plugin demonstrates.
Unprocessed jobs for a removed plugin produce failed receipts.

There is no independently updatable module ABI yet. Changing any plugin requires
a complete app rebuild. The current partition table has a single factory app,
so OTA firmware replacement is also future work.

## Optional SQLite, implemented last

```bash
make run-sqlite
```

This enables `/api/databases/{name}/query` and the `sql_insert` plugin for
`minicloud/db/insert`. A query is a transaction containing 1–8 prepared
statements, optional scalar parameters, and at most 100 rows / 8 KiB output:

```json
{"statements":[
  {"sql":"CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, body TEXT)"},
  {"sql":"INSERT INTO notes(body) VALUES(?)","params":["Hello kitchen"]},
  {"sql":"SELECT id, body FROM notes LIMIT 10"}
]}
```

POST that JSON with the admin token. Up to four named files are capped at approximately
512 KiB each, SQL text at 2 KiB, and progress is limited to about 100 ms. ATTACH,
arbitrary PRAGMAs and binary query results are denied. `sql_insert` consumes
`{"event_id":"sample-1","database":"home","record":{"message":"Hello"}}`
and inserts into `worker_events`, with the job ID as a primary key so a retry
after an interrupted cloud commit does not duplicate the insert.

**SQLite is deliberately excluded from C6 firmware**; no performance or RAM fit
on that board is claimed. Desktop/Raspberry Pi is the first supported home for
this optional service. SQL queries are currently an API, not an Angular SQL
editor. Workers can invoke the SQL insert plugin from the management UI.

## C6 deployment

The messageboard was deployed to the identified non-Touch C6 on September 30,
2026. Follow [DEPLOY.md](DEPLOY.md) for exact Git Bash commands, identity guards,
private configuration, replacement versus update modes, acceptance and recovery.
The runbook is suitable for a future Luna delegation; Luna was not used today.

Open **http://minicloud.local/** for the public messageboard and
**http://minicloud.local/admin** for management. The observed DHCP address was
**192.168.1.163**; it can change. The management token and household Wi-Fi settings
are in the ignored `minicloud_rs/.env`. `make firmware` loads that file (explicit
environment variables override it), bundles Angular and builds with ESP-IDF
5.5.3 / `+esp` / `riscv32imac-esp-espidf`. It only builds; the guarded
`scripts/deploy-c6.py` performs an explicitly selected replacement or update.

The final app was **1,398,624 bytes** of the 2 MiB factory partition. Initial
live acceptance measured 240,568 bytes free heap and a 225,280-byte largest
block. Its boot minimum was 210,080 bytes while exercising public posting,
text sizes, actual expiry, read dismissal, MQTT QoS1, blob MIME/ETag streaming
and a 110,080-byte RGB565 image command. A separate restart retained the welcome
notice and its original expiry, rejoined Wi-Fi and served the mDNS name again.
There is no SD dependency and no SQLite in this firmware.

The final serial captures had no startup panic or LCD driver errors. Physical
LCD appearance, maximum-load capacity, power-cut recovery and all reconnect
cases still need observation/testing; the small live trial does not establish
those. Full measurements and image hashes are recorded in the deployment guide.

Wi-Fi retries continue indefinitely, allowing 60 seconds for association/DHCP
and five seconds between failed attempts. The display stays usable while joining.
The new firmware does not yet reproduce `screen_info`'s RGB LED POST patterns.

## Checks

```bash
make check
```

Checks Rust formatting and Clippy, core restart/storage/notification tests,
optional SQLite rollback/idempotency tests, and a real-binary HTTP/MQTT smoke
with Eclipse Paho. The smoke uses disposable data and ephemeral ports, checks
streamed file bytes/MIME/ETags, independent MQTT client compatibility, auth,
notification ordering, image references and restart recovery. UI compilation is
checked with `make web`; visual browser QA requires an available browser.


### Kitchen layout update (prepared for PC review)

The display is landscape, 320 x 172. Whole words wrap greedily without a
hyphenation dictionary. Only a token wider than an entire line gets split with
an explicit hyphen. Long messages get multiple pages at eight seconds per page;
BOOT still skips to the next message. The public web preview uses the same page
layout and the API reports page, pages and page_text. Non-ASCII glyphs still
use the built-in font's question-mark fallback.

NanaCoin sends two economy cards every five minutes. Minicloud replaces the
previous card in each slot, interleaves the cards with household messages, and
removes the original Minicloud welcome notice when stats arrive. With no
messages, the stats cards alternate. Ordinary notices retain their original
24-hour deadlines and public dismissal behavior.

Raw image geometry is now 320 x 172, big-endian RGB565 (still 110,080 bytes).
Regenerate existing portrait RGB565 files from their original images in the
file browser. The updated acceptance probe makes landscape color bars and
cleans up all its test messages; it no longer leaves a welcome message.

The landscape firmware has been compiled, but actual panel direction, kitchen
readability, DMA heap usage and BOOT behavior need hardware review after USB
reconnection. No firmware was flashed during this update.
