// Dev-server proxy: the API comes from a mastomini-bots server, so the
// browser only talks to localhost:4200 and everything is same-origin, as it
// is when the board serves the app itself.
//
//   npm start                                   -> desktop server on :8090 (make run)
//   MASTOBOTS=192.168.1.170 npm start           -> a board (Git Bash)

const raw = (process.env['MASTOBOTS'] ?? 'localhost:8090').trim();
const target = /^https?:\/\//i.test(raw) ? raw : `http://${raw}`;

console.log(`[proxy] /api, /ca -> ${target}`);

const entry = { target, secure: false, changeOrigin: false, proxyTimeout: 60_000, timeout: 60_000 };

export default {
  '/api': entry,
  '/ca': entry,
};
