# NanaCoin bots: plan

Status: planning, September 30, 2026. Nothing here is implemented.

Two new bot types in `mastomini_bots`, both reading from (and one writing to)
`nanacoin_rs` with NanaCoin API keys:

1. **`nana_news`**: posts what happened in the economy, twice a day.
2. **`trader_1` … `trader_6`**: one bot type listed six times. Each is a
   nonhuman NanaCoin member with its own money, key, strategy and settings.

## What exists today (and what it means)

| Fact | Where | Consequence |
|---|---|---|
| A bot type can be listed many times with different ids; each gets its own settings, schedule, `run.state` | `src/bots/mod.rs` (`llm_reply`/`llm_post`) | Six traders is six entries, no framework change |
| Settings are flat; `Toggle` and `Secret` kinds exist; the admin app draws the form | `src/settings.rs` | Category checkboxes are `Toggle`s; the NanaCoin key is a `Secret` |
| `Schedule` is `Daily` (one time), `Every N minutes` (UTC-aligned) or `Manual` | `src/schedule.rs` | "2x a day at local times" is not expressible; `Every 720` drifts with DST |
| `Run` holds one connection: a Mastodon server and token | `src/bot.rs` | Needs a NanaCoin client beside it |
| A bot is off until it has a Mastodon server and key | README "Add a bot" | A trader that never posts can't be turned on |
| The bots board trusts mastomini's household CA + public roots | `src/household_trust.rs` | NanaCoin uses a different CA ("NanaCoin Home CA", `nanacoin_rs/certs/home-ca.der`) |
| `MEMBERS = 16` | `nanacoin_rs/src/domain.rs:10` | 6 traders use 6 of 16 member slots |
| `QUOTES = 16`, economy-wide | `nanacoin_rs/src/forex.rs:5` | 6 band traders with a bid and an ask each take 12 of 16 slots |
| `GET /audit` is the business event stream, but Nana-only, 16 per page | `client/history.rs:227` | No member-readable "what happened since X" feed |
| `GET /transactions` covers money movements only (365-record ring) | API.md | New listings, lotto opened, quotes posted, loan requests aren't transactions |
| Lotto creation is Nana-only; draws are service steps, not commands | `lotto.rs:105` | "Lotto ended / winner" has no audit row today (roadmap already lists this gap) |
| Loan offers target one borrower; `REQUESTED` loans are visible to all | `loans.rs` `visible_to` | "Take loans < 5%" = accept offers made *to the bot*; lending = respond to public requests |
| Idempotency keys must be `g<generation>:<...>`, 1–80 bytes | API.md | Bot keys must be deterministic per slot so retries dedupe |
| Purchases create a physical fulfillment TODO | API.md "Physical fulfillment" | A bot buying goods/services gets IOUs for real-world work. Keep traders to forex, lotto and loans |

## NanaCoin API changes (nanacoin_rs)

Ordered by need. **N1 blocks the news bot.** N2–N4 should land before traders
go live with real balances.

### N1. Member-readable activity feed (required)

`GET /api/v1/activity?after=<sequence>&limit=<n>`

- Any active member, or a read-only key (N2). A sanitized public projection of
  the business audit, plus the service events that aren't commands.
- Response: `{ incarnation, generation, sequence, truncated, events: [...] }`.
  `truncated: true` when `after` is older than what is retained; the bot then
  says nothing about the gap, or posts one "some activity was missed" line.
- Each event: `seq`, `at`, `kind`, `actor` (id + display name), `subject` (typed
  id), `title`, `amount`, `currency`/`decimals`. Plain fields, no command JSON.
- Kinds, first version: `member_joined`, `listing_opened` (side, kind),
  `listing_sold`, `listing_cancelled`, `good_deed_posted`, `good_deed_claimed`,
  `lotto_opened`, `lotto_drawn` (winner, pool), `lotto_paid_out` (savings lottos
  after the hold), `loan_requested`, `loan_funded`, `loan_paid`, `quote_posted`,
  `quote_taken` (rate, coins), `gift_request_opened`, `gift_request_closed`,
  `art_minted`, `art_sold`.
- Never: memos, private offer messages, zero-value messages, fulfillment
  dispute reasons, loan notes, identity/credential changes, reversal reasons.
  Write the exclusion list as a test, the way `Audit::validate_public` is.
