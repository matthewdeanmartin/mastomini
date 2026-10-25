# mastomini-bots (proof of concept)

Many Mastodon bots on their own ESP32-S3, with an admin site that shows what
they are doing. Tuned for [mastomini](../mastomini_rs) at `mastomini.local`,
but a bot can post to any Mastodon server.

- **Bots are Rust**, compiled into the firmware (`src/bots/`).
- **A scheduler** runs them at local times across daylight saving, retries
  failures without double-posting, and never runs a stale slot.
- **One admin** signs in to the admin app (`mastomini_bots_ui`, Angular) at
  `https://mastomini-bots.local/app/`.
- **HTTPS from mastomini's household CA**: devices that trust mastomini
  already trust this board.

Bots in this build:

| Bot | Does |
|---|---|
| `good_morning` | The time bot: "Good morning, it is Saturday, September 26, 2026" every day at 07:30 US Eastern. The time, zone, message template (`{{date}}`, `{{weekday}}`, `{{time}}`, `{{zone}}`) and visibility are all settings. |
| `llm_reply` | Reply Guy: a cloud-dwelling character with opinions, goals and conversation memory. Checks mentions every 5 minutes and posts a varied daily thought at noon. |
| `llm_post` | Writes a new post with a language model. By default once a day at 12:00. |
| `fishtank` | A daily emoji aquarium at 10:00. Reconstructs moving fish from seed `42` and start date `2026-10-08`; no model, saved board or old-post reads. |
| `mars_rover` | Mars Expedition: a seeded rover exploring an 8×8 Mars map. Polls commands every 5 minutes; daily mission log at noon. Its game state lives in checkpoints in the mission thread. No model calls. |
| `nana_news` | NanaCoin news: twice a day (08:00, 20:00) posts what happened in the economy — lottos, listings, loan requests… One checkbox per category; crowded categories roll up into one line. Needs a NanaCoin read key. |
| `feeds` | Feeds from other servers: each night (03:00 by default) fetches the RSS feeds of accounts household members follow on other servers, makes small JPEG previews of their pictures, and hands both to mastomini with a device key from its admin page. Never posts. Design: [`spec/remote_follow_rss.md`](../spec/remote_follow_rss.md). |
| `sre` | SRE digest: at 07:30 reads every board's `/.well-known/ops/health` (miniframework `ops` feature) and posts a followers-only headline with what is wrong and what changed since yesterday, plus one reply per unhealthy board. Read-only; a model may optionally write the headline. Design: [`spec/OPS.md`](../../microcontroller/miniframework/spec/OPS.md). |
| `trader_1` … `trader_6` | NanaCoin traders: each a bot member with its own money, key and strategy (forex band, lotto, borrower, bot bank, steady buyer, rebalancer, mean reversion). Dry run until turned off; posts its trades when it has a Mastodon key. |

`llm_reply` and `llm_post` are one bot type (`src/bots/llm.rs`) listed twice
with different defaults. Either can be switched to the other mode.
Reply mode can also post daily; switch off **Also post daily while answering
mentions** for replies alone. The first poll at or after **Daily post time**
posts for that local date, even if nobody has mentioned it. Missed dates are
not backfilled. A stored date and a date-based posting key prevent duplicates
across polls, retries and restarts. Post mode uses the same daily key.

The static [character card](src/bots/cloud_character.txt) gives Reply Guy a
disembodied cloud existence, fond but opinionated voice, goals, and a desire
to be a welcome regular. **Character card** is editable and is combined with
existing **System instructions**, so saved custom prompts keep working.
Daily briefs combine 12 topics, 8 approaches, 8 moods, 6 goals and 6 forms:
27,648 distinct combinations before the cycle repeats. The slot's local date
chooses the brief deterministically, including on retries. The brief is
appended even to an existing saved post prompt.

**Remember conversations** makes a separate LLM summarizer call after each
successful reply. **Memory summarizer model** can select a different model;
empty uses the device default. It stores plain-text notes (at most 1,000
characters per summary, eight summaries per bot) in the bot's persistent
state. Notes older than 14 idle days are ignored, and the summarizer is asked
to discard old events. Public conversation notes inform future posts and
replies; restricted notes are scoped to the thread, author and visibility
and never enter daily posts. No mentions means no summarizer call. A failed
summary is logged without retrying the already posted reply or losing old
notes. Each LLM entry keeps its own memory; use reply mode with daily posts
on the same entry for both to share a character's conversation history.

