# mastomini household app

The Angular app the board serves at `/app/`. It does what a Mastodon client
can't:

- **Set up a device** and **Trust** (no sign-in): the server address, which
  apps to use on which device (`src/app/pages/connect-guide.ts`), and
  certificate steps once HTTPS exists.
- **My account:** signed-in devices, password, sign out everywhere.
- **Members** (admins): invite and reset links with QR codes, add with a
  password, disable, silence, suspend, roles, delete.
- **Server**, **Health** (diagnostics, setting the clock without internet)
  and **Security** (owner).

Posting, timelines and profiles stay in Mastodon apps. Design:
`../spec/06-household-app.md`.

Modelled on `nanacoin_ui`: standalone components, signals, zoneless change
detection, lazy routes, hash routing. It uses plain `fetch` instead of
HttpClient, since every kilobyte ends up in the board's flash.

## Run it

Git Bash, two terminals:

```bash
# 1: the desktop server
cd ../mastomini_rs && make run

# 2: this app, with /api and /oauth proxied to it
npm install    # once
npm start      # http://localhost:4200/app/
```

`MASTOMINI=192.168.1.161 npm start` proxies to a board instead
(`proxy.conf.mjs`).

Sign in with an account on that server. The app registers itself as an OAuth
app the first time each browser signs in, then uses the server's normal
sign-in page (PKCE), exactly like a Mastodon app.

```bash
npm test         # unit tests (vitest)
npm run build    # production build into dist/
```

## Embedding

`make web` in `../mastomini_rs` builds this app and bundles it
(`scripts/build-web.sh`, `scripts/bundle-web.mjs`). `make run-web` runs the
desktop server with it at <http://127.0.0.1:8080/app/>. Firmware builds
(`make firmware`) always include it. The bundle limit is 600 kB; the app is
about 117 KiB gzipped now.
