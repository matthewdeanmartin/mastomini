# Stored files, copyright registry and bank trust

Minicloud stores the file bytes. NanaCoin stores an edition's URL, SHA-256,
creator, edition owner, license and sale price. An edition sale or gift does
not transfer copyright. Explicit registry updates transfer copyright.

## Configure one credential per bank

1. Generate a random shared secret of at least 32 characters. Keep it in the
   bank's private configuration, never in Angular, browser storage or source control.
2. Configure NanaCoin's environment or private `nanacoin_rs/.env`:

   ```dotenv
   NANACOIN_MINICLOUD_URL=https://minicloud.example
   NANACOIN_MINICLOUD_BANK=nanacoin.local
   NANACOIN_MINICLOUD_SECRET=<shared-secret>
   ```

3. In minicloud Management → Banks, enter that bank identifier and credential,
   the exact NanaCoin browser origin (for example `https://nanacoin.local`), and
   the minicloud audience matching `NANACOIN_MINICLOUD_URL`, with no trailing slash.
4. Rebuild/restart the NanaCoin server after changing firmware configuration.
   Signed-in users can now use the art form's minicloud controls without
   copying per-user minicloud keys.

Minicloud's current HTTP adapter serves HTTP. Use an HTTPS reverse proxy for
the public minicloud origin used by NanaCoin art: editions already require
HTTPS URLs, and an HTTPS NanaCoin page cannot fetch an HTTP minicloud endpoint.
The proxy must preserve Authorization, Origin, request bodies and methods.
The bank URL also supplies screen notifications; preserve the `/api/screen/*`
routes through that proxy.

NanaCoin's existing PKCE login authenticates the user. A session-only
`GET /api/v1/minicloud/session` returns `{url, token, owner, namespace,
expires_at}`. API keys, including read keys, cannot obtain delegation tokens.
The token expires in five minutes and is an HMAC-SHA256 authenticated bank
assertion, not an OAuth access token or JWT. It uses this exact wire format:

```text
mc1.<base64url-no-padding JSON claims>.<base64url-no-padding HMAC>
claims = {bank, username, aud, iat, exp}
HMAC input = "mc1." + encoded claims
```

Minicloud validates the signature, trusted bank, exact audience, synchronized
clock, issuance time and expiry. Identity is `username@bank`. Revoking a bank
or rotating its credential immediately rejects its existing tokens. Disabling
a bank user prevents new tokens; an issued token lasts until its five-minute
expiry. Bank credentials never appear in API responses. Trust also allows
CORS from that bank's exact UI origin; it does not grant admin access.

## APIs

All `/api/blobs` and `/api/copyrights` routes accept an admin bearer token or
a delegated bank token. Public `/blobs/...` URLs continue to serve file bytes.

| Method | Route | Behavior |
| --- | --- | --- |
| GET | `/api/blobs?q=term` | List/search stored file metadata |
| PUT | `/api/blobs/{bucket}/{key...}` | Stream file upload; Content-Type and Content-Length required |
| DELETE | `/api/blobs/{bucket}/{key...}` | Delete an unregistered file |
| GET | `/api/copyrights?q=term` | List/search copyright records |
| GET | `/api/copyrights/{bucket}/{key...}` | Read one record |
| PUT | `/api/copyrights/{bucket}/{key...}` | Create/update/transfer a record |
| DELETE | `/api/copyrights/{bucket}/{key...}` | Delete a record with JSON `{revision}` |
| GET | `/api/banks` | Admin: list trust configuration without credentials |
| PUT | `/api/banks/{id}` | Admin: trust/rotate `{id, secret, origin, audience}` |
| DELETE | `/api/banks/{id}` | Admin: revoke bank trust |
| GET | `/api/storage` | Admin: card availability and bytes per tier |
| POST | `/api/storage/migrate` | Admin: migrate `{bucket, key, tier: "card" or "onboard"}` |

A copyright write body is:

```json
{
  "bucket": "art",
  "key": "namespace/hash",
  "hash": "64-character SHA-256 of stored bytes",
  "owner": "alice@nanacoin.local",
  "title": "Moonlit garden",
  "license": "Personal profile display",
  "revision": 0
}
```

Use revision 0 to create; use the returned revision for updates and deletion.
Stale writes return 409. The hash must match a stored file. File content can
have only one registry record, including across aliases. Registered files
cannot be overwritten or deleted until their copyright record is deleted.
Bank users initially register only files they uploaded, with themselves as
owner. The current copyright owner can update, transfer or delete the record.
Admins can manage all records. Search includes file name, owner, title and
license; file search includes bucket, key, MIME and hash.

Bank uploads use bucket `art` and keys starting with the first 16 hexadecimal
characters of SHA-256(`username@bank`) followed by `/`. Users can replace or
delete only their own unregistered uploads. Limits remain 256 KiB per file,
48 files, 48 copyrights, 8 trusted banks, 3 MiB onboard logical file bytes and
64 KiB total metadata. Moving files to card frees onboard space while keeping
the file count and metadata bounds.

## Onboard and TF storage

Uploads go onboard first. Management → Files shows the tier and buttons to
move a file to TF or back onboard. URLs, hashes and copyright records remain
unchanged. Migration streams the copy, checks its SHA-256 and length, syncs
the destination, commits its tier to both recovery slots, then reclaims source
bytes no longer referenced by either slot. Multiple names can share bytes;
those bytes remain until all references to the old tier are gone.

The non-Touch Waveshare ESP32-C6-LCD-1.47 mounts a preformatted FAT TF card
on the shared SPI2 bus: MISO GPIO5, MOSI GPIO6, clock GPIO7, CS GPIO4.
The card enters SPI mode before LCD traffic starts. Firmware never formats
the card and keeps metadata on onboard SPIFFS. Card files live under
`/sdcard/minicloud`. Insert a card before boot; restart after changing cards.
If existing metadata needs an unavailable card, startup refuses to reset data.

Desktop uses `MINICLOUD_CARD_PATH` pointing to an existing separate directory
on the mounted card. Minicloud does not format or mount desktop devices.

## Recover a forgotten board admin token

The board's admin bearer token is compiled from private
`minicloud_rs/.env` (`MINICLOUD_ADMIN_TOKEN`), not a stored password. Your chosen token (at least seven characters, including a seven-digit PIN)
is used by the firmware build. Keep the
file private; use that value in Management after the update.

Build from `minicloud_rs` with `bash scripts/build-c6.sh`. When ready to
flash, use the existing guarded `scripts/deploy-c6.py --port <port> --update`
workflow. `--update` retains SPIFFS and NVS and does not need the old HTTP
token. `--replace` erases storage and is not the token-recovery workflow.
The firmware artifact was compiled only; hardware still requires an explicit
deployment and runtime verification.

Hardware reference: [Waveshare non-Touch board pinout](https://docs.waveshare.com/ESP32-C6-LCD-1.47).
PKCE reference: [RFC 7636](https://www.rfc-editor.org/rfc/rfc7636).
