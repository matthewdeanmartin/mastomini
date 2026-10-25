# SRE sprint 1: actionable findings

Status: **built 2026-10-24, not flashed.** Bots board only; no board firmware change.
Code: `mastomini_bots/src/bots/sre_findings.rs` (triage, memory, evidence),
wired into `sre.rs` (`judge`, `evidence`). `make test` and `make lint` pass.

## As built (differences from the plan below)

- **Resets are events found by the boot time moving** (now minus `uptime_ms`),
  not by the "Last reset: X" text, which stays the same until the next reset.
  A crash from last month no longer shows up as a warning every day. On first
  sight of a board, only a reset in the last 26 h counts as news.
- **The RST case (owner, 2026-10-24: "several boards need RST to come back"):**
  `reset pin` after the previous digest saw the board down → `reset.hung`
  (notify). A second RST in 7 days → `reset.hung.repeat` (issue). RST resets keep
  RTC memory, so the error log's `previous` section shows what the board was
  doing when it hung. That's the evidence for the firmware bug.
- Error-log reads happen only for a new crash, watchdog, brownout, RST or
  unknown reset: `/.well-known/incidents` (16 KiB cap) and `/api/v1/log?limit=0`
  (8 KiB cap, just the `previous` tail). Both are public on the board, so no keys are needed.
- Baselines use 5 readings per board (one per run) and a median. They need 3
  readings first, so heap stays quiet for 3 days after deploying (`heap.learning`
  is ignore by default). A finding needs the board's own check off-ok **and**
  the value below 75% of usual (heap, largest block), or 6 dB below usual (Wi-Fi).
  Known limit: a slow leak becomes the new "usual" within 5 runs. `sre_trends`
  (housemetrics) is still the leak detector.
- `heap.allocations` fires on *new* failures since the last look, not on the
  since-boot count.
- The board status in the headline and history is now the triage verdict:
  ok, warn (digest), or fail (notify or issue). Notify and issue lines start with
  "Needs you:". Routing to channels comes in sprint 2.
- Overrides live in the setting `runbook_rules` (`rule = action` lines). Sprint
  3's Runbooks tab edits that setting.
- Memory is the state key `sre_memory`, capped at 1536 bytes. When it's full,
  old readings are dropped first, then resets, then open findings.
  **Pre-existing risk:** if the whole bot record ever goes over 4000 bytes, the
  service ignores the save error (`let _ = self.save(i)`), so state silently
  stops persisting. Worth a guard in sprint 3.

## Plan (as written before building)
Overview: [sre-0-overview.md](sre-0-overview.md).

## Problem

Today's digest:

> meritledger.local fail: heap.internal: 12 KiB internal RAM free, lowest 4 KiB …
> gamematcher.local warn: reset: Last reset: crash
> housemetrics.local warn: reset: Last reset: watchdog

Every line raises the same questions: is this normal? Is it new? Do I do anything?

## Goals

1. **Read the error log.** For each board that isn't ok, or whose reset was
   `crash`/`watchdog`/`brownout`, fetch `/.well-known/incidents` (bounded, like
   health) and use `previous` (what survived the reset: counters, newest
   events, log tail). The digest then says *what* crashed, not just "crash".
2. **Classify resets honestly.**
   - `power on` = unplugged or power cut. One line ("restarted from power-on"),
     never a warning. Several boards at once → the existing correlated-outage line.
   - `crash`/`watchdog` = a fault. Show the last events and log lines from
     `previous`. Say "this is a software fault, not an unplug."
   - `brownout` → `power` runbook ("supply or cable").
   - Repeats: count resets per board over a 7-day window in bot state.
     One crash = digest. Two or more in 7 days = notify (sprint 2) and issue (sprint 6).
3. **Judge each board against its own normal.** Low heap happens all the time.
   - Keep a per-board baseline in bot state: rolling median and p10 of
     `heap.internal` free, `heap.largest`, Wi-Fi RSSI. Use housemetrics series
     when `collector` is set, otherwise the bot's own samples.
   - A finding fires only when the value is clearly below the board's own p10
     (e.g. 25% below, for 2+ samples), or the trend reaches the board's floor
     before the next quiet window (existing `sre_trends`), or `AllocationFailed`
     incidents appear. "12 KiB free, normal for meritledger (usual 10-14)" is
     *not* a finding.
   - Until there's enough history (say 3 days), report "learning baseline" and stay quiet.
   - Budget: per-board state must fit the 4000-byte NVS string limit. Store
     quantized summaries, not raw samples.
4. **A runbook table in code** maps (check id or incident kind, condition) →
   action: `ignore` | `digest` | `notify` | `issue`, plus one plain sentence on
   what to do. Defaults come from the OPS.md table. Sprint 3 makes them
   editable; this sprint stores overrides in settings so the UI has something
   to edit.
5. **Track each finding over time.** Give each finding a fingerprint
   (board + runbook + key detail, e.g. panic site), and states new / ongoing (day N) / resolved.
   The digest leads with new, then worsening, then ongoing (one line each),
   then resolved.
6. **Rewrite the digest lines** to cover what happened, whether it's normal,
   and what to do:
   > gamematcher.local crashed twice since Tue (software fault, not an unplug).
   > Last events before reset: alloc failed 4096 B; worker stalled. Runbook: crash.
   > Next: filed as an issue once the tracker exists; for now, check the log tail at
   > https://gamematcher.local/.well-known/incidents.

## Tests

- Fixture health and incident bodies for power-on, crash with `previous`,
  watchdog, and brownout. The power-on case produces no warning.
- Baseline: 3 days of stable 12 KiB → no finding. A drop to 6 KiB → finding.
  Allocation failures → a finding regardless of baseline.
- Fingerprint state survives a failed post (like `a_failed_post_keeps_the_old_statuses_for_the_retry`).
- State size test: 8 boards with full baselines stay under 4000 bytes.

## Not in this sprint

Notifications (sprint 2), UI (sprint 3), and filing issues (sprint 6).
