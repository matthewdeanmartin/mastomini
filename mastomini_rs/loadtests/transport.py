"""Read-only, anonymous transport benchmark for a live mastomini board:
HTTP/1.1 vs HTTP/2, per-endpoint latency, page loads and concurrency.

    uv run --project loadtests python loadtests/transport.py --label before --address 192.168.1.161
    make bench-transport ADDRESS=192.168.1.161 LABEL=after

Safe on a household's board: GET only, from a fixed allowlist of public
pages and endpoints, no sign-in, no tokens, no request bodies, no
redirects followed. It never writes the store. Concurrency stays within
the board's eight TLS slots.

Phases, for each transport in --modes:

  endpoints  every public read, --samples times in a row on one warm
             connection: p50/p95, bytes, and the server's own split
             (Server-Timing app/lock/body) for micro-optimisation.
  cold       --repeats page views from nothing: the household app (/app/
             and the scripts and stylesheets it loads) plus the public API
             reads, on fresh connections with no TLS session to resume.
             HTTP/1.1 opens up to --page-connections connections (browsers:
             6); HTTP/2 uses one.
  warm       --levels concurrent clients repeat the public reads on open
             connections for --duration seconds each, pausing --think
             seconds between rounds.

HTTP/2 runs only when the board offers it by ALPN. Results go to
loadtests/reports/transport-<label>-<time>.json; keep the ones you compare
(e.g. loadtests/baselines/).
"""
import argparse
import gzip
import http.client
import json
import re
import socket
import ssl
import statistics
import threading
import time
from pathlib import Path

import h2.config
import h2.connection
import h2.events

ROOT = Path(__file__).resolve().parent
CRATE = ROOT.parent
# Public, read-only, answered without a token (Mastodon's anonymous API and
# mastomini's public pages). Anything else is refused by `allowed`.
API = [
    "/api/v1/instance",
    "/api/v2/instance",
    "/api/v1/instance/rules",
    "/api/v1/instance/extended_description",
    "/api/v1/custom_emojis",
    "/.well-known/nodeinfo",
    "/nodeinfo/2.0",
    "/.well-known/oauth-authorization-server",
    "/api/mastomini/v1/version",
    "/api/mastomini/v1/status",
]
PAGES = ["/about", "/favicon.ico", "/avatars/original/missing.png"]
HEALTH = ["/api/mastomini/v1/version", "/api/mastomini/v1/status", "/metrics"]


def allowed(path):
    """The allowlist: fixed reads, household-app files and generated avatars."""
    return (path in API or path in PAGES or path in HEALTH
            or re.fullmatch(r"/app/[\w.\-]*", path) is not None
            or re.fullmatch(r"/avatars/\d+\.png", path) is not None)


class Target:
    def __init__(self, args):
        self.host = args.host
        self.address = args.address
        self.scheme = args.scheme
        self.port = args.port or (443 if args.scheme == "https" else 80)
        self.ca = args.ca
        self.timeout = args.timeout

    def connect(self, alpn):
        """A fresh connection (no TLS session resumption). Returns
        (socket, setup seconds, protocol chosen)."""
        started = time.perf_counter()
        raw = socket.create_connection((self.address, self.port), timeout=self.timeout)
        raw.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        if self.scheme != "https":
            return raw, time.perf_counter() - started, None
        context = ssl.create_default_context(cafile=str(self.ca))
        context.set_alpn_protocols(alpn)
        sock = context.wrap_socket(raw, server_hostname=self.host)
        return sock, time.perf_counter() - started, sock.selected_alpn_protocol()


def server_timing(value):
    return {name: float(ms) for name, ms in re.findall(r"(\w+);dur=([0-9.]+)", value or "")}


def h1_get(sock, host, path):
    """One keep-alive GET; returns (status, body, seconds, server timing)."""
    if not allowed(path):
        raise ValueError(f"not on the read-only allowlist: {path}")
    started = time.perf_counter()
    sock.sendall(f"GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept-Encoding: gzip\r\n\r\n".encode())
    response = http.client.HTTPResponse(sock)
    response.begin()
    data = response.read()
    seconds = time.perf_counter() - started
    if response.getheader("Content-Encoding") == "gzip":
        data = gzip.decompress(data)
    if response.will_close:
        raise ConnectionResetError("server closed the connection")
    return response.status, data, seconds, server_timing(response.getheader("Server-Timing"))


