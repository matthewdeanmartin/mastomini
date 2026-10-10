"""Provision an EMPTY mastomini board and fill it with a small, realistic
household: an owner, members invited the way the household app does it,
posts with hashtags and mentions, replies, follows, favourites and boosts.

    uv run --project loadtests python loadtests/seed_board.py --address 192.168.1.164 \\
        --host mastomini-p4.local --ca ../mastomini_rs/certs-p4/household-ca.crt

For test boards only. It refuses a board that is already provisioned: it
never adds strangers to a real household. Everything goes through the public
flows (/setup, /api/mastomini/v1/admin/invites, /api/mastomini/v1/codes/redeem,
OAuth with PKCE, the Mastodon API), so seeding also exercises them.

Accounts and passwords go to <repo>/.local/seed-<host>.json (repo-ignored).
Writes are rate-limited on the board (120 per account and 600 in all per
hour); the defaults use about 85, and a 429 waits and retries.
"""
import argparse
import base64
import hashlib
import http.client
import json
import random
import re
import secrets
import sys
import time
from pathlib import Path
from urllib.parse import urlencode

from diag_common import CRATE, board_args, connect, tls_context

OOB = "urn:ietf:wg:oauth:2.0:oob"
SCOPES = "read write follow"
TOPICS = ["garden", "dinner", "bikes", "books", "weather", "music", "chores", "birds"]
LINES = [
    "Tomatoes finally turned red. #{t}",
    "Who moved the scissors again? Asking for a friend.",
    "Tried a new recipe tonight, it was better than expected. #{t}",
    "Reminder: recycling goes out Thursday.",
    "Saw a heron by the creek this morning! #{t}",
    "Reading by the window while it rains. #{t}",
    "Anyone up for a walk after lunch?",
    "The new playlist is on the kitchen speaker. #{t}",
    "Fixed the squeaky door hinge. Small victories.",
    "Weekend plan: absolutely nothing. #{t}",
    "A longer post, so timelines are realistic: " + "lorem ipsum dolor sit amet " * 12,
    "Photos later, the light was perfect today. #{t}",
]


class Board:
    """HTTPS to the board by address, with SNI and Host set to its name; one
    keep-alive connection, reopened when the board closes it."""

    def __init__(self, args):
        self.args = args
        self.conn = None

    def request(self, method, path, form=None, body=None, token=None, retries=4):
        headers = {"Host": self.args.host, "Accept": "application/json"}
        data = None
        if form is not None:
            data = urlencode(form)
            headers["Content-Type"] = "application/x-www-form-urlencoded"
        elif body is not None:
            data = json.dumps(body)
            headers["Content-Type"] = "application/json"
        if token:
            headers["Authorization"] = f"Bearer {token}"
        for attempt in range(retries):
            try:
                if self.conn is None:
                    self.conn = http.client.HTTPSConnection(self.args.host, 443, timeout=30)
                    self.conn.sock = connect(self.args, tls_context(self.args, alpn=("http/1.1",)))
                self.conn.request(method, path, data, headers)
                response = self.conn.getresponse()
                payload = response.read()
            except (OSError, http.client.HTTPException):
                self.conn = None
                time.sleep(1 + attempt)
                continue
            if response.status == 429:
                wait = int(response.getheader("Retry-After") or 30)
                print(f"    rate limited; waiting {wait} s")
                time.sleep(wait)
                continue
            return response.status, payload
        raise SystemExit(f"{method} {path}: no answer after {retries} attempts")

    def json(self, method, path, **kw):
        status, payload = self.request(method, path, **kw)
        if status >= 300:
            raise SystemExit(f"{method} {path}: HTTP {status} {payload[:200]!r}")
        return json.loads(payload) if payload else None


