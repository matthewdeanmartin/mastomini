"""Low-rate, read-only evidence capture while reproducing client failures.

Records JSONL immediately, so evidence survives an interrupted run. Public
board snapshots use its IP; TLS keeps certificate/hostname verification.
DNS runs in a killable child: socket timeouts do not bound getaddrinfo().
No credentials, request bodies, Wi-Fi identifiers or free-form logs are saved.
"""
import argparse
import http.client
import ipaddress
import json
import socket
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

from diag_common import board_args, tls_context


def dns_probe(host, timeout):
    started = time.monotonic()
    code = ("import socket,sys,json; print(json.dumps(sorted({r[4][0] for r in "
            "socket.getaddrinfo(sys.argv[1],443,type=socket.SOCK_STREAM)})))")
    try:
        result = subprocess.run([sys.executable, "-c", code, host],
                                capture_output=True, text=True, timeout=timeout)
        outcome = {"addresses": json.loads(result.stdout)} if result.returncode == 0 else {"error": "lookup_failed"}
    except subprocess.TimeoutExpired:
        outcome = {"error": "lookup_timeout"}
    return {**outcome, "ms": round((time.monotonic() - started) * 1000, 2)}


def snapshot(address, host, path, timeout):
    conn = http.client.HTTPConnection(address, timeout=timeout)
    try:
        conn.request("GET", path, headers={"Host": host, "Connection": "close"})
        response = conn.getresponse()
        if response.status != 200:
            raise ValueError(f"HTTP {response.status}")
        body = response.read(131073)
        if len(body) > 131072:
            raise ValueError("snapshot too large")
        return json.loads(body)
    finally:
        conn.close()


def safe_sys(value):
    # Explicit fields: do not accidentally retain SSID/MAC/IP or future secrets.
    return {**{k: value.get(k) for k in ("build", "uptime_ms", "reset_reason", "heap", "net")},
            "wifi": {k: value.get("wifi", {}).get(k) for k in ("rssi", "disconnects", "last_reason")}}


def safe_incidents(value):
    return {k: value.get(k) for k in ("boot_id", "dropped", "overwritten", "events", "samples",
                                     "counters", "high_water", "last_handshake_ms", "max_handshake_ms")}


def counter_delta(previous, current):
    """Never subtract across a reboot or an absent baseline."""
    if not previous or not current:
        return None
    if previous.get("boot_id") != current.get("boot_id"):
        return None
    before = {c["kind"]: c["count"] for c in previous.get("counters") or []}
    return {c["kind"]: c["count"] - before[c["kind"]]
            for c in current.get("counters") or []
            if c["kind"] in before and c["count"] != before[c["kind"]]}


def https_probe(args, context):
    started = time.monotonic()
    result = {"stage": "tcp"}
    raw = None
    sock = None
    try:
        raw = socket.create_connection((args.address, 443), timeout=args.timeout)
        connected = time.monotonic()
        result["tcp_ms"] = round((connected - started) * 1000, 2)
        result["stage"] = "tls"
        sock = context.wrap_socket(raw, server_hostname=args.host)
        secured = time.monotonic()
        result["tls_ms"] = round((secured - connected) * 1000, 2)
        result["stage"] = "http"
        sock.sendall((f"GET /api/v1/instance HTTP/1.1\r\nHost: {args.host}\r\n"
                      "Connection: close\r\n\r\n").encode("ascii"))
        response = http.client.HTTPResponse(sock)
        response.begin()
        result["status"] = response.status
        body = response.read(131073)
        if len(body) > 131072:
            raise ValueError("response too large")
        result["http_ms"] = round((time.monotonic() - secured) * 1000, 2)
        result["stage"] = "complete"
    except (OSError, ValueError, http.client.HTTPException) as exc:
        result["error"] = type(exc).__name__
    finally:
        if sock is not None:
            sock.close()
        if raw is not None:
            raw.close()
    result["ms"] = round((time.monotonic() - started) * 1000, 2)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    board_args(parser)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=60)
    parser.add_argument("--interval", type=float, default=10, help="seconds between samples, excluding collection")
    parser.add_argument("--timeout", type=float, default=5)
    args = parser.parse_args()
    ipaddress.ip_address(args.address)  # Snapshot retrieval must bypass DNS.
    if args.insecure:
        parser.error("this capture requires certificate verification")
    if args.samples < 1 or args.interval < 1 or args.timeout <= 0:
        parser.error("samples/interval must be >= 1 and timeout > 0")
    if any(c in args.host for c in "\r\n"):
        parser.error("invalid hostname")
    context = tls_context(args, alpn=("http/1.1",))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    previous = None
    with args.output.open("a", encoding="utf-8") as output:
        for index in range(args.samples):
            row = {"at": datetime.now(timezone.utc).isoformat()}
            for name, path, sanitize in (("sys", "/api/v1/sys", safe_sys),
                                         ("incidents", "/.well-known/incidents", safe_incidents)):
                try:
                    row[name] = sanitize(snapshot(args.address, args.host, path, args.timeout))
                except (OSError, ValueError, http.client.HTTPException) as exc:
                    row[name + "_error"] = type(exc).__name__
            current = row.get("incidents")
            row["counter_delta"] = counter_delta(previous, current)
            previous = current
            row["https"] = https_probe(args, context)
            row["dns"] = dns_probe(args.host, args.timeout)
            output.write(json.dumps(row, separators=(",", ":")) + "\n")
            output.flush()
            print(f"{row['at']} HTTPS={row['https']} DNS={row['dns']} "
                  f"incidents={row['counter_delta']}", flush=True)
            if index + 1 < args.samples:
                time.sleep(args.interval)


if __name__ == "__main__":
    main()