class H2Client:
    """One HTTP/2 connection; a batch of requests is multiplexed, never more
    at once than the server's SETTINGS_MAX_CONCURRENT_STREAMS."""

    def __init__(self, target):
        self.target = target
        self.sock, self.setup, chosen = target.connect(["h2", "http/1.1"])
        if target.scheme == "https" and chosen != "h2":
            self.sock.close()
            raise RuntimeError(f"the board chose {chosen!r}, not h2")
        self.conn = h2.connection.H2Connection(h2.config.H2Configuration(client_side=True, header_encoding="utf-8"))
        self.conn.initiate_connection()
        self.sock.sendall(self.conn.data_to_send())
        while True:
            data = self.sock.recv(65536)
            if not data:
                raise ConnectionResetError("server closed the connection")
            events = self.conn.receive_data(data)
            outgoing = self.conn.data_to_send()
            if outgoing:
                self.sock.sendall(outgoing)
            if any(isinstance(e, h2.events.RemoteSettingsChanged) for e in events):
                break

    def batch(self, paths):
        """Returns [(path, status, bytes, seconds, server timing)]."""
        for path in paths:
            if not allowed(path):
                raise ValueError(f"not on the read-only allowlist: {path}")
        streams, pending, results = {}, list(paths), []

        def start_more():
            while pending and self.conn.open_outbound_streams < self.conn.remote_settings.max_concurrent_streams:
                path = pending.pop(0)
                stream = self.conn.get_next_available_stream_id()
                self.conn.send_headers(stream, [(":method", "GET"), (":scheme", self.target.scheme),
                                                (":authority", self.target.host), (":path", path),
                                                ("accept-encoding", "gzip")], end_stream=True)
                streams[stream] = {"path": path, "status": 0, "body": b"", "timing": {},
                                   "sent": time.perf_counter(), "done": False}

        while pending or not all(s["done"] for s in streams.values()):
            start_more()
            outgoing = self.conn.data_to_send()
            if outgoing:
                self.sock.sendall(outgoing)
            data = self.sock.recv(65536)
            if not data:
                raise ConnectionResetError("server closed the connection")
            for event in self.conn.receive_data(data):
                s = streams.get(getattr(event, "stream_id", None))
                if isinstance(event, h2.events.ResponseReceived):
                    headers = dict(event.headers)
                    s["status"] = int(headers[":status"])
                    s["timing"] = server_timing(headers.get("server-timing"))
                elif isinstance(event, h2.events.DataReceived):
                    s["body"] += event.data
                    self.conn.acknowledge_received_data(event.flow_controlled_length, event.stream_id)
                elif isinstance(event, h2.events.StreamEnded):
                    s["done"] = True
                    results.append((s["path"], s["status"], len(s["body"]),
                                    time.perf_counter() - s["sent"], s["timing"]))
                elif isinstance(event, h2.events.StreamReset):
                    s["done"] = True
                    results.append((s["path"], 0, 0, time.perf_counter() - s["sent"], {}))
                elif isinstance(event, h2.events.ConnectionTerminated):
                    raise ConnectionResetError(f"GOAWAY {event.error_code}")
            outgoing = self.conn.data_to_send()
            if outgoing:
                self.sock.sendall(outgoing)
        return results

    def close(self):
        try:
            self.conn.close_connection()
            self.sock.sendall(self.conn.data_to_send())
        except OSError:
            pass
        self.sock.close()


