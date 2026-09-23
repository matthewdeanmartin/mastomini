"""Strict, read-only check of a running mastomini board over HTTP.

Changes nothing on the board: no provisioning, no app registration, no posts.
Retries while the board boots. Ends with "Board probe passed" or exits 1.

Run with the client-test environment:
    uv run --project clienttests python scripts/probe-board.py --address 192.168.1.50
"""
from __future__ import annotations

import argparse
import pathlib
import re
import sys
import time

import requests

CRATE = pathlib.Path(__file__).resolve().parent.parent


def local_version() -> str:
    text = (CRATE / 'Cargo.toml').read_text()
    return re.search(r'^version\s*=\s*"([^"]+)"', text, re.M).group(1)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--address', required=True, help='board IP or hostname, e.g. mastomini.local')
    parser.add_argument('--wait', type=int, default=90, help='seconds to wait for boot')
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

    if failures:
        print(f'Board probe FAILED ({len(failures)} checks)')
        return 1
    print('Board probe passed')
    return 0


if __name__ == '__main__':
    sys.exit(main())
