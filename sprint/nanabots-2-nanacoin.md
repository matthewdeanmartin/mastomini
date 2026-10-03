# NanaBots sprint 2: NanaCoin changes (nanacoin_rs)

Status: **done** September 30, 2026 (owner said go). In `nanacoin_rs` and
`nanacoin_ui`; not committed. `make check` passes in nanacoin_rs (fmt, clippy
S3 + S2, tests S3 + S2, HTTP smoke); nanacoin_ui builds and its 327 tests
pass. The bots' client was smoke-tested against a real desktop server: feed,
read-key refusal, lotto/band/bot-bank trades, no repeats on a re-run.

### What shipped, and where it differs from the contract below

- **Board data survives the upgrade** (spec/FORWARD_COMPATIBLE_DATA_CHANGES.md,
  newer than the AGENTS.md "disposable data" note). So bot members and read
  keys are new commands at the end of `Command` (`CreateBot`, `SetReadKey`),
  not new fields; kind, read keys and members 17–32 are kept in a new
  checkpoint extension. A fixture written by the release firmware (`64f1c7f`)
  is in `tests/fixtures/pre-bots/` and opens: balances, lotto tickets, a loan,
  a quote and an API key all survive, then gain bots and read keys.
- `GET /me/api-key` keeps `active`/`created_at` (the full key) and adds
  `full` and `read`. `POST` answers with `scope` too.
- `GET /users/user-N/api-key` also works (Nana: whether the bot has a key).
- `member_joined` needs a name and role on the identity row, so a key made in
  the same second as the member isn't a second "join".
- No `editions` on `art_minted` (minting has no edition count).
- Events about objects since recycled (an old sold listing, a replaced
  lotto) are left out.
- The bots' idempotency keys gained `m<money epoch>`: NanaCoin refuses money
  requests whose key names an old currency epoch. Found while checking the
  client against the server; fixed in `mastomini_bots/src/nanacoin.rs`.
- **Certificates (item 8) are not done**: still the owner's choice.
- UI: Nana's Members tab adds bots (no password to type), a bot badge, and
  Make/Revoke a bot's key (shown once). Settings → API Keys shows a full and
  a read-only key. The demo backend does the same.
Plan: [spec/nanacoin-bots.md](../spec/nanacoin-bots.md).
Sprint 1 ([nanabots-1-framework.md](nanabots-1-framework.md)) writes the bots
against the contract below. Change the contract here first if the server
needs to differ, then fix `mastomini_bots/src/nanacoin.rs` to match.

## Decisions (owner, September 30, 2026)

- Many bots are allowed: raise the member cap.
- Bots get the initial grant exactly like people, if Nana sets one. No special case.
- Accounts are flagged as bots. **Bots cannot claim good deeds.**
- Bots may buy lotto tickets, lend and borrow.

## Contract the bots rely on

Everything not listed here already exists and is used as documented in
`nanacoin_rs/API.md`: `GET /status` (`decimals`, `journal_generation`),
`GET/POST /quotes`, `POST /quotes/quote-N/take|cancel`, `GET /lottos`,
`POST /lottos/N/tickets`, `GET /loans`, `POST /loans/request`,
`POST /loans/N/offer|accept|repay`. All mutations get an `Idempotency-Key`.

### 1. Member kind

- `POST /users` accepts `kind: "human" | "bot"` (default `human`). Nana only.
  Fixed at creation.
- Every user view (`/me`, `/users`, `/users/user-N`) includes `kind`.
- A bot member's offer on a `good_deed` listing is `403` with error
  `bot_good_deed`. Accepting one already made (impossible after this change) is
  not a concern for development data.
- Bots may hold a password (Nana sets one) but normally never log in.

### 2. Nana mints a bot's key

`POST /users/user-N/api-key`, Nana only, target must be `kind: bot`, body `{}`.
Returns `{ "api_key": "nc_…", "created_at": … }`, replacing any previous full
key for that member. `DELETE` revokes it. Same storage as `/me/api-key`.

### 3. Read-only keys