class H1Client:
    """Up to `width` keep-alive HTTP/1.1 connections, opened as needed; a
    batch is spread across them like a browser does."""

    def __init__(self, target, width):
        self.target, self.width = target, width
        self.socks, self.setups = [], []
        self._lock = threading.Lock()

    def _sock(self, i):
        with self._lock:
            while len(self.socks) <= i:
                sock, setup, _ = self.target.connect(["http/1.1"])
                self.socks.append(sock)
                self.setups.append(setup)
            return self.socks[i]

    def batch(self, paths):
        results, errors = [], []
        lanes = [paths[i::self.width] for i in range(min(self.width, len(paths)))]

        def run(i, lane):
            try:
                sock = self._sock(i)
                for path in lane:
                    status, data, seconds, timing = h1_get(sock, self.target.host, path)
                    results.append((path, status, len(data), seconds, timing))
            except (OSError, http.client.HTTPException) as error:
                errors.append(f"{type(error).__name__}: {error}")
                with self._lock:
                    if i < len(self.socks):
                        try:
                            self.socks[i].close()
                        except OSError:
                            pass
                        del self.socks[i:]

        threads = [threading.Thread(target=run, args=(i, lane)) for i, lane in enumerate(lanes)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        if errors:
            raise ConnectionResetError("; ".join(errors[:3]))
        return results

    def close(self):
        for sock in self.socks:
            sock.close()


def client(target, mode, width):
    return H2Client(target) if mode == "h2" else H1Client(target, width)


def health(target):
    """Uptime, store counts and memory, from public endpoints (plain HTTP,
    so a board with broken HTTPS still answers)."""
    out = {}
    try:
        sock = socket.create_connection((target.address, 80), timeout=target.timeout)
        for path in HEALTH:
            status, data, _, _ = h1_get(sock, target.host, path)
            if status != 200:
                continue
            if path == "/metrics":
                line = data.decode().strip().split(" ")
                if len(line) >= 2:
                    fields = dict(kv.split("=", 1) for kv in line[1].split(","))
                    out["metrics"] = {k: float(v.rstrip("i")) for k, v in fields.items()
                                      if k.startswith(("heap_", "uptime", "requests", "errors", "rejected",
                                                       "tls_", "store_"))}
            else:
                body = json.loads(data)
                for key in ("uptime_ms", "fingerprint", "version", "accounts", "statuses", "available"):
                    if key in body:
                        out[key] = body[key]
        sock.close()
    except (OSError, ValueError, http.client.HTTPException) as error:
        out["error"] = f"{type(error).__name__}: {error}"
    return out


def percentile(values, p):
    if not values:
        return None
    values = sorted(values)
    return values[min(len(values) - 1, round(p / 100 * (len(values) - 1)))]


def summarize(samples):
    ms = [s[3] * 1000 for s in samples]
    return {
        "requests": len(samples),
        "errors": sum(1 for s in samples if s[1] == 0 or s[1] >= 500),
        "non_200": sum(1 for s in samples if s[1] != 200),
        "p50_ms": round(percentile(ms, 50), 1) if ms else None,
        "p95_ms": round(percentile(ms, 95), 1) if ms else None,
        "p99_ms": round(percentile(ms, 99), 1) if ms else None,
    }


def site_files(target):
    """The household app's index and the scripts/stylesheets it loads,
    plus the public pages that answer 200."""
    sock, _, _ = target.connect(["http/1.1"])
    try:
        status, data, _, _ = h1_get(sock, target.host, "/app/")
        files = []
        if status == 200:
            found = re.findall(r'(?:src|href)="([^"]+\.(?:js|css))"', data.decode("utf-8", "replace"))
            files = ["/app/"] + ["/app/" + p.split("/")[-1] for p in dict.fromkeys(found)]
        pages = [p for p in PAGES if h1_get(sock, target.host, p)[0] == 200]
        api = [p for p in API if h1_get(sock, target.host, p)[0] == 200]
        return [f for f in files if allowed(f)], pages, api
    finally:
        sock.close()


def endpoints(target, mode, paths, args):
    """Each path --samples times in a row on one warm connection."""
    out = {}
    c = client(target, mode, 1)
    try:
        for path in paths:
            samples = []
            for _ in range(args.samples):
                samples.extend(c.batch([path]))
            ms = [s[3] * 1000 for s in samples]
            app = [s[4].get("app") for s in samples if s[4].get("app") is not None]
            lock = [s[4].get("lock") for s in samples if s[4].get("lock") is not None]
            out[path] = {"status": samples[-1][1], "bytes": samples[-1][2],
                         "p50_ms": round(percentile(ms, 50), 2), "p95_ms": round(percentile(ms, 95), 2),
                         "server_app_ms": round(statistics.median(app), 3) if app else None,
                         "server_lock_ms": round(statistics.median(lock), 3) if lock else None}
    finally:
        c.close()
    return out


def cold(target, mode, paths, args):
    views = []
    for _ in range(args.repeats):
        started = time.perf_counter()
        try:
            c = client(target, mode, args.page_connections)
            results = c.batch(paths)
            setups = [c.setup] if mode == "h2" else c.setups
            c.close()
            views.append({"ms": (time.perf_counter() - started) * 1000, "connections": len(setups),
                          "setup_ms": [round(s * 1000) for s in setups],
                          "bytes": sum(r[2] for r in results), "statuses": sorted({r[1] for r in results})})
        except (OSError, RuntimeError) as error:
            views.append({"error": f"{type(error).__name__}: {error}"})
        time.sleep(args.think)
    good = [v for v in views if "ms" in v]
    return {
        "views": views,
        "page_ms_median": round(statistics.median(v["ms"] for v in good)) if good else None,
        "page_ms_p95": round(percentile([v["ms"] for v in good], 95)) if good else None,
        "setup_ms_median": round(statistics.median(s for v in good for s in v["setup_ms"])) if good else None,
        "failed_views": len(views) - len(good),
    }


def warm(target, mode, paths, level, args):
    samples, failures, lock = [], [], threading.Lock()
    deadline = time.perf_counter() + args.duration

    def vclient():
        c = None
        while time.perf_counter() < deadline:
            try:
                if c is None:
                    c = client(target, mode, args.warm_connections)
                results = c.batch(paths)
                with lock:
                    samples.extend(results)
            except (OSError, RuntimeError) as error:
                with lock:
                    failures.append(f"{type(error).__name__}: {error}")
                if c:
                    c.close()
                c = None
                time.sleep(1)
            time.sleep(args.think)
        if c:
            c.close()

    started = time.perf_counter()
    threads = [threading.Thread(target=vclient) for _ in range(level)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    seconds = time.perf_counter() - started
    return summarize(samples) | {
        "level": level,
        "requests_per_s": round(len(samples) / seconds, 1),
        "connection_failures": len(failures),
        "failure_examples": sorted(set(failures))[:3],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--label", required=True, help="names the report, e.g. before")
    parser.add_argument("--address", required=True, help="board IP (mDNS is slow on Windows)")
    parser.add_argument("--host", default="mastomini.local", help="TLS name and Host header")
    parser.add_argument("--scheme", default="https", choices=["https", "http"])
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--ca", type=Path, default=CRATE / "certs" / "household-ca.crt")
    parser.add_argument("--modes", nargs="+", default=["h1", "h2"], choices=["h1", "h2"])
    parser.add_argument("--samples", type=int, default=20, help="sequential GETs per endpoint")
    parser.add_argument("--repeats", type=int, default=6, help="cold page views")
    parser.add_argument("--levels", type=int, nargs="+", default=[1, 3, 6])
    parser.add_argument("--duration", type=float, default=30.0, help="seconds per warm level")
    parser.add_argument("--think", type=float, default=0.5, help="seconds between a client's rounds")
    parser.add_argument("--page-connections", type=int, default=6)
    parser.add_argument("--warm-connections", type=int, default=1)
    parser.add_argument("--timeout", type=float, default=30.0)
    args = parser.parse_args()
    target = Target(args)

    report = {"label": args.label, "started": time.strftime("%Y-%m-%dT%H:%M:%S"), "host": args.host,
              "address": args.address, "scheme": args.scheme,
              "settings": {k: getattr(args, k) for k in ("samples", "repeats", "levels", "duration", "think",
                                                        "page_connections", "warm_connections")}}
    report["health_before"] = health(target)
    files, pages, api = site_files(target)
    report["site_files"], report["pages"], report["api"] = files, pages, api
    print(f"{len(files)} app files, {len(pages)} pages, {len(api)} API reads; "
          f"health {report['health_before']}")
    report["modes"] = {}
    for mode in args.modes:
        if mode == "h2":
            try:
                H2Client(target).close()
            except RuntimeError as error:
                report["modes"]["h2"] = {"skipped": str(error)}
                print(f"h2: skipped ({error})")
                continue
        result = {"endpoints": endpoints(target, mode, api + pages + files[:1], args)}
        for path, e in result["endpoints"].items():
            print(f"{mode} {path:45} {e['status']} {e['bytes']:>7}B  p50 {e['p50_ms']:>7} ms  "
                  f"p95 {e['p95_ms']:>7} ms  server app {e['server_app_ms']} lock {e['server_lock_ms']}")
        result["cold"] = cold(target, mode, files + api, args)
        c = result["cold"]
        print(f"{mode} cold page view: median {c['page_ms_median']} ms, p95 {c['page_ms_p95']} ms, "
              f"setup median {c['setup_ms_median']} ms, failed {c['failed_views']}")
        result["warm"] = []
        for level in args.levels:
            w = warm(target, mode, api, level, args)
            result["warm"].append(w)
            print(f"{mode} warm x{level}: {w['requests_per_s']} req/s, p50 {w['p50_ms']} ms, "
                  f"p95 {w['p95_ms']} ms, p99 {w['p99_ms']} ms, errors {w['errors']}, "
                  f"connection failures {w['connection_failures']}")
        report["modes"][mode] = result
    report["health_end"] = health(target)
    before, end = report["health_before"], report["health_end"]
    if before.get("uptime_ms") and end.get("uptime_ms") and end["uptime_ms"] < before["uptime_ms"]:
        report["rebooted"] = True
        print("WARNING: the board restarted during the run")
    out = ROOT / "reports" / f"transport-{args.label}-{time.strftime('%Y%m%d-%H%M%S')}.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps(report, indent=2))
    print(f"health end {end}")
    print(f"report: {out}")


if __name__ == "__main__":
    main()
