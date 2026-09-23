"""Sprint 2 smoke test: provision, OAuth code flow, verify, restart, verify.

Uses plain HTTP requests (no Mastodon library) so failures point at the
server's wire behaviour.
"""

from __future__ import annotations

import base64
import hashlib
import secrets
import sys
from urllib.parse import urlencode

import requests

from harness import APP_REDIRECT, ServerProcess, submit_sign_in


def check(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"FAIL: {message}")
    print(f"ok   {message}")


def main() -> int:
    server = ServerProcess().start()
    try:
        info = requests.get(server.url("/api/mastomini/v1/status"), timeout=5).json()
        check(info["provisioned"] is False, "fresh store is unprovisioned")
        server.provision()
        check(requests.get(server.url("/api/v2/instance"), timeout=5).json()["title"] == "Test Home", "instance title set")

        app = requests.post(
            server.url("/api/v1/apps"),
            data={"client_name": "smoke", "redirect_uris": APP_REDIRECT, "scopes": "read write follow"},
            timeout=5,
        ).json()
        check(bool(app["client_id"]) and bool(app["client_secret"]), "app registered")

        verifier = secrets.token_urlsafe(48)
        challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
        authorize = server.url(
            "/oauth/authorize?"
            + urlencode(
                {
                    "response_type": "code",
                    "client_id": app["client_id"],
                    "redirect_uri": APP_REDIRECT,
                    "scope": "read write follow",
                    "state": "smoke",
                    "code_challenge": challenge,
                    "code_challenge_method": "S256",
                }
            )
        )
        code = submit_sign_in(authorize, "alice", "alicepw")
        check(len(code) > 20, "sign-in form returned a code")

        token = requests.post(
            server.url("/oauth/token"),
            data={
                "grant_type": "authorization_code",
                "client_id": app["client_id"],
                "code": code,
                "redirect_uri": APP_REDIRECT,
                "code_verifier": verifier,
            },
            timeout=5,
        ).json()["access_token"]
        auth = {"Authorization": f"Bearer {token}"}
        me = requests.get(server.url("/api/v1/accounts/verify_credentials"), headers=auth, timeout=5).json()
        check(me["username"] == "alice", "verify_credentials returns the owner")

        server.restart()
        me = requests.get(server.url("/api/v1/accounts/verify_credentials"), headers=auth, timeout=5)
        check(me.status_code == 200, "token still valid after restart")

        requests.post(server.url("/oauth/revoke"), data={"token": token}, timeout=5)
        me = requests.get(server.url("/api/v1/accounts/verify_credentials"), headers=auth, timeout=5)
        check(me.status_code == 401, "revoked token is refused")
        print("smoke: all checks passed")
        return 0
    finally:
        server.close()


if __name__ == "__main__":
    sys.exit(main())