### Fishtank continuity

Configure **Tank seed**, **Start date (YYYY-MM-DD)**, zone and posting time,
then connect a Mastodon account and enable `fishtank`. Keep seed, start date
and zone fixed for continuity. Four distinct fish move one grid square per
iteration, bounce at walls and avoid occupied squares. Every run recreates
the initial tank and advances `local_slot_date - start_date + 1` iterations.
Thus 36 days after the start it posts iteration 37, including after skipped
days or a restart. Before the start date it posts nothing. A late retry uses
the original slot date, and its seed/start/iteration key prevents duplicate
boards. The compact aquarium fits mastomini's 140-character limit. Replay
is bounded at 100,000 days to protect the board from a broken clock.

### Mars Expedition

Connect a dedicated account to **Mars Expedition** and enable it. The first
run posts a landing message and creates a mission thread. Reply in that
thread, mentioning the account (for example `@mars scan`). Commands take
effect at the next poll, with three processed per poll by default. Each
result mentions its player and continues the mission's checkpoint chain;
the rover does not create separate game threads for each command.

| Command | Result |
|---|---|
| `help` | A short introduction, current rover state, first commands and links to detailed help |
| `help moves`, `help energy`, `help samples` | Coordinates, terrain and movement; daily exploration and charging; collecting and naming discoveries |
| `status` | Sol, location, battery, sample count and weather |
| `map` | Nearby directions, coordinates and terrain in words, including map edges and blocked cliffs; unknown squares stay hidden until scanned or visited |
| `move north/east/south/west` | Move one square; `n/e/s/w` also work. Costs 10 battery, or 15 entering a crater. Cliffs and map edges block movement. |
| `scan` | Reveal the surrounding 3×3 area and report the site's terrain; costs 8 battery |
| `sample` or `sample rock` | Collect the current site's material; costs 15 battery. A site can only be sampled once. |
| `charge` | Solar charge: +25 battery in clear weather, +5 during a dust storm. At most once per sol; battery caps at 100. |
| `name discovery Kevin` | Name the sampled current site; 1–8 ASCII letters, digits, `_` or `-`. The checkpoint retains the latest named site. |

The rover starts at D4 over basalt. Terrain elsewhere is deterministically
generated from **Mars terrain seed**: red dust, basalt, hematite, water ice,
craters and cliffs. These are fictional game discoveries. Each daily tick
charges the battery (+20 clear / +5 storm) and attempts one deterministic
autonomous move. For scheduling, a “sol” is one local calendar day, with its
daily tick on the first poll at or after **Daily mission log time**. Missed
days replay those ticks in order, up to 100,000 days. Commands before the
daily tick act on the preceding sol's checkpoint.

Everyone controls one shared rover; the aim is to explore and collect samples,
with no fixed ending. Posts spell out battery and sample counts. Daily logs
explain automatic movement and report how many missed days were replayed.

Every committed update ends with a separate `MR2:` checkpoint in the thread,
collapsed behind the content warning **Game save (ignore)**. Players do not
need to read or copy this data. It contains:
seed, epoch, last tick day, position, battery, scanned/sample bitmaps,
processed notification cursor, mission root, last manual charge and latest
named site. Numeric fields use base36 to fit Mastomini's 140-character limit.
Long responses use numbered continuations, preserving lines and word boundaries,
followed by the complete checkpoint; state is never truncated. The first
landing narrative anchors the mission, so players can reply to readable posts
as well as saves. This version starts a fresh mission and ignores old `MR1`
saves; its posting keys are also versioned. The bot reads its own account history,
including replies, newest first to recover. It does not store game state or
the command cursor on the board, and never treats another account's copied
checkpoint as authoritative.

The command cursor is committed only when the result checkpoint posts.
Each command and daily tick has a stable posting key, so retries do not
spend battery twice. Quiet polls make no posts. Old pre-landing mentions,
bot accounts, direct messages, mentions outside this mission and commands
with a different visibility are ignored; their cursor may be recorded in a
collapsed save post. Keep visibility and zone fixed during a
mission. Changing seed or start date selects a different mission thread.

Keep the mission checkpoints in Mastodon: they are the save game. A malformed
bot-authored checkpoint, failed history read, or exhausted 400-post search
stops the run instead of silently resetting. Commands are discovered through
mention notifications, so include the rover's `@account` in each reply.

