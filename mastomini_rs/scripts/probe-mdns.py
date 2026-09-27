"""Read-only diagnosis of IP access, mDNS replies, and the OS name resolver.

Sends an IPv4 mDNS A query from an ephemeral port (RFC 6762 legacy query):
the question is multicast, the reply unicast. This does not prove multicast
replies to UDP 5353 reach the OS resolver. No firewall or DNS settings change.
"""
from __future__ import annotations

import argparse
import json
import socket
import struct
import subprocess
import sys
import time
import urllib.request


def name_at(packet: bytes, start: int) -> tuple[str, int]:
    labels = []
    at = start
    end = None
    seen = set()
    while True:
        if at in seen or at >= len(packet):
            raise ValueError('bad DNS name')
        seen.add(at)
        size = packet[at]
        if size & 0xc0 == 0xc0:
            if at + 1 >= len(packet):
                raise ValueError('short DNS pointer')
            end = end if end is not None else at + 2
            at = ((size & 0x3f) << 8) | packet[at + 1]
        elif size == 0:
            return '.'.join(labels).lower(), end if end is not None else at + 1
        else:
            if size > 63 or at + 1 + size > len(packet):
                raise ValueError('bad DNS label')
            labels.append(packet[at + 1:at + 1 + size].decode('ascii'))
            at += size + 1


def addresses(packet: bytes, name: str) -> list[str]:
    if len(packet) < 12:
        return []
    _, flags, questions, answers, authority, additional = struct.unpack('!6H', packet[:12])
    if not flags & 0x8000:
        return []
    at = 12
    found = []
    try:
        for _ in range(questions):
            _, at = name_at(packet, at)
            at += 4
        for _ in range(answers + authority + additional):
            owner, at = name_at(packet, at)
            kind, cls, ttl, size = struct.unpack('!HHIH', packet[at:at + 10])
            at += 10
            data = packet[at:at + size]
            if len(data) != size:
                raise ValueError('short DNS record')
            if owner == name.lower().rstrip('.') and kind == 1 and cls & 0x7fff == 1 and ttl and size == 4:
                found.append(socket.inet_ntoa(data))
            at += size
    except (ValueError, struct.error):
        return []
    return sorted(set(found))


def query(name: str, destination: str, interface: str, seconds: float) -> list[str]:
    labels = name.rstrip('.').encode('ascii').split(b'.')
    if any(not part or len(part) > 63 for part in labels):
        raise ValueError('invalid host name')
    packet = struct.pack('!6H', 0x4d4d, 0, 1, 0, 0, 0)
    packet += b''.join(bytes([len(part)]) + part for part in labels) + b'\0\0\1\0\1'
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.bind((interface, 0))
        sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF, socket.inet_aton(interface))
        sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_TTL, 255)
        began = time.monotonic()
        found = set()
        first_reply = None
        for _ in range(2):
            sock.sendto(packet, (destination, 5353))
            until = time.monotonic() + seconds / 2
            while time.monotonic() < until:
                sock.settimeout(max(0.01, until - time.monotonic()))
                try:
                    response, _ = sock.recvfrom(9000)
                except socket.timeout:
                    break
                matching = addresses(response, name)
                if matching and first_reply is None:
                    first_reply = time.monotonic() - began
                found.update(matching)
            if found:
                break
        timing = f'first reply {first_reply:.3f}s' if first_reply is not None else 'no reply'
        print(f'{destination}:5353: {sorted(found) or "no matching A reply"}; '
              f'{timing}, observed for {time.monotonic() - began:.2f}s', flush=True)
        return sorted(found)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--address', required=True, help='known board IPv4 address from its boot log')
    parser.add_argument('--name', default='mastomini.local')
    parser.add_argument('--interface', help='local IPv4 interface, default route to board')
    args = parser.parse_args()
    socket.inet_aton(args.address)
    if not args.name.rstrip('.').endswith('.local'):
        parser.error('--name must be a .local name')
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as route:
        route.connect((args.address, 5353))
        interface = args.interface or route.getsockname()[0]
    print(f'Board {args.address}, name {args.name}, local interface {interface}', flush=True)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    ip_ok = False
    try:
        with opener.open(f'http://{args.address}/api/mastomini/v1/version', timeout=5) as response:
            info = json.load(response)
            ip_ok = response.status == 200 and 'fingerprint' in info
            print(f'IP HTTP: {response.status}, firmware {info.get("fingerprint")}, '
                  f'uptime {info.get("uptime_ms")} ms', flush=True)
    except (OSError, ValueError) as error:
        print(f'IP HTTP failed: {error}', flush=True)
    replies = []
    for destination in [args.address, '224.0.0.251']:
        try:
            replies.append(query(args.name, destination, interface, 3))
        except OSError as error:
            print(f'UDP query failed: {error}', flush=True)
            replies.append([])
    began = time.monotonic()
    os_ok = False
    try:
        result = subprocess.run([sys.executable, '-c',
            'import json,socket,sys; print(json.dumps(sorted({r[4][0] for r in '
            'socket.getaddrinfo(sys.argv[1],80,type=socket.SOCK_STREAM)})))', args.name],
            capture_output=True, text=True, timeout=25)
        if result.returncode == 0:
            resolved = json.loads(result.stdout)
            os_ok = args.address in resolved
            print(f'OS resolver: {resolved}; expected address present={os_ok}')
        else:
            print(f'OS resolver failed: {result.stderr.strip().splitlines()[-1]}')
    except subprocess.TimeoutExpired:
        print('OS resolver timed out after 25 seconds')
    print(f'OS lookup elapsed {time.monotonic() - began:.2f}s')
    if not ip_ok:
        print('IP access failed too: check boot/Wi-Fi/address/server before blaming mDNS.')
    elif args.address in replies[1] and not os_ok:
        print('Board answers multicast questions; investigate OS resolver/cache, firewall, '
              'or multicast replies to port 5353. The tested replies were unicast.')
    elif args.address in replies[0] and args.address not in replies[1]:
        print('Board answers direct mDNS but not multicast queries on this interface: '
              'check multicast routing, AP isolation, VPN and firewall.')
    elif not os_ok:
        print('HTTP works but discovery is unverified: mDNS responder, UDP path, '
              'name conflict, or interface filtering remain possible.')
    else:
        print('IP access and OS name resolution both work now; compare elapsed times.')
    return 0 if ip_ok and os_ok else 1


if __name__ == '__main__':
    sys.exit(main())
