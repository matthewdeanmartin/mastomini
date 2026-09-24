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
| Discovery | `GET /api/v1/instance`, `GET /api/v2/instance` (with `configuration.urls.about`, `terms_of_service`, `privacy_policy`), `GET /.well-known/oauth-authorization-server`, `GET /.well-known/nodeinfo` + `/nodeinfo/2.0`, `GET /api/v1/instance/rules`, `/extended_description`, `/terms_of_service` (+ `/:date`), `/privacy_policy` |
| About pages (HTML, no sign-in) | `GET /about` (description, rules, admins), `/terms-of-service`, `/privacy-policy` |
| Apps & OAuth | `POST /api/v1/apps`, `GET /api/v1/apps/verify_credentials`, `GET /oauth/authorize` (HTML), `POST /oauth/authorize` (form), `POST /oauth/token` (`authorization_code` with/without PKCE, `client_credentials`), `POST /oauth/revoke` |
| Me | `GET /api/v1/accounts/verify_credentials` (with `source`), `PATCH /api/v1/accounts/update_credentials` (display name, note, fields, locked, bot, avatar, header, source.privacy/sensitive/language) |
| Accounts | `GET /api/v1/accounts/:id`, `/:id/statuses` (with `pinned`, `exclude_replies`, `exclude_reblogs`, `only_media`, `tagged`), `/:id/followers`, `/:id/following`, `GET /api/v1/accounts/relationships`, `GET /api/v1/accounts/lookup`, `GET /api/v1/accounts/search` |
| Statuses | `POST /api/v1/statuses`, `GET/DELETE /api/v1/statuses/:id`, `GET /:id/context`, `POST /:id/favourite`, `/unfavourite`, `/reblog`, `/unreblog`, `/bookmark`, `/unbookmark`, `/pin`, `/unpin`, `GET /:id/favourited_by`, `/reblogged_by` |
| Timelines | `GET /api/v1/timelines/home`, `/public` (`local` is implied), `/tag/:hashtag` |
| Notifications (RAM only, see 02) | `GET /api/v1/notifications` (types/exclude_types/account_id), `GET /:id`, `POST /clear`, `POST /:id/dismiss`, `GET /api/v1/notifications/unread_count` |
| Markers | `GET/POST /api/v1/markers` (RAM only, reset on reboot) |
| Prefs & misc | `GET /api/v1/preferences`, `GET /api/v1/custom_emojis` (`[]`), `GET /api/v1/announcements` (`[]` or admin announcements, later), `GET /api/v1/followed_tags` (`[]`) |
| Search | `GET /api/v2/search` (accounts, hashtags, statuses: linear scan over RAM), `resolve=true` never goes off-box |

## Tier 2 — commonly used

