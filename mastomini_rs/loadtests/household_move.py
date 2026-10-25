"""Move a household between boards (spec/board_migration_plan.md, path 2),
from the command line instead of the household app's *Move to another board*.

    uv run --project loadtests python loadtests/household_move.py export --address 192.168.1.161
    uv run --project loadtests python loadtests/household_move.py import --address 192.168.1.164 \\
        --file ../.local/move/mastomini-20261021.mmexport

export: signs in as the owner (OAuth + PKCE, scope read write), downloads the
encrypted archive (POST /api/mastomini/v1/admin/export) with a generated
passphrase, then revokes its token on the source. The owner comes from
--username/MASTOMINI_PASSWORD or the repo-root .env (USER, PASSWORD).

import: refuses a provisioned target, uploads the archive
(POST /api/mastomini/v1/import), then revokes the move token there too: the
archive holds every session, including the one that made it.

Everything goes to <repo>/.local/move/ (repo-ignored): the archive, its
passphrase and the move token. The archive holds password and token hashes;
delete the folder once the new board is verified.
"""
import argparse
import base64
import getpass
import hashlib
import http.client
import json
import os
import re
import secrets
import struct
import sys
import time
from pathlib import Path
from urllib.parse import urlencode

from diag_common import CRATE, board_args, connect, tls_context

APP_NAME = "mastomini household move"
OOB = "urn:ietf:wg:oauth:2.0:oob"
MOVE_DIR = CRATE.parent / ".local" / "move"


def request(args, method, path, data=None, headers=None, timeout=120):
    conn = http.client.HTTPSConnection(args.host, 443, timeout=timeout)
    conn.sock = connect(args, tls_context(args, alpn=("http/1.1",)), timeout=timeout)
    all_headers = {"Host": args.host, "Accept": "application/json", **(headers or {})}
    if isinstance(data, dict):
        data = urlencode(data)
        all_headers["Content-Type"] = "application/x-www-form-urlencoded"
    conn.request(method, path, data, all_headers)
    response = conn.getresponse()
    body = response.read()
    conn.close()
    return response.status, body


def status(args, wait_clock=True) -> dict:
    """The board's status; by default waits for SNTP, since a board refuses
    writes (503) until its clock is set, which takes a while after boot."""
    deadline = time.monotonic() + 300
    while True:
        try:
            code, body = request(args, "GET", "/api/mastomini/v1/status", timeout=20)
        except OSError:
            code, body = 0, b""
        if code == 200:
            reply = json.loads(body)
            if not wait_clock or reply.get("clock") == "synced":
                return reply
        if time.monotonic() > deadline:
            raise SystemExit(f"status: not ready after 5 minutes (HTTP {code}, clock {body[:200]!r})")
        time.sleep(5)


def owner(args) -> tuple[str, str]:
    env = {}
    path = CRATE.parent / ".env"
    if path.exists():
        for line in path.read_text().splitlines():
            key, _, value = line.partition("=")
            env[key.strip()] = value.strip().strip('"')
    username = args.username or env.get("USER")
    if not username:
        raise SystemExit("--username, or USER in the repo-root .env")
    password = os.environ.get("MASTOMINI_PASSWORD") or (env.get("PASSWORD") if not args.username else None) \
        or getpass.getpass(f"password for {username}: ")
    return username, password


def sign_in(args, username: str, password: str) -> dict:
    code, body = request(args, "POST", "/api/v1/apps",
                         {"client_name": APP_NAME, "redirect_uris": OOB, "scopes": "read write"})
    if code != 200:
        raise SystemExit(f"registering the app failed: HTTP {code}")
    app = json.loads(body)
    verifier = secrets.token_urlsafe(48)
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    code, page = request(args, "POST", "/oauth/authorize", {
        "client_id": app["client_id"], "redirect_uri": OOB, "response_type": "code",
        "scope": "read write", "code_challenge": challenge, "code_challenge_method": "S256",
        "username": username, "password": password, "decision": "allow"})
    match = re.search(rb'<code id="code">([^<]+)</code>', page)
    if not match:
        reason = re.search(rb"Wrong username or password|Too many[^<]*", page)
        raise SystemExit(f"sign-in failed (HTTP {code}): {reason.group(0).decode() if reason else 'no code'}")
    code, body = request(args, "POST", "/oauth/token", {
        "grant_type": "authorization_code", "code": match.group(1).decode(), "redirect_uri": OOB,
        "client_id": app["client_id"], "client_secret": app["client_secret"], "code_verifier": verifier})
    if code != 200:
        raise SystemExit(f"exchanging the code failed: HTTP {code}")
    return {"client_id": app["client_id"], "client_secret": app["client_secret"],
            "access_token": json.loads(body)["access_token"]}