The NanaCoin bots (`src/bots/nana_news.rs`, `src/bots/trader/`) use NanaCoin's
activity feed, bot members and read keys. See [the plan](../spec/nanacoin-bots.md).

## Set up the NanaCoin bots

In the admin app, open **Bots → Set up NanaCoin bots**. Enter both server
addresses and sign in once as Nana and once as the Mastomini owner/admin.
Review the seven usernames, trading strategies and optional initial grants,
then choose **Create and connect**. Setup creates six NanaCoin bot members
and seven Mastomini bot accounts, obtains their keys and saves the connections.
The news bot uses Nana's read-only key and needs no NanaCoin member.

New bots stay off, traders start in dry-run, and initial grants default to
off. Check the connections and review each bot before enabling it. If Nana
already has a read key, replacement needs the explicit checkbox because it
revokes that existing key. You can instead leave the news bot unselected.

Update NanaCoin and Mastomini first: NanaCoin must report `member_capacity`
in `/api/v1/status`; Mastomini needs the admin bot enrollment endpoints.
Remote credentials require HTTPS (localhost HTTP is allowed for development).
Both devices must trust the household CA.

The desktop alternative uses the same worker:

```sh
make setup-nanacoin ARGS="--url https://mastomini-bots.local --ca ../mastomini_rs/certs/household-ca.crt --admin owner"
```

Passwords are entered through hidden prompts, never command-line arguments.
`--plan path.json` selects nonsecret `nanacoin`, `mastomini` and `bots` values;
each bot has `id`, `username`, `display_name`, `strategy` and `initial_grant`.
Without a plan, the command restores the previous selections or suggests all
seven bots. `--replace-news-read-key` explicitly allows replacing Nana's key.

If interrupted, run setup again with the original servers and usernames.
Generated credentials and progress are saved before and after remote writes,
so a retry does not duplicate members or initial grants. Existing connected
bots retain their keys and settings. Administrator passwords stay in RAM;
progress responses and the saved plan contain no passwords or API keys.
Protect the bot store, which contains the generated bot credentials.

`make coverage` writes the Rust library unit-test report to `.local/coverage`.
It covers the portable scheduler, API, enrollment, storage and desktop client;
ESP-IDF firmware code requires separate board validation.

## Try it on the desktop

```sh
make run-web          # builds the admin app, serves http://127.0.0.1:8090/app/
```

1. Open the app. The first visit sets the admin password (at least 4 characters; no character-type rules).
   If it is forgotten, use the [USB password recovery procedure](DEPLOY.md#forgotten-admin-password-owner-requested-usb-recovery)
   to reopen setup once while preserving bot settings and API keys.
2. On mastomini, make an API key for the bot's account (household app → **My
   account → API keys**). A bot with its own name needs its own member.
3. In **Bots → Settings**, enter the server (`https://mastomini.local`, or
   `http://127.0.0.1:8080` for `make run` in `../mastomini_rs`) and the key, and turn it on.
4. **Check Mastodon key** verifies the server key without calling a model.
   **Check mentions now** polls for new mentions; no new eligible mentions
   means no model calls. Posting bots use **Run now** to post. **Activity**
   shows each step.

`MASTOBOTS_RESOLVE=mastomini.local=192.168.1.161` skips name lookups
(Windows resolves `.local` names slowly). Otherwise each name is looked up
once and kept for ten minutes. Settings are kept in `mastomini-bots.json`,
which holds API keys, so it is gitignored.

## Language models (OpenRouter)

**Device → OpenRouter** holds one API key and a default model for the whole
device. The default model is `google/gemma-4-31b-it`: cheap, and what
mawkingbird uses. Make the key at openrouter.ai with a credit limit.

The key goes in and never comes back out. A bot can use another model in its
own **Model** setting. **Test OpenRouter (1 model request)** explicitly makes
one tiny completion to show OpenRouter answers. The ordinary Mastodon key
check never calls OpenRouter.

The LLM bot's settings:

