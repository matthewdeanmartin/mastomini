"""Summarize a browser HAR export for one host: a request timeline and what
kind of failures it shows, with every token redacted.

    uv run --project loadtests python loadtests/har_summary.py "C:/path/site_Archive.har"
    uv run --project loadtests python loadtests/har_summary.py export.har --host mastomini.local --all

A HAR is easy to get (devtools Network tab -> Save All As HAR) but carries
the session's token in headers and URLs: this prints only redacted URLs and
no headers. It cannot say why a connection closed (firefox_log.py can), but
it shows the shape:

  - new connection: a request that paid for TCP + TLS (connect/ssl > 0);
    nearly every request doing so means the browser keeps losing its
    HTTP/2 connection;
  - status 0: no usable response (the console says "CORS request did not
    succeed"). Failing right after a handshake points at the connection, a
    GET with no timings at all usually means its preflight failed first.
"""
import argparse
import collections
import datetime
import json
import sys
from pathlib import Path

from diag_common import redact

PHASES = ("blocked", "dns", "connect", "ssl", "send", "wait", "receive")


def started(entry) -> float:
    return datetime.datetime.fromisoformat(entry["startedDateTime"].replace("Z", "+00:00")).timestamp()


def summarize(har: dict, host: str) -> dict:
    entries = sorted((e for e in har["log"]["entries"] if host in e["request"]["url"]), key=started)
    t0 = started(entries[0]) if entries else 0.0
    rows = []
    for e in entries:
        timings = e.get("timings", {})
        new_conn = timings.get("ssl", -1) > 0 or timings.get("connect", -1) > 0
        never_sent = all(timings.get(k, -1) < 0 for k in PHASES)
        rows.append({
            "at": round(started(e) - t0, 2),
            "ms": round(e.get("time", 0)),
            "status": e["response"]["status"],
            "version": e["response"].get("httpVersion") or "-",
            "method": e["request"]["method"],
            "path": redact(e["request"]["url"].split(host, 1)[1]),
            "new_connection": new_conn,
            "never_sent": never_sent,
            "timings": {k: round(timings.get(k, -1)) for k in PHASES},
        })
    failed = [r for r in rows if r["status"] == 0]
    return {
        "requests": len(rows),
        "failed": len(failed),
        "by_status": collections.Counter((r["method"], r["status"]) for r in rows),
        "new_connections": sum(r["new_connection"] for r in rows),
        "failed_after_handshake": sum(r["new_connection"] and r["timings"]["wait"] <= 0 for r in failed),
        "failed_never_sent": sum(r["never_sent"] for r in failed),
        "rows": rows,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("har", type=Path)
    parser.add_argument("--host", default="mastomini.local")
    parser.add_argument("--all", action="store_true", help="list every request, not only failures")
    args = parser.parse_args()
    s = summarize(json.loads(args.har.read_text(encoding="utf-8")), args.host)
    if not s["requests"]:
        raise SystemExit(f"no requests to {args.host} in {args.har}")
    print(f"{s['requests']} requests to {args.host}, {s['failed']} failed (status 0)")
    print("  by method/status:", dict(s["by_status"]))
    print(f"  {s['new_connections']} of {s['requests']} opened a new connection (TCP + TLS)")
    print(f"  failures right after a handshake: {s['failed_after_handshake']}; "
          f"never sent (e.g. preflight failed): {s['failed_never_sent']}")
    print(f"\n{'start':>7} {'ms':>6} {'st':>3} {'ver':8} {'method':7} path  | "
          + ",".join(PHASES))
    for r in s["rows"]:
        if args.all or r["status"] == 0:
            t = ",".join(str(r["timings"][k]) for k in PHASES)
            print(f"{r['at']:7.2f} {r['ms']:6} {r['status']:>3} {r['version']:8} {r['method']:7} "
                  f"{r['path'][:60]} | {t}")
    sys.exit(1 if s["failed"] else 0)


if __name__ == "__main__":
    main()
