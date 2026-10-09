"""A read-only token for the diagnostics, by the same OAuth flow a Mastodon
client uses (authorization code + PKCE, out-of-band redirect).

    uv run --project loadtests python loadtests/oauth_token.py --address 192.168.1.161 --username matt
    uv run --project loadtests python loadtests/oauth_token.py --address 192.168.1.161 --revoke

The password comes from MASTOMINI_PASSWORD or a prompt; it is never stored
or printed, and neither is the token. The token, with the app's client id
and secret, goes to <repo>/.local/diag-token (repo-ignored), where
h2_burst.py and browser_session.py find it. One app ("mastomini
diagnostics") is registered once and reused, so repeated runs do not pile up
registered apps.

Mind the session cap: a member keeps at most 8 sessions, and signing in a
ninth evicts the least recently used one. Revoke the diagnostic token when
done (--revoke), so it never costs a real device its session.
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
import sys
from pathlib import Path
from urllib.parse import urlencode

from diag_common import DEFAULT_TOKEN_FILE, board_args, connect, tls_context

APP_NAME = "mastomini diagnostics"
OOB = "urn:ietf:wg:oauth:2.0:oob"


def post(args, path: str, fields: dict) -> tuple[int, bytes]:
    conn = http.client.HTTPSConnection(args.host, 443, timeout=30)
    conn.sock = connect(args, tls_context(args, alpn=("http/1.1",)))
    conn.request("POST", path, urlencode(fields),
                 {"Content-Type": "application/x-www-form-urlencoded", "Host": args.host})
    response = conn.getresponse()
    body = response.read()
    conn.close()
    return response.status, body


def load(path: Path) -> dict:
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return {}


def save(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data))
    try:
        os.chmod(path, 0o600)
    except OSError:
        pass


def app(args, saved: dict) -> dict:
    if saved.get("client_id") and saved.get("client_secret"):
        return saved
    status, body = post(args, "/api/v1/apps",
                        {"client_name": APP_NAME, "redirect_uris": OOB, "scopes": "read"})
    if status != 200:
        raise SystemExit(f"registering the app failed: HTTP {status}")
    reply = json.loads(body)
    return {"client_id": reply["client_id"], "client_secret": reply["client_secret"]}


def sign_in(args) -> None:
    saved = load(args.token_file)
    creds = app(args, saved)
    password = os.environ.get("MASTOMINI_PASSWORD") or getpass.getpass(f"password for {args.username}: ")
    status, page, verifier = authorize(args, creds, password)
    if b"unknown client" in page and creds is saved:
        # The board drops apps without live tokens once it holds 32: register again.
        creds = app(args, {})
        status, page, verifier = authorize(args, creds, password)
    match = re.search(rb'<code id="code">([^<]+)</code>', page)
    if not match:
        reason = re.search(rb"Wrong username or password|Too many[^<]*|unknown client", page)
        raise SystemExit(f"sign-in failed (HTTP {status}): "
                         f"{reason.group(0).decode() if reason else 'no authorization code in the page'}")
    status, body = post(args, "/oauth/token", {
        "grant_type": "authorization_code", "code": match.group(1).decode(), "redirect_uri": OOB,
        "client_id": creds["client_id"], "client_secret": creds["client_secret"], "code_verifier": verifier})
    if status != 200:
        raise SystemExit(f"exchanging the code failed: HTTP {status}")
    token = json.loads(body)
    save(args.token_file, {**creds, "access_token": token["access_token"], "scope": token.get("scope")})
    print(f"read token saved to {args.token_file} (scope {token.get('scope')}). "
          "Revoke it when done: --revoke")


def authorize(args, creds: dict, password: str) -> tuple[int, bytes, str]:
    """The sign-in form, as a browser posts it; returns the page and the PKCE verifier."""
    verifier = secrets.token_urlsafe(48)
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    status, page = post(args, "/oauth/authorize", {
        "client_id": creds["client_id"], "redirect_uri": OOB, "response_type": "code", "scope": "read",
        "code_challenge": challenge, "code_challenge_method": "S256",
        "username": args.username, "password": password, "decision": "allow"})
    return status, page, verifier


def revoke(args) -> None:
    saved = load(args.token_file)
    if not saved.get("access_token"):
        raise SystemExit(f"no token in {args.token_file}")
    status, _ = post(args, "/oauth/revoke", {"token": saved["access_token"],
                                             "client_id": saved["client_id"],
                                             "client_secret": saved["client_secret"]})
    # Keep the app's credentials: the next sign-in reuses the registration.
    save(args.token_file, {k: v for k, v in saved.items() if k in ("client_id", "client_secret")})
    print(f"revoked (HTTP {status}); the app registration is kept for next time")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    board_args(parser)
    parser.add_argument("--username")
    parser.add_argument("--token-file", type=Path, default=DEFAULT_TOKEN_FILE)
    parser.add_argument("--revoke", action="store_true")
    args = parser.parse_args()
    if args.revoke:
        revoke(args)
    elif args.username:
        sign_in(args)
    else:
        parser.error("--username (to sign in) or --revoke")
    sys.exit(0)


if __name__ == "__main__":
    main()
