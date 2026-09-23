# Running the household

The first account, created during [first-time setup](../installation.md#first-time-setup),
is the **owner**. The owner can make other members **admins**. Admins add
members, moderate, and edit the server's settings. Nobody, not even the owner,
can read other people's [direct messages](../security/encryption.md).

There is no admin web app yet. Admins use the board's own page for adding
members, and the household API (`/api/mastomini/v1`) or a Mastodon app with
admin features for the rest. The examples below use `curl` with an admin's
access token (`$TOKEN`) and the board's address (`$BOARD`, for example
`http://192.168.1.161`).

## Members

Add a member from the board's page (**Add a family member**, confirmed with an
admin's password), or:

```bash
curl -X POST "$BOARD/api/mastomini/v1/admin/members" -H "Authorization: Bearer $TOKEN" \
  -d username=sam -d password=changeme -d display_name=Sam
```

Usernames are 1–20 characters of `a-z`, `0-9` and `_`, and can't be changed
later. A household holds up to 16 accounts.

!!! warning "Initial passwords"
    The admin who creates an account knows its first password. The member
    should change it: until they do, that admin could sign in as them and read
    their direct messages.

List members with `GET /api/mastomini/v1/admin/members`.

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

## Health

`GET /api/mastomini/v1/status` (no sign-in) shows whether the household is set
up, the version, the number of accounts and posts, how full storage is, and
how many old posts have been removed to make room.
