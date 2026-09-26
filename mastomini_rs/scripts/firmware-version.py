"""Does the board run the firmware in this working tree? Read-only.

Asks the board's public GET /api/mastomini/v1/version and compares its build
fingerprint with one computed here the way build.rs computes it: a SHA-256
over every firmware input (Rust sources, lockfile, sdkconfig, certificates,
assets, the household app's sources), committed or not. Test-only files are
left out, so editing a test does not call for a flash.

Exit codes: 0 up to date, 1 the board needs flashing, 2 the board could not
be asked (or predates the version endpoint, which also means: flash it).

    uv run --project clienttests python scripts/firmware-version.py --address 192.168.1.161
    uv run --project clienttests python scripts/firmware-version.py --local
"""
from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import subprocess
import sys

import requests

CRATE = pathlib.Path(__file__).resolve().parent.parent
REPO = CRATE.parent
VERSION_PATH = '/api/mastomini/v1/version'

# Keep in step with FINGERPRINT_INPUTS in build.rs.
INPUTS = [
    'mastomini_rs/Cargo.toml',
    'mastomini_rs/Cargo.lock',
    'mastomini_rs/build.rs',
    'mastomini_rs/sdkconfig.defaults',
    'mastomini_rs/partitions.csv',
    'mastomini_rs/components_esp32s3.lock',
    'mastomini_rs/src',
    'mastomini_rs/assets',
    'mastomini_rs/scripts/bundle-web.mjs',
    'mastomini_rs/certs/mastomini.crt',
    'mastomini_rs/certs/household-ca.der',
    'mastomini_rs/certs/certificate.json',
    'mastomini_ui/src',
    'mastomini_ui/public',
    'mastomini_ui/package.json',
    'mastomini_ui/package-lock.json',
    'mastomini_ui/angular.json',
    'mastomini_ui/tsconfig.json',
    'mastomini_ui/tsconfig.app.json',
    'mastomini_ui/proxy.conf.mjs',
]


def test_only(name: str) -> bool:
    return name in ('tests', 'tests.rs', 'testkit.rs') or name.endswith('.spec.ts')


def collect(repo: pathlib.Path, path: pathlib.Path, out: list[str]) -> None:
    if test_only(path.name):
        return
    if path.is_dir():
        for child in path.iterdir():
            collect(repo, child, out)
    elif path.is_file():
        out.append(path.relative_to(repo).as_posix())


def fingerprint(repo: pathlib.Path = REPO) -> str:
    """build.rs's fingerprint of the working tree."""
    files: list[str] = []
    missing: list[str] = []
    for item in INPUTS:
        path = repo / item
        if path.exists():
            collect(repo, path, files)
        else:
            missing.append(item)
    digest = hashlib.sha256()
    # Rust sorts the UTF-8 bytes; so does this.
    for name in sorted(files, key=lambda f: f.encode()):
        data = (repo / name).read_bytes()
        digest.update(name.encode() + b'\0' + str(len(data)).encode() + b'\0' + data)
    for item in sorted(missing, key=lambda f: f.encode()):
        digest.update(b'missing\0' + item.encode() + b'\0')
    return digest.hexdigest()[:12]


def git(*args: str) -> str:
    try:
        return subprocess.run(['git', '-C', str(REPO), *args], capture_output=True,
                              text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return ''


def board_build(address: str, timeout: float) -> dict:
    """The board's /version over plain HTTP: it needs no certificate, and a
    board whose HTTPS is broken is exactly one that may need flashing."""
    url = f'http://{address}{VERSION_PATH}'
    response = requests.get(url, timeout=timeout)
    if response.status_code == 404:
        raise LookupError('the board has no version endpoint: its firmware predates it')
    response.raise_for_status()
    return response.json()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument('--address', help='Board IP or name, e.g. 192.168.1.161')
    target.add_argument('--local', action='store_true', help="Print this tree's fingerprint only")
    parser.add_argument('--timeout', type=float, default=10.0)
    parser.add_argument('--json', action='store_true', help='Machine-readable result')
    args = parser.parse_args()

    local = fingerprint()
    commit = git('rev-parse', '--short=12', 'HEAD')
    dirty = bool(git('status', '--porcelain', '--untracked-files=no'))
    if args.local:
        print(local)
        return 0

    try:
        board = board_build(args.address, args.timeout)
    except (requests.RequestException, LookupError, ValueError) as e:
        result = {'status': 'unknown', 'reason': str(e), 'local': local}
        print(json.dumps(result) if args.json else f'Could not read the board build: {e}')
        return 2

    current = board.get('fingerprint') == local
    result = {
        'status': 'current' if current else 'stale',
        'local': {'fingerprint': local, 'commit': commit, 'dirty': dirty},
        'board': board,
    }
    if args.json:
        print(json.dumps(result, indent=2))
    else:
        tree = f"{commit or 'no commit'}{' + uncommitted changes' if dirty else ''}"
        built = f"{board.get('commit') or 'unknown commit'}{' (dirty)' if board.get('dirty') else ''}"
        print(f"working tree  {local}  {tree}")
        print(f"board         {board.get('fingerprint')}  {built}, built {board.get('built_at')}, "
              f"up {int(board.get('uptime_ms', 0)) // 1000} s")
        if current:
            print('Board firmware matches this working tree.')
        else:
            print('Board firmware differs from this working tree: flash it (make deploy PORT=COM11).')
    return 0 if current else 1


if __name__ == '__main__':
    sys.exit(main())
