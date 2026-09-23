"""Contract tests for the apps / OAuth endpoints."""

from __future__ import annotations

import re

import httpx2 as httpx
from mastodon import Mastodon


def test_create_app_returns_client_credentials(live_server: str) -> None:
    client_id, client_secret = Mastodon.create_app("test-app", api_base_url=live_server)
    assert client_id
    assert client_secret


def test_client_credentials_grant_and_app_verify(live_server: str) -> None:
    client_id, client_secret = Mastodon.create_app("verify-app", api_base_url=live_server)

    token_resp = httpx.post(
        f"{live_server}/oauth/token",
        data={
            "grant_type": "client_credentials",
            "client_id": client_id,
            "client_secret": client_secret,
            "scope": "read",
        },
    )
    assert token_resp.status_code == 200
    app_token = token_resp.json()["access_token"]

    verify = httpx.get(
        f"{live_server}/api/v1/apps/verify_credentials",
        headers={"Authorization": f"Bearer {app_token}"},
    )
    assert verify.status_code == 200
    body = verify.json()
    assert body["name"] == "verify-app"


def test_self_service_account_creation(live_server: str) -> None:
    client_id, client_secret = Mastodon.create_app("signup-app", api_base_url=live_server)
    client = Mastodon(client_id=client_id, client_secret=client_secret, api_base_url=live_server)

    user_token = client.create_account(
        username="newbie",
        password="hunter2hunter2",
        email="newbie@example.test",
        agreement=True,
    )
    assert user_token

    user_client = Mastodon(access_token=user_token, api_base_url=live_server)
    me = user_client.account_verify_credentials()
    assert me.username == "newbie"


def test_duplicate_username_signup_is_rejected(live_server: str) -> None:
    client_id, client_secret = Mastodon.create_app("dup-app", api_base_url=live_server)

    # "alice" already exists in the seed; signing up again must 422.
    token = _app_token(live_server, client_id, client_secret)
    resp = httpx.post(
        f"{live_server}/api/v1/accounts",
        data={
            "username": "alice",
            "email": "alice2@example.test",
            "password": "hunter2hunter2",
            "agreement": "true",
        },
        headers={"Authorization": f"Bearer {token}"},
    )
    assert resp.status_code == 422


def test_signup_requires_agreement(live_server: str) -> None:
    client_id, client_secret = Mastodon.create_app("agree-app", api_base_url=live_server)
    token = _app_token(live_server, client_id, client_secret)
    resp = httpx.post(
        f"{live_server}/api/v1/accounts",
        data={
            "username": "noagree",
            "email": "noagree@example.test",
            "password": "hunter2hunter2",
        },
        headers={"Authorization": f"Bearer {token}"},
    )
    assert resp.status_code == 422


def test_authorization_code_grant_issues_user_token(live_server: str) -> None:
    client_id, client_secret = Mastodon.create_app("authz-app", api_base_url=live_server)
    # The picker mints an opaque ``mockcode_<username>`` code for any local account.
    code = _authorize_code(live_server, client_id, "alice")

    resp = httpx.post(
        f"{live_server}/oauth/token",
        data={
            "grant_type": "authorization_code",
            "client_id": client_id,
            "client_secret": client_secret,
            "code": code,
            "redirect_uri": "urn:ietf:wg:oauth:2.0:oob",
        },
    )
    assert resp.status_code == 200
    token = resp.json()["access_token"]
    me = Mastodon(access_token=token, api_base_url=live_server).account_verify_credentials()
    assert me.username == "alice"


def test_authorization_code_grant_rejects_disabled_account(live_server: str) -> None:
    # Create a fresh user, mint a code for it, then disable it via the admin action.
    created = httpx.post(f"{live_server}/api/v1/_mock/dev_user", json={}).json()
    username = created["username"]

    client_id, client_secret = Mastodon.create_app("disabled-app", api_base_url=live_server)
    code = _authorize_code(live_server, client_id, username)

    admin = httpx.post(f"{live_server}/api/v1/_mock/dev_user", json={"admin": True}).json()
    target_id = httpx.get(
        f"{live_server}/api/v1/accounts/verify_credentials",
        headers={"Authorization": f"Bearer {created['access_token']}"},
    ).json()["id"]
    disable = httpx.post(
        f"{live_server}/api/v1/admin/accounts/{target_id}/action",
        json={"type": "disable"},
        headers={"Authorization": f"Bearer {admin['access_token']}"},
    )
    assert disable.status_code == 200

    # The code is still well-formed, but the account can no longer authenticate.
    resp = httpx.post(
        f"{live_server}/oauth/token",
        data={
            "grant_type": "authorization_code",
            "client_id": client_id,
            "client_secret": client_secret,
            "code": code,
            "redirect_uri": "urn:ietf:wg:oauth:2.0:oob",
        },
    )
    assert resp.status_code == 403


def test_unsupported_grant_type_rejected(live_server: str) -> None:
    resp = httpx.post(f"{live_server}/oauth/token", data={"grant_type": "password"})
    assert resp.status_code == 400


