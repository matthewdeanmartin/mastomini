"""Read-only TLS queue probe: established clients, new TLS, and silent TCP.

Run separately from other benchmarks. The silent peer deliberately occupies
one handshake slot until its socket/handshake timeout (about five seconds on
the current firmware).
No login, request bodies, tokens, or application data are saved.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import http.client
import json
from pathlib import Path
import socket
import ssl
import statistics
import threading
import time


def summary(rows):
    times = sorted(r['ms'] for r in rows)
    return dict(samples=len(rows), mean_ms=statistics.mean(times),
                median_ms=statistics.median(times), max_ms=max(times))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--address', required=True)
    parser.add_argument('--hostname', default='mastomini.local')
    parser.add_argument('--ca', type=Path, default=Path('certs/household-ca.crt'))
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    context = ssl.create_default_context(cafile=str(args.ca))
    report = dict(address=args.address, tests={})

    def connect(secure=True, session=None):
        raw = socket.create_connection((args.address, 443 if secure else 80), timeout=10)
        raw.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        if not secure:
            return raw
        try:
            return context.wrap_socket(raw, server_hostname=args.hostname, session=session)
        except BaseException:
            raw.close()
            raise

    def get(sock):
        started = time.perf_counter()
        sock.sendall(f'GET /api/v2/instance HTTP/1.1\r\nHost: {args.hostname}\r\n\r\n'.encode())
        response = http.client.HTTPResponse(sock)
        try:
            response.begin()
            body = response.read(1024 * 1024 + 1)
            assert response.status == 200, response.status
            assert len(body) <= 1024 * 1024 and response.isclosed()
            assert not response.will_close, 'Server closed persistent connection'
            return dict(ms=(time.perf_counter()-started)*1000, bytes=len(body),
                        server_timing=response.getheader('Server-Timing'))
        finally:
            response.close()

    # Warm every connection before the barrier: no setup in measured samples.
    for secure, count in [(False, 4), (True, 4), (True, 6)]:
        peers = []
        try:
            for _ in range(count):
                peers.append(connect(secure))
                get(peers[-1])
            barrier = threading.Barrier(count, timeout=10)
            def worker(peer):
                barrier.wait()
                return [get(peer) for _ in range(10)]
            with ThreadPoolExecutor(max_workers=count) as pool:
                rows = [r for batch in pool.map(worker, peers) for r in batch]
            name = f'{"https" if secure else "http"}_{count}_fully_established'
            report['tests'][name] = dict(summary=summary(rows), rows=rows)
            print(name, json.dumps(summary(rows)), flush=True)
        finally:
            for peer in peers:
                peer.close()

    hot = connect()
    try:
        get(hot)
        def cold():
            started = time.perf_counter()
            peer = connect()
            try:
                get(peer)
                return (time.perf_counter()-started)*1000
            finally:
                peer.close()
        with ThreadPoolExecutor(max_workers=1) as pool:
            future = pool.submit(cold)
            time.sleep(.1)
            rows = [get(hot) for _ in range(10)]
            report['tests']['hot_during_cold_handshake'] = dict(
                summary=summary(rows), rows=rows, cold_total_ms=future.result(timeout=10))
        # A TCP peer that never sends ClientHello, as a stalled/speculative
        # connection can do. Its presence must not block unrelated ready work.
        silent = socket.create_connection((args.address, 443), timeout=10)
        try:
            time.sleep(.1)
            rows = [get(hot) for _ in range(5)]
            assert silent.recv(1) == b'', 'Handshake timeout did not close peer'
            report['tests']['hot_during_silent_handshake'] = dict(summary=summary(rows), rows=rows)
        finally:
            silent.close()
        # Verify the deployed ticket optimization independently of persistence.
        session = hot.session
        started = time.perf_counter()
        resumed = connect(session=session)
        try:
            get(resumed)
            report['tests']['resumption'] = dict(reused=resumed.session_reused,
                total_ms=(time.perf_counter()-started)*1000)
        finally:
            resumed.close()
    finally:
        hot.close()
    args.output.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps({k: {n: v for n, v in result.items() if n != 'rows'}
                      for k, result in report['tests'].items()}, indent=2))


if __name__ == '__main__':
    main()
