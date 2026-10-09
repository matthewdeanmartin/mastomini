"""A browser's startup burst on ONE HTTP/2 connection: a CORS preflight and a
GET for each of a web client's first requests, all sent at once, before the
board's SETTINGS can arrive (browsers do exactly this).

    uv run --project loadtests python loadtests/h2_burst.py --address 192.168.1.161
    make diag-h2-burst ADDRESS=192.168.1.161

Passes when every stream is answered with CORS headers. A reset stream
(REFUSED_STREAM, code 7) is what a browser shows as "CORS request did not
succeed": that is how the board's old 8-stream limit broke Mastodon clients.
Exit status 1 if any stream fails, so it can gate a deploy.

Safe: OPTIONS and GET only, on a fixed list of read endpoints. Without
--token-file the GETs carry a dummy token and are answered 401, which still
proves the stream was served.
"""
import argparse
import sys
import time
from pathlib import Path

import h2.config
import h2.connection
import h2.events

from diag_common import board_args, connect, read_token, redact, tls_context

# A Mastodon client's first requests (Mawkingbird, Elk, Phanpy all do some of these).
PATHS = [
    "/api/v1/instance",
    "/api/v2/instance",
    "/api/v1/accounts/verify_credentials",
    "/api/v1/followed_tags?limit=100",
    "/api/v1/announcements",
    "/api/v1/custom_emojis",
    "/api/v1/preferences",
    "/api/v1/filters",
    "/api/v2/filters",
    "/api/v1/lists",
    "/api/v1/markers?timeline[]=home&timeline[]=notifications",
    "/api/v1/timelines/home?limit=40",
    "/api/v1/notifications?limit=40",
    "/api/v1/trends/tags",
    "/api/v1/featured_tags",
    "/api/v1/conversations",
]


def text(value):
    return value.decode() if isinstance(value, bytes) else value


def burst(args, token: str) -> tuple[list[dict], str | None, float]:
    """One connection, every request at once. Returns per-stream results."""
    sock = connect(args, tls_context(args, alpn=("h2",)), timeout=args.timeout)
    if sock.selected_alpn_protocol() != "h2":
        raise SystemExit(f"the board did not negotiate h2 (got {sock.selected_alpn_protocol()!r}); "
                         "build it with HTTP2=1")
    conn = h2.connection.H2Connection(h2.config.H2Configuration(client_side=True))
    conn.initiate_connection()
    sock.sendall(conn.data_to_send())
    streams = {}
    for path in PATHS[: args.requests]:
        for method in ("OPTIONS", "GET"):
            sid = conn.get_next_available_stream_id()
            headers = [(":method", method), (":path", path), (":scheme", "https"),
                       (":authority", args.host), ("origin", args.origin), ("accept", "application/json")]
            if method == "OPTIONS":
                headers += [("access-control-request-method", "GET"),
                            ("access-control-request-headers", "authorization")]
            else:
                headers += [("authorization", f"Bearer {token}")]
            conn.send_headers(sid, headers, end_stream=True)
            streams[sid] = {"method": method, "path": path, "status": None, "cors": None,
                            "bytes": 0, "ended": False, "reset": None}
    sock.sendall(conn.data_to_send())
    start = time.monotonic()
    closed = None
    while (not all(s["ended"] or s["reset"] is not None for s in streams.values())
           and time.monotonic() - start < args.timeout):
        try:
            data = sock.recv(65536)
        except OSError as e:
            closed = repr(e)
            break
        if not data:
            closed = "connection closed"
            break
        for event in conn.receive_data(data):
            if isinstance(event, h2.events.ResponseReceived):
                headers = {text(k): text(v) for k, v in event.headers}
                streams[event.stream_id]["status"] = headers.get(":status")
                streams[event.stream_id]["cors"] = headers.get("access-control-allow-origin")
            elif isinstance(event, h2.events.DataReceived):
                streams[event.stream_id]["bytes"] += len(event.data)
                conn.acknowledge_received_data(event.flow_controlled_length, event.stream_id)
            elif isinstance(event, h2.events.StreamEnded):
                streams[event.stream_id]["ended"] = True
            elif isinstance(event, h2.events.StreamReset):
                streams[event.stream_id]["reset"] = event.error_code
            elif isinstance(event, h2.events.ConnectionTerminated):
                closed = f"GOAWAY code {event.error_code}, last stream {event.last_stream_id}"
        out = conn.data_to_send()
        if out:
            sock.sendall(out)
        if closed:
            break
    sock.close()
    return list(streams.values()), closed, time.monotonic() - start


def good(stream: dict) -> bool:
    return bool(stream["ended"] and stream["status"] and stream["cors"])


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    board_args(parser)
    parser.add_argument("--origin", default="https://mawkingbird.com", help="the web client's origin")
    parser.add_argument("--requests", type=int, default=len(PATHS),
                        help=f"distinct endpoints (2 streams each, max {len(PATHS)})")
    parser.add_argument("--repeat", type=int, default=3, help="bursts, each on a new connection")
    parser.add_argument("--token-file", type=Path, help="a read token (oauth_token.py); default: a dummy")
    parser.add_argument("--timeout", type=float, default=30.0)
    parser.add_argument("--verbose", action="store_true", help="list every stream, not only failures")
    args = parser.parse_args()
    token = read_token(args.token_file) if args.token_file else "not-a-real-token"
    failed_bursts = 0
    for n in range(args.repeat):
        results, closed, seconds = burst(args, token)
        ok = sum(good(s) for s in results)
        print(f"burst {n + 1}: {ok}/{len(results)} streams answered with CORS in {seconds:.2f}s"
              + (f"; {closed}" if closed else ""))
        for s in results:
            if args.verbose or not good(s):
                flag = "" if good(s) else "   <-- FAIL"
                reset = f" reset={s['reset']}" if s["reset"] is not None else ""
                print(f"    {s['method']:7} {redact(s['path']):55} status={s['status']} "
                      f"cors={s['cors']} bytes={s['bytes']}{reset}{flag}")
        failed_bursts += ok != len(results)
    sys.exit(1 if failed_bursts else 0)


if __name__ == "__main__":
    main()
