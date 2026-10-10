"""A real browser engine (Playwright's Firefox or Chromium) using the board the
way a cross-origin Mastodon web client does: a startup burst of API calls,
later bursts after idle gaps, optionally with large downloads mixed in.

    uv run --project loadtests playwright install firefox chromium   # once
    uv run --project loadtests python loadtests/browser_session.py --address 192.168.1.161
    uv run --project loadtests python loadtests/browser_session.py --address 192.168.1.161 \\
        --token-file ../.local/diag-token --gaps 2,30,90 --big 4 --moz-log ../.local/moz.log

The page is served from 127.0.0.1, so every call is cross-origin with a CORS
preflight, as from mawkingbird.com. A call that rejects with a TypeError is
what the console reports as "CORS request did not succeed": the browser got
no usable response (reset or refused connection, aborted stream).

  --gaps     seconds of idle before each later burst; 300+ crosses the
             board's idle timeout (a reconnect: TLS handshakes per request)
  --big N    N fetches of the app bundle per burst (no-cors, ~85 KiB each)
  --squat N  N other HTTPS clients occupy TLS slots during each gap, so the
             board must evict the browser's idle connection
  --moz-log  Firefox only: write Firefox's networking log there, for
             firefox_log.py (it names why each connection closed)
  --trust    only check that the browser trusts the board's certificate:
             no certificate errors ignored, the OS trust store in use
             (where the household CA is installed), https://<host>/

Note: Playwright's browsers are not your everyday profile. Firefox's Local
Network Access prompts and prefs (network.lna.*) do not apply here, so this
cannot reproduce that failure; firefox_log.py on an about:logging capture can.

Safe: GET and OPTIONS on read endpoints only; without --token-file a dummy
token is sent and the API answers 401.
"""
import argparse
import http.server
import json
import os
import socket
import ssl
import sys
import threading
import time
from pathlib import Path

from diag_common import REPORTS, board_args, read_token, redact, tls_context
from transfer_stalls import bundle_path

STARTUP = [
    "/api/v1/instance", "/api/v2/instance", "/api/v1/accounts/verify_credentials",
    "/api/v1/followed_tags?limit=100", "/api/v1/announcements", "/api/v1/custom_emojis",
    "/api/v1/preferences", "/api/v2/filters", "/api/v1/lists",
    "/api/v1/markers?timeline[]=home&timeline[]=notifications",
    "/api/v1/timelines/home?limit=40", "/api/v1/notifications?limit=40",
    "/api/v1/trends/tags", "/api/v1/featured_tags", "/api/v1/conversations",
]
LATER = ["/api/v1/timelines/home?limit=40", "/api/v1/notifications?limit=40",
         "/api/v1/accounts/verify_credentials", "/api/v1/announcements",
         "/api/v1/followed_tags?limit=100"]

BURST = """async ([board, paths, token]) => {
  const results = await Promise.all(paths.map(async p => {
    const started = performance.now();
    const big = p.startsWith('/app/');
    try {
      const response = await fetch(board + p, big
        ? {mode: 'no-cors', cache: 'no-store'}
        : {headers: {Authorization: 'Bearer ' + token}, cache: 'no-store'});
      const body = big ? await response.arrayBuffer() : await response.text();
      return {path: p, ok: true, status: response.status, bytes: body.byteLength ?? body.length,
              ms: Math.round(performance.now() - started)};
    } catch (e) {
      return {path: p, ok: false, error: String(e), ms: Math.round(performance.now() - started)};
    }
  }));
  const protocols = [...new Set(performance.getEntriesByType('resource')
    .map(e => e.nextHopProtocol).filter(Boolean))];
  performance.clearResourceTimings();
  return {results, protocols};
}"""


