# 06 — Household app (embedded Angular)

The Mastodon REST API has no way to create members, set passwords, manage the
server, or install trust. No third-party client can do these things either. The
household app fills that gap. A second job is **getting each person into a
real Mastodon app** and then getting out of the way.

**Scope rule:** the app does what a Mastodon client can't: account security
(devices, password, sign out everywhere), household administration, and trust
and connection help. Anything a regular client does (posting, timelines,
profile text, following) stays out.

It is an Angular build (`mastomini_ui/`, modelled on nanacoin_ui: standalone
components, signals, zoneless, lazy routes, hash routing) embedded in the
firmware and served at **`/app/`** (`src/web.rs`, `scripts/build-web.sh`,
`scripts/bundle-web.mjs`). Only gzip copies are embedded, looked up by exact
path, with ETags, `no-cache` on `index.html` and `immutable` on hashed files.
Hash routing means `/app/` is the only page, so no client-route fallback list
is needed. `--features bundled-web` includes it; the `esp32` feature implies
it. Budget: ≤ 600 KiB; the first build is 104 KiB gzipped.

## Plain HTML pages (server-rendered, no JavaScript)

The board also serves small server-rendered pages (`src/api/setup.rs`,
firmware `net.rs`), so a household never needs a terminal or an HTTP tool:

| Page | Purpose |
|---|---|
| `/setup/wifi` (setup network only) | Choose the home Wi-Fi; shows the board's new address |
| `/` before setup | Create the household: name, owner username, password |
| `/` after setup | *Set up my phone* (`/app/#/connect`), *Household app*, *Trust this server*, and short no-JavaScript connect instructions with the address the visitor used |
| `/setup/<code>` | Redeem an invite (username, optional display name, password twice) or a reset code (password twice) |
| `/setup/password` | Change your own password (username, current password, new password twice); signs out every device |

These stay server-rendered, not Angular, where JavaScript and a sign-in can't
be assumed:

- `/oauth/authorize`: every Mastodon app's in-app browser uses it.
- `/setup/wifi` and first-time setup: captive-portal browsers, and no account
  exists yet to sign in with.
- `/setup/<code>`: the invitee has no account yet, and the link must open in
  whatever browser a text message opens.
- `/setup/password`: kept as a no-sign-in fallback; cheap.

