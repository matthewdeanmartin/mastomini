# NanaBots sprint 3: integration

Status: after sprints 1 and 2.
Plan: [spec/nanacoin-bots.md](../spec/nanacoin-bots.md).

## Goals

1. **Contract check.** Run the sprint 1 client against a real desktop
   `nanacoin_rs` (`make run`, port 8080). Fix every difference between
   [sprint 2's contract](nanabots-2-nanacoin.md) and what shipped, on whichever
   side is wrong.
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
