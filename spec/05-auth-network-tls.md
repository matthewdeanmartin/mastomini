# 05 — Authentication, network and TLS

## Accounts and roles

| Role | Can |
|---|---|
| `owner` (exactly one at a time, created by the setup wizard) | everything `admin` can do, plus transfer ownership and change TLS/transport settings |
| `admin` | create/disable/delete members, issue invite and reset codes, edit server settings, delete any post (not read DMs: they are encrypted, see "Direct messages") |
| `member` | ordinary Mastodon use, edit own profile/password, manage own devices |

There is always at least one enabled owner, enforced in the domain (nanacoin
enforces the same rule for its last active Nana).

## Passwords

- PBKDF2-HMAC-SHA256 verifiers with per-account salt, ported from `nanacoin_rs/src/auth.rs`.
  Work factor: nanacoin uses 1,000 rounds for ESP32 latency. mastomini checks a
  password only on OAuth login (not on each request), so it can afford more. Target
  ~250 ms per verification on the board. That number is measured and then fixed in
  the roadmap. Stored verifiers include the round count, so it can be raised later.
- Minimum length is 4 characters. The policy is relaxed on purpose: to reach the
  board at all you have to be on the household network, so knowing the Wi-Fi
  password is effectively the second factor.
- 5 failed attempts lock the username for 5 minutes (RAM counter, as nanacoin).
- Nobody sets passwords for other people. An admin issues a **one-time code**
  (invite for new members, reset for existing ones). The member opens
  `https://<host>/setup/<code>` in the household app and chooses their own
  password. Codes are stored hashed, expire in 7 days, and are single use.

## OAuth (what Mastodon clients do)

Clients follow the standard Mastodon flow. mastomini implements it faithfully:

1. `POST /api/v1/apps` → client id + secret (secret stored hashed). Rate-limited
   to 10/hour per source IP. The 32-app cap and eviction rules are in 03.
2. The client opens `GET /oauth/authorize?response_type=code&client_id=…&redirect_uri=…&scope=…&state=…[&code_challenge=…&code_challenge_method=S256]`
   in a browser or `ASWebAuthenticationSession`.
3. The server renders a **small server-side HTML page** with no JavaScript
   required: the app name, requested scopes, username + password fields, and
   Authorize / Deny buttons. There is no cookie session. The form re-posts the
   OAuth parameters in hidden fields, and each authorization requires the password.
   This works in any in-app browser and doesn't depend on the Angular bundle loading.
4. `POST /oauth/authorize` validates the credentials, redirect URI (exact match to a
   registered URI, or `urn:ietf:wg:oauth:2.0:oob`, which shows the code on a page),
   and scopes (must be a subset of the app's). It issues a 60-second single-use code
   (RAM) and returns `302` to `redirect_uri?code=…&state=…`. Custom schemes
   (`ivory://`, `tusky://`) are allowed.
5. `POST /oauth/token` with `grant_type=authorization_code` (+ `code_verifier` if a
   challenge was given) → access token. Also `client_credentials` → an app-only
   token, which can only call `apps/verify_credentials` and public metadata.
   `password` grant: not supported (not in Mastodon 4.4+ either).
6. `POST /oauth/revoke` erases the token.

`/.well-known/oauth-authorization-server` advertises `authorization_code` and
`client_credentials`, `S256`, and the scopes list. Scopes: `read`, `write`,
`follow`, `push`, `profile`, and the `read:*` / `write:*` sub-scopes. They are
enforced coarsely, as in mastodon_mock's optional mode: GET needs `read`/`profile`,
mutations need `write`. `profile` alone allows only `verify_credentials`.

### Tokens

- 32 random bytes, base64url. Only `sha256(token)` is stored, keyed by its prefix.
- **Tokens don't expire.** Mastodon's don't, and clients never refresh. nanacoin's
  8-hour RAM sessions are wrong for this use case. A token stays valid until
  revoked, until the account's `token_epoch` changes (password change, "sign out
  everywhere", disable, role change), or until it is evicted by the per-account cap.
- The household app lists a member's tokens as "devices" (app name, created date,
  last used in RAM since boot) with a revoke button.
- The household app itself is a first-party OAuth client, registered at
  provisioning, using PKCE. It uses the same tokens as any other client.

## HTTP / HTTPS modes

Inherited from nanacoin's `CONNECTION_SECURITY.md`, with the same two modes:

- **Easy mode** (default): HTTPS on 443 and plain HTTP on 80 both serve everything.
  Native Mastodon apps will use HTTPS anyway. HTTP exists so a new device can load
  `/trust`. **Built** (Sprint 8 first slice; user guide `docs/security/https.md`):
  two `EspHttpServer`s share the service (`src/bin/esp32.rs`, 5 TLS + 4 HTTP
  sockets); `Request::secure` marks the TLS listener. OAuth discovery and
  pagination links use the origin the client connected with
  (`Call::origin`), so an app on HTTPS is never sent to an HTTP sign-in page;
  identity URLs (accounts, posts) keep the configured base URL, which on the
  board now defaults to `https://mastomini.local`. HTTPS responses keep the
  connection open (a handshake costs about 1.1 s at 240 MHz; a request on an
  open connection about 0.05 s); HTTP responses still close it.
