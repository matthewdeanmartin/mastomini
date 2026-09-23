# Open questions

Each question has a proposed answer. The spec assumes that answer unless
someone overrides it.

| # | Question | Proposed answer | Affects |
|---|---|---|---|
| 3 | Version string to advertise: 4.3.0 or 4.4.x? | 4.3.0: enough for PKCE/OAuth metadata and grouped notifications, with fewer 4.4+ features clients might try | 04 |
| 6 | Should `public` posts be visible without signing in (e.g. a kitchen-display tablet)? | No in v1. A later "read-only display token" would be cleaner than anonymous access | 04 |
| 7 | Does any client choke on `max_media_attachments: 0`? | Unknown. Measure in Phase 5. If needed, advertise 1 and reject uploads with a clear `422` | 04 |
| 8 | Hostname: `mastomini.local` fixed, or build-time configurable? | Build-time configurable, default `mastomini` | 01, 05 |
| 9 | Should the admin be able to delete others' posts? | Yes for public/private posts. Never for DMs they aren't in | 06 |
| 10 | OTA updates over Wi-Fi? | Not in v1 (needs a second 4 MiB app slot, which would come out of `store`). USB deploys as in nanacoin | 01 |
| 11 | Export format for posts before they roll off | JSON (Mastodon `Status` array) + a static HTML page, streamed | roadmap Phase 6 |
| 12 | Flash + NVS encryption? | Not in v1. Revisit once USB recovery flows are settled | 05 |
| 13 | Polls in scope at all? | Later phase. Cheap with bitmask votes, and fun for "what's for dinner" | 04 |
| 14 | Streaming: worth a TLS socket or two? | Later. Measure what the chosen clients do without it first | 04 |

## Resolved

| # | Question | Decision |
|---|---|---|
| 1 | NVS entity store vs nanacoin's event journal | Entity store. nanacoin is an accounting ledger; mastomini needs no accounting structures |
| 2 | Share nanacoin's CA | Own CA by default. `MASTOMINI_CA_DIR` makes it easy to sign with any existing household CA (05) |
| 4 | Short passwords/PINs | Allowed, minimum 4 characters. Being on the household Wi-Fi is the second factor |
| 5 | Everyone follows everyone by default | No. Follows work exactly as on Mastodon |
| — | Ephemeral state (markers, auth codes, counters, idempotency keys) | RAM/PSRAM only, never flash (02) |
