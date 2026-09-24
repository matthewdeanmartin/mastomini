# Encryption at rest

mastomini encrypts **direct messages** at rest. A direct message can only be
decrypted with the password of someone in the conversation, or with a device
that one of them signed in on. Everything else on the board (public and
followers-only posts, profiles, follows) is stored unencrypted.

This page explains what that protects, how it works, and, just as important,
where it stops.

## In short

| Someone who has… | Can they read a direct message? |
|---|---|
| A participant's app, signed in | Yes: that's the point |
| A participant's password | Yes |
| The owner or an admin account, through any app or API | No |
| An admin looking at a report about the message | No: the report shows the comment, not the message |
| A copy of the board's flash (a stolen board, a flash dump) | No, **unless a participant's password is weak enough to guess** (see below) |
| Physical control of the board, and the patience to change its firmware and wait for someone to read their messages | Yes (see below) |

Who wrote to whom and when is **not** encrypted.

## How it works

**Every member has a key pair** (X25519). The public half is stored as is. The
secret half is stored only in encrypted form (ChaCha20-Poly1305), under a key
derived from the member's password with PBKDF2-SHA256.

**Signing in unlocks it for that device.** The password is present only when an
account is created, at sign-in and at a password change. At sign-in the server
opens the secret key with the password and encrypts a second copy under a key
derived from the new device's access token (HKDF-SHA256). Access tokens are 256
random bits. The server keeps only their SHA-256 hash, which does not give that
key. Signing out or revoking a device deletes its copy, and so does a password
change (which signs out every device) or deleting the account.

**Each direct message is encrypted once** with a fresh random key
(ChaCha20-Poly1305). That key is then encrypted separately for the author and
for each person mentioned, using an X25519 exchange with a fresh one-time key.
The stored post keeps no text and no content warning, only the encrypted
envelope. The post's id is bound into the encryption, so an envelope can't be
moved to another post.

**Reading happens in memory, per request.** When an app asks for a direct
message, the server opens that device's copy of the reader's key with the
app's token, decrypts, answers, and wipes the key from memory when the request
ends. Nothing decrypted is ever written to flash.

**Edits stay encrypted.** An edited direct message is encrypted again, and no
earlier version is kept (edit history of other posts is stored as plain text).
Polls can't be added to direct messages, because their options would not be
encrypted.

**A password change keeps old messages readable:** the secret key is encrypted
again under the new password. There is no other way back in: a member who
forgets their password and uses a reset link gets a new key, and their old
direct messages can't be read any more.

## Limitations

### Short passwords

mastomini allows passwords as short as 4 characters, on purpose: being on the
household Wi-Fi is treated as the second factor. That is reasonable for
signing in over the network, where five wrong guesses lock the account for
five minutes.

It is **not** enough against someone holding a copy of the flash. They can
guess offline, as fast as their hardware allows. PBKDF2 slows each guess down,
but a 4-character password falls in minutes. Once a participant's password is
guessed, their key opens and so do their direct messages.

**Encryption at rest is only as strong as the weakest password in the
conversation.** Members who care should use a long passphrase.

### An admin who chooses someone's password knows it

Invite and reset links let members choose their own passwords. An admin can
still create an account with a password they choose; until the member changes
it, that admin could sign in as them. Invite members with a link, or have them
change their password after their first sign-in.

### The server has to decrypt to serve apps

Mastodon apps don't do end-to-end encryption: they expect the server to hand
them readable text. So the board decrypts in memory while it answers. Someone
with physical control of the board could install modified firmware that
records messages as they are read. Encryption at rest protects stored data; it
doesn't make the server untrusted.

### Metadata is not encrypted

To route and thread messages, the server stores in plain text: who sent a
direct message, who it was sent to, when, and what it replies to.

### Only direct messages

Followers-only, unlisted and public posts are not encrypted. They're readable
by everyone they're meant for, and from a copy of the flash. So are profiles,
follows, blocks, mutes and reports.

### Older devices and accounts

- A device signed in before encryption existed has no copy of the key. It
  shows "🔒 Encrypted direct message. Sign in again on this device to read it"
  until the member signs out and in.
- An account created before encryption existed gets its key pair at its next
  sign-in. Until then, nobody can send it a direct message (the app shows
  "@name needs to sign in again before they can receive direct messages").
- Direct messages stored before encryption existed stay unencrypted.

### Not the whole flash

This is encryption of direct messages by the application, not encryption of
the board's flash. The ESP32-S3 also supports hardware flash encryption, which
would cover everything stored. It permanently burns keys into the chip
(eFuses), makes USB recovery harder, and needs its own key management design,
so it is not used.

## For reviewers

- Code: [`src/crypto.rs`](https://github.com/matthewdeanmartin/mastomini/blob/main/mastomini_rs/src/crypto.rs)
  (the primitives) and
  [`src/domain/dm.rs`](https://github.com/matthewdeanmartin/mastomini/blob/main/mastomini_rs/src/domain/dm.rs)
  (keys, device copies, envelopes).
- Crates: `x25519-dalek`, `chacha20poly1305`, `hkdf`, `pbkdf2`, `sha2`,
  `zeroize` (RustCrypto and dalek).
- Design and threat model: `spec/05-auth-network-tls.md`, "Direct messages".
- Tests check that no direct-message text or content warning appears anywhere
  in the store, that admins and the server without a token can't read them,
  that keys survive restarts and password changes, that revoking a device
  removes its copy, and that a power cut at any point while sending leaves
  nothing half-written.
