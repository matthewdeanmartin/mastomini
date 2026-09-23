# mastomini

mastomini is a private, Mastodon-compatible server for one household, running on
a single ESP32-S3 microcontroller board. Family members point an ordinary
Mastodon app at the board, sign in, and post short messages to each other.

- **Private.** It does not federate: nothing is sent to, or fetched from, other
  Mastodon servers. New accounts are created by a household admin; there is no
  public sign-up.
- **Small.** Up to 16 accounts, 140-character posts (counted the way Mastodon
  counts them), text only: no pictures or video. When storage fills up, the
  oldest posts are deleted automatically; pinned posts are kept.
- **Mastodon-shaped.** It speaks the Mastodon REST API, so existing apps and
  libraries work against it. Its behaviour is checked against the
  [mastodon_mock](https://github.com/matthewdeanmartin/mastodon_mock) contract
  suite.
- **Household features.** Follows, replies, favourites, boosts, bookmarks, pins,
  notifications, blocks, mutes, reports, admin moderation, and collections.
  Direct messages are [encrypted at rest](security/encryption.md).

## Where to start

| You want to | Read |
|---|---|
| Put mastomini on a board, or try it on a PC | [Installation](installation.md) |
| Sign in from a phone or computer | [Connecting Mastodon apps](usage/connecting-apps.md) |
| Add family members, set the rules, moderate | [Running the household](usage/administration.md) |
| Know what is protected and what is not | [Encryption at rest](security/encryption.md), [HTTPS and certificates](security/https.md) |
| Change the code | [Contributor guide](contributing/index.md) |

## Status

mastomini runs on real hardware over plain HTTP on the home network. HTTPS is
planned but [not implemented yet](security/https.md). The design documents in
[`spec/`](https://github.com/matthewdeanmartin/mastomini/tree/main/spec) record
what is built, what is planned, and why.
