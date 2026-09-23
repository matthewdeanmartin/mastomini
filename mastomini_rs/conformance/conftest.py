"""Run mastodon_mock's contract suite against the real mastomini desktop binary.

The copied tests (tests/, see sync_upstream.py) expect mastodon_mock's seed:
alice, bob, carol (locked) and alice following bob, with bearer tokens named
"alice_token", "bob_token" and "carol_token". Here every test gets a fresh
desktop server with its own store. The accounts are created the way a
household creates them (provision the owner, then add members) and every
token comes from the genuine OAuth code flow through the sign-in form.

Seeding runs once per session against a real server; each test then gets its
own server started on a byte-for-byte copy of that store file (tokens are
persisted, so they stay valid). Isolation without a reset endpoint.

The literal token names in the tests are mapped to those real tokens at the
client boundary: in ``Mastodon(access_token=...)`` and in raw httpx2 and
requests calls.
The server has no test shortcut.

alice is the owner, so she is also the household admin the mock's admin
tests assume. Known differences live in deviations.py, applied at collection.
"""

from __future__ import annotations

import shutil
import sys
from collections.abc import Iterator
from pathlib import Path

import httpx2
import pytest
import requests
from mastodon import Mastodon

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "clienttests"))

from harness import REDIRECT, ServerProcess, submit_sign_in  # noqa: E402

from deviations import apply as apply_deviations  # noqa: E402

SCOPES = ["read", "write", "follow", "push", "admin:read", "admin:write"]
PASSWORD = "test-password"

# alias -> real token, for the test currently running.
_TOKENS: dict[str, str] = {}


def _real(token: str | None) -> str | None:
    return _TOKENS.get(token, token) if token else token


# --- token aliasing at the client boundary -----------------------------------

_mastodon_init = Mastodon.__init__


def _patched_init(self: Mastodon, *args: object, **kwargs: object) -> None:
    if isinstance(kwargs.get("access_token"), str):
        kwargs["access_token"] = _real(kwargs["access_token"])  # type: ignore[arg-type]
    _mastodon_init(self, *args, **kwargs)  # type: ignore[arg-type]


Mastodon.__init__ = _patched_init  # type: ignore[method-assign]

_httpx_send = httpx2.Client.send


def _patched_send(self: httpx2.Client, request: httpx2.Request, **kwargs: object) -> httpx2.Response:
    auth = request.headers.get("Authorization", "")
    if auth.startswith("Bearer "):
        request.headers["Authorization"] = f"Bearer {_real(auth[7:])}"
    body = request.read()
    if body and any(alias.encode() in body for alias in _TOKENS):
        for alias, real in _TOKENS.items():
            body = body.replace(alias.encode(), real.encode())
        headers = {k: v for k, v in request.headers.items() if k.lower() != "content-length"}
        request = httpx2.Request(request.method, request.url, headers=headers, content=body)
    return _httpx_send(self, request, **kwargs)  # type: ignore[arg-type]


httpx2.Client.send = _patched_send  # type: ignore[method-assign]

_requests_send = requests.Session.send


def _patched_requests_send(
    self: requests.Session, request: requests.PreparedRequest, **kwargs: object
) -> requests.Response:
    auth = request.headers.get("Authorization", "")
    if auth.startswith("Bearer "):
        request.headers["Authorization"] = f"Bearer {_real(auth[7:])}"
    body = request.body
    if isinstance(body, str):
        body = body.encode()
    if body and any(alias.encode() in body for alias in _TOKENS):
        for alias, real in _TOKENS.items():
            body = body.replace(alias.encode(), real.encode())
        request.body = body
        request.headers["Content-Length"] = str(len(body))
    return _requests_send(self, request, **kwargs)  # type: ignore[arg-type]


requests.Session.send = _patched_requests_send  # type: ignore[method-assign]


def pytest_collection_modifyitems(items: list[pytest.Item]) -> None:
    apply_deviations(items)


# --- the household -----------------------------------------------------------


def sign_in(base: str, username: str) -> str:
    client_id, client_secret = Mastodon.create_app(
        f"conformance-{username}", api_base_url=base, redirect_uris=REDIRECT, scopes=SCOPES
    )
    client = Mastodon(client_id=client_id, client_secret=client_secret, api_base_url=base)
    url = client.auth_request_url(redirect_uris=REDIRECT, scopes=SCOPES, allow_http=True)
    code = submit_sign_in(url, username, PASSWORD)
    return client.log_in(code=code, redirect_uri=REDIRECT, scopes=SCOPES, allow_http=True)


def seed(server: ServerProcess) -> dict[str, str]:
    """mastodon_mock's TEST_SEED, minus the remote account (no federation)."""
    r = requests.post(
        server.url("/api/mastomini/v1/provision"),
        data={"username": "alice", "password": PASSWORD, "display_name": "Alice", "title": "Home"},
        timeout=10,
    )
    r.raise_for_status()
    tokens = {"alice": sign_in(server.base, "alice")}
    for name in ("bob", "carol"):
        r = requests.post(
            server.url("/api/mastomini/v1/admin/members"),
            data={"username": name, "password": PASSWORD, "display_name": name.title()},
            headers={"Authorization": f"Bearer {tokens['alice']}"},
            timeout=10,
        )
        r.raise_for_status()
        tokens[name] = sign_in(server.base, name)
    carol = Mastodon(access_token=tokens["carol"], api_base_url=server.base)
    carol.account_update_credentials(locked=True)
    alice = Mastodon(access_token=tokens["alice"], api_base_url=server.base)
    alice.account_follow(alice.account_lookup("bob").id)
    return tokens


@pytest.fixture(scope="session")
def seeded_store() -> Iterator[tuple[Path, dict[str, str]]]:
    """One real household, seeded through the API, then shut down."""
    server = ServerProcess().start()
    try:
        tokens = seed(server)
        server.stop()
        yield server.store, tokens
    finally:
        server.close()


@pytest.fixture()
def live_server(seeded_store: tuple[Path, dict[str, str]]) -> Iterator[str]:
    template, tokens = seeded_store
    server = ServerProcess()
    shutil.copyfile(template, server.store)
    server.start()
    try:
        _TOKENS.update({f"{name}_token": token for name, token in tokens.items()})
        yield server.base
    finally:
        _TOKENS.clear()
        server.close()


# The session-scoped "fast" variants reset state between tests upstream; a
# fresh server per test gives the same isolation.
fast_server = live_server


def _client(base: str, name: str) -> Mastodon:
    return Mastodon(access_token=f"{name}_token", api_base_url=base)


@pytest.fixture()
def alice(live_server: str) -> Mastodon:
    return _client(live_server, "alice")


@pytest.fixture()
def bob(live_server: str) -> Mastodon:
    return _client(live_server, "bob")


@pytest.fixture()
def carol(live_server: str) -> Mastodon:
    return _client(live_server, "carol")


alice_fast, bob_fast, carol_fast = alice, bob, carol


@pytest.fixture()
def mastodon_client(live_server: str) -> Mastodon:
    """The integration suite's seed: bob has two posts on alice's home."""
    bob = _client(live_server, "bob")
    bob.status_post("hello from the seed")
    bob.status_post("a second seed post")
    return _client(live_server, "alice")
