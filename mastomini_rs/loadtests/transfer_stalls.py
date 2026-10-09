"""Large-response delivery: how long a big body takes to arrive after the TLS
handshake, and how often it stalls mid-transfer.

    uv run --project loadtests python loadtests/transfer_stalls.py --address 192.168.1.161
    make diag-transfer ADDRESS=192.168.1.161

Fetches the household app's JavaScript bundle (tens of KiB, public, gzip)
--runs times, each on a fresh connection, and reports the transfer time
after the handshake (p50/p90/max) and stalls: gaps between arriving chunks
longer than --stall-ms. A gap of ~1 s is TCP waiting out a retransmission
timeout. With --scheme both it also measures plain HTTP, which separates the
TLS write path from the network.

Measured October 2026: an 11.5 KiB TCP send buffer gave p90 0.77 s, max
2.66 s and 5 stalls in 15; 32 KiB gave p90 0.47 s, max 0.52 s and none.

Safe: GET of public, static files only.
"""
import argparse
import gzip
import re
import socket
import sys
import time

from diag_common import board_args, connect, percentile, tls_context


def bundle_path(args) -> str:
    """The app's main script, read from /app/ (its name carries a content hash)."""
    if args.path:
        return args.path
    sock = socket.create_connection((args.address, 80), timeout=15)
    sock.sendall(f"GET /app/ HTTP/1.1\r\nHost: {args.host}\r\nAccept-Encoding: gzip\r\n"
                 "Connection: close\r\n\r\n".encode())
    body = b""
    while chunk := sock.recv(65536):
        body += chunk
    sock.close()
    head, _, payload = body.partition(b"\r\n\r\n")
    if b"content-encoding: gzip" in head.lower():
        payload = gzip.decompress(payload)
    match = re.search(rb'src="(main-[\w-]+\.js)"', payload)
    if not match:
        raise SystemExit("could not find the app bundle in /app/; pass --path")
    return "/app/" + match.group(1).decode()


def fetch(args, path: str, tls: bool):
    """One fresh connection, one GET; returns (handshake s, transfer s, bytes, stalls)."""
    started = time.monotonic()
    if tls:
        sock = connect(args, tls_context(args, alpn=("http/1.1",)), timeout=30)
    else:
        sock = socket.create_connection((args.address, 80), timeout=30)
    ready = time.monotonic()
    sock.sendall(f"GET {path} HTTP/1.1\r\nHost: {args.host}\r\nAccept-Encoding: gzip\r\n"
                 "Connection: close\r\n\r\n".encode())
    sent = last = time.monotonic()
    got = stalls = 0
    while data := sock.recv(65536):
        now = time.monotonic()
        if now - last > args.stall_ms / 1000:
            stalls += 1
        last = now
        got += len(data)
    sock.close()
    return ready - started, time.monotonic() - sent, got, stalls


def run(args, path: str, tls: bool) -> dict:
    times, sizes, stalls = [], set(), 0
    for _ in range(args.runs):
        _, transfer, got, stalled = fetch(args, path, tls)
        times.append(transfer)
        sizes.add(got)
        stalls += stalled
    times.sort()
    return {"scheme": "https" if tls else "http", "runs": args.runs, "bytes": max(sizes),
            "p50": percentile(times, 0.5), "p90": percentile(times, 0.9),
            "max": times[-1], "min": times[0], "stalls": stalls}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    board_args(parser)
    parser.add_argument("--path", help="what to fetch (default: the app bundle)")
    parser.add_argument("--runs", type=int, default=15)
    parser.add_argument("--stall-ms", type=float, default=300.0)
    parser.add_argument("--scheme", choices=["https", "http", "both"], default="https")
    args = parser.parse_args()
    path = bundle_path(args)
    schemes = {"https": [True], "http": [False], "both": [False, True]}[args.scheme]
    for tls in schemes:
        r = run(args, path, tls)
        print(f"{r['scheme']:5} {path} ~{r['bytes']} B x{r['runs']}: transfer after handshake "
              f"p50 {r['p50']:.2f}s p90 {r['p90']:.2f}s max {r['max']:.2f}s min {r['min']:.2f}s; "
              f"stalls >{args.stall_ms:.0f} ms: {r['stalls']}")
    sys.exit(0)


if __name__ == "__main__":
    main()