| Area | Endpoints |
|---|---|
| Relationships | follow/unfollow (with `reblogs`, `notify`), `remove_from_followers`, **block/unblock, mute/unmute (with `notifications`, `duration`), `GET /api/v1/blocks`, `/mutes` (done, see "Moderation")**, **follow requests (done): following a locked account asks first; `GET /api/v1/follow_requests`, `POST /:account_id/authorize`/`reject`, `follow_request` notifications, `Relationship.requested`/`requested_by`, `source.follow_requests_count`; unlocking lets everyone waiting in (as on Mastodon)**, `POST /api/v1/accounts/:id/note` (RAM + store, tiny) |
| Statuses | **`PUT /api/v1/statuses/:id` (edit: text, content warning, sensitive, language, poll), `GET /:id/history` (up to 3 previous versions, oldest first, then the current one), `GET /:id/source` (done).** People who boosted a post get an `update` notification; newly mentioned people a `mention`. Direct messages are re-encrypted on edit and keep no history. **`POST /:id/mute`/`unmute` (conversation mute, done)** |
| Lists | **Done.** CRUD, `replies_policy` (`followed`/`list`/`none`), `exclusive` (members' posts stay off home), `GET/POST/DELETE /:id/accounts` (only people you follow, as on Mastodon; unfollowing removes them), `GET /api/v1/accounts/:id/lists`, `GET /api/v1/timelines/list/:id` |
| Filters | **Done.** v2 CRUD with `keywords_attributes`, `/:id/keywords`, `/keywords/:id`, `/:id/statuses`, `/statuses/:id`; v1 as a view (one v1 filter per keyword). Matching statuses get `Status.filtered` (keyword and status matches) in the filter's contexts; nothing is removed server-side, clients warn, blur or hide as on Mastodon. Expired filters stop matching. Direct messages match on the reader's decrypted text |
| Favourites / bookmarks | `GET /api/v1/favourites`, `/bookmarks` (paginated by reaction record id, as Mastodon does) |
| Conversations | **Done.** `GET /api/v1/conversations`, `POST /:id/read`, `DELETE /:id`. Derived from `direct` statuses: a conversation is a thread of direct messages, named by its first message. Read and hidden state are RAM only, like markers (a restart shows everything as read); a hidden conversation comes back when a new message arrives |
| Tags | `GET /api/v1/tags/:name`, follow/unfollow, featured tags (`[]`) |
| Grouped notifications | `GET /api/v2/notifications`, `/api/v2/notifications/unread_count` (mastodon_mock has the grouping logic) |
| Trends | `GET /api/v1/trends/tags`, `/trends/statuses`, `/trends/links` all return `[]`. No feed algorithms |
| Suggestions / directory | `GET /api/v2/suggestions` (household members you don't follow), `GET /api/v1/directory` |

## Tier 3 — later or never

| Endpoint | Plan |
|---|---|
| Streaming (`/api/v1/streaming`, WebSocket + SSE) | **Later.** `configuration.urls.streaming` (v2) and `urls.streaming_api` (v1) are `null`, the standard "no streaming" signal: clients such as Elk then open no WebSockets. `/api/v1/streaming/health` is 404, never `OK`. A client that ignores the signal and connects anyway is refused by ESP-IDF's HTTP server itself (400, about 30 ms, before any app code: WebSocket support is not compiled in). Measured on the board (2026-09-23): refused attempts mixed with API calls stay at median 40–75 ms up to 4 simultaneous connections; at 8 simultaneous connects a few requests wait 1 or 3 s (TCP SYN retries after the listen backlog of 5 overflows). Raising the HTTP sockets from 6 to 10 changed nothing, so it stays 6. When implemented: 2 concurrent streams max, user + public only; it holds a socket (a TLS session once HTTPS exists), so revisit the socket budget |
| Polls | **Done** (moved up). `poll[options][]` (2–4, ≤ 50 characters, distinct), `poll[expires_in]` (5 minutes to 7 days), `multiple`, `hide_totals`; `GET /api/v1/polls/:id`, `POST /:id/votes`. Votes are a `u16` bitmask per option, so one write per vote. No voting on your own poll, once only. Not allowed in direct messages (the options would be unencrypted). When a poll ends, author and voters get a `poll` notification (RAM, checked on each request). Editing the options resets the votes |
| Scheduled statuses | Never in v1 (`[]` from `GET /api/v1/scheduled_statuses`). Needs a timer and a valid clock |
| Media (`/api/v1/media`, `/api/v2/media`) | `422` "media attachments are not supported"; `configuration.media_attachments` advertises `max_media_attachments: 0` if clients tolerate it (check in the client matrix) |
| Push (`/api/v1/push/subscription`) | `404` in v1. Later maybe, if the board has internet access |
| Account creation (`POST /api/v1/accounts`) | `registrations: false`. Returns `403`. Members are created in the household app |
| Reports | **Done**, see "Moderation" |
| Admin API (`/api/v1/admin/*`) | **Accounts and reports only**, see "Moderation". Domain/e-mail/IP blocks, trends, measures, dimensions, retention and announcements: `404` (no federation, no sign-ups, no analytics) |
| Collections | **Done** (Mastodon 4.6), see "Collections" |
| Translation, preview cards, quotes, annual reports, domain blocks, endorsements | No. Neutral values in entities |

## Moderation

Everything here persists (spec/02 `mm_rel`, `mm_rx`, `mm_mod`) and survives a
reboot. Notifications stay RAM only.

**Blocks** (`POST /api/v1/accounts/:id/block`, `/unblock`, `GET /api/v1/blocks`).
Blocking ends follows in both directions and drops the notifications the two
exchanged. The blocked member can't follow the blocker again, and can't see the
blocker's posts at all (`404`, as Mastodon's `StatusPolicy`). The blocker can
still open a blocked member's post by id, but neither sees the other on the home,
public or tag timelines, in threads, in search, in suggestions or in the
directory. `Relationship.blocking` / `blocked_by` are real.

**Mutes** (`/mute` with `notifications` (default `true`) and `duration` in
seconds (`0` = forever), `/unmute`, `GET /api/v1/mutes` with `mute_expires_at`).
The muted member disappears from the muter's timelines and threads, and from
their notifications unless `notifications=false`. Nothing changes for the muted
member. Expired mutes simply stop applying.

**Conversation mutes** (`POST /api/v1/statuses/:id/mute`, `/unmute`). Stored as a
reaction record on the oldest status of the thread still in flash, so a whole
thread goes quiet. `Status.muted` is real. A thread mute suppresses every
notification about a status in that thread.

Timeline filtering follows Mastodon's feed filter: an entry is hidden when its
author, its booster or anyone it mentions is blocked or muted by the viewer (the
viewer's own posts are always kept).

**Reports** (`POST /api/v1/reports`, `GET /api/v1/reports` for your own). A report
names a member, up to 10 of their posts the reporter can see, a comment (500
characters), a category (`spam`, `legal`, `violation`, `other`; `violation` when
rule ids are given) and rule ids (1-based, as `/api/v1/instance/rules`). Every
admin gets an `admin.report` notification carrying the `Report`. The reported
member is not told. 32 reports are kept; when full, the oldest resolved report
is dropped, and if none is resolved a new report gets `422`.

**Account moderation**, by admins, through the Mastodon admin API or the
household API (spec/06). Nobody moderates themselves or the owner, and only the
owner moderates admins.

| Action | Effect |
|---|---|
| `disable` / `enable` | Can't sign in; every existing token answers `403 "Your login is currently disabled"` until enabled, then the member's devices work again without signing in |
| `silence` / `unsilence` | Mastodon's "limited": off the public and tag timelines, and no notifications, for members who don't follow them. `Account.limited: true` |
| `suspend` / `unsuspend` | Can't sign in (tokens `403`), all their posts hidden from everyone, profile blanked with `Account.suspended: true`. Reversible |
| `sensitive` / `unsensitive` | Everyone but the author sees their posts as `sensitive: true` (Mastodon's `StatusSerializer`) |
| delete | Only after `suspend`, as Mastodon's `AccountPolicy#destroy?`. Tombstone first, then everything the account made is purged; boot finishes an interrupted purge (spec/02) |
| delete a post | Household API only. Never a direct message the admin isn't part of |

Admin API endpoints: `GET /api/v1/admin/accounts` (v1 filters) and
`/api/v2/admin/accounts` (`origin`, `status`, `permissions`, `username`,
`display_name`), `GET /api/v1/admin/accounts/:id`, `POST /:id/action`
(`type`, `report_id`), `/enable`, `/unsilence`, `/unsuspend`, `/unsensitive`,
`/approve` and `/reject` (`403`: nobody is ever pending), `DELETE /:id`;
`GET /api/v1/admin/reports` (`resolved`, `account_id`, `target_account_id`),
`GET`/`PUT /:id`, `/assign_to_self`, `/unassign`, `/resolve`, `/reopen`. They need
an admin account **and** an `admin:read[:…]` / `admin:write[:…]` scope.
`Admin::Account.email` is `""` and `ip` is `null`: neither exists here.
`Admin::Report.statuses` leaves out direct messages the admin isn't part of
(the household rule wins over Mastodon's; see open question #15).

## Collections

Mastodon 4.6 collections: a member's curated list of other members, like a
starter pack. Response shapes follow docs.joinmastodon.org/methods/collections.

| Endpoint | Returns |
|---|---|
| `POST /api/v1/collections` (`name`, `description`, `discoverable`, `sensitive`, `language`, `tag_name`, `account_ids[]`) | `{"collection": Collection}` |
| `GET /api/v1/collections/:id` | `{"collection": Collection, "accounts": [curator, featured…]}` |
| `PATCH /api/v1/collections/:id` | `{"collection": Collection}` |
| `DELETE /api/v1/collections/:id` | `{}` |
| `POST /api/v1/collections/:id/items` (`account_id`) | `{"collection_item": CollectionItem}` |
| `DELETE /api/v1/collections/:id/items/:item_id` | `{}` |
| `POST /api/v1/collections/:id/items/:item_id/revoke` | `{}` |
| `GET /api/v1/accounts/:id/collections`, `/in_collections` (`limit` ≤ 80, `offset`) | `{"collections": [...]}` |

Household rules:

- **Auto-accept.** Items are `accepted` immediately. The featured member gets an
  `added_to_collection` notification and can revoke. A revoked item stays (state
  `revoked`, visible only to the curator) so the curator can't simply add them
  back. Deleting a revoked item does nothing.
- Only members with `discoverable: true` can be featured. Nobody features
  themselves. Suspended members and members blocked either way can't be added,
  and drop out of existing collections for everyone but the curator.
- `discoverable: false` collections are left out of `accounts/:id/collections`
  (except for the curator) but can still be opened by id.
- Scopes `read:collections` / `write:collections` (covered by `read` / `write`).
- `item_count` counts accepted items. `local` is always `true`.
- mastomini still advertises `api_versions.mastodon: 2`, so clients that gate
  collections on API version 10 won't show them yet (open question #16).

## Semantics that differ from Mastodon

- **No remote anything.** `acct` is always the bare username. `url` / `uri`
  are `https://<host>/@<username>` and `https://<host>/@<username>/<id>`. The server
  serves no HTML for these URLs except a redirect to the household app.
- **Rules and terms.** A new household starts with three rules (be patient, the
  microcontroller is slow; don't unplug it; obey the laws where it physically
  is). Without admin-written terms, `terms_of_service` is generated from the
  server name and rules, effective from the day the household was set up. The
  privacy policy is generated and describes what is stored and that direct
  messages are encrypted.
- **Direct messages are encrypted** (05 "Direct messages"). They are not
  searchable, and a participant on a device signed in before encryption sees a
  "sign in again" placeholder instead of the text.
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
`in_reply_to_id`, and `poll[...]` (not with `direct`). `media_ids[]` must be empty
(`422` otherwise), and `scheduled_at` gives `422`.

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
