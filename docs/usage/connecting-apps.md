# Connecting Mastodon apps

Each family member needs an account first: an admin creates it (see
[Running the household](administration.md)). Then:

1. In a Mastodon app, choose to sign in to a server and type the board's
   address, for example `192.168.1.161` or `mastomini.local`. The board's own
   page (its address in a browser) shows the address to use.
2. The app opens mastomini's sign-in page. Enter your username and password,
   then **Approve**.
3. That's it. Follow each other: a new account follows nobody, just like on
   Mastodon. The app's suggestions list the household members you don't follow
   yet.

Signing in is also what unlocks [encrypted direct messages](../security/encryption.md)
on that device. A device that was signed in before encryption existed shows
"Sign in again on this device to read it" on direct messages: sign out and in
once.

## Which apps work

mastomini currently serves **plain HTTP** on the home network
([HTTPS is not implemented yet](../security/https.md)). That limits which apps
can connect:

| Client | Status |
|---|---|
| [Mastodon.py](https://github.com/halcy/Mastodon.py) and scripts | Tested: the project's test suites sign in with it through the real sign-in page |
| Phone apps (Ivory, Ice Cubes, Tusky, the official app, …) | Not tested yet. Many phone apps require HTTPS and will refuse a plain-HTTP server |
| Web clients served over HTTPS (Phanpy, Elk) | Won't work: browsers block an HTTPS page from calling a plain-HTTP server |

The app-by-app compatibility matrix is planned (see `spec/07-testing.md`).
Until HTTPS exists, expect desktop and self-hosted clients to be the most
likely to work.

## What's different from a public Mastodon server

- **140 characters** per post, content warning included. Links count as 23,
  as on Mastodon.
- **No pictures or video**, no polls, no scheduled posts, no quote posts.
- **Nobody outside the household.** Search and mentions only find household
  members. `@user@mastomini.local` and `@user` are the same person.
- **Sign-in required for everything**, including the public timeline.
- **Notifications are forgotten when the board restarts.** Posts, follows and
  everything else are kept.
- **Old posts disappear** when storage fills up (oldest first; pinned posts
  stay). Keep copies of anything you want to keep.
- **Direct messages aren't searchable**, because they are encrypted.
- **Visibility:** *public* and *unlisted* both mean "everyone in the
  household" (public posts also appear on the public timeline), *followers only*
  means your followers, and *direct* means only the people mentioned.

Rules, terms of service and the privacy policy are at `/about`,
`/terms-of-service` and `/privacy-policy` on the board, and apps that show a
server's rules and terms pick them up from the Mastodon API.