- Needs new noncash audit rows for service-driven steps: lotto draw, lotto
  payout complete, loan paid, offer settled. The roadmap's "storage and
  transaction foundations" section already calls for these.
- Bounded by `AUDIT_CACHE` (1024 S3 / 128 S2). At two runs a day this is
  ample for a household.

This one endpoint also gives traders a trade tape (`quote_taken` rates) for
price-following strategies, so no separate `/forex/trades` is needed.

### N2. Scoped API keys

- `scope: "read"` keys: every GET, refused on every mutation. The news bot uses
  one. A leaked news-bot key then can't move money.
- A read key should not need its own member. Proposal: a member may hold one
  read key in addition to their full key. Nana makes an "observer" read key on
  her own account for `nana_news`; no member slot is spent.
- Optional, defense in depth for traders: a server-side `daily_spend_limit` on a
  key. The bot also enforces limits, but the board holds the key, and the
  server is the only place a limit can't be bypassed.

### N3. Nonhuman members

- `kind: "bot"` on members (Nana sets it at creation; not changeable by the
  member). Shown with a badge in the UI and on profiles. Exposed in `/users`.
- `POST /api/v1/users/user-N/api-key` (Nana only, bot members only): mint the
  bot's key directly. Today Nana would have to log in *as* the bot with a
  password to call `POST /me/api-key`.
- Policy questions to settle in code, not by accident:
  - May bots claim good deeds? (Recommend no: those reward people.)
  - Do bots get the initial grant? (Recommend no: Nana funds them deliberately,
    so bots don't dilute everyone's coins on creation.)
  - Do bot trades count toward GDP/production totals? (Forex isn't production
    anyway; say so explicitly in the totals docs.)

### N4. Capacity guards

- **Members:** 6 traders + humans in 16 slots. Choose one: run fewer traders
  (2–3 is plenty to start), or raise `MEMBERS` (cost: `[u32; MEMBERS]` tickets per
  lotto, credit masks, member-indexed arrays; review memory on S2), or wait for
  the roadmap's party/account split. **Decision needed.**
- **Quotes:** add a per-member open-quote cap (e.g. 2, or 1 for `kind: bot`) so
  bots can't fill the 16-slot book and lock humans out. Return `507 capacity`
  with a distinct error code (`member_quote_limit`) so the bot can log it.
- Same idea for loan offers (32 slots) if lender bots are enabled.

### N5. Small conveniences (nice to have)

- `apr_bps` on loan views (annualized from `rate_bps`/`rate_days`), so every
  client doesn't redo the arithmetic and get it subtly wrong.
- `status` (OPEN/CLOSED/DRAWN/PAID) on lotto views, if not already derivable
  from `step` without internal knowledge.
- Confirm deterministic keys like `g3:mmb-trader_2-1727680000000-bid` are fine
  (the docs say `<random>`; bots need replay-stable keys).

No webhooks or websockets: two polls a day doesn't need push.

## mastomini_bots framework changes

| Change | Why |
|---|---|
| `src/nanacoin.rs`: a small client (`activity`, `quotes`, `lottos`, `loans`, `me`, and the mutations), using the existing `HttpClient` | Both bots |
| `Run::nanacoin()` reading the `nanacoin_server` (Text) and `nanacoin_key` (Secret) settings | Both bots; keys already never come back out |
| Trust the NanaCoin Home CA as a second household anchor, with the same name constraints | HTTPS to `nanacoin.local` |
| `Schedule::Daily` takes up to 4 times (`08:00, 20:00`) | "Runs 2x a day", in local time across DST |
| `BotInfo::needs_mastodon: bool` | A trader with no Mastodon key can still be switched on |
| `Setting::group("Post about")` so the admin app draws consecutive toggles as one checkbox column | The news bot's category column |
| `Setting::shown_when("strategy", "forex_band")` | Each trader shows only its strategy's parameters |
| Idempotency helper `run.nanacoin_key(what)` -> `g<gen>:mmb:<bot>:<slot>:<what>` | Money retries are safe |

Settings live in NVS: six traders' settings are fine, but keep strategy
parameters as a few short values, not long text.

## Bot 1: `nana_news`

**Runs:** daily at 08:00 and 20:00 (settings). **Needs:** a NanaCoin read key,
a Mastodon key.