def test_token_revocation(live_server: str) -> None:
    client = Mastodon(access_token="alice_token", api_base_url=live_server)
    assert client.account_verify_credentials().username == "alice"

    resp = httpx.post(f"{live_server}/oauth/revoke", data={"token": "alice_token"})
    assert resp.status_code == 200

    revoked = httpx.get(
        f"{live_server}/api/v1/accounts/verify_credentials",
        headers={"Authorization": "Bearer alice_token"},
    )
    assert revoked.status_code == 401


def test_oauth_server_metadata(live_server: str) -> None:
    resp = httpx.get(f"{live_server}/.well-known/oauth-authorization-server")
    assert resp.status_code == 200
    body = resp.json()
    assert body["token_endpoint"].endswith("/oauth/token")
    assert "client_credentials" in body["grant_types_supported"]


def test_oauth_userinfo(live_server: str) -> None:
    resp = httpx.get(
        f"{live_server}/oauth/userinfo",
        headers={"Authorization": "Bearer alice_token"},
    )
    assert resp.status_code == 200
    assert resp.json()["preferred_username"] == "alice"


def test_mock_dev_user_creates_usable_token(live_server: str) -> None:
    resp = httpx.post(f"{live_server}/api/v1/_mock/dev_user", json={})
    assert resp.status_code == 200
    body = resp.json()
    assert body["role"] == "user"
    assert body["username"].startswith("user_")

    me = httpx.get(
        f"{live_server}/api/v1/accounts/verify_credentials",
        headers={"Authorization": f"Bearer {body['access_token']}"},
    )
    assert me.status_code == 200
    assert me.json()["username"] == body["username"]


def test_mock_dev_user_admin_role_and_explicit_username(live_server: str) -> None:
    resp = httpx.post(
        f"{live_server}/api/v1/_mock/dev_user",
        json={"admin": True, "username": "moderator1"},
    )
    assert resp.status_code == 200
    body = resp.json()
    assert body["role"] == "admin"
    assert body["username"] == "moderator1"

    # A second create with the same username is rejected.
    dup = httpx.post(f"{live_server}/api/v1/_mock/dev_user", json={"username": "moderator1"})
    assert dup.status_code == 422


def test_mock_dev_users_lists_tokened_accounts(live_server: str) -> None:
    created = httpx.post(f"{live_server}/api/v1/_mock/dev_user", json={}).json()

    resp = httpx.get(f"{live_server}/api/v1/_mock/dev_users")
    assert resp.status_code == 200
    users = resp.json()
    usernames = {u["username"] for u in users}
    # The seeded accounts with tokens (alice/bob/carol) and the freshly created user.
    assert "alice" in usernames
    assert created["username"] in usernames
    # Every listed user carries a usable token + role.
    for user in users:
        assert user["access_token"]
        assert user["role"] in ("user", "admin")


def test_verify_credentials_reports_role_for_staff(live_server: str) -> None:
    admin = httpx.post(f"{live_server}/api/v1/_mock/dev_user", json={"admin": True}).json()
    user = httpx.post(f"{live_server}/api/v1/_mock/dev_user", json={}).json()

    admin_me = httpx.get(
        f"{live_server}/api/v1/accounts/verify_credentials",
        headers={"Authorization": f"Bearer {admin['access_token']}"},
    ).json()
    assert admin_me["role"] is not None
    assert admin_me["role"]["name"] == "Admin"

    # Ordinary users get the default "everyone" role (id -99, no permissions),
    # matching real Mastodon 4.x — the Role schema requires an object, not null.
    user_me = httpx.get(
        f"{live_server}/api/v1/accounts/verify_credentials",
        headers={"Authorization": f"Bearer {user['access_token']}"},
    ).json()
    assert user_me["role"]["id"] == "-99"
    assert user_me["role"]["permissions"] == "0"


def _authorize_code(base_url: str, client_id: str, username: str) -> str:
    """Drive the account-picker to mint an authorization code for ``username``.

    Uses the ``oob`` redirect so the code comes back in the HTML body rather than a
    302 redirect we'd have to follow.
    """
    resp = httpx.post(
        f"{base_url}/oauth/authorize",
        data={
            "username": username,
            "client_id": client_id,
            "redirect_uri": "urn:ietf:wg:oauth:2.0:oob",
            "scope": "read write",
        },
    )
    resp.raise_for_status()
    match = re.search(r"Authorization code: (\S+)</p>", resp.text)
    assert match, f"no code in authorize response: {resp.text!r}"
    return match.group(1)


def _app_token(base_url: str, client_id: str, client_secret: str) -> str:
    """Obtain an app-only token via the client_credentials grant."""
    resp = httpx.post(
        f"{base_url}/oauth/token",
        data={
            "grant_type": "client_credentials",
            "client_id": client_id,
            "client_secret": client_secret,
            "scope": "read write",
        },
    )
    resp.raise_for_status()
    return str(resp.json()["access_token"])