- **Secure mode** (not built yet; owner switches it on over HTTPS once all devices trust the
  certificate): HTTP serves only `/`, `/trust`, `/ca`. Switching modes bumps every
  account's token epoch, so everyone signs in again (the same as nanacoin's
  session revocation).
- USB recovery build to return to Easy mode, as nanacoin does
  (`MASTOMINI_RECOVER_HTTP=1`).

## Certificates: two supported models

### Model A — household CA + `mastomini.local` (default, offline)

- Generated on the build PC by `scripts/certs.sh` (`make certs`, `make
  reissue-cert`), `scripts/certs-check.sh` and `scripts/rotate-certs.sh`,
  ported from nanacoin's `dev-certs.sh`, `test-certs.sh` and
  `rotate-certs.sh`. RSA-3072 CA for 100 years; RSA-2048 leaf for **820
  days**, not nanacoin's 100 years, because Apple refuses server certificates
  valid for more than 825 days even under a user-installed CA. Renewal
  reissues the leaf from the same CA, so devices keep their trust. Both
  certificates start a day early (signed with `openssl ca -startdate`, the
  only way OpenSSL 3.2 can): made in a US evening, a start of "now" showed as
  tomorrow's date, and slow device clocks would reject it. SAN
  `mastomini.local`, `localhost`, `127.0.0.1`, plus `MASTOMINI_CERT_NAMES` /
  `MASTOMINI_CERT_IPS`. The CA private key never leaves the PC.
- The CA carries **name constraints**: household names (`local`,
  `localhost`, `lan`, `home.arpa`, `internal`) and private IPv4 ranges only,
  so trusting it can't expose public sites even if its key leaks
  (`MASTOMINI_CA_NAME_CONSTRAINTS=0` to omit).
- `/trust` (server-rendered, no JavaScript, both listeners) has per-platform
  instructions and the CA fingerprint; `/ca` serves the DER certificate and
  `/ca.pem` the PEM.
- **Works for**: desktop browsers, iOS/iPadOS (after "full trust" in Certificate
  Trust Settings), and so iOS apps such as Ivory, Ice Cubes, Mona and Mastodon.
  Also macOS apps.
- **Doesn't work for most Android apps.** Since Android 7, apps ignore
  user-installed CAs unless the app opts in via `network_security_config`. Tusky
  and the official app generally don't. Android `.local` resolution is also
  inconsistent between versions and apps. Model B is the Android answer.

### Model B — real domain + publicly trusted certificate (optional)

- The household owns a domain, e.g. `social.example.com`. It resolves to the board's
  LAN IP through the router's local DNS, a Pi-hole entry, or a public A record
  pointing at the private IP.
- **Watch out**: many routers and Pi-hole/dnsmasq enable *DNS rebinding
  protection* and drop public answers that point at private IPs. The household
  app's setup guide covers this.
- A PC script (`make renew-cert DOMAIN=…`) obtains a Let's Encrypt certificate by
  **DNS-01** (acme.sh or certbot with a DNS provider plugin), so the board never
  needs to be reachable from the internet and never holds DNS provider credentials.
  The script then uploads the certificate and key to
  `POST /api/mastomini/v1/tls` with an owner token over HTTPS. The board validates
  the key/cert match, SAN and expiry, stores them in `mm_cfg` (full chain ≤ 8 KiB),
  and restarts its TLS listener.
- Request an RSA-2048 key unless ECDSA P-256 has been verified on the board's
  mbedTLS/esp-tls configuration. nanacoin switched to RSA after ECDSA trouble with
  its own certificate, and acme.sh now defaults to ECDSA.
- Let's Encrypt certificates are short-lived (90 days today, shrinking in the next
  few years). The household app shows the expiry date and warns 21 days ahead. If
  the uploaded certificate expires, the board falls back to Model A so the household
  CA path keeps working.
- One board serves **one hostname identity to Mastodon clients**. Account URLs and
  `acct` resolution use the configured primary hostname. If both names are in use,
  clients configured with the other name still work, because the API is
  host-agnostic and only rendered URLs contain the primary name.

### Bring-your-own household CA