**How:** read `/activity?after=<state.seq>`; group by category; drop disabled
categories; post; save `state.seq`. On the first run, start from now (like the
reply bot does), not the backlog. Each post uses `run.post_keyed(..., "<seq>")`
so a retried run never double-posts.

**Settings:**

| Setting | Kind | Default |
|---|---|---|
| Runs, times, zone | schedule | on, 08:00 + 20:00, US Eastern |
| NanaCoin server, NanaCoin key | Text, Secret | `https://nanacoin.local`, — |
| Most posts per run | Number 1–20 | 6 |
| Who sees its posts | Choice | public (everyone) |
| Hashtag | Text, optional | `#nanacoin` |
| **Post about** (checkbox column) | Toggle each | see below |

**Categories and templates** (fixed in code; each fits in 140 characters
before the hashtag; amounts formatted with the economy's `decimals`):

| On by default | Category | Template |
|---|---|---|
| ✅ | Lotto opened | `🎟 New lotto: {title}. {price} NC a ticket, closes {day} {time}.` |
| ✅ | Lotto drawn | `🎉 {winner} won "{title}": {pool} NC!` |
| ✅ | New for sale | `🛒 {who} is selling {title} for {price}.` |
| ✅ | New wanted | `🙋 {who} wants {title}, pays {price}.` |
| ✅ | Good deeds | `⭐ Good deed: {title}. Reward {price}.` |
| ✅ | Loan requests | `🏦 {who} wants to borrow {amount} NC at {apr}%/yr.` |
| ✅ | Gift requests | `🙏 {who} is asking: {title}.` |
| ✅ | New members | `👋 Welcome, {name}!` |
| ✅ | Daily summary | `📊 Since last time: {n} sales, {m} NC moved, 1 NC = {rate}¢.` |
| ☐ | Sold | `✅ {title} sold for {price}.` |
| ☐ | Forex quotes | `💱 {who} {buys/sells} {coins} NC at {rate}¢.` |
| ☐ | Forex trades | `💱 {coins} NC traded at {rate}¢.` |
| ☐ | Loans funded | `🤝 {lender} lent {borrower} {amount} NC.` |
| ☐ | Loans paid off | `✔ {borrower} paid off a loan.` |
| ☐ | Art | `🎨 {who} made "{title}" ({n} editions).` |
| ☐ | Bot trades | traders' own events, off so traders don't flood the feed |

**Chattiness control:** when a category has more events than fit, it becomes
one roll-up post: `🛒 5 new for sale: lamp, bike, cookies, +2 more.` The "most
posts per run" cap applies after roll-ups; the summary post is always last.

**Privacy:** only what every member can already read. Warn in the setting's
help text if the Mastodon server isn't a `.local` household server: the
household's economy becomes public.

## Bot 2: `trader_1` … `trader_6`

**Account:** a NanaCoin member with `kind: bot` (N3), funded by Nana, with its
own full-scope key. **Runs:** every 60 minutes by default (Every N; 15 minimum).
**Mastodon:** optional; when set, it posts its own trades (`🤖 bought 20 NC at
11¢`), which is a nice transparency log for a bot spending real household money.

**Common settings (every strategy):**

| Setting | Purpose |
|---|---|
| Strategy | Choice: one of the strategies below |
| Dry run | Toggle, **default yes**: logs what it would do, no orders |
| Keep at least | NC and ¢ reserve it never spends below |
| Most per run / per day | Spend caps; the day counter lives in `run.state` |
| Post my trades | Toggle; needs a Mastodon key |

**Guard rails (in code, not settings):** re-read balances before each order;
never more than one order per object per slot (keyed); stop the run on the
first 409 insufficient-funds or capacity error; never take its own quote; never
trade with another trader bot unless a setting allows it (stops two bots
ping-ponging the book).

### Requested strategies

1. **Forex band** — "buy at X, sell at Y".
   Settings: buy at ≤ X ¢/NC, sell at ≥ Y ¢/NC, size per trade, most NC to hold,
   most ¢ to hold, mode (`take`: hit existing quotes in the band; `make`: keep
   its own BID at X and ASK at Y on the book; `both`). Refuses X ≥ Y on save.
   As maker it re-posts expired quotes and cancels its own when the band changes.
2. **Lotto regular** — "always buy 10 NC of lotto".
   Settings: spend per lotto, kinds (Simple / Delayed / Savings), skip if ticket
   price over P. Buys once per lotto (keyed by lotto id), before it closes.
   Savings lottos return principal plus interest, so this doubles as a saving
   strategy; say so in the help text.
3. **Cheap borrower** — "take all loans under 5%".
   Accepts loan offers made *to this bot* whose APR < N%, total borrowed ≤ cap,
   installment it can cover from its balance. Repays early when it has cash
   above the reserve. People have to offer loans to the bot first; post a
   standing loan request (`POST /loans/request`) to invite them.

### Four more strategies to implement

4. **Bot bank (lender)** — the mirror of #3, and the most useful to the
   household. Funds public loan requests with APR ≥ N%, amount ≤ M, total lent
   ≤ cap, per-borrower cap, optional "only borrowers who've paid off a loan
   before". Gives members somewhere to borrow when no human wants to lend.
5. **Steady buyer (dollar-cost averaging)** — every run, buy (or sell) K NC
   at the best quote if the rate is within a ceiling (floor). Teaches the idea,
   and gives the forex book a predictable counterparty.
6. **Rebalancer** — keep the value split at a target, e.g. 50% NC / 50% ¢
   at the last traded rate. Trades only when off by more than D%. Buys after
   falls and sells after rises without predicting anything.
7. **Mean reversion** — keep the last N trade rates from `/activity`
   (`quote_taken`) in `run.state`; buy when the best ask is D% under their
   average, sell when the best bid is D% over. The first strategy that reacts
   to the market rather than fixed numbers, and a stepping stone to the LLM bot.

Also considered: **Philanthropist** (gives K NC a week to open gift requests or
the lowest balance): charming, but it's a gift loop rather than trading, so
it fits better as a later "allowance" bot. **Bargain hunter** (buys cheap
listings) was rejected: purchases create real-world fulfillment TODOs a bot
can't receive.

### Toward the LLM trader (roadmap)

Shape strategies now so an LLM is just another strategy: each strategy gets
the same plain-lines market snapshot (balances, best bid/ask, open lottos,
loan requests, last 10 trades) and returns at most a few actions from a fixed
vocabulary (`take quote-7 10`, `bid 11 20`, `buy_tickets lotto-41 2`,
`accept loan-9`, `hold`). The LLM strategy writes those lines; the same
parser, caps and guard rails check them. The model never sees the key and can't
invent an action outside the vocabulary.

## Sprints

| Sprint | Repo | Contents |
|---|---|---|
| N1 | nanacoin_rs | `/activity` feed + noncash audit rows for draws/payouts/settlement; exclusion test |
| N2 | nanacoin_rs | Read-scoped keys; `kind: bot` members; Nana mints bot keys; per-member quote cap; `apr_bps`; member-cap decision |
| B1 | mastomini_bots | NanaCoin client + trust anchor; multi-time `Daily`; `needs_mastodon`; setting groups / `shown_when` (admin UI) |
| B2 | mastomini_bots | `nana_news` + e2e against desktop nanacoin (`make run` in nanacoin_rs) and mastomini |
| B3 | mastomini_bots | Trader core, guard rails, dry run; strategies 1–3 |
| B4 | mastomini_bots | Strategies 4–7 |
| later | both | LLM strategy; optional server-side spend limit on keys |

Testing: unit tests with the fake `HttpClient` for every strategy decision
(table of book states -> expected actions); `make e2e` grows a NanaCoin desktop
server; each money action is retried in a test to prove it doesn't repeat.

## Decisions (owner, September 30, 2026)

1. **Member slots**: many bots allowed; raise `MEMBERS` (sprint 2).
2. **Bot policy**: bots are flagged as bots; they get the initial grant if
   Nana sets one, like anyone; they **can't** claim good deeds; they may buy
   lotto tickets, lend and borrow.
3. **Traders posting**: on by default when a Mastodon key is set.
4. **News visibility**: public ("listed") by default.

Work is split into [sprint 1](../sprint/nanabots-1-framework.md) (bots
framework and bots), [sprint 2](../sprint/nanabots-2-nanacoin.md) (NanaCoin; waits for the
owner's go) and [sprint 3](../sprint/nanabots-3-integration.md) (integration).
The sprint 2 doc holds the API contract the bots are written against.
