"""Strict, read-only check of a running mastomini board over HTTP and HTTPS.

Changes nothing on the board: no provisioning, no app registration, no posts.
Retries while the board boots. Ends with "Board probe passed" or exits 1.

HTTPS is verified strictly against certs/household-ca.crt, for the board's
name (mastomini.local) even when --address is an IP: never with
verification turned off. The board must present exactly certs/mastomini.crt
and serve exactly certs/household-ca.der at /ca, i.e. this build's files.

Run with the client-test environment:
    uv run --project clienttests python scripts/probe-board.py --address 192.168.1.50
"""
from __future__ import annotations

import argparse
import hashlib
import pathlib
import re
import socket
import ssl
import sys
import time

import requests
from requests.adapters import HTTPAdapter

CRATE = pathlib.Path(__file__).resolve().parent.parent


def local_version() -> str:
    text = (CRATE / 'Cargo.toml').read_text()
    return re.search(r'^version\s*=\s*"([^"]+)"', text, re.M).group(1)


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
    try:
        der = presented_certificate(address, name, ca)
    except (OSError, ssl.SSLError) as e:
        check(False, f'HTTPS verifies against the household CA for {name}: {e}')
        return
    check(True, f'HTTPS verifies against the household CA for {name}')
    built = ssl.PEM_cert_to_DER_cert((certs / 'mastomini.crt').read_text())
    check(hashlib.sha256(der).digest() == hashlib.sha256(built).digest(),
          "the board presents this build's certificate (certs/mastomini.crt)")

    https = requests.Session()
    https.verify = str(ca)
    https.mount('https://', NamedHost(name))
    base = f'https://{address}'
    status = https.get(f'{base}/api/mastomini/v1/status', timeout=10).json()
    check(status.get('secure') is True and status.get('mode') == 'easy',
          f"status over HTTPS says secure, Easy mode (mode={status.get('mode')!r})")
    meta = https.get(f'{base}/.well-known/oauth-authorization-server', timeout=10).json()
    check(str(meta.get('authorization_endpoint', '')).startswith('https://'),
          'OAuth metadata over HTTPS points to HTTPS endpoints')
    ca_der = (certs / 'household-ca.der').read_bytes()
    served = requests.get(f'http://{address}/ca', timeout=10)
    check(served.status_code == 200 and served.content == ca_der,
          '/ca over HTTP serves exactly certs/household-ca.der')
    fingerprint = ':'.join(f'{b:02X}' for b in hashlib.sha256(ca_der).digest())
    trust = requests.get(f'http://{address}/trust', timeout=10)
    check(trust.status_code == 200 and fingerprint in trust.text, '/trust shows the CA fingerprint')
    print(f'info CA fingerprint {fingerprint}')


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--address', required=True, help='board IP or hostname, e.g. mastomini.local')
    parser.add_argument('--wait', type=int, default=90, help='seconds to wait for boot')
    parser.add_argument('--name', default='mastomini.local',
                        help='name the HTTPS certificate must be valid for (default mastomini.local)')
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
            instance = requests.get(f'{base}/api/v1/instance', timeout=5)
            break
        except requests.RequestException as e:
            if time.monotonic() > deadline:
                print(f'FAIL board not reachable at {base}: {e}')
                return 1
            time.sleep(2)

    version = local_version()
    check(instance.status_code == 200, f'/api/v1/instance answers 200 (got {instance.status_code})')
    info = instance.json()
    check(f'mastomini {version}' in info.get('version', ''), f"firmware version is {version} (board says {info.get('version')!r})")
    check(info.get('configuration', {}).get('statuses', {}).get('max_characters') == 140, '140-character limit advertised')
    check(info.get('registrations') is False, 'registrations closed')

    status = requests.get(f'{base}/api/mastomini/v1/status', timeout=5).json()
    check(status.get('available') is True, 'store available (not latched by a storage failure)')
    print(f"info provisioned={status.get('provisioned')} accounts={status.get('accounts')} "
          f"statuses={status.get('statuses')} store_used={status.get('store_used')}")

    v2 = requests.get(f'{base}/api/v2/instance', timeout=5)
    check(v2.status_code == 200 and 'domain' in v2.json(), '/api/v2/instance answers')
    missing = requests.get(f'{base}/api/v1/does-not-exist', timeout=5)
    check(missing.status_code == 404 and missing.json().get('error') == 'Record not found', 'unknown routes give Mastodon-shaped 404')
    pre = requests.options(f'{base}/api/v1/statuses', timeout=5)
    check(pre.headers.get('Access-Control-Allow-Origin') == '*', 'CORS preflight answers')
    page = requests.get(f'{base}/oauth/authorize', params={'client_id': 'probe', 'redirect_uri': 'x:y'},
                        timeout=5, allow_redirects=False)
    check(page.status_code == 400 and 'text/html' in page.headers.get('Content-Type', ''),
          'OAuth page rejects an unknown client without redirecting')
    png = requests.get(f'{base}/avatars/original/missing.png', timeout=5)
    check(png.content.startswith(b'\x89PNG'), 'default avatar served')
    meta = requests.get(f'{base}/.well-known/oauth-authorization-server', timeout=5)
    check(meta.json().get('code_challenge_methods_supported') == ['S256'], 'OAuth metadata advertises PKCE S256')
    probe_https(args.address, args.name, check)

    if failures:
        print(f'Board probe FAILED ({len(failures)} checks)')
        return 1
    print('Board probe passed')
    return 0


if __name__ == '__main__':
    sys.exit(main())