By default `make certs` creates a CA dedicated to mastomini in
`mastomini_rs/.local/ca`. To reuse an existing household CA (nanacoin's,
mkcert's, or any other one kept on the build PC), set `MASTOMINI_CA_DIR` to
the directory holding its `rootCA.pem` and `rootCA-key.pem`:

```bash
MASTOMINI_CA_DIR=../../nanacoin/nanacoin_rs/.local/ca make reissue-cert
```

The script then only signs a new `mastomini.local` leaf with that CA (an empty
directory gets a new CA). It never copies the CA private key into this repo or
the firmware, and `rotate-certs` leaves a shared CA alone. Devices that already
trust that CA work with no `/trust` step. `certs-check` validates the leaf
against whichever CA was used, and `/ca` serves that CA's public certificate.

## Naming and discovery

- mDNS hostname `mastomini` (build-time configurable), plus an `_https._tcp` service
  record, using the managed `espressif/mdns` component as nanacoin does.
- Warn in `/trust` and the setup guide: do not run two boards with the same mDNS
  name.
- The client setup guide recommends a DHCP reservation for the board, which Model B
  and Android need.

## Time

Snowflake IDs and `created_at` need a real clock.

- SNTP after Wi-Fi (optional LAN NTP server at build time, as nanacoin does).
- Until the clock is valid, **reads work and writes return `503`**, with
  `Retry-After`.
- **Fallback for households without internet:** a signed-in admin's household app
  can post the browser's time to `POST /api/mastomini/v1/clock`. The board accepts it
  only while SNTP has not synced, and only if it is later than the newest stored ID.
  Diagnostics mark it as "approximate". This avoids a permanently read-only board
  when the internet is down after a power cut. **Built:** the time is anchored
  to the platform's monotonic uptime in RAM (`State::clock_anchor`); requests
  then report `clock: "manual"`, and a synced clock always wins. The household
  app's Health page offers the button while the clock isn't synced.
- IDs are guarded to be monotonic whatever the clock does (02).

## Direct messages

Direct messages are encrypted so that only their participants can read them
(`src/crypto.rs`, `src/domain/dm.rs`).

- **Keys.** Every member has an X25519 key pair (`mm_key` `k` + slot). The public
  half is stored as is. The secret half is sealed (ChaCha20-Poly1305) with a key
  derived from the member's password (PBKDF2-SHA256, own salt, same round count
  as the verifier). The member's password is only present when an account is
  created, at sign-in and at a password change, so those are the only moments
  the secret can be opened.
- **Devices.** At sign-in, the opened secret rides along with the one-use
  authorization code (RAM, 60 s) and is sealed again for the token it becomes:
  a key derived from the token with HKDF (`mm_key` `w` + token key suffix). The
  token record stores `sha256(token)`, which does not give that key. Revoking a
  token, a password change (all tokens end) and account deletion erase the seals.
- **Messages.** A direct message is encrypted once with a random content key; the
  content key is wrapped for the author and each mentioned member via X25519
  against a fresh ephemeral key and HKDF. The envelope (`mm_stat` `d` + id) is
  written first, then the status record (the commit point) with empty text and
  content warning. The status id is bound into the ciphertext.
- **Reading.** For each request with a user token, the server opens that
  device's seal, keeps the secret for that request only, and zeroes it
  afterwards. A participant whose device has no seal (signed in before
  encryption existed) sees "🔒 Encrypted direct message. Sign in again on this
  device to read it."
- **Members without a key** (accounts from before this) get one at their next
  sign-in. Until then nobody can send them a direct message (`422`
  "@name needs to sign in again before they can receive direct messages").
- A password change re-seals the secret under the new password: old messages stay
  readable. There is no recovery: if a password is reset without the old one
  (a reset code), the member gets a new key pair and their old direct messages
  become unreadable.

What it protects against: anyone reading the flash (a dump, a stolen board), an
admin or the owner through any API or tool, and anyone with the store but no
participant's password or live token.

What it does not protect against:

- **Short passwords.** Passwords may be 4 characters (see "Passwords"). With a
  flash dump, a short password falls to offline guessing despite PBKDF2, and
  with it that member's key. The encryption is as strong as the password.
- **The admin who set the password.** Invite and reset codes let members choose
  their own passwords, but an admin can still create a member with a password
  they choose (`POST /admin/members`, the "Add a family member" form). Until the
  member changes it, that admin knows it.
- **Modified firmware.** The server decrypts in RAM to serve Mastodon apps
  (they don't do end-to-end encryption). Firmware changed to log what it serves
  would see messages as they are read.
- **Metadata.** Who wrote to whom and when, and the reply structure, are stored
  in plain text: the server needs them to route and thread messages.
- Followers-only posts are not encrypted, only direct messages.

## Privacy and threat model

- Protected: casual access by other devices on the LAN (TLS, tokens), one member
  reading another's DMs through any API, and DM contents in flash (see "Direct
  messages", with its limits).
- Not protected: other data on a stolen board (posts, profiles and password
  verifiers can be read from a flash dump), and a compromised household device.
  ESP32 flash encryption + NVS encryption would cover the rest of the flash. It
  burns eFuses irreversibly, complicates USB recovery and needs its own key
  management design, so it is not in v1.
- Nothing leaves the LAN. The board makes no outbound connections except SNTP.