def revoke(args, session: dict) -> int:
    code, _ = request(args, "POST", "/oauth/revoke", {
        "token": session["access_token"], "client_id": session["client_id"],
        "client_secret": session["client_secret"]})
    return code


def private_write(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    try:
        os.chmod(path, 0o600)
    except OSError:
        pass


def header(archive: bytes) -> dict:
    """The archive's plain header (domain/transfer.rs)."""
    if archive[:8] != b"MMEXPORT":
        raise SystemExit("not a mastomini export (no MMEXPORT magic)")
    version, schema, created_ms, records = struct.unpack_from("<HHQI", archive, 8)
    return {"version": version, "schema": schema, "records": records,
            "created": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(created_ms / 1000))}


def export(args) -> None:
    before = status(args)
    if not before.get("provisioned"):
        raise SystemExit("REFUSED: the source board has no household to export")
    print(f"source {args.address}: {before['accounts']} accounts, {before['statuses']} statuses")
    username, password = owner(args)
    session = sign_in(args, username, password)
    code, body = request(args, "GET", "/api/mastomini/v1/me/devices",
                         headers={"Authorization": f"Bearer {session['access_token']}"}, timeout=30)
    if code == 200:
        print(f"owner sessions after this sign-in: {len(json.loads(body))} (cap 8)")
    passphrase = secrets.token_urlsafe(24)
    started = time.monotonic()
    code, archive = request(args, "POST", "/api/mastomini/v1/admin/export",
                            {"password": password, "passphrase": passphrase},
                            headers={"Authorization": f"Bearer {session['access_token']}"}, timeout=180)
    if code != 200:
        revoke(args, session)
        raise SystemExit(f"export failed: HTTP {code} {archive[:200]!r}")
    seconds = time.monotonic() - started
    info = header(archive)
    out = args.out or MOVE_DIR / f"{args.host.split('.')[0]}-{info['created'][:10].replace('-', '')}.mmexport"
    private_write(out, archive)
    private_write(out.with_suffix(".passphrase"), passphrase.encode())
    private_write(out.with_suffix(".session"), json.dumps(session).encode())
    digest = hashlib.sha256(archive).hexdigest()
    print(f"export: {len(archive)} bytes in {seconds:.1f} s, sha256 {digest}")
    print(f"  schema {info['schema']}, {info['records']} records, taken {info['created']}")
    print(f"  {out} (+ .passphrase, .session)")
    print(f"revoked the move token on the source (HTTP {revoke(args, session)})")


def import_(args) -> None:
    target = status(args)
    if target.get("provisioned"):
        raise SystemExit("REFUSED: the target board is provisioned; import only onto an erased store")
    archive = args.file.read_bytes()
    info = header(archive)
    passphrase = args.file.with_suffix(".passphrase").read_text().strip()
    print(f"target {args.address}: unprovisioned; importing schema {info['schema']}, "
          f"{info['records']} records taken {info['created']}")
    started = time.monotonic()
    code, body = request(args, "POST", "/api/mastomini/v1/import", archive, {
        "Content-Type": "application/octet-stream", "X-Mastomini-Passphrase": passphrase}, timeout=300)
    if code != 200:
        raise SystemExit(f"import failed: HTTP {code} {body[:300]!r}")
    print(f"import: {json.loads(body)} in {time.monotonic() - started:.1f} s")
    session_file = args.file.with_suffix(".session")
    if session_file.exists():
        print(f"revoked the move token on the target (HTTP {revoke(args, json.loads(session_file.read_text()))})")
        session_file.unlink()
    after = status(args)
    print(f"target now: provisioned={after['provisioned']}, {after['accounts']} accounts, "
          f"{after['statuses']} statuses")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    ex = sub.add_parser("export", help="download the household from the source board")
    board_args(ex)
    ex.add_argument("--username", help="the owner (default USER in the repo-root .env)")
    ex.add_argument("--out", type=Path, help="archive path (default <repo>/.local/move/)")
    im = sub.add_parser("import", help="restore the household onto an erased board")
    board_args(im)
    im.add_argument("--file", type=Path, required=True, help="the .mmexport (passphrase beside it)")
    args = parser.parse_args()
    export(args) if args.command == "export" else import_(args)
    sys.exit(0)


if __name__ == "__main__":
    main()
