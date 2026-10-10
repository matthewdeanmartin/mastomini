"""What a client sees when the board closes an idle connection, and whether
TLS session resumption works.

    uv run --project loadtests python loadtests/idle_close.py --address 192.168.1.161 --idle 330
    uv run --project loadtests python loadtests/idle_close.py --address 192.168.1.161 --resume 8

--idle: opens an HTTP/2 connection, makes one request, then waits and
reports what arrives: a clean close is GOAWAY (code 0) followed by EOF at
the board's idle timeout (300 s since October 2026). A bare reset, or
nothing at all, is the kind of close that leaves a browser sending its next
request into a dead connection.

--resume N: N connections in a row, each resuming the previous TLS session
(a ticket), each answering one HTTP/2 request: resumed handshakes take tens
of milliseconds instead of about one second of ECDHE.

--handshakes N: N full handshakes (no resumption), then N resumed ones,
with p50/p90, the negotiated TLS version and cipher, and the board
certificate's key. --tls 1.2 or 1.3 pins the version (default: best offered).

Safe: one public GET or OPTIONS per connection.
"""
import argparse
import socket
import ssl
import time

import h2.config
import h2.connection
import h2.events

from diag_common import board_args, connect, tls_context


def h2_request(sock, host: str, timeout=10.0) -> tuple[h2.connection.H2Connection, str]:
    conn = h2.connection.H2Connection(h2.config.H2Configuration(client_side=True))
    conn.initiate_connection()
    sock.sendall(conn.data_to_send())
    sid = conn.get_next_available_stream_id()
    conn.send_headers(sid, [(":method", "GET"), (":path", "/api/v1/instance"), (":scheme", "https"),
                            (":authority", host)], end_stream=True)
    sock.sendall(conn.data_to_send())
    sock.settimeout(timeout)
    status = None
    while True:
        data = sock.recv(65536)
        if not data:
            return conn, f"EOF before the answer (status {status})"
        for event in conn.receive_data(data):
            if isinstance(event, h2.events.ResponseReceived):
                status = dict(event.headers).get(b":status", b"?").decode()
            elif isinstance(event, h2.events.StreamEnded):
                return conn, f"answered {status}"
            elif isinstance(event, h2.events.StreamReset):
                return conn, f"RST_STREAM {event.error_code}"
            elif isinstance(event, h2.events.ConnectionTerminated):
                return conn, f"GOAWAY {event.error_code}"
        out = conn.data_to_send()
        if out:
            sock.sendall(out)


def watch_idle(args):
    sock = connect(args, tls_context(args, alpn=("h2",)))
    conn, result = h2_request(sock, args.host)
    print(f"request: {result}; now idle for up to {args.idle:g} s")
    start = time.monotonic()
    sock.settimeout(1.0)
    seen = []
    while time.monotonic() - start < args.idle:
        try:
            data = sock.recv(65536)
        except TimeoutError:
            continue
        except (ssl.SSLError, OSError) as e:
            seen.append((time.monotonic() - start, f"{type(e).__name__}: {e}"))
            break
        if not data:
            seen.append((time.monotonic() - start, "EOF (clean TCP close)"))
            break
        for event in conn.receive_data(data):
            if isinstance(event, h2.events.ConnectionTerminated):
                seen.append((time.monotonic() - start,
                             f"GOAWAY code {event.error_code}, last stream {event.last_stream_id}"))
            elif isinstance(event, h2.events.PingReceived):
                seen.append((time.monotonic() - start, "PING from the board"))
    for at, what in seen:
        print(f"  at {at:6.1f} s: {what}")
    if not seen:
        print("  nothing arrived: the connection is still open")


def resume(args):
    ctx = tls_context(args, alpn=("h2",))
    session = None
    for n in range(args.resume):
        started = time.monotonic()
        sock = connect(args, ctx, session=session)
        handshake = (time.monotonic() - started) * 1000
        _, result = h2_request(sock, args.host)
        print(f"#{n} handshake {handshake:5.0f} ms resumed={sock.session_reused!s:5} "
              f"{sock.version()} alpn={sock.selected_alpn_protocol()} -> {result}")
        session = sock.session
        sock.close()
        time.sleep(0.5)


def handshakes(args):
    import statistics
    ctx = tls_context(args, alpn=("h2",))
    if args.tls:
        version = {"1.2": ssl.TLSVersion.TLSv1_2, "1.3": ssl.TLSVersion.TLSv1_3}[args.tls]
        ctx.minimum_version = ctx.maximum_version = version
    full, resumed, session, info = [], [], None, None
    for n in range(2 * args.handshakes):
        resume = n >= args.handshakes and session is not None
        started = time.monotonic()
        sock = connect(args, ctx, session=session if resume else None)
        ms = (time.monotonic() - started) * 1000
        if info is None:
            der = sock.getpeercert(binary_form=True)
            info = f"{sock.version()} {sock.cipher()[0]}, board key {key_kind(der)}"
        if resume and not sock.session_reused:
            print(f"  #{n}: resumption refused, full handshake {ms:.0f} ms")
        (resumed if resume and sock.session_reused else full).append(ms)
        _, result = h2_request(sock, args.host)
        if not result.startswith("answered"):
            print(f"  #{n}: {result}")
        session = sock.session
        sock.close()
        time.sleep(0.3)
    print(info)
    for label, values in (("full", full), ("resumed", resumed)):
        if values:
            values.sort()
            print(f"  {label:8} handshakes x{len(values)}: p50 {statistics.median(values):.0f} ms "
                  f"p90 {values[min(len(values) - 1, int(0.9 * len(values)))]:.0f} ms "
                  f"min {values[0]:.0f} max {values[-1]:.0f}")


def key_kind(der: bytes) -> str:
    """RSA or EC P-256, read from the certificate's algorithm identifiers."""
    if bytes.fromhex("2a8648ce3d0201") in der:  # id-ecPublicKey
        return "EC P-256" if bytes.fromhex("2a8648ce3d030107") in der else "EC (other curve)"
    if bytes.fromhex("2a864886f70d010101") in der:  # rsaEncryption
        return "RSA"
    return "unknown"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    board_args(parser)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--idle", type=float, help="seconds to wait on an idle connection")
    mode.add_argument("--resume", type=int, help="resumed connections to make")
    mode.add_argument("--handshakes", type=int, help="full, then resumed, handshakes to time")
    parser.add_argument("--tls", choices=["1.2", "1.3"], help="pin the TLS version (--handshakes)")
    args = parser.parse_args()
    if args.idle is not None:
        watch_idle(args)
    elif args.handshakes:
        handshakes(args)
    else:
        resume(args)


if __name__ == "__main__":
    main()
