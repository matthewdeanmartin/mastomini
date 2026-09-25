# HTTPS and certificates

mastomini serves **HTTPS and plain HTTP side by side** ("Easy mode"): HTTPS
on port 443 and HTTP on port 80 on the board, both serving everything. The
HTTPS certificate is signed by a **household certificate authority (CA)**
that you create on your build computer. Each device installs that CA once,
and after that browsers and apps can check they are talking to the board.

Certificates from a real domain name, which every device trusts without
installing anything, are on the roadmap (see [What is not built yet](#what-is-not-built-yet)).

## What it protects, and what it doesn't

A device that trusts the household CA and uses an `https://` address gets
an encrypted, authenticated connection. Nobody else on the Wi-Fi can read or
change its passwords, tokens, posts or direct messages as they travel.

Easy mode leaves plain HTTP on for everything, so that:

- a new device can load `/trust` and download the CA before it trusts
  anything;
- apps and bookmarks set up with `http://` keep working.

That means **a device using an `http://` address is still unencrypted**, and
HTTPS on one device doesn't protect accounts used over HTTP on another. Move
each device over (below). A Secure mode that turns HTTP off is planned but not
built yet.

## Owner: turn HTTPS on

On the build computer, from `mastomini_rs/` in Git Bash:

```bash
make certs     # creates the household CA and the board's certificate
make deploy PORT=COM11
```

`make certs` prints the CA's **SHA-256 fingerprint**. Keep it handy (a
photo, a note on your phone): it is how each device checks it downloaded the
right certificate. `make firmware` and `make deploy` run `make certs`
themselves, so a fresh checkout gets certificates on its first build.

`make certs` creates only what is missing, and never replaces a certificate
or CA. It always checks the files it ends up with (`make certs-check` runs the
check alone).

Then, on each device:

1. Open `http://mastomini.local/trust` (or `http://<board IP>/trust`).
2. Tap **Download the certificate** and compare the fingerprint on the page
   with the one `make certs` printed. The page came over plain HTTP, so on its
   own it can't prove it is genuine; the comparison does.
3. Install and trust it. The page has steps for iPhone/iPad, Android, Mac,
   Windows, Linux and Firefox. On iPhone and iPad, installing the profile and
   turning on **full trust** (Settings → General → About → Certificate Trust
   Settings) are two separate steps. Both are needed.
4. Open `https://mastomini.local/`. There should be no warning. **If there is
   one, don't click past it**: the CA isn't trusted yet, so go back to step 3.
5. Point apps at `https://mastomini.local` (sign in again in apps that were set
   up with `http://`).

The household app's **Security** page (owner only) shows the CA fingerprint,
the certificate's names and expiry date, and whether you're connected over
HTTPS right now.

### Android

Chrome on Android trusts a CA you install. Most Android **apps** (Tusky,
the official Mastodon app) ignore user-installed CAs by design, so they will
refuse the board until the real-domain option exists. Android is also
inconsistent about resolving `.local` names; use the board's IP address there
(see [Names and addresses](#names-and-addresses)).

## What the household CA can do

A device that trusts a CA believes any certificate the CA signs. To limit
that, the CA `make certs` creates carries **name constraints**: it can only
vouch for household names (`.local`, `.lan`, `.home.arpa`, `.internal`,
`localhost`) and private addresses (`10.x`, `172.16–31.x`, `192.168.x`,
`127.x`). Even if its private key leaked, it could not impersonate your bank
or any other public site to your devices. `/trust` says whether the CA being
served has these limits.

The CA's private key stays on the build computer, in
`mastomini_rs/.local/ca/rootCA-key.pem`. It is never copied into the firmware,
the repository, or anything the board serves. **Back it up somewhere private**
(an encrypted USB stick, a password manager's file attachment): without it
you can still use the current certificate, but renewing it means every
device must trust a new CA.

## Files

| File | What it is | Where it goes |
|---|---|---|
| `.local/ca/rootCA.pem` | The household CA's public certificate | Served (as `/ca`) via `certs/` |
| `.local/ca/rootCA-key.pem` | The CA's private key | **Nowhere.** Stays on this computer; back it up |
| `certs/mastomini.crt` | The board's certificate, signed by the CA | Embedded in the firmware |
| `certs/mastomini.key` | The board's private key | Embedded in the firmware (necessarily) |
| `certs/household-ca.crt`, `.der` | Copies of the CA's public certificate | Served at `/ca.pem` and `/ca` |
| `certs/certificate.json` | Names and expiry, for the Security page | Embedded in the firmware |

Paths are relative to `mastomini_rs/`. Both directories are gitignored.

## Names and addresses

The certificate is valid for `mastomini.local` (the mDNS name), `localhost`
and `127.0.0.1` (the desktop server). Add more in `mastomini_rs/.env`, or in
the environment, and reissue:

```bash
# mastomini_rs/.env
MASTOMINI_HOSTNAME=mastomini          # the mDNS name, without .local
MASTOMINI_CERT_IPS=192.168.1.161      # the board's address, for devices that can't resolve .local
MASTOMINI_CERT_NAMES=mastomini.lan    # names your router's DNS gives the board
```

```bash
make reissue-cert && make deploy PORT=COM11
```

An IP address in the certificate only helps if it doesn't change: give the
board a DHCP reservation on the router first. Changing `MASTOMINI_HOSTNAME`
needs a reissue too; `make certs-check` (and so every build) refuses a
certificate that doesn't match the hostname.

## Renewal

The board's certificate is valid for **820 days**. Apple devices refuse server
certificates valid for longer than 825 days, even from a CA you installed
yourself. The CA itself lasts 100 years. Both start a day before they are
made: devices show dates in UTC, so a certificate made in the evening would
otherwise say it is valid from tomorrow, and a device whose clock runs behind
would reject it. `make certs` prints the dates in this computer's time.

Before it expires (the Security page counts down, and `make certs-check` and
every build warn inside 60 days):

```bash
make reissue-cert      # new certificate, same CA: devices keep trusting it
make deploy PORT=COM11
```

`make reissue-cert` moves the old certificate and key to
`.local/cert-backups/<time>-reissue/` rather than deleting them.

## Bring your own CA

`MASTOMINI_CA_DIR` points `make certs` at another CA directory holding
`rootCA.pem` and `rootCA-key.pem`, the file names nanacoin and mkcert use.
Devices that already trust that CA then need no `/trust` step:

```bash
MASTOMINI_CA_DIR=../../nanacoin/nanacoin_rs/.local/ca make reissue-cert
MASTOMINI_CA_DIR="$(mkcert -CAROOT)" make reissue-cert
```

Put `MASTOMINI_CA_DIR` in your environment (or type it every time) so later
renewals use the same CA. If the directory holds no CA yet, `make certs`
creates one there. A CA made elsewhere probably has no name constraints, and
`/trust` says so.

## Replacing the CA

Only when the CA's private key is lost or may have leaked:

```bash
make rotate-certs      # archives everything to .local/cert-backups/<time>/, then creates a new CA
make deploy PORT=COM11
```

Every device must then remove the old CA and trust the new one from `/trust`.
With `MASTOMINI_CA_DIR` set, rotation leaves that shared CA alone and only
replaces this project's certificate.

## Desktop development server

`make run` serves HTTPS on `https://localhost:8443` next to
`http://127.0.0.1:8080` once `make certs` has run (`HTTPS_PORT=9443 make run`
to move it). Running the binary directly, HTTPS is on only when
`MASTOMINI_HTTPS_PORT` is set; see `src/bin/desktop.rs` for the other
variables. Test servers never set it, so they never compete for the port.

```bash
curl --cacert certs/household-ca.crt https://localhost:8443/api/mastomini/v1/status
```

On Windows, curl uses the system TLS library (schannel), which needs
`--ssl-no-revoke` for a private CA (it has no revocation list to check) and
cannot match IP addresses in certificates: use `localhost`, not `127.0.0.1`.
Python, browsers and phones check IP addresses normally.

The desktop server's TLS comes from tiny_http 0.12, which pins an old rustls
(0.20) that gets no more fixes. It is for development on your own computer;
don't expose it to a network you don't trust. The board uses ESP-IDF's
mbedTLS.

`clienttests/test_https.py` (part of `make client-test`) runs the real
`scripts/certs.sh` into a temporary directory and signs a Mastodon.py client
in over HTTPS with strict verification. `make probe-board` checks a running
board the same way (see `mastomini_rs/DEPLOY.md`).

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| Browser warning on `https://mastomini.local` | The device doesn't trust the CA yet, or on iPhone full trust is off. Redo the `/trust` steps. Never click past the warning |
| Warning only on `https://<IP>` | The IP isn't in the certificate. Add it with `MASTOMINI_CERT_IPS` and reissue, or use the name |
| Worked before, now warns everywhere | Certificate expired (reissue), or the CA was rotated (trust the new one) |
| An Android app refuses to connect | Expected: Android apps ignore installed CAs. Wait for the real-domain option |
| `make certs` says the certificate is not valid for the host | `MASTOMINI_HOSTNAME` changed. `make reissue-cert` |
| Firmware build: `certs/... is missing` | `make certs` (the build normally runs it for you) |
| The first HTTPS page takes about a second | Normal: setting up an HTTPS connection costs the board about 1.1 s. Later requests on the same connection take about 0.05 s. If many devices connect at the same moment, a few may need a retry |

## What is not built yet

- **Secure mode**: an owner switch that turns plain HTTP off except for `/`,
  `/trust` and `/ca`, signs everyone out, and has a USB recovery build
  (as in nanacoin). The design is in `spec/05-auth-network-tls.md`.
- **A real domain with a public certificate** (Let's Encrypt by DNS-01,
  uploaded to the board), which Android apps and devices without the CA would
  accept.
- Testing with phone apps on real devices.