| Setting | |
|---|---|
| Runs, time, zone, every | When it runs: the same four settings every scheduled bot has |
| Does | Answers mentions, or writes new posts |
| System instructions | Who the bot is and how it writes; sent as the system message |
| Character card | Static personality, opinions and goals, combined with system instructions |
| Also post daily / Daily post time | In reply mode, post once per local day at or after this time |
| Remember conversations / Memory summarizer model | Persist recent conversation summaries using a separate LLM call |
| Prompt for a new post / for a reply | The request |
| Bio for prompts | Fills `{{bio}}`; empty uses the bot account's profile bio |
| Model, Longest answer | Which model; a token cap as a cost guard |
| Replies per run, Answer other bots | Loop and cost guards |
| Who sees its posts | Visibility of new posts; replies are never more public than what they answer |

Every text setting may use these variables, all plain text:

| Variable | Holds |
|---|---|
| `{{bot_name}}`, `{{bot_acct}}` | The bot's display name and `@name` |
| `{{bio}}` | The bio setting, else the profile bio |
| `{{date}}`, `{{weekday}}`, `{{time}}`, `{{zone}}` | Now, in the bot's time zone |
| `{{max_characters}}` | The server's post limit (mastomini: 140; Mastodon: 500) |
| `{{recent_posts}}` | The bot's latest posts, one per line, so it doesn't repeat itself |
| `{{daily_prompt}}` | A deterministic combinatorial creative brief for the slot's local date |
| `{{memory}}` | Recent summary notes appropriate to this post or thread, or no recent conversations |
| `{{conversation}}` | The thread being answered, oldest first, one `@name: text` line per post |
| `{{reply_to}}`, `{{reply_to_author}}` | The post being answered, and its author's `@name` |

A setting that uses an unknown `{{variable}}` is refused when saved, with the
list of the ones it may use.

Guard rails on what the model writes:
- Its reply is cleaned: no `<think>` blocks, "Sure, here's…" openers, code
  fences or quotes around the whole reply.
- It is cut to the server's character limit.
- It is posted as plain text.
- It can't notify anyone the conversation didn't already include. An invented
  `@name` is defused with a zero-width space.

The reply bot's other rules:
- It never answers itself, or other bots unless allowed.
- It leaves direct messages alone.
- On its first run it starts from "now" instead of the backlog.
- It answers at most a few mentions per run.
- A reply is sent once even if the run is retried: the Idempotency-Key is
  the answered post's id.

## Add a bot

1. Copy `src/bots/good_morning.rs` (or `llm.rs` for one that uses a model)
   to `src/bots/<name>.rs`. Fill in:
   - **`info()`**: a short id (`a-z0-9_`, at most 13; never rename one in use),
     name, description, how late a run may still happen
     (`grace_minutes`), `uses_llm`, and `needs_mastodon` (`false` for a bot
     that can run without a Mastodon key and posts only when it has one).
   - **`settings()`**: what the admin can set, as a list of `Setting`s. The
     admin app draws the form from it; a bot writes no UI code. Kinds:
     `Text`, `LongText` (with the `{{placeholders}}` it may use), `Number`,
     `Choice`, `Time`, `Times` (up to four, `08:00, 20:00`), `Zone`,
     `Toggle`, `Secret`. Start from `schedule_settings(...)` for the four
     settings every scheduled bot shares (its time of day takes up to four
     times). `.group("Heading")` draws consecutive settings under one
     heading; `.shown_when("strategy", &["band"])` shows a setting only for
     some values of another. Both are display only.
   - **`ready()`** (optional): refuse turning the bot on until its own
     settings are complete (a NanaCoin key). **`check()`** (optional): what
     **Check connections** verifies; the default checks the Mastodon key.
   - **`schedule()`**: `schedule_from(settings)` for those.
   - **`run()`**: what it does.
     - Read settings from `run.settings` (`get`, `number`, `yes`, `time`,
       `tz`).
     - Post with `run.post(...)`, which is safe to retry. Use `run.post_keyed(...)`
       for several posts in a run.
     - Use `run.mastodon()` for anything else in the API, `run.llm()` for
       the device's OpenRouter, and `run.nanacoin()` for NanaCoin (with
       `nanacoin_settings()` among the bot's settings). Money requests get
       a replay-safe Idempotency-Key per slot.
     - Keep memory between runs in `run.state`: it is saved even when the run
       fails.
     - Use `run.slot_ms`, not the clock, for what a post says: a retry of the
       07:30 run is still the 07:30 run.
     - Write to the activity log with `run.log(...)`.
     - Return `RunError::permanent` for failures that retrying cannot fix.
