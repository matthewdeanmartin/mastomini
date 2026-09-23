# 04 — Mastodon API surface

## Principles

1. **Same wire shapes as Mastodon.** Entities (`Account`, `Status`,
   `Notification`, `Relationship`, `Instance`, …) are serialized with every field
   Mastodon defines, so strictly typed clients (Swift `Codable`, Kotlin
   serialization) decode them without errors. When a feature doesn't exist here,
   send its neutral value (`media_attachments: []`, `emojis: []`, `card: null`),
   never omit a required field.
2. **Reference implementation: `mimb/mastodon_mock`.** Its routers, serializers,
   pagination and notification side effects are the behavioural oracle. Its
   `spec/03-api-coverage.md` is the endpoint checklist. The shapes are checked against
   the pinned `mastodon-openapi` schema, as `mastodon_mock/openapi_compare.py` does.
3. **Tiers.** Tier 1 is what clients need to start up and use the timeline. Tier 2 is
   what they touch often enough that a `404` looks broken. Tier 3 is later or never.
   Anything unimplemented returns a Mastodon-shaped `404 {"error":"Record not found"}`
   or, for list endpoints clients poll, `200 []`, whichever the client matrix shows
   degrades best.
4. **Authentication required everywhere** except: instance metadata, OAuth,
   `/.well-known/*`, `/api/v1/custom_emojis`, avatar/header/static files, and
   `/trust` + `/ca`. The public and tag timelines require a token. Real Mastodon
   servers can also require sign-in for timelines. Advertise that in
   `/api/v2/instance` using whichever field the pinned `mastodon-openapi` schema
   defines for it (check the name during Phase 2 rather than guessing).

## Version advertised

`/api/v1/instance.version = "4.3.0 (compatible; mastomini 0.1.0)"` and
`/api/v2/instance.api_versions = {"mastodon": 2}`. Clients gate features on
the version string. Claiming 4.3 enables OAuth server metadata and grouped
notifications detection. It does not make clients try quote posts or other 4.4+
features we don't implement. (Open question: 4.3 vs 4.4.)

## Tier 1 — required for any client to work

| Area | Endpoints |
|---|---|
| Discovery | `GET /api/v1/instance`, `GET /api/v2/instance`, `GET /.well-known/oauth-authorization-server`, `GET /.well-known/nodeinfo` + `/nodeinfo/2.0`, `GET /api/v1/instance/rules` |
| Apps & OAuth | `POST /api/v1/apps`, `GET /api/v1/apps/verify_credentials`, `GET /oauth/authorize` (HTML), `POST /oauth/authorize` (form), `POST /oauth/token` (`authorization_code` with/without PKCE, `client_credentials`), `POST /oauth/revoke` |
| Me | `GET /api/v1/accounts/verify_credentials` (with `source`), `PATCH /api/v1/accounts/update_credentials` (display name, note, fields, locked, bot, avatar, header, source.privacy/sensitive/language) |
| Accounts | `GET /api/v1/accounts/:id`, `/:id/statuses` (with `pinned`, `exclude_replies`, `exclude_reblogs`, `only_media`, `tagged`), `/:id/followers`, `/:id/following`, `GET /api/v1/accounts/relationships`, `GET /api/v1/accounts/lookup`, `GET /api/v1/accounts/search` |
| Statuses | `POST /api/v1/statuses`, `GET/DELETE /api/v1/statuses/:id`, `GET /:id/context`, `POST /:id/favourite`, `/unfavourite`, `/reblog`, `/unreblog`, `/bookmark`, `/unbookmark`, `/pin`, `/unpin`, `GET /:id/favourited_by`, `/reblogged_by` |
| Timelines | `GET /api/v1/timelines/home`, `/public` (`local` is implied), `/tag/:hashtag` |
| Notifications (RAM only, see 02) | `GET /api/v1/notifications` (types/exclude_types/account_id), `GET /:id`, `POST /clear`, `POST /:id/dismiss`, `GET /api/v1/notifications/unread_count` |
| Markers | `GET/POST /api/v1/markers` (RAM only, reset on reboot) |
| Prefs & misc | `GET /api/v1/preferences`, `GET /api/v1/custom_emojis` (`[]`), `GET /api/v1/announcements` (`[]` or admin announcements, later), `GET /api/v1/filters` + `/api/v2/filters` (`[]` until Tier 2), `GET /api/v1/lists` (`[]` until Tier 2), `GET /api/v1/followed_tags` |
| Search | `GET /api/v2/search` (accounts, hashtags, statuses: linear scan over RAM), `resolve=true` never goes off-box |

## Tier 2 — commonly used

