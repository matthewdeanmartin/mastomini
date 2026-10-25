# SRE sprint 5: zero-paste config (discovery and pairing)

Status: planned, last. Changes miniframework firmware, so **every board gets
reflashed**: plan a deploy trip, one board first, then check `/api/v1/status`.

## Goals

1. **mDNS discovery**: boards with the `ops` feature advertise
   `_miniops._tcp` (TXT: app, build, ops version). The bots board browses it, and
   the Fleet tab shows "discovered, not monitored" boards with an Add button.
   The hand-typed `targets` list stays, but only as an override.
2. **A pairing flow per board**, replacing pasted `ops_safe_key` and `ops_approval_key`:
   - On `/sre`, click **Pair**. The bot calls `POST /.well-known/ops/pair` on
     the board, which shows a 6-digit code on its own web page (and LED blink
     if it has one).
   - You confirm the code on the board's page, while signed in as that board's admin.
   - The board issues scoped keys (safe and approval) to the bot over HTTPS,
     bound to the bot's identity. They're stored as secrets in the bots board.
     Revocable from the board's page.
   - Approval keys still need the existing per-action approval. Pairing grants
     the ability to ask, not to act.
3. Any secret not covered by pairing (Mastodon admin read key, SMTP, ntfy,
   tracker token) keeps sprint 3's "where it comes from" text and test button.

## Risks

- Mixed fleets during rollout: unpaired boards keep working with pasted keys.
- TLS trust: pairing runs over the household CA. Check the bots board trusts each board's certificate first.

## Tests

- Desktop: pair, confirm, receive keys, revoke → 401 on the next action.
- A wrong code or an expired code (2 minutes) is refused. A second pairing replaces the first.
- Discovery fixture: three advertised boards, one already monitored.