2. Add `mod <name>;` and `Box::new(<name>::<Type>)` in `src/bots/mod.rs`.
3. `make test` (includes a check that every bot's id and schedule are valid),
   then build the firmware.

A new bot starts **off** until the admin gives it a server and key. A type
can be listed more than once with different ids: each is its own bot with
its own settings, schedule and memory.

### Keep it light

Pick the lightest format that works, in this order:
1. **Plain text.**
2. **One item per line**, like `@alice: text` for a conversation.
3. **Flat string maps** with `key → value`, which is what settings and
   `run.state` are.

Give a model plain lines, not JSON: it's fewer tokens, and people can read
the prompt. Ask models for plain text, not structured output, and clean what
comes back (`text::clean_completion`). JSON is only what the Mastodon and
OpenRouter APIs speak. On the board every byte of settings is flash and every
token is money.

## How it behaves

| Situation | What happens |
|---|---|
| Slot time arrives | The bot runs once for that slot; the slot is stored, so a restart does not repeat it |
| Server unreachable, 5xx, 429 | Tried again after 1, 5, 15, then 30 minutes with the same Idempotency-Key, while within the grace period |
| Key rejected (401/403), post rejected (422) | Not retried; shown as failed |
| Board off at the slot time, back within the grace period | Runs late |
| Back after the grace period | Recorded as **missed**; waits for the next slot |
| Turned on after today's slot | Waits for tomorrow's; use **Run now** to post now |
| Clock not set yet (no internet time) | Nothing runs; the activity log says so |

API keys go into the admin app and never come back out (`key_set` only).
Admin sessions live in RAM, so a restart signs the admin out. Five wrong
passwords lock sign-in for a minute. The password is PBKDF2-SHA256.

## HTTPS and speed

```sh
make certs            # certs/: a certificate for mastomini-bots.local from
                      # ../mastomini_rs/.local/ca (the SAME household CA)
MASTOMINI_BOTS_CERT_IPS=192.168.1.170 make reissue-cert   # also valid by IP
```

This refuses to create a second household CA. Desktop and firmware now use
[miniframework](../../microcontroller/miniframework)'s connection loop. On the
S3, TLS handshakes run on core 0 and serving runs on core 1, with four HTTPS
and three HTTP connections and a 512 KiB budget for pending responses.
`TCP_NODELAY`, TLS session tickets and disabled Wi-Fi modem sleep are retained.
`CONFIG_FREERTOS_HZ=1000` is required for the framework's 1 ms waits.

The app still owns its admin API, authentication, `/app/`, `/ca`, `/ca.pem`
and cache policies. The framework supplies public machine diagnostics at
`/api/v1/sys`, `/api/v1/log` and `/metrics`. Both builds optionally support
HTTP/2 with `--features http2`; normal firmware builds use HTTP/1.1.

For the bots' own requests, the board resolves `mastomini.local` once over
mDNS, connects by address while verifying the name, keeps connections open,
and resumes TLS sessions. Household names are verified against the household
CA, public servers against the standard CA bundle.

The portable library forbids unsafe Rust. Firmware denies it except in the
reviewed heap/RSSI sampler, outbound ESP-IDF client and mbedTLS household trust
callbacks. The client's `Send` implementation is required by the shared
`HttpClient` trait: each handle remains exclusively owned by one worker.
The immutable household trust anchor requires `Send`/`Sync` because both
workers share it. Timer, reset reason, mDNS, Wi-Fi and serving use safe APIs.
NVS remains in partition `store`, namespace `bots`, with the existing string
encoding and one-time recovery marker; framework initialization never erases it.

The framework is a sibling path dependency, so `--locked` pins registry
packages but cannot freeze its source. Keep a matching checkout alongside
this repository; a future Git dependency pinned to a commit or private
registry release can make framework upgrades reproducible.

## Firmware

To put it on a board, follow [DEPLOY.md](DEPLOY.md): `make deploy` (upgrade)
and `make install` (first install) refuse the mastomini board, by its layout
and its MAC.

```sh
make firmware         # C:/tmp/mmb/xtensa-esp32s3-espidf/release/mastomini-bots-*.bin; no board access
```

Wi-Fi credentials come from `MASTOMINI_WIFI_SSID` / `MASTOMINI_WIFI_PASSWORD`
in `.env`, `../.env` or `../mastomini_rs/.env`. The board retries joining
with backoff instead of giving up. Partitions are in `partitions.csv`:
settings live in the `store` NVS partition.

**Not done in this proof of concept:**
- A Wi-Fi setup network for boards without built-in credentials.
- Streaming replies (the reply bot polls on its schedule).
- Model price display and choice from a list (mawkingbird has both).
- Activity history that survives a restart (it is RAM only; each bot's last
  run and check are stored).

## Tests

```sh
make check            # fmt, clippy, Rust tests, admin app tests, end to end
make e2e              # a real mastomini and mastomini-bots: good_morning posts via the admin API
make e2e-llm OPENROUTER_ENV_FILE=path/to/.env   # the LLM bots with a real model
```

`make e2e-llm` reads `OPENROUTER_API_KEY` from that file without printing it.
It makes a few small calls to Gemma (fractions of a cent). `make e2e` skips
these tests when no key is set.

## Scheduled household posts

The built-in timer service accepts mastomini handoffs using an existing configured
Mastodon API key. It registers on the first handoff and runs separately from LLM
jobs. See [setup and behaviour](../docs/usage/scheduled-posts.md).

### HTTP caching

The app shell and unversioned assets revalidate with content ETags. Only
fingerprinted JS/CSS cache for one year with `immutable`; a UI rebuild changes
those URLs. Public `/api/v1/version` uses a SHA-256 ETag and a 60-second lifetime
with mandatory revalidation after expiry (a new firmware build changes its tag).
`/ca` and `/ca.pem` use content ETags with `max-age=0, must-revalidate`, so a
certificate replacement is picked up on the next request. All session state,
bot configuration, activity, diagnostics, scheduler queues, credentials,
mutations and JSON errors remain `no-store`. No private browser cache needs
clearing after sign-out or configuration changes. These conditional responses
save transfer bytes without retaining another response cache in board RAM.

### Healthy Morse light

In **Device → Healthy light message**, the admin can save a phrase of 1–80
ASCII letters, numbers, spaces or Morse punctuation. The default is
`Robots have feelings too!`. It persists across restarts and updates on the
next free maintenance pass (normally one second).

Healthy playback spells the phrase in dim cyan with 200 ms dots, 600 ms dashes,
and standard symbol/letter/word spacing, then three green healthy blinks.
Startup, bot activity, missing clock, disconnection and faults take priority;
returning to health starts the phrase again. Playback uses the existing LED
task without acquiring the service lock. GPIO selection remains a build setting.
The session-protected `GET/PUT /api/v1/device/light` API uses `{ "phrase": "..." }`
and `Cache-Control: no-store`; no API keys or bot settings are changed.

## Public status pages

Bookmark `https://mastomini-bots.local/status` for mastomini outages. Both
services serve `/status` without a login or bundled Angular build. The mastomini
page links to the independent bots page; both apps include a Status link.

The bots device probes `https://mastomini.local/api/mastomini/v1/public-status`
every 30 minutes using its existing household TLS trust and resolver. Install the
updated mastomini build too: older builds lack this endpoint and fail checks.
The observer currently targets the standard household hostname, not arbitrary
bot destinations. It reuses the existing timer worker and HTTP client with a
five-second HTTP timeout, and defers checks when a scheduled post is due within
ten seconds. It never holds the service mutex during network I/O. No extra task,
stack, or TLS client is allocated. Desktop and ESP32 both run it.

The page shows current availability, 90 UTC days of successful/total probe
counts, and the last 24 observed availability transitions. Gray days have no
data. Percentages describe observed checks, not continuous measured uptime.
History is saved at UTC day rollover and on transitions; an abrupt power loss
can lose up to a day of unchanged-state observations. After restart or 61 minutes without
a fresh observation, current state is unknown. Monitoring requires a set clock.
The local page checks storage availability and clock readiness; it does not
exercise posting, media, or all APIs. No credentials or admin diagnostics are
published. Automated transitions are observations, not operator-written notices.

Keep the bots device on separate power where possible. A shared router, network,
or power outage can still take both pages offline. These `.local` pages are
reachable on the household network; this change does not publish an internet
status service. No firmware is flashed by this change.

Resource budget: at most 48 scheduled probes per day during steady operation
(one initial probe after startup), with no retries between checks. An open status
page refreshes every 30 minutes and skips automatic refresh while hidden; manual
refresh only reads cached observations on bots. Mastomini's probe endpoint reads
the clock and storage latch directly, bypassing maintenance and authentication.
History uses 90 compact `(u32, u16, u16)` day records (720 bytes) and 24 timestamp/
boolean transitions (384 bytes on current targets), plus small struct/vector
metadata. Capacities are reserved exactly to avoid vector doubling. JSON is
bounded by a test to 2.5 KiB, excluding NVS overhead; no per-probe logs are kept.
Normal unchanged status costs one history write per UTC day, with additional
writes on observed transitions (at most one per probe). The shared standalone
HTML is about 5.6 KiB of firmware flash per service; no Angular bundle is required.
These are data-layout budgets, not measured ESP32 peak TLS/HTTP heap usage.

## Civilization in a Shoebox and Bureau of Lost Things

Two additional bots appear in the admin catalogue, disabled until configured.
Both use the Device page's OpenRouter model (Gemma by default), and each should
have its own Mastodon bot account and API key.

- **Civilization in a Shoebox** (`shoebox`) runs daily at noon Eastern by default.
  It starts with three inhabitants and publishes a single-choice poll. Polls
  close after 23 hours; a run waits if the previous poll is still open. Highest
  vote count wins, with random ties; an empty vote chooses randomly among all
  options. Gemma follows `src/bots/shoebox_rules.txt`, carrying a bounded summary
  of the current game. Victory or extinction starts a new game on the following
  day. Malformed model output is rejected. Generated turns survive ordinary
  failed-run retries, and posting uses a stable game/turn idempotency key.
- **Bureau of Lost Things** (`lost_things`) checks every five minutes and posts
  one discovery after noon Eastern. Reply directly to an item post to claim it.
  The latest seven items accept claims. Gemma judges the fictional claim;
  accepted items cannot be awarded again. Bot accounts, direct messages and
  replies outside the catalogue are ignored. Item numbers persist across runs.

Both bots store bounded state on the bot device; preserve its storage when
upgrading firmware. Clearing bot state loses the game/catalogue. Historical
posts remain on Mastomini, subject to its normal ring eviction. A hard power
loss before the scheduler saves a run can lose that run's state; API idempotency
protects duplicate submissions only while the server retains its key.

### Capacity checked on 2026-10-17

The live `mastomini.local` provisioning endpoint reported **7 available account
slots** and **12 available API-key slots**. Diagnostics showed 9/16 active
accounts, 93/4096 retained statuses, and 1,251/258,048 storage entries used
(about 0.5%). Two new accounts fit, leaving five account slots. No new accounts
were provisioned by this check.

The server expansion raises capacity to 32 accounts and 64 API keys after
flashing schema-8 firmware, widening account masks and extending slot keys. Deleted accounts retain their slots.
Deleting old posts does not create account capacity. Status storage already
uses ring eviction. There is currently no per-bot age-based deletion setting.
Leave deletion off at this usage; any future opt-in cleanup must protect live
polls, open Bureau items, and checkpoint history used by bots such as Mars.

## Public instructions and account profiles

Browse `/bots` on the bot board without signing in. Every bot has an instruction
page at `/bots/<id>`, with its purpose, participation rules and current schedule.
No API keys, prompts, game memory or private settings appear on these pages.

Every account connected with a Mastodon API key automatically publishes a short
bio, marks itself as a bot, and puts **Instructions** first in its profile fields.
This also runs when scheduled posting is off; it does not publish a timeline
status or advance a game. The remaining three fields are preserved, excluding
an old Instructions field. Give each bot its own account.

On **Device → Public bot pages**, set the address followers use to reach the
board (default `https://mastomini-bots.local`). An address change triggers new
profile links. Use **Publish profile** on a bot card to republish manually.
The key needs `read:accounts` and `write:accounts` (Mastomini's normal `read write`
bot keys include both). Errors appear on the card and in Activity; temporary
failures retry after 30 minutes, while rejected credentials/scopes require a
configuration change or manual retry. Successful publication is remembered in
separate `p.<id>` records, preserving the game's storage budget. A new account,
key, bio or board address triggers publication again.

Instruction pages link back to the bot account with `rel="me"`. Mastomini now
renders URL profile fields as links with `rel="me"`, both in account API responses
and the public profile page. Editable source fields remain plain text. This does
not claim a verified checkmark: Mastomini does not perform external verification.
These are public pages on your network; Internet access requires an externally
reachable board address. Deploy both the bots firmware and Mastomini firmware
to get the complete reciprocal-link behavior.