class Page(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.end_headers()
        self.wfile.write(b"<!doctype html><title>mastomini browser session</title>")

    def log_message(self, *_):
        pass


def squatters(args, n: int) -> list:
    """n other HTTPS clients, each served once and then left connected."""
    ctx = tls_context(args, alpn=("http/1.1",))
    held = []
    for _ in range(n):
        try:
            sock = ctx.wrap_socket(socket.create_connection((args.address, 443), timeout=15),
                                   server_hostname=args.host)
            sock.sendall(f"GET /api/mastomini/v1/version HTTP/1.1\r\nHost: {args.host}\r\n\r\n".encode())
            sock.recv(4096)
            held.append(sock)
        except (OSError, ssl.SSLError) as e:
            print(f"    squatter failed: {e}")
    return held


def trust(args, sync_playwright) -> int:
    """Load the board by name with certificate checking on: does this browser
    accept the board's certificate (key type, usages, chain to the household CA)?"""
    url = f"https://{args.host}/api/mastomini/v1/version"
    with sync_playwright() as pw:
        engine = getattr(pw, args.browser)
        # Firefox reads the OS trust store only when asked; Chromium always does.
        options = {"firefox_user_prefs": {"security.enterprise_roots.enabled": True}}             if args.browser == "firefox" else {}
        browser = engine.launch(**options)
        page = browser.new_context().new_page()
        try:
            response = page.goto(url, timeout=30_000)
            print(f"{args.browser}: trusted, HTTP {response.status} from {url}")
            return 0
        except Exception as e:  # Playwright reports TLS failures as errors
            print(f"{args.browser}: NOT trusted or unreachable: {str(e).splitlines()[0]}")
            return 1
        finally:
            browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    board_args(parser)
    parser.add_argument("--browser", choices=["firefox", "chromium"], default="firefox")
    parser.add_argument("--board", help="URL the page calls (default https://<address>)")
    parser.add_argument("--token-file", type=Path, help="oauth_token.py's file; default: a dummy token")
    parser.add_argument("--rounds", type=int, default=1, help="startup bursts, back to back")
    parser.add_argument("--gaps", default="2,10,30", help="idle seconds before each later burst ('' for none)")
    parser.add_argument("--big", type=int, default=0, help="app-bundle downloads added to each burst")
    parser.add_argument("--squat", type=int, default=0, help="other TLS clients during each gap")
    parser.add_argument("--moz-log", type=Path, help="Firefox networking log file (Firefox only)")
    parser.add_argument("--report", type=Path, help="JSON report (default: loadtests/reports/)")
    parser.add_argument("--trust", action="store_true", help="only check certificate trust (see above)")
    args = parser.parse_args()

    from playwright.sync_api import sync_playwright

    if args.trust:
        sys.exit(trust(args, sync_playwright))

    board = args.board or f"https://{args.address}"
    token = read_token(args.token_file) if args.token_file else "not-a-real-token"
    gaps = [float(g) for g in args.gaps.split(",") if g.strip()]
    extra = []
    if args.big:
        args.path = None
        bundle = bundle_path(args)
        extra = [f"{bundle}?n={i}" for i in range(args.big)]

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Page)
    threading.Thread(target=server.serve_forever, daemon=True).start()

    plan = [("startup", 0.0, STARTUP)] * args.rounds + [(f"after {g:g}s idle", g, LATER) for g in gaps]
    log, failures = [], 0
    with sync_playwright() as pw:
        engine = getattr(pw, args.browser)
        launch = {}
        if args.moz_log and args.browser == "firefox":
            launch["env"] = {**os.environ, "MOZ_LOG": "timestamp,nsHttp:5,nsSocketTransport:4,nsHostResolver:4",
                             "MOZ_LOG_FILE": str(args.moz_log.resolve())}
        browser = engine.launch(**launch)
        # Playwright's browsers do not carry the household CA.
        page = browser.new_context(ignore_https_errors=True).new_page()
        page.goto(f"http://127.0.0.1:{server.server_port}/")
        for label, gap, paths in plan:
            held = squatters(args, args.squat) if gap and args.squat else []
            time.sleep(gap)
            started = time.monotonic()
            out = page.evaluate(BURST, [board, paths + extra, token])
            for sock in held:
                sock.close()
            seconds = time.monotonic() - started
            bad = [r for r in out["results"] if not r["ok"]]
            failures += len(bad)
            # The bundle downloads are no-cors: opaque, status 0 by design.
            statuses = sorted({r["status"] for r in out["results"] if r["ok"] and not r["path"].startswith("/app/")})
            print(f"{label:18} {seconds:6.2f}s protocols {out['protocols']} statuses {statuses} "
                  f"failed {len(bad)}/{len(out['results'])}")
            for r in bad:
                print(f"    FAIL {redact(r['path'])} after {r['ms']} ms: {r['error']}")
            log.append({"label": label, "gap": gap, "seconds": round(seconds, 3), **out})
        browser.close()

    report = args.report or REPORTS / f"browser-{args.browser}-{time.strftime('%Y%m%d-%H%M%S')}.json"
    report.parent.mkdir(parents=True, exist_ok=True)
    report.write_text(redact(json.dumps({"browser": args.browser, "board": board, "bursts": log}, indent=1)))
    print(f"TOTAL failed {failures}; report {report}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
