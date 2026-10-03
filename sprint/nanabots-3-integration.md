# NanaBots sprint 3: integration

Status: after sprints 1 and 2.
Plan: [spec/nanacoin-bots.md](../spec/nanacoin-bots.md).

## Before flashing

- Sprint 2 keeps existing board data: a household saved by the release
  firmware opens under the new firmware (fixture test). Still, a `make
  deploy` is a one-way door (old firmware can't read a journal holding the
  new commands), so check `/api/v1/status` after the first board.
- Sessions don't survive a restart; keys do.
- Decide the certificate question (sprint 2, item 8) before testing HTTPS
  from the bots board.
- A test household needs: provision, a bot member (`POST /users` with
  `kind: bot`), its key (`POST /users/user-N/api-key`), a member's read key,
  a lotto, a listing, a quote and a loan request. Script it as part of the
  e2e work, behind a Make target.

## Goals

1. **Contract check.** Done once by hand on September 30 (feed, read key,
   lotto, band and bot-bank trades against a desktop server; one fix: the
   money epoch in keys). Keep it as an automated test, below.
2. **e2e tests** in `mastomini_bots/e2e/` (pytest, like `test_good_morning.py`):
   - Start nanacoin desktop + mastomini desktop + mastomini-bots desktop.
   - Provision NanaCoin; Nana creates two bot members and a read key.
   - `nana_news`: make a listing, open a lotto, post a quote; **Run now**;
     the posts appear on mastomini; run again and nothing is repeated;
     disabled categories stay silent.
   - Traders: one forex band bot and one lotto bot in dry run (no money
     moves, the log says what it would do), then live: trades land, and a
     retried run moves no money twice (re-send with the same key).
   - Two band bots don't trade with each other by default.
   - A bot member is refused a good deed.
3. **Certificates.** Apply sprint 2's choice; test HTTPS from the bots board to
   `nanacoin.local`.
4. **Board soak.** Flash a bots board with all 8 nanacoin bots on. Watch heap
   (`/diag`), NVS record sizes (each bot's record must stay under the 4000-byte
   NVS string limit), and nanacoin's request counters for a day.
5. **Docs.** README bot table and settings; DEPLOY note on creating bot
   members and keys; nanacoin API.md for the new routes.

## Make targets

- `make e2e-nanacoin` in `mastomini_bots`: builds `../../nanacoin/nanacoin_rs`
  desktop and runs the nanacoin e2e tests. `make check` includes it only when
  the nanacoin checkout is present.

## Done when

All of the above pass under `make check`, and one real day on the board has
posted news twice with no duplicates and no failed runs.
