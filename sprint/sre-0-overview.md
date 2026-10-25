# SRE bot epic: actionable, reachable, tracked

Status: SRE sprints 1-4 and tracker sprint 1 built 2026-10-24. The bots board runs SRE 1-4; the tracker runs on the desktop until ops.local exists. Planned: tracker UI, pairing. See [platform board proposal](platform-board-proposal.md), which would change sprints 4-7.
Code: `mastomini_bots/src/bots/sre*.rs`, `mastomini_bots_ui/src/app/pages/sre-*.ts`,
board side in `microcontroller/miniframework/crates/miniframework/src/{ops,incidents}.rs`,
runbook table in `microcontroller/miniframework/spec/OPS.md`.
New app: `microcontroller/miniframework/apps/issue_tracker` (generic, P4 + SD card).

## The three problems

1. **Findings aren't actionable.** The digest repeats the board's own check
   text ("Last reset: crash", "12 KiB internal RAM free"). It doesn't say whether
   that's normal for this board, new or ongoing, or what to do. There's no
   email: the bot has no email, SMTP or ntfy code and only posts to Mastodon.
2. **Config is opaque.** 15+ settings in a pop-open panel. Keys like
   `ops_safe_key` don't say where they come from.
3. **No issue tracker.**

## What already exists (don't rebuild it)

- Boards already keep an error log at `/.well-known/incidents`. It holds
  48 events, per-kind counters, heap/Wi-Fi samples, and a `previous` section
  saved in RTC memory, so it survives a crash, watchdog or software reset
  (not power loss). `logcap.rs` keeps the log tail across a crash. **The SRE
  bot never reads any of it.** This is the "application error log" for problem #1.
- Each health check carries a `runbook` id (`crash`, `wifi`, `restart-planned`,
  `power`, `storage`, `clock`, `checkpoint`). The bot drops it.
- `reset_reason()` already tells an unplug apart from a fault. An unplug is
  `power on`; `crash` (panic) and `watchdog` are real faults. So
  "crash = probably unplugged" is not true; the bot should say so.
- `sre_trends` reads housemetrics heap series. `sre_history` keeps status history.

## Decisions (owner, 2026-10-24)

| Topic | Decision |
|---|---|
| Notify channels | Email (SMTP relay), Mastodon DM, ntfy. Tracker only when the runbook says so: bugs and actionable items, not every finding. |
| Error log | Pull. The bot polls each board's error log (`/.well-known/incidents`), which already survives a reboot. Apps add their own entries with `incidents::record`. |
| Tracker scope | REST API with per-reporter tokens, dedup by fingerprint, Angular web UI, labels/assignee/priority. Generic, not tied to the SRE bot. |
| Runbook rules | Built-in defaults, editable on the SRE page. **Low heap is relative**: alert only when a board is lower than its own normal, not below a fixed floor. |
| SRE UI | New `/sre` section in `mastomini_bots_ui` that replaces the pop-open panel. |
| Config | mDNS auto-discovery, a pairing flow per board, and every remaining key says where it comes from (with a test button). |
| Tracker storage | SD card on a new P4. |
| Paths | Tracker: `microcontroller/miniframework/apps/issue_tracker`. |

## Sprints

| # | Sprint | Needs hardware? | Depends on |
|---|---|---|---|
| 1 | [Actionable findings](sre-1-actionable.md) | no | none |
| 2 | [Notifications](sre-2-notify.md) | no | 1 |
| 3 | [SRE section of the site](sre-3-ui.md) | no | 1, 2 |
| 4 | [Tracker core (API + SD)](tracker-1-core.md) | P4 for the last step only | none; can run in parallel with 1-3 |
| 5 | [Tracker web UI + its own notifications](tracker-2-ui.md) | P4 | 4 |
| 6 | [SRE files issues](sre-4-issues.md) | P4 tracker deployed | 1, 4 |
| 7 | [Zero-paste config: discovery + pairing](sre-5-pairing.md) | reflash every board | 3 |

Pairing goes last because it changes firmware on every board (a deploy trip).
Sprints 1-3 and 6 only flash the bots board.

## Answered (2026-10-24)

- **P4 networking:** the tracker P4 will have Wi-Fi.
- **Email:** Brevo (HTTPS API, like gamematcher/meritledger). Gmail SMTP is
  deferred: it needs a raw TLS socket transport the bots firmware lacks.
- **ntfy:** ntfy.sh with a private topic.
- **Auto-close:** the bot may close an issue, especially once a finding has
  been gone for N days with N large (default 30).
- **Tracker login:** the same household auth as the other boards. SSO is
  wanted someday, not now.
- **RST pain:** several boards need RST to come back after power returns.
  Sprint 1 now detects this (`reset.hung`) and keeps the error log as evidence.
  A firmware fix belongs in miniframework once that evidence is in.