def sign_in(board: Board, app: dict, username: str, password: str) -> str:
    verifier = secrets.token_urlsafe(48)
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    status, page = board.request("POST", "/oauth/authorize", form={
        "client_id": app["client_id"], "redirect_uri": OOB, "response_type": "code", "scope": SCOPES,
        "code_challenge": challenge, "code_challenge_method": "S256",
        "username": username, "password": password, "decision": "allow"})
    match = re.search(rb'<code id="code">([^<]+)</code>', page)
    if not match:
        raise SystemExit(f"sign-in failed for {username} (HTTP {status})")
    token = board.json("POST", "/oauth/token", form={
        "grant_type": "authorization_code", "code": match.group(1).decode(), "redirect_uri": OOB,
        "client_id": app["client_id"], "client_secret": app["client_secret"], "code_verifier": verifier})
    return token["access_token"]


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    board_args(parser)
    parser.add_argument("--title", default="Test household")
    parser.add_argument("--owner", default="owner")
    parser.add_argument("--members", default="alice,bob,carol", help="comma-separated usernames")
    parser.add_argument("--posts", type=int, default=12, help="posts per person")
    parser.add_argument("--seed", type=int, default=42, help="random seed (repeatable content)")
    parser.add_argument("--out", type=Path, help="credentials file (default <repo>/.local/seed-<host>.json)")
    args = parser.parse_args()
    rng = random.Random(args.seed)
    out = args.out or CRATE.parent / ".local" / f"seed-{args.host}.json"
    board = Board(args)

    status = board.json("GET", "/api/mastomini/v1/status")
    if status.get("provisioned"):
        raise SystemExit("REFUSED: this board is already provisioned; seed only an empty test board.")

    people = [args.owner] + [m.strip() for m in args.members.split(",") if m.strip()]
    passwords = {p: secrets.token_urlsafe(12) for p in people}
    print(f"provisioning {args.host} with owner {args.owner}")
    s, page = board.request("POST", "/setup", form={
        "title": args.title, "username": args.owner,
        "password": passwords[args.owner], "password_confirm": passwords[args.owner]})
    if s != 200 or b"All set" not in page:
        raise SystemExit(f"setup failed (HTTP {s})")

    app = board.json("POST", "/api/v1/apps", form={
        "client_name": "mastomini seed", "redirect_uris": OOB, "scopes": SCOPES})
    tokens = {args.owner: sign_in(board, app, args.owner, passwords[args.owner])}
    for member in people[1:]:
        invite = board.json("POST", "/api/mastomini/v1/admin/invites", token=tokens[args.owner])
        board.json("POST", "/api/mastomini/v1/codes/redeem", body={
            "code": invite["code"], "username": member, "password": passwords[member],
            "display_name": member.capitalize()})
        tokens[member] = sign_in(board, app, member, passwords[member])
        print(f"  member {member} joined")
    ids = {p: board.json("GET", "/api/v1/accounts/verify_credentials", token=t)["id"]
           for p, t in tokens.items()}
    # Saved now, so an interrupted run never leaves accounts nobody can sign in to.
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps({"host": args.host, "address": args.address, "app": app,
                               "accounts": {p: {"id": ids[p], "password": passwords[p], "token": tokens[p]}
                                            for p in people}}, indent=1))
    limit = (board.json("GET", "/api/v2/instance").get("configuration", {})
             .get("statuses", {}).get("max_characters", 500))

    for p in people:  # everyone follows everyone
        for other in people:
            if other != p:
                board.json("POST", f"/api/v1/accounts/{ids[other]}/follow", token=tokens[p])
    posts = []
    for n in range(args.posts):
        for p in people:
            text = rng.choice(LINES).format(t=rng.choice(TOPICS))
            if n % 4 == 3:
                text = f"@{rng.choice([o for o in people if o != p])} " + text
            text = text[:limit].rstrip()  # the instance's character limit
            reply_to = rng.choice(posts)["id"] if posts and n % 3 == 2 else None
            form = {"status": text, "visibility": "public"}
            if reply_to:
                form["in_reply_to_id"] = reply_to
            posts.append(board.json("POST", "/api/v1/statuses", form=form, token=tokens[p]))
    for p in people:  # a few favourites and boosts of others' posts
        others = [s for s in posts if s["account"]["username"] != p]
        for s in rng.sample(others, min(4, len(others))):
            board.json("POST", f"/api/v1/statuses/{s['id']}/favourite", token=tokens[p])
        for s in rng.sample(others, min(2, len(others))):
            board.json("POST", f"/api/v1/statuses/{s['id']}/reblog", token=tokens[p])

    final = board.json("GET", "/api/mastomini/v1/status")
    print(f"seeded {len(people)} accounts, {len(posts)} posts; board reports "
          f"{final.get('accounts', '?')} accounts, {final.get('statuses', '?')} statuses")
    print(f"credentials (passwords, read/write tokens) in {out}")
    sys.exit(0)


if __name__ == "__main__":
    main()