| Area | Endpoints |
|---|---|
| Relationships | follow/unfollow (with `reblogs`, `notify`), block/unblock, mute/unmute, `GET /api/v1/blocks`, `/mutes`, follow requests (only if `locked` accounts are allowed; household default is unlocked), `POST /api/v1/accounts/:id/note` (RAM + store, tiny) |
| Statuses | `PUT /api/v1/statuses/:id` (edit), `GET /:id/history`, `GET /:id/source`, `POST /:id/mute`/`unmute` (conversation mute, bitmask) |
| Lists | full CRUD + `accounts` add/remove, `GET /api/v1/timelines/list/:id` |
| Filters | v2 CRUD + keywords, applied server-side as `Status.filtered` |
| Favourites / bookmarks | `GET /api/v1/favourites`, `/bookmarks` (paginated by reaction record id, as Mastodon does) |
| Conversations | `GET /api/v1/conversations`, `POST /:id/read`, `DELETE /:id` (derived from `direct` statuses) |
| Tags | `GET /api/v1/tags/:name`, follow/unfollow, featured tags (`[]`) |
| Grouped notifications | `GET /api/v2/notifications`, `/api/v2/notifications/unread_count` (mastodon_mock has the grouping logic) |
| Trends | `GET /api/v1/trends/tags`, `/trends/statuses`, `/trends/links` all return `[]`. No feed algorithms |
| Suggestions / directory | `GET /api/v2/suggestions` (household members you don't follow), `GET /api/v1/directory` |

## Tier 3 — later or never

| Endpoint | Plan |
|---|---|
| Streaming (`/api/v1/streaming`, WebSocket + SSE) | **Later.** `configuration.urls.streaming` (v2) and `urls.streaming_api` (v1) are `null`, the standard "no streaming" signal: clients such as Elk then open no WebSockets. `/api/v1/streaming/health` is 404, never `OK`. A client that ignores the signal and connects anyway is refused by ESP-IDF's HTTP server itself (400, about 30 ms, before any app code: WebSocket support is not compiled in). Measured on the board (2026-09-23): refused attempts mixed with API calls stay at median 40–75 ms up to 4 simultaneous connections; at 8 simultaneous connects a few requests wait 1 or 3 s (TCP SYN retries after the listen backlog of 5 overflows). Raising the HTTP sockets from 6 to 10 changed nothing, so it stays 6. When implemented: 2 concurrent streams max, user + public only; it holds a socket (a TLS session once HTTPS exists), so revisit the socket budget |
| Polls | Later phase. Votes are a `u16` bitmask per option |
| Scheduled statuses | Never in v1 (`[]` from `GET /api/v1/scheduled_statuses`). Needs a timer and a valid clock |
| Media (`/api/v1/media`, `/api/v2/media`) | `422` "media attachments are not supported"; `configuration.media_attachments` advertises `max_media_attachments: 0` if clients tolerate it (check in the client matrix) |
| Push (`/api/v1/push/subscription`) | `404` in v1. Later maybe, if the board has internet access |
| Account creation (`POST /api/v1/accounts`) | `registrations: false`. Returns `403`. Members are created in the household app |
| Reports | Later maybe: a report becomes a notification to the admin |
| Admin API (`/api/v1/admin/*`) | No. The household app uses `/api/mastomini/v1` instead |
| Translation, preview cards, quotes, collections, annual reports, domain blocks, endorsements | No. Neutral values in entities |

## Semantics that differ from Mastodon

- **No remote anything.** `acct` is always the bare username. `url` / `uri`
  are `https://<host>/@<username>` and `https://<host>/@<username>/<id>`. The server
  serves no HTML for these URLs except a redirect to the household app.
- **Visibility inside a household.** `public` and `unlisted` both mean "every
  signed-in household member". The difference only affects whether the post appears
  on the public/local timeline, as in Mastodon. `private` means followers only.
  `direct` means mentioned members only. Direct messages are never shown to the
  admin through any API, including the household app. (They are not encrypted on
  flash, see 05.)
- **Follows work as on Mastodon.** A new account follows nobody. Members follow
  each other themselves; the household app's member directory and
  `/api/v2/suggestions` make that one tap.
- **`locked` accounts** are supported (follow requests), but the household app does
  not encourage them.
- **Deletion of the parent** leaves `in_reply_to_id` pointing to a missing status, as
  in Mastodon. `context` skips missing ancestors.
- **Eviction is invisible to the API.** Evicted statuses simply `404`.

## Status creation

`POST /api/v1/statuses` accepts both form and JSON bodies (clients use both), and
`status`, `spoiler_text`, `sensitive`, `visibility`, `language`,
`in_reply_to_id`. `media_ids[]` must be empty (`422` otherwise), `poll[...]` gives
`422` until polls exist, and `scheduled_at` gives `422`.

Processing order: auth → governor check → parse/validate (length, visibility,
reply target visible to author) → idempotency lookup (RAM, 1 h, per account + key)
→ allocate id → eviction if needed → `set` status → update RAM → build the response.

Rendered `content` is produced at read time from the stored text. That
covers HTML escaping, `<p>`/`<br>` from newlines, linkified URLs (`rel="nofollow
noopener" target="_blank"`, display text truncated as Mastodon does), mentions as
`<span class="h-card"><a class="u-url mention" …>@<span>user</span></a></span>`,
and hashtags as `<a class="mention hashtag" …>#<span>tag</span></a>`. The mention and
tag markup must match Mastodon's exactly, because clients parse these classes to make
links tappable.

## Pagination

Identical to Mastodon and mastodon_mock: `max_id`, `since_id`, `min_id`,
`limit`, a `Link` header with `next`/`prev`, and an opaque ID ordering. For
favourites and bookmarks the cursor is the reaction record id, not the status id
(same as Mastodon). Pages are computed by scanning RAM indexes newest-first.
Sizes this small need no secondary indexes.

## Errors

`{"error": "..."}` with Mastodon's status codes: `401` missing or invalid token
(`WWW-Authenticate: Bearer`), `403` wrong scope or disabled account, `404`,
`422` validation (`Validation failed: …`), `429` governor/rate limit with
`X-RateLimit-*` headers, `503` while the clock is invalid (writes only) or the
store is latched unavailable.

## CORS

As on Mastodon: `Access-Control-Allow-Origin: *` on `/api/*`, `/oauth/token`,
`/oauth/revoke` and `/.well-known/*`, with no credentials, so that browser clients
such as Phanpy or Elk, running from their own origin, can call the board if the
browser trusts its certificate.
