"""Read-only client burst benchmark. Never provisions, logs in, or writes posts.

Token comes from MASTOMINI_BENCH_TOKEN; response bodies and tokens are not saved.
Each worker owns a Session. Reuse mode reports its first request separately;
later requests reuse a connection when the server permits it. New mode creates
a Session for every GET. Results are client wall time, not handler CPU time.
"""

from __future__ import annotations

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import json
import math
import os
import re
import subprocess
import sys
from pathlib import Path
import threading
import time
from urllib.parse import urlsplit

import requests
from requests.adapters import HTTPAdapter

DEFAULT_PATHS = ["/api/v2/instance", "/api/v1/accounts/verify_credentials",
                 "/api/v1/timelines/home?limit=20", "/api/v1/notifications?limit=20",
                 "/api/v1/featured_tags"]


class NamedHost(HTTPAdapter):
    """Connect by IP while retaining certificate hostname verification and SNI."""

    def __init__(self, name: str):
        self.name = name
        super().__init__()

    def init_poolmanager(self, *args, **kwargs):
        kwargs.update(server_hostname=self.name, assert_hostname=self.name)
        super().init_poolmanager(*args, **kwargs)


def endpoint(value: str) -> str:
    parsed = urlsplit(value)
    if not value.startswith("/") or value.startswith("//") or parsed.scheme or parsed.netloc or parsed.fragment:
        raise argparse.ArgumentTypeError("Use a relative /api/... path, not another host")
    if not parsed.path.startswith("/api/") or "\\" in value or any(ord(c) < 32 for c in value):
        raise argparse.ArgumentTypeError("Only /api/ GET endpoints are benchmarked")
    return value


def percentile(values: list[float], fraction: float) -> float | None:
    if not values:
        return None
    return round(sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)], 3)


def summarize(rows: list[dict]) -> dict:
    success = [r["ms"] for r in rows if isinstance(r["status"], int) and 200 <= r["status"] < 300]
    stages = {name: percentile([r["server_timing_ms"][name] for r in rows if name in r.get("server_timing_ms", {})], .95)
              for name in ("app", "body", "lock")}
    return {"samples": len(rows), "successful": len(success), "server_p95_ms": stages,
            "statuses": dict(Counter(str(r["status"]) for r in rows)),
            "success_p50_ms": percentile(success, .5),
            "success_p95_ms": percentile(success, .95),
            "success_max_ms": round(max(success), 3) if success else None,
            "all_max_ms": round(max((r["ms"] for r in rows), default=0), 3)}


