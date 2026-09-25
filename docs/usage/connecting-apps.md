# Connecting Mastodon apps

Each family member needs an account first: an admin creates it (see
[Running the household](administration.md)). Then:

1. In a Mastodon app, choose to sign in to a server and type the board's
   address, for example `192.168.1.161` or `mastomini.local`. On the board's
   page, **Set up my phone** shows the address to use, with a Copy button, and
   which apps to try on your kind of device.
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

mastomini serves HTTPS with a **household certificate** and plain HTTP side
by side. Apps that insist on HTTPS work once the device trusts the household
certificate: follow [HTTPS and certificates](../security/https.md), then enter
the server as `https://mastomini.local`.

| Client | Status |
|---|---|
| [Mastodon.py](https://github.com/halcy/Mastodon.py) and scripts | Tested over HTTP and HTTPS: the test suites sign in through the real sign-in page |
| iPhone and Mac apps (Ivory, Ice Cubes, Mona, the official app) | Should work over HTTPS once the device trusts the certificate (with full trust on iPhone). Not tested on real devices yet |
| Android apps (Tusky, the official app, …) | Most ignore certificates you install, so they need a real domain name, which isn't built yet |
| Web clients served over HTTPS (Phanpy, Elk) | Only if the browser trusts the household certificate. Not tested yet |

The app-by-app compatibility matrix is planned (see `spec/07-testing.md`).

## What's different from a public Mastodon server

- **140 characters** per post, content warning included. Links count as 23,
  as on Mastodon.
- **No pictures or video**, no scheduled posts, no quote posts. Everyone's
  picture is their initial on a colour.
- **Polls** have 2 to 4 options and last up to a week. Direct messages can't
  have polls.
- **Editing** works as on Mastodon, and the three previous versions are kept.
  Edited direct messages keep no history.
- **Lists and filters** work as on Mastodon. Filters mark matching posts; your
  app decides whether to warn, blur or hide them.
- **Locked accounts** get follow requests, as on Mastodon.
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
