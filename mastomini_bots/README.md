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
| `llm_reply` | Answers people who mention it, with a language model through OpenRouter. By default it checks every 5 minutes. |
| `llm_post` | Writes a new post with a language model. By default once a day at 12:00. |

`llm_reply` and `llm_post` are one bot type (`src/bots/llm.rs`) listed twice
with different defaults. Either can be switched to the other mode.

## Try it on the desktop

```sh
make run-web          # builds the admin app, serves http://127.0.0.1:8090/app/
```

1. Open the app. The first visit sets the admin password.
2. On mastomini, make an API key for the bot's account (household app → **My
   account → API keys**). A bot with its own name needs its own member.
3. In **Bots → Settings**, enter the server (`https://mastomini.local`, or
   `http://127.0.0.1:8080` for `make run` in `../mastomini_rs`) and the key, and turn it on.
4. **Check API key** signs in only; **Run now** posts straight away. **Activity**
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
own **Model** setting. **Check API key** on an LLM bot also makes one tiny
completion, to show OpenRouter answers.

The LLM bot's settings:

| Setting | |
|---|---|
| Runs, time, zone, every | When it runs: the same four settings every scheduled bot has |
| Does | Answers mentions, or writes new posts |
| System instructions | Who the bot is and how it writes; sent as the system message |
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
     (`grace_minutes`), and `uses_llm`.
   - **`settings()`**: what the admin can set, as a list of `Setting`s. The
     admin app draws the form from it; a bot writes no UI code. Kinds:
     `Text`, `LongText` (with the `{{placeholders}}` it may use), `Number`,
     `Choice`, `Time`, `Zone`, `Toggle`, `Secret`. Start from
     `schedule_settings(...)` for the four settings every scheduled bot shares.
   - **`schedule()`**: `schedule_from(settings)` for those.
   - **`run()`**: what it does.
     - Read settings from `run.settings` (`get`, `number`, `yes`, `time`,
       `tz`).
     - Post with `run.post(...)`, which is safe to retry. Use `run.post_keyed(...)`
       for several posts in a run.
     - Use `run.mastodon()` for anything else in the API, and `run.llm()` for
       the device's OpenRouter.
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

This refuses to create a second household CA. The board serves the admin site
with the transport measured on mastomini's board: one write per response,
`TCP_NODELAY`, and TLS session tickets, so a returning browser reconnects in
tens of milliseconds instead of a ~1 s handshake. It also turns Wi-Fi modem
sleep off.

For the bots' own requests, the board resolves `mastomini.local` once over
mDNS, connects by address while verifying the name, keeps connections open,
and resumes TLS sessions. Household names are verified against the household
CA, public servers against the standard CA bundle.

## Firmware

To put it on a board, follow [DEPLOY.md](DEPLOY.md): `make deploy` (upgrade)
and `make install` (first install) refuse the mastomini board, by its layout
and its MAC.

```sh
make firmware         # C:/mmb/xtensa-esp32s3-espidf/release/mastomini-bots-*.bin; no board access
```

Wi-Fi credentials come from `MASTOMINI_WIFI_SSID` / `MASTOMINI_WIFI_PASSWORD`
in `.env`, `../.env` or `../mastomini_rs/.env`. The board retries joining
with backoff instead of giving up. Partitions are in `partitions.csv`:
settings live in the `store` NVS partition.

**Not done in this proof of concept:**
- A Wi-Fi setup network for boards without built-in credentials.
- A deployment to a real board: DEPLOY.md and its scripts are written and
  checked, but have not been run against hardware yet.
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