def batch(base: str, path: str, workers: int, rounds: int, mode: str,
          token: str, verify: str | bool, timeout: float, tls_name: str | None = None) -> list[dict]:
    barrier = threading.Barrier(workers)

    def session() -> requests.Session:
        result = requests.Session()
        # Avoid proxy/netrc credentials influencing a local-board experiment.
        result.trust_env = False
        result.verify = verify
        if tls_name:
            result.mount("https://", NamedHost(tls_name))
            port = urlsplit(base).port
            result.headers["Host"] = tls_name + (f":{port}" if port and port != 443 else "")
        if token:
            result.headers["Authorization"] = f"Bearer {token}"
        return result

    def worker(index: int) -> list[dict]:
        rows = []
        http = session()
        try:
            barrier.wait()
            for number in range(rounds):
                if mode == "new" and number:
                    http.close()
                    http = session()
                started = time.perf_counter()
                size = 0
                metrics = {}
                headers_ms = None
                try:
                    with http.get(base + path, timeout=timeout, allow_redirects=False, stream=True) as response:
                        headers_ms = (time.perf_counter() - started) * 1000
                        metrics = {name: float(value) for name, value in re.findall(r"(app|body|lock);dur=([0-9.]+)", response.headers.get("Server-Timing", ""))}
                        status: int | str = response.status_code
                        size = len(response.content)
                except requests.RequestException as exc:
                    # Exception text may include URLs/credentials: save only its class.
                    status = type(exc).__name__
                rows.append({"worker": index, "phase": "first" if number == 0 else "subsequent",
                             "status": status, "bytes": size,
                             "headers_ms": headers_ms, "server_timing_ms": metrics,
                             "ms": (time.perf_counter() - started) * 1000})
        finally:
            http.close()
        return rows

    with ThreadPoolExecutor(max_workers=workers) as pool:
        return [row for result in pool.map(worker, range(workers)) for row in result]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base", help="Origin, e.g. https://mastomini.local")
    parser.add_argument("--path", action="append", type=endpoint, help="Repeat for specific endpoints")
    parser.add_argument("--concurrency", nargs="+", type=int, default=[1, 2, 4])
    parser.add_argument("--rounds", type=int, default=5, help="Requests per worker per endpoint/mode")
    parser.add_argument("--mode", choices=["new", "reuse", "both"], default="both")
    parser.add_argument("--timeout", type=float, default=30)
    parser.add_argument("--ca", help="Trusted household CA PEM; default uses normal certificate verification")
    parser.add_argument("--tls-name", help="HTTPS certificate hostname/SNI when connecting to an IP; verification stays enabled")
    parser.add_argument("--output", type=Path, help="Save metadata, summaries and raw timings as JSON")
    parser.add_argument("--label", default="", help="Firmware/build or experiment label stored with results")
    args = parser.parse_args()
    base = urlsplit(args.base)
    if base.scheme not in ("http", "https") or not base.hostname or base.username or base.password or base.query or base.fragment or base.path not in ("", "/"):
        parser.error("base must be an HTTP(S) origin with no credentials, path, query or fragment")
    if not all(1 <= n <= 16 for n in args.concurrency) or not 2 <= args.rounds <= 1000 or not 0 < args.timeout <= 120:
        parser.error("Use concurrency 1..16, rounds 2..1000, timeout >0..120")
    if args.tls_name and (base.scheme != "https" or not re.fullmatch(r"[A-Za-z0-9.-]+", args.tls_name)):
        parser.error("tls-name requires HTTPS and a plain DNS hostname")
    token = os.environ.get("MASTOMINI_BENCH_TOKEN", "")
    paths = args.path or DEFAULT_PATHS
    if not token and args.path is None:
        parser.error("Set MASTOMINI_BENCH_TOKEN or choose public endpoints with --path")
    output = {"created_utc": datetime.now(timezone.utc).isoformat(), "base": args.base.rstrip("/"),
              "label": args.label,
              "tls_name": args.tls_name,
              "authenticated": bool(token), "rounds_per_worker": args.rounds,
              "measurement": "client wall time including connect, TLS, queue, handler and transfer; no automatic retries",
              "results": []}
    # requests' connect timeout does not bound the OS resolver. Fail once,
    # promptly, instead of hanging through every sample on an unavailable .local.
    began = time.perf_counter()
    try:
        resolved = subprocess.run([sys.executable, "-c",
            "import socket,json,sys; print(json.dumps(sorted({r[4][0] for r in socket.getaddrinfo(sys.argv[1],None,type=socket.SOCK_STREAM)})))",
            base.hostname], capture_output=True, text=True, timeout=args.timeout, check=True)
        output["dns_preflight"] = {"ms": (time.perf_counter()-began)*1000, "addresses": json.loads(resolved.stdout)}
    except (subprocess.TimeoutExpired, subprocess.CalledProcessError) as exc:
        output["dns_preflight"] = {"ms": (time.perf_counter()-began)*1000, "error": type(exc).__name__}
        if args.output:
            args.output.write_text(json.dumps(output, indent=2)+"\n", encoding="utf-8")
        print("DNS preflight failed; no HTTP latency samples collected. Use a resolvable hostname or an IP for HTTP.")
        return 1
    failed = False
    for workers in args.concurrency:
        for mode in (["new", "reuse"] if args.mode == "both" else [args.mode]):
            for path in paths:
                print(f"Measuring {path}: workers={workers}, mode={mode}", flush=True)
                rows = batch(output["base"], path, workers, args.rounds, mode, token, args.ca or True, args.timeout, args.tls_name)
                phases = {phase: summarize([r for r in rows if r["phase"] == phase])
                          for phase in ("first", "subsequent")}
                output["results"].append({"path": path, "concurrency": workers, "mode": mode,
                                          "phases": phases, "samples": rows})
                failed |= any(s["successful"] != s["samples"] for s in phases.values())
                print(json.dumps(phases), flush=True)
    if args.output:
        args.output.write_text(json.dumps(output, indent=2) + "\n", encoding="utf-8")
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
