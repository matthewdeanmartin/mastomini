# SRE sprint 4: the SRE bot files issues

Status: **built and deployed to the bots board 2026-10-24.** `bots/sre_issues.rs`.
`make check` passed: 196 Rust tests, 19 UI tests, 12 e2e. The request shapes
were checked against the live desktop tracker.

## As built

- Settings group "Issue tracker": `tracker_url` (e.g. https://ops.local),
  `tracker_token` (scope triage, so it can also close), `tracker_close_days` (30).
- Only findings whose rule is `issue` are filed: by default
  `reset.fault.repeat`, `reset.hung.repeat` and `heap.allocations`. Change
  this on the Runbooks tab.
- Fingerprint `host|family`: first and repeated crashes share `crash`, and RST
  hangs share `hang`, so one board's crashes are one issue with a count.
- Order: judge, then file, then notify, then post. The notice and the reply line end in
  "Issue #12", "#12 (reopened)" or "#12 (3 reports)".
- Stale closing: open `sre` issues not currently firing and not reported for
  N days are closed as `stale` with a comment. At most 4 per run, and a
  "Closed as stale" line goes in the digest.
- The tracker being down: up to 3 filings wait in state (bodies cut to 600
  bytes) for the next run. A 4xx is logged and not retried.

## Plan (as written before building)
Status: planned. Needs SRE sprint 1 and tracker sprint 1 (deployed or desktop).

## Goals

1. **Settings**: tracker URL and a `report`-scoped token. Setup explains
   "tracker → Tokens → New, scope report" and has a test button.
2. **File only when the runbook says `issue`.** Defaults:
   - Repeated `crash`/`watchdog` (2+ in 7 days), or any crash with an
     allocation failure in `previous`.
   - Heap well below the board's own baseline for 2+ days, or allocation failures.
   - `storage` write failures. NanaCoin write failures are p0 and also page.
   - Firmware drift from `expected_builds` lasting more than 7 days.
   - Not filed: power-on resets, a single crash, ordinary Wi-Fi blips, "learning baseline".
3. **Issue content**: title `gamematcher.local: crash (watchdog) x3 in 7 days`.
   The body gives what happened, the board's normal, the runbook text, the
   `previous` events and log tail (bounded), build, and uptime. Labels:
   `sre`, `board:gamematcher`, `runbook:crash`. Priority from the rule.
   The fingerprint comes from sprint 1, so recurrences update the same issue.
4. **Close the loop**: when a finding resolves, comment "not seen for N days".
   Close only if the owner chooses auto-close (open question). The digest and
   the `/sre` Findings tab link to the issue.
5. **Tracker down**: queue up to N pending filings in state, retry on the next
   run, and say so in the digest. Never block the digest on the tracker.

## Tests

- e2e: desktop tracker + bots + a fixture board that crashes twice → exactly
  one issue, then a third crash → count 3 and no new issue; the issue closes,
  the board crashes again → reopened.
- Tracker unreachable → the digest still posts and the filing is queued.
