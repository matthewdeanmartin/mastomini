# NanaBots sprint 1: bots framework and the bots

Status: **done** September 30, 2026, against the assumed sprint 2 contract.
`make check` passes (fmt, clippy, Rust tests, admin app tests, mastomini e2e).
Not run: `make firmware` (the changes are platform-neutral library code), and
anything against a real NanaCoin, which is sprint 3.
Plan: [spec/nanacoin-bots.md](../spec/nanacoin-bots.md).
Scope: `mastomini_bots` and `mastomini_bots_ui` only. NanaCoin is not touched;
the client is written against [sprint 2's contract](nanabots-2-nanacoin.md)
and tested with a scripted HTTP double. Real end-to-end is sprint 3.

## Framework

1. **Several times a day.** The shared `time` setting takes 1–4 times
   (`08:00, 20:00`). `Schedule::Daily` holds a list of times. Old
   single-time values stay valid.
2. **Setting groups and conditions.** `Setting::group("Post about")` lets
   the admin app draw consecutive settings under one heading (the news bot's
   checkbox column). `Setting::shown_when("strategy", &["band"])` hides a
   setting unless another setting has one of those values (each trader shows
   only its strategy's fields). Both are display-only; the server validates
   every value as before.
3. **Mastodon optional.** `BotInfo::needs_mastodon`. When false, the bot
   can be turned on without a Mastodon server and key; `Run::has_mastodon()`
   says whether it may post.
4. **Bot hooks.** `Bot::ready(&Settings)`: refuses turning on until the
   bot's own requirements are met (a NanaCoin key). `Bot::check(&mut Run)`:
   what **Check connections** verifies (default: the Mastodon key).
5. **NanaCoin client** (`src/nanacoin.rs`) over the existing `HttpClient`,
   with `nanacoin_settings()` (server + key) and `Run::nanacoin()`.
   Deterministic idempotency keys `g<generation>:mmb:<bot>:<slot>:<what>`.

## Bots

- **`nana_news`**: twice a day, reads `/activity`, posts concise lines per
  category, rolls up crowded categories, at most N posts a run. Default
  visibility: public. One checkbox per category.
- **`trader_1` … `trader_6`**: one type. Mastodon is optional; when set, it
  posts its trades (on by default). Dry run is on by default. Strategies:
  1. Forex band (buy at X, sell at Y; take, make, or both)
  2. Lotto regular
  3. Cheap borrower (accept offers under N% APR)
  4. Bot bank (fund requests at N% APR or more)
  5. Steady buyer (dollar-cost averaging)
  6. Rebalancer (target NC/¢ split)
  7. Mean reversion (from recent trade rates)

  Each strategy is a pure function `market snapshot + settings -> actions`.
  The executor applies the guard rails (reserve, per-run and per-day caps,
  never its own quote, no bot-to-bot unless allowed, stop on the first
  money error). The LLM strategy later produces the same actions.

## Tests

- Schedule: several times across daylight saving; describe.
- Settings: times validation; group/shown_when serialize.
- Service: a Mastodon-optional bot turns on with no Mastodon key; `ready`
  refuses without a NanaCoin key.
- Client: request shapes, keys, error mapping (`retryable`).
- News: categories on/off, roll-ups, the post cap, first run starts at now,
  a retry composes the same posts, reset. (Feed truncation is only logged;
  it has no test of its own.)
- Trader: a decision table per strategy; guard rails; dry run sends no
  mutation; trades are posted only when Mastodon is set.
- Admin app: group headings, `shown_when` hiding.

## Notes for sprint 3

- `Schedule::Every 60` now reads "Every hour" (was "Every 1 hours").
- The shared `time` setting is now kind `times`; old single values stay valid.
- Traders refuse real trades from a member whose `kind` isn't `bot` (dry
  run still works on any account), so live trading needs sprint 2.
- Trade posts are best effort: a failed Mastodon post is logged, never
  retried as a run, so money actions are never repeated for a post.
- Each bot's NVS record (config, settings, memory) must stay under 4000
  bytes; traders keep at most 20 forex rates in memory.

## Not in this sprint

The NanaCoin changes (sprint 2), real e2e and board soak (sprint 3), the LLM
strategy.