The admin forms that used to be on `/` ("Add a family member", "Invite or
reset a password", confirmed with an admin password on every submit) are gone:
the app's Members page does both, with a normal sign-in.

## Routes

Built (paths under `/app/#/`): `/connect` and `/trust` (public), `/signin`,
`/me`, `/admin/members` (including "add with a password"), `/admin/server`,
`/admin/health` (`/diag`, clock button), `/admin/security` (read-only until
Sprint 8).

| Route | Who | Purpose |
|---|---|---|
| `/` | anyone | Landing: "This is the <household> mastomini." Buttons: *Set up my phone*, *Sign in*, *Trust this server* |
| `/setup` | first boot only | Provisioning wizard: server name, owner username + password, hostname confirmation. Only available while the store is unprovisioned. It then disappears (as nanacoin's provisioning does) |
| `/setup/:code` | code holder | Redeem an invite or reset code: choose password (and username/display name for invites) |
| `/trust` | anyone, HTTP ok | Certificate install instructions per platform, fingerprint display, `/ca` download (ported from nanacoin `assets/trust.html`) |
| `/connect` | anyone | **Client setup guide** (below) |
| `/me` | member | Password change, "sign out everywhere", devices. Profile text is edited in Mastodon apps (scope rule). Avatars: see 02 "Avatars and headers" and open question 17 |
| `/me/devices` | member | Part of `/me`: signed-in apps/devices (token list), revoke |
| `/admin/members` | admin | Member list, create invite, issue reset code, disable/enable, change role, delete (typed confirmation) |
| `/admin/server` | admin | Name, description, rules, contact account |
| `/admin/security` | owner | Easy/Secure mode switch (nanacoin flow), real-domain certificate status + expiry, household CA fingerprint |
| `/admin/health` | admin | Diagnostics: heap/PSRAM, store usage and low-watermark distance, eviction count and oldest-retained-post date, write-governor counters, clock source, uptime (nanacoin's "Machine health" page, adapted) |
| ~~`/home`~~ | — | Dropped by the scope rule: a Mastodon web client (Phanpy, Elk) or app does this |

## Client setup guide (`/connect`)

It detects the platform from the User-Agent and shows the relevant path first.
Every guide ends with "Server: `mastomini.local`" (or the real domain), and a
**Copy** button.

| Platform | Recommended path | Notes to show |
|---|---|---|
| iPhone / iPad | Trust the CA (`/trust`), then Ivory / Ice Cubes / Mona / Mastodon | "Full trust" step screenshots. Needs Model A or B |
| Mac | Trust CA in Keychain (or Model B), then Ivory / Ice Cubes / Mona | |
| Android | **Model B required** for most apps. Tusky / Moshidon / Mastodon with the real domain | Explain why the household CA isn't enough, and the DNS-rebinding caveat |
| Windows / Linux desktop | Browser + household app, or Whalebird / Tuba | mastodon_mock's README has notes on which desktop clients accepted local certificates |
| Web clients (Phanpy, Elk) | Only if the browser trusts the certificate and the client is purely client-side | CORS is enabled. Server-side-rendered clients can't reach a LAN box |

The guide is data-driven (a JSON table in the bundle) so the tested-client matrix
from Phase 5 can update it without code changes.

## Authentication inside the app

The app signs in through the same `/oauth/authorize` page as any client (PKCE
S256, with a pure-TypeScript SHA-256 because `crypto.subtle` is missing on
plain HTTP), and keeps its token in `localStorage`. So it exercises the same
flow as native apps, and a bug in OAuth shows up in our own UI first.

Deviation from the first plan: instead of one first-party app registered at
provisioning, each browser registers the app with `POST /api/v1/apps` at its
first sign-in, with its own `/app/` URL as the redirect URI. The redirect URI
then always matches the address typed (IP, `mastomini.local`, or the dev
server). A stored registration is checked with a `client_credentials` grant
before each sign-in, since the server drops apps without tokens when its 32
slots fill. Scopes: `read write` (the household API checks roles, not admin
scopes).

## `/api/mastomini/v1` (household admin API)

JSON, bearer token, same error shape as the Mastodon API. Every mutation takes an
`Idempotency-Key`.

| Method & path | Role | Notes |
|---|---|---|
| `GET /status` | anyone | **done.** provisioned?, server name, version, `clock` (`synced`/`manual`/`unset`), `mode` (`http` until Sprint 8), `https`, primary hostname, `household_app`, counts |
| `POST /provision` | anyone, only while unprovisioned | creates the owner + first-party app. Single use |
| `POST /codes/redeem` | anyone with code | **done.** `{code, password, username?, display_name?}` → the member (as in `GET /admin/members`). The code is erased before the account is created or the password set, so a power cut can lose a code but never allow a second use. A reset gives the member a new key pair (old DMs unreadable) and signs out every device |
| `POST /me/password` | member | **done.** `{current, new}`. Always signs out every device, this one included: a committed password change never leaves an old session alive (the `sign_out_everywhere` flag was dropped) |
| `POST /me/sign_out_everywhere` | member | **done.** Bumps the token epoch |
| `GET /me/devices`, `DELETE /me/devices/:id` | member | **done.** Newest first: `id`, `app {name, website}`, `scopes`, `created_at`, `last_used_at` (since boot, else `null`), `current` |
| `PUT /me/avatar`, `PUT /me/header` | member | raw image body, size-checked. Chunked variant if the TLS measurement forces it (01) |
| `GET /admin/members` | admin | includes disabled/role/last-seen-since-boot |
| `POST /admin/invites` | admin | **done.** → `{id, kind, code, url, created_at, expires_at}` (show `url` as a QR code in the UI). Up to 8 live codes: expired ones go first, then the oldest |
| `POST /admin/members/:id/reset` | admin | **done.** → the same, plus `account_id` and `username`. Does **not** change the password until redeemed. Moderation rules: not yourself, not the owner, admins only by the owner. A new reset for the same member replaces the old one |
| `GET /admin/codes`, `DELETE /admin/codes/:id` | admin | **done.** Live codes without the code itself (only its hash is stored) / cancel one |
| `POST /admin/members/:id/disable` / `enable` / `silence` / `unsilence` / `suspend` / `unsuspend` | admin | **done.** Same rules as the Mastodon admin API (04 "Moderation"): not yourself, not the owner, admins only by the owner. Disable does **not** bump the token epoch: tokens answer `403` while disabled and work again once enabled, as on Mastodon |
| `POST /admin/members/:id/role` | owner | **done.** `{role: "admin" \| "member"}` |
| `DELETE /admin/members/:id` | admin | **done.** `{confirm: "<username>"}`; tombstone + purge (02). Unlike the Mastodon admin API, no prior suspension is needed: the typed confirmation is the safeguard |
| `DELETE /admin/statuses/:id` | admin | **done.** Cannot target DMs the admin isn't party to (they get `404`) |
| `GET/PUT /admin/server` | admin | **done.** `title`, `description`, `rules[]` (≤ 8 × 140 B), `terms` (≤ 3,000 B plain text; `""` restores the generated terms). Changing the terms resets their effective date |
| `POST /admin/transport` | owner, HTTPS only | Easy/Secure (nanacoin semantics) |
| `POST /tls`, `DELETE /tls` | owner, HTTPS only | upload/remove real-domain certificate |
| `POST /clock` | admin | **done.** `{ms}`; `409` once synced, `422` if not later than the newest record (05 "Time") |
| `GET /diag` | admin | **done.** Platform (target, uptime, internal/PSRAM heap free and low-water, reset reason, Wi-Fi RSSI; the board fills these through `Ctx::platform`), clock, store (entries, watermark, evictions, oldest post, boot repairs), record counts against limits, governor. The public summary is `GET /status`; no separate `/diag/static` |
| `GET /admin/security` | owner | **done.** Transport (mode, https), certificate and CA (`null` until Sprint 8), password policy, app/token use, per member: role, devices, whether they have a DM key |
| `GET /admin/export` (later) | owner | streaming JSON (or HTML) archive of all retained posts for keeping before eviction |