- `POST /me/api-key` accepts `scope: "full" | "read"` (default `full`).
- A member holds at most one key of each scope; making one replaces only that
  scope's key. `GET /me/api-key` reports both: `{ full: {active, created_at},
  read: {active, created_at} }`. `DELETE /me/api-key?scope=read`.
- A read key gets every GET the member gets. Any other method is `403` with
  error `read_only_key`.
- The news bot uses a read key made by Nana on her own account, so it spends no
  member slot.

### 4. Activity feed

`GET /api/v1/activity?after=<seq>&limit=<n>`: any active member, any scope.
`limit` 1–50, default 50. Oldest first, `seq > after`.

```json
{
  "incarnation": 3, "generation": 7, "sequence": 912,
  "decimals": 4, "truncated": false,
  "events": [
    {"seq": 905, "at": 1790500000, "kind": "listing_opened",
     "actor": "user-3", "actor_name": "Robin", "actor_bot": false,
     "subject": "listing-41", "title": "Bike tune-up", "side": "SELL",
     "amount": 50000}
  ]
}
```

- `truncated: true` when events after `after` were already evicted (the bot
  skips the gap). `sequence` is the newest sequence; a bot starting fresh uses
  it as its cursor.
- On economy reset (`incarnation` changes) a bot restarts from `sequence`.
- Fields are flat and optional by kind. `amount`/`coins` are NC minor units;
  `rate` is ¢ per whole NC; `at` is unix seconds.

| `kind` | Fields beyond seq/at/kind/actor* |
|---|---|
| `member_joined` | — (actor is the new member) |
| `listing_opened` | subject `listing-N`, title, side `SELL`/`BUY`, amount (price) |
| `listing_sold` | subject, title, amount, `other`/`other_name` (buyer) |
| `good_deed_posted` | subject, title, amount (reward) |
| `good_deed_claimed` | subject, title, amount, other/other_name (claimant) |
| `lotto_opened` | subject `lotto-N`, title, amount (ticket price), `closes_at`, `lotto_kind` |
| `lotto_drawn` | subject, title, amount (pool), other/other_name (winner) |
| `loan_requested` | subject `loan-N`, amount, `apr_bps` |
| `loan_funded` | subject, amount, apr_bps, other/other_name (borrower; actor is lender) |
| `loan_paid` | subject, amount, other/other_name (lender; actor is borrower) |
| `quote_posted` | subject `quote-N`, side `BID`/`ASK`, rate, coins |
| `quote_taken` | subject, side (of the quote), rate, coins, other/other_name (taker) |
| `gift_request_opened` | subject, title, amount (target, optional) |
| `art_minted` | subject, title, `editions` |

Never in the feed: memos, offer messages, zero-value messages, loan notes,
dispute reasons, reversal reasons, identity or credential changes. Write the
exclusion list as a test beside `Audit::validate_public`.

`lotto_drawn` and `loan_paid` come from service steps, not commands: add the
noncash audit rows the roadmap already calls for.

### 5. Loan views get `apr_bps`

`apr_bps = rate_bps * 365 / rate_days` (rounded down), on every loan view
and in the feed. Clients compare this, not their own arithmetic.

### 6. Capacity

- Raise `MEMBERS` from 16 to 32. Review member-indexed arrays: lotto
  `tickets: [u32; MEMBERS]`, `credit_blocked` (a `u16` bitmask today:
  `1u16 << (id - 1)`, so it must become `u32`), checkpoint row sizes and the
  S2 memory budget.
- Per-member open-quote cap: 2. Over it: `507` with error `member_quote_limit`.
- Per-member open loan-offer cap: 4, error `member_loan_limit`.

### 7. Idempotency keys

Bots send deterministic keys: `g<generation>:mmb:<bot>:<slot>:<what>`, at
most 80 bytes. Confirm `<random>` in the docs is only a recommendation, and
document that replay-stable keys are the intended use for automated clients.

### 8. Certificates

The bots board trusts mastomini's household CA (name-constrained to
`.local`, `.lan`, private IPs…). NanaCoin's leaf is signed by a different,
unconstrained "NanaCoin Home CA". Pick one (owner):
- **Recommended:** re-sign `nanacoin.local`'s leaf with the mastomini household
  CA (`../mastomini_rs/.local/ca`), so every household device trusts one CA.
  Devices that installed the NanaCoin CA need the mastomini one instead.
- Or: teach the bots board a second anchor (firmware TLS change in
  `household_tls.rs`; still restricted to household names by `verify_names`).
- Or: NanaCoin in Easy mode over HTTP on the LAN (no TLS; not recommended).

## Tests

Desktop server tests for each item. The feed: one test per kind, the
exclusion test, truncation, reset. Read key: every mutating route is refused.
Good deeds: bot refused, human allowed.
