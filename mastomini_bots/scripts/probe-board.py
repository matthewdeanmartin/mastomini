"""Strict, read-only check of a running mastomini-bots board over HTTP and HTTPS.

Changes nothing on the board: it never sets the admin password, signs in,
saves settings or runs a bot. Retries while the board boots. Ends with
"Board probe passed" or exits 1.

HTTPS is verified strictly against certs/household-ca.crt, for the board's
name (mastomini-bots.local) even when --address is an IP: never with
verification turned off. The board must present exactly certs/server.crt and
serve exactly certs/household-ca.der at /ca, i.e. this build's files.

Run with mastomini's client-test environment:
    uv run --project ../mastomini_rs/clienttests python scripts/probe-board.py --address 192.168.1.170
"""
from __future__ import annotations

import argparse
import hashlib
import pathlib
import re
import socket
import ssl
import subprocess
import sys
import time

import requests
from requests.adapters import HTTPAdapter

CRATE = pathlib.Path(__file__).resolve().parent.parent


def local_version() -> str:
    text = (CRATE / 'Cargo.toml').read_text()
    return re.search(r'^version\s*=\s*"([^"]+)"', text, re.M).group(1)


def local_commit() -> str:
    try:
        return subprocess.run(['git', '-C', str(CRATE), 'rev-parse', '--short=12', 'HEAD'],
                              capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return ''


class NamedHost(HTTPAdapter):
    """Connect to an address but verify the certificate for `name` (SNI too)."""

    def __init__(self, name: str) -> None:
        self.name = name
        super().__init__()

    def init_poolmanager(self, *args, **kwargs):  # type: ignore[no-untyped-def]
        kwargs['server_hostname'] = self.name
        kwargs['assert_hostname'] = self.name
        super().init_poolmanager(*args, **kwargs)


def presented_certificate(address: str, name: str, ca: pathlib.Path) -> bytes:
    """The DER certificate the board presents, after strict verification."""
    context = ssl.create_default_context(cafile=str(ca))
    with socket.create_connection((address, 443), timeout=10) as raw:
        with context.wrap_socket(raw, server_hostname=name) as tls:
            return tls.getpeercert(binary_form=True) or b''


def probe_https(address: str, name: str, check) -> None:  # type: ignore[no-untyped-def]
    certs = CRATE / 'certs'
    ca = certs / 'household-ca.crt'
    if not ca.exists():
        check(False, f'{ca} exists (run make certs)')
        return
    started = time.monotonic()
    try:
        der = presented_certificate(address, name, ca)
    except (OSError, ssl.SSLError) as e:
        check(False, f'HTTPS verifies against the household CA for {name}: {e}')
        return
    check(True, f'HTTPS verifies against the household CA for {name} '
                f'(handshake {1000 * (time.monotonic() - started):.0f} ms)')
    built = ssl.PEM_cert_to_DER_cert((certs / 'server.crt').read_text())
    check(der == built, "the board presents this build's certificate (certs/server.crt)")

    https = requests.Session()
    https.verify = str(ca)
    https.mount('https://', NamedHost(name))
    status = https.get(f'https://{address}/api/v1/status', timeout=15).json()
    check(status.get('secure') is True, 'status over HTTPS says the connection is secure')
    check(bool(status.get('https')) and name in status['https'].get('names', []),
          f'status names the certificate for {name}')
    ca_der = (certs / 'household-ca.der').read_bytes()
    served = requests.get(f'http://{address}/ca', timeout=10)
    check(served.status_code == 200 and served.content == ca_der, '/ca serves exactly certs/household-ca.der')
    print(f"info household CA fingerprint {':'.join(f'{b:02X}' for b in hashlib.sha256(ca_der).digest())}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--address', required=True, help='board IP or hostname, e.g. mastomini-bots.local')
    parser.add_argument('--wait', type=int, default=90, help='seconds to wait for boot')
    parser.add_argument('--name', default='mastomini-bots.local',
                        help='name the HTTPS certificate must be valid for (default mastomini-bots.local)')
    args = parser.parse_args()
    base = f'http://{args.address}'
    failures: list[str] = []

    def check(ok: bool, message: str) -> None:
        print(f"{'ok  ' if ok else 'FAIL'} {message}")
        if not ok:
            failures.append(message)

    deadline = time.monotonic() + args.wait
    while True:
        try:
            status = requests.get(f'{base}/api/v1/status', timeout=5)
            break
        except requests.RequestException as e:
            if time.monotonic() > deadline:
                print(f'FAIL board not reachable at {base}: {e}')
                return 1
            time.sleep(2)

    check(status.status_code == 200, f'/api/v1/status answers 200 (got {status.status_code})')
    info = status.json()
    build = info.get('build', {})
    check(build.get('name') == 'mastomini-bots',
          f"this is a mastomini-bots board (it says {build.get('name')!r}; mastomini has no such field)")
    version = local_version()
    check(build.get('version') == version, f"firmware version is {version} (board says {build.get('version')!r})")
    commit = local_commit()
    print(f"info board build: commit {build.get('commit')}{' (dirty)' if build.get('dirty') else ''}, "
          f"built {build.get('built_at')}; this tree: {commit or 'unknown'}")
    print(f"info admin password set={not info.get('setup_needed')}, clock={info.get('clock')}")

    app = requests.get(f'{base}/app/', timeout=10, headers={'Accept-Encoding': 'gzip'})
    check(app.status_code == 200 and '<app-root>' in app.text, 'the admin app is served at /app/')
    check(requests.get(base, timeout=10, allow_redirects=False).headers.get('Location') == '/app/',
          '/ redirects to /app/')
    bots = requests.get(f'{base}/api/v1/bots', timeout=10)
    check(bots.status_code == 401, 'the bot list needs a sign-in (401 without one)')
    missing = requests.get(f'{base}/api/v1/does-not-exist', timeout=10)
    check(missing.status_code in (401, 404), 'unknown API routes are refused')
    probe_https(args.address, args.name, check)

    if failures:
        print(f'Board probe FAILED ({len(failures)} checks)')
        return 1
    print('Board probe passed')
    return 0


if __name__ == '__main__':
    sys.exit(main())
