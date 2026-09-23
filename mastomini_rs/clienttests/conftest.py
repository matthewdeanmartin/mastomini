"""Fixtures: a real desktop server and Mastodon.py clients signed in via OAuth."""

from __future__ import annotations

from collections.abc import Iterator

import pytest
from mastodon import Mastodon

from harness import REDIRECT, ServerProcess, submit_sign_in

SCOPES = ["read", "write", "follow"]


def sign_in(base: str, username: str, password: str) -> Mastodon:
    """What a Mastodon app does on first launch: register, authorize, log in."""
    client_id, client_secret = Mastodon.create_app(
        f"mastomini-tests-{username}",
        api_base_url=base,
        redirect_uris=REDIRECT,
        scopes=SCOPES,
    )
    client = Mastodon(client_id=client_id, client_secret=client_secret, api_base_url=base)
    # allow_http: the desktop build is plain HTTP; the board serves HTTPS.
    url = client.auth_request_url(redirect_uris=REDIRECT, scopes=SCOPES, allow_http=True)
    code = submit_sign_in(url, username, password)
    client.log_in(code=code, redirect_uri=REDIRECT, scopes=SCOPES, allow_http=True)
    return client


class Household:
    def __init__(self, server: ServerProcess) -> None:
        self.server = server
        server.provision("alice", "alicepw")
        self.alice = sign_in(server.base, "alice", "alicepw")
        server.add_member(self.alice.access_token, "bob", "bobpw")
        self.bob = sign_in(server.base, "bob", "bobpw")

    def reconnect(self, client: Mastodon) -> Mastodon:
        """A client after the server restarted: same token, fresh object."""
        return Mastodon(access_token=client.access_token, api_base_url=self.server.base)


@pytest.fixture
def server() -> Iterator[ServerProcess]:
    s = ServerProcess().start()
    try:
        yield s
    finally:
        s.close()


@pytest.fixture
def household(server: ServerProcess) -> Household:
    return Household(server)
