// Dev-server proxy: the API and the OAuth pages come from a mastomini server,
// so the browser only ever talks to localhost:4200 and everything is
// same-origin, as it is when the board serves the app itself.
//
//   npm start                                  -> desktop server on :8080 (make run)
//   MASTOMINI=192.168.1.161 npm start          -> a board (Git Bash)

const raw = (process.env['MASTOMINI'] ?? 'localhost:8080').trim();
const target = /^https?:\/\//i.test(raw) ? raw : `http://${raw}`;

console.log(`[proxy] /api, /oauth -> ${target}`);

// The board can be slow to answer its first request after a reboot.
const entry = { target, secure: false, changeOrigin: false, proxyTimeout: 60_000, timeout: 60_000 };

export default {
  '/api': entry,
  '/oauth': entry,
};
