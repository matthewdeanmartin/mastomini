# Running the household

The first account, created during [first-time setup](../installation.md#first-time-setup),
is the **owner**. The owner can make other members **admins**. Admins add
members, moderate, and edit the server's settings. Nobody, not even the owner,
can read other people's [direct messages](../security/encryption.md).

Admins use the **household app** at `http://<board>/app/` (linked from the
board's page): members, invite and reset links (with a QR code), moderation,
server settings and health. Everyone can use it for their own devices and
password. It signs in through the same page as Mastodon apps. The household
API (`/api/mastomini/v1`) and Mastodon apps with admin features work too. The examples below use `curl` with an admin's
access token (`$TOKEN`) and the board's address (`$BOARD`, for example
`http://192.168.1.161`).

## Members

The best way to add someone is an **invite link**. In the household app, open
**Members** and choose **Create invite link**, then send the link (or show the
QR code) to the new member. They open it,
choose their own username and password, and are ready to sign in. A link works
once and expires after 7 days. Up to 8 links can be open at once; creating a
ninth drops the oldest.

With the household API:

```bash
curl -X POST "$BOARD/api/mastomini/v1/admin/invites" -H "Authorization: Bearer $TOKEN"
# -> {"id": "...", "kind": "invite", "code": "...", "url": "http://.../setup/<code>", "expires_at": "..."}
```

Usernames are 1–20 characters of `a-z`, `0-9` and `_`, and can't be changed
later. A household holds up to 16 accounts.

You can also create an account with a password you choose (**Members** → "Or
add someone with a password you choose", or
`POST /api/mastomini/v1/admin/members` with `username`, `password`,
`display_name`).

!!! warning "Passwords you choose for someone"
    The admin who creates an account with a password knows that password. The
    member should change it: until they do, that admin could sign in as them
    and read their direct messages. Invite links avoid this.

List members with `GET /api/mastomini/v1/admin/members`.

### Forgotten passwords

Nobody sets a password for someone else. In **Members**, choose **Reset
password** next to the member and send them the link; they choose a new
password. Their password doesn't change until they use the
link. Or:

```bash
curl -X POST "$BOARD/api/mastomini/v1/admin/members/<account id>/reset" -H "Authorization: Bearer $TOKEN"
```

Using a reset link signs the member out on every device, and their earlier
direct messages can no longer be read (they were locked with the old password;
see [encryption](../security/encryption.md)). The same rules as moderation
apply: admins can't reset their own password (they change it instead) or the
owner's, and only the owner can reset an admin's. A new reset link for the same
member replaces the previous one.

Open links are listed (without the codes themselves) by
`GET /api/mastomini/v1/admin/codes`, and `DELETE /api/mastomini/v1/admin/codes/<id>`
cancels one.

### Changing your own password and devices

Anyone can change their password under **My account** in the household app
(or on the board's page, **Change your password**). It signs you out on
every device; sign in again with the new password.

With a token from the household API:

| Request | Does |
|---|---|
| `POST /api/mastomini/v1/me/password` with `current`, `new` | Change your password; every device is signed out |
| `POST /api/mastomini/v1/me/sign_out_everywhere` | Sign out every device |
| `GET /api/mastomini/v1/me/devices` | Your signed-in apps: name, when signed in, last used since the last restart, and which one is making this request |
| `DELETE /api/mastomini/v1/me/devices/<id>` | Sign one of them out |

## Rules, terms of service and the about page

A new household starts with three rules:

1. Agree to be patient, the microcontroller is slow
2. Agree to not unplug the microcontroller
3. Agree to obey all laws in the jurisdiction of the microcontroller's physical location

The terms of service are generated from the household's name and rules, and the
privacy policy is generated from what the server actually stores. They're shown
at `/about`, `/terms-of-service` and `/privacy-policy`, and through the
Mastodon API.

Admins can change the name, the description (shown on `/about`), the rules (up
to 8, each up to 140 bytes) and the terms (plain text, up to 3,000 bytes):

```bash
curl -X PUT "$BOARD/api/mastomini/v1/admin/server" -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"description": "Our family noticeboard", "rules": ["Be kind", "No spoilers"]}'
```

Sending `"terms": ""` goes back to the generated terms. Changing the terms
updates their effective date.

A board that was set up before default rules existed has no rules. Set them
once with the command above.

## Moderation

Every member can **block** and **mute** others, and **mute a conversation**,
from their Mastodon app. They can also **report** a member or their posts; each
admin gets a notification. The reported member isn't told.

Admins can act on a member:

| Action | Effect | Undo |
|---|---|---|
| Disable | Can't sign in; their apps stop working | Enable: their apps work again |
| Silence | Left out of the public and hashtag timelines for people who don't follow them, and can't send those people notifications | Unsilence |
| Suspend | Can't sign in, and all their posts are hidden | Unsuspend |
| Delete | The account and everything it made is removed for good | None |
| Delete a post | Removes one post (never a direct message the admin isn't part of) | None |

Nobody can act on themselves or the owner, and only the owner can act on
admins or change roles.

With the household API:

```bash
M=/api/mastomini/v1/admin/members
curl -X POST "$BOARD$M/<account id>/suspend" -H "Authorization: Bearer $TOKEN"   # or disable, enable, silence, unsilence, unsuspend
curl -X POST "$BOARD$M/<account id>/role" -H "Authorization: Bearer $TOKEN" -d role=admin   # owner only
curl -X DELETE "$BOARD$M/<account id>" -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" -d '{"confirm": "<username>"}'
curl -X DELETE "$BOARD/api/mastomini/v1/admin/statuses/<post id>" -H "Authorization: Bearer $TOKEN"
```

Mastodon apps with admin screens can use the standard admin API
(`/api/v1/admin/accounts` and `/api/v1/admin/reports`) too. That needs a token
with the `admin:read` / `admin:write` scopes, and, as on Mastodon, an account
must be suspended before the admin API will delete it.

## Announcements

Admins can manage household announcements through the REST API. Use an admin
account's OAuth token with `admin:read admin:write` scopes (`BOARD` is the board's
HTTPS origin, and `TOKEN` the token):

```bash
curl "$BOARD/api/v1/admin/announcements" -H "Authorization: Bearer $TOKEN"
curl -X POST "$BOARD/api/v1/admin/announcements" -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" -d '{"text":"Dinner at six!","published":true}'
curl -X PATCH "$BOARD/api/v1/admin/announcements/<id>" -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" -d '{"text":"Dinner at seven!"}'
curl -X POST "$BOARD/api/v1/admin/announcements/<id>/unpublish" -H "Authorization: Bearer $TOKEN"
curl -X DELETE "$BOARD/api/v1/admin/announcements/<id>" -H "Authorization: Bearer $TOKEN"
```

`GET /api/v1/admin/announcements/<id>` retrieves one announcement; `PUT` is also
accepted for edits. Create a draft with `published:false`, then POST to
`/<id>/publish`. Optional `starts_at` and `ends_at` use RFC3339 timestamps
(for example `2026-10-01T18:00:00-04:00`); an empty string clears a date. Optional
`all_day` is a boolean. Publication windows are checked when members read the list.
The limit is 16 announcements, each with at most 2048 UTF-8 bytes of plain text.
Delete old announcements to free capacity. Text is escaped when rendered.

Members' clients use `GET /api/v1/announcements`, POST `/<id>/dismiss`, and
PUT/DELETE `/<id>/reactions/<emoji>` (URL-encode the emoji). Read and reaction
state survives restarts. `?with_dismissed=true` includes read announcements;
at most eight Unicode reaction types fit on each notice. These member endpoints
use `read:announcements` / `write:announcements` or their parent scopes.

## Health

**Health** in the household app (admins) shows the clock, how full storage is
and the date of the oldest post kept, the board's memory, uptime, reason for
the last restart and Wi-Fi signal, and how much of each limit is in use. The
same data is `GET /api/mastomini/v1/diag` (admin token).
`GET /api/mastomini/v1/status` (no sign-in) has a short summary.

### When the board can't get the time

The board takes the time from the internet after each start. Until it has it,
reading works but nobody can post. If the internet is down, an admin can open
**Health** and choose **Set the board's clock**: the board uses that device's
time until internet time is available again. It only accepts a time later than
the newest post.

## Security

**Security** (owner only) shows how the board is reached (HTTPS and plain
HTTP), the household certificate's fingerprint, the board certificate's names
and expiry date (with a warning when renewal is due), members still waiting
for their direct-message key (they need to sign in once), each member's number
of signed-in devices, and the password rules. The Secure mode switch, which
would turn plain HTTP off, is not built yet (see [HTTPS](../security/https.md)).

## Avatars

Everyone gets a generated avatar: the first letter of their username on a
colour chosen for them. Uploading pictures isn't supported, which keeps the
board's small storage for posts.
