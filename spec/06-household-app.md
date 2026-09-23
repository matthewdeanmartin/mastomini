# 06 — Household app (embedded Angular)

The Mastodon REST API has no way to create members, set passwords, manage the
server, or install trust. No third-party client can do these things either. The
household app fills that gap. A second job is **getting each person into a
real Mastodon app** and then getting out of the way.

It is an Angular production build embedded in the firmware and served from flash
(nanacoin's `web.rs` / `build-web.sh` pipeline: gzip, ETags, immutable hashed
assets, known-route fallback to `index.html`). Budget: ≤ 600 KiB gzipped.

## Already built: plain HTML setup pages

Before the Angular app exists, the board serves small server-rendered pages
(`src/api/setup.rs`, firmware `net.rs`), so a household never needs a
terminal or an HTTP tool:

| Page | Purpose |
|---|---|
| `/setup/wifi` (setup network only) | Choose the home Wi-Fi; shows the board's new address |
| `/` before setup | Create the household: name, owner username, password |
| `/` after setup | How to connect a Mastodon app (with the address the visitor used), member list, "Add a family member" (admin password required) |

The Angular routes below replace these over time; the Wi-Fi page stays in
firmware because it runs before the board is on the network.

## Routes

| Route | Who | Purpose |
|---|---|---|
| `/` | anyone | Landing: "This is the <household> mastomini." Buttons: *Set up my phone*, *Sign in*, *Trust this server* |
| `/setup` | first boot only | Provisioning wizard: server name, owner username + password, hostname confirmation. Only available while the store is unprovisioned. It then disappears (as nanacoin's provisioning does) |
| `/setup/:code` | code holder | Redeem an invite or reset code: choose password (and username/display name for invites) |
| `/trust` | anyone, HTTP ok | Certificate install instructions per platform, fingerprint display, `/ca` download (ported from nanacoin `assets/trust.html`) |
| `/connect` | anyone | **Client setup guide** (below) |
| `/me` | member | Profile (display name, bio, fields), avatar/header upload with client-side crop + resize, password change, "sign out everywhere" |
| `/me/devices` | member | Signed-in apps/devices (token list), revoke |
| `/admin/members` | admin | Member list, create invite, issue reset code, disable/enable, change role, delete (typed confirmation) |
| `/admin/server` | admin | Name, description, rules, contact account |
| `/admin/security` | owner | Easy/Secure mode switch (nanacoin flow), real-domain certificate status + expiry, household CA fingerprint |
| `/admin/health` | admin | Diagnostics: heap/PSRAM, store usage and low-watermark distance, eviction count and oldest-retained-post date, write-governor counters, clock source, uptime (nanacoin's "Machine health" page, adapted) |
| `/home` (Phase 6, optional) | member | Minimal timeline + composer for people who won't install an app. Deliberately tiny: home timeline, post, reply, favourite |

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

The app registers itself as a first-party OAuth application at provisioning time and
signs in through the same `/oauth/authorize` page as any client (PKCE,
`redirect_uri = https://<host>/auth/callback`). It stores the token in
`localStorage`. So it exercises the same flow as native apps, and a bug in OAuth
shows up in our own UI first.

## `/api/mastomini/v1` (household admin API)

JSON, bearer token, same error shape as the Mastodon API. Every mutation takes an
`Idempotency-Key`.

| Method & path | Role | Notes |
|---|---|---|
| `GET /status` | anyone | provisioned?, server name, version, clock status, mode, primary hostname |
| `POST /provision` | anyone, only while unprovisioned | creates the owner + first-party app. Single use |
| `POST /codes/redeem` | anyone with code | `{code, password, username?, display_name?}` |
| `POST /me/password` | member | `{current, new, sign_out_everywhere: bool}` |
| `POST /me/sign_out_everywhere` | member | bumps token epoch |
| `GET /me/devices`, `DELETE /me/devices/:id` | member | token list / revoke |
| `PUT /me/avatar`, `PUT /me/header` | member | raw image body, size-checked. Chunked variant if the TLS measurement forces it (01) |
| `GET /admin/members` | admin | includes disabled/role/last-seen-since-boot |
| `POST /admin/invites` | admin | → one-time code + URL (show as QR code in the UI) |
| `POST /admin/members/:id/reset` | admin | → one-time reset code. Does **not** change the password until redeemed |
| `POST /admin/members/:id/disable` / `enable` / `role` | admin | disable bumps token epoch |
| `DELETE /admin/members/:id` | admin | `{confirm: "<username>"}`; tombstone + purge (02) |
| `DELETE /admin/statuses/:id` | admin | cannot target DMs the admin isn't party to |
| `GET/PUT /admin/server` | admin | server settings |
| `POST /admin/transport` | owner, HTTPS only | Easy/Secure (nanacoin semantics) |
| `POST /tls`, `DELETE /tls` | owner, HTTPS only | upload/remove real-domain certificate |
| `POST /clock` | admin | only while clock is unsynced (05) |
| `GET /diag`, `GET /diag/static` | admin (diag summary: anyone) | as nanacoin |
| `GET /admin/export` (later) | owner | streaming JSON (or HTML) archive of all retained posts for keeping before eviction |
