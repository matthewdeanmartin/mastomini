# Minicloud management and kitchen screen

Angular client for [the Rust minicloud service](../minicloud_rs/README.md).
The public `/` page previews the board and lets anyone dismiss messages;
`/admin` provides the token-protected file browser, screen composer and workers.

The usual local entry point is `make run` in `../minicloud_rs`: it builds and
embeds this UI, serving everything from `http://127.0.0.1:8090`.

For frontend development, start the Rust service, then from Git Bash:

```bash
npm ci
npm start
```

Open `http://localhost:4202/` or `/admin`. The development proxy forwards API
and blob requests to Rust on port 8090. `npm run build` produces the production
assets; `../minicloud_rs/scripts/bundle-web.py` packs them into gzip files for
embedding in read-only firmware flash.

Image preparation happens in the browser: upload an original plus a 172 × 320
big-endian RGB565 copy for the C6 LCD. No server-side image decoder is needed.
The management token is held in memory only, and is cleared on Lock or reload.
