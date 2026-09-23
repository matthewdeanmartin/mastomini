# HTTPS and certificates

!!! warning "Not implemented yet"
    mastomini currently serves **plain HTTP only**, on port 80 on the board
    and port 8080 on the desktop build. There is no HTTPS, no household
    certificate authority, and no certificate management. This page is a
    placeholder: it will be written when that work lands (roadmap Sprint 8).

## What that means today

- Traffic between apps and the board is not encrypted on the home network.
  Anyone on the same Wi-Fi who captures it could read posts as they travel, and
  could capture a password or an access token.
- Access tokens and sign-in pages travel in the clear on the local network.
  Being on the household Wi-Fi is what keeps outsiders out, so keep that
  network's password private.
- Many phone apps and all HTTPS-hosted web clients won't connect to a
  plain-HTTP server (see [Connecting Mastodon apps](../usage/connecting-apps.md)).
- [Encryption at rest](encryption.md) protects direct messages stored on the
  board. It doesn't protect them on the network.

## What is planned

The design is in `spec/05-auth-network-tls.md`, and the roadmap
(`spec/roadmap.md`, Sprint 8) lists:

- a household certificate authority, or reuse of an existing one
  (`MASTOMINI_CA_DIR`), with a `/trust` page and `/ca` download to install it
  on each device;
- an "Easy" (HTTP) and "Secure" (HTTPS only) mode switch;
- an optional uploaded certificate for a real domain name, for devices that
  won't trust a household CA or `.local` names (notably Android), and a
  `make renew-cert` helper;
- testing on real phones.

This page will cover setting that up once it exists.
