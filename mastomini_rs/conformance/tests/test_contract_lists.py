"""Contract tests for the user-list endpoints (CRUD + membership)."""

from __future__ import annotations

import pytest
from mastodon import Mastodon
from mastodon.errors import MastodonNotFoundError


def test_list_crud_lifecycle(alice: Mastodon) -> None:
    created = alice.list_create("reading")
    assert created.title == "reading"

    fetched = alice.list(created.id)
    assert fetched.id == created.id

    assert any(lst.id == created.id for lst in alice.lists())

    updated = alice.list_update(created.id, "reading-now")
    assert updated.title == "reading-now"

    alice.list_delete(created.id)
    with pytest.raises(MastodonNotFoundError):
        alice.list(created.id)


def test_list_membership_add_remove(alice: Mastodon, bob: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id
    created = alice.list_create("members")

    alice.list_accounts_add(created.id, [bob_id])
    members = alice.list_accounts(created.id)
    assert any(a.id == bob_id for a in members)

    alice.list_accounts_delete(created.id, [bob_id])
    members_after = alice.list_accounts(created.id)
    assert all(a.id != bob_id for a in members_after)


def test_list_owned_by_other_user_is_404(alice: Mastodon, bob: Mastodon) -> None:
    created = alice.list_create("private-list")
    with pytest.raises(MastodonNotFoundError):
        bob.list(created.id)


def test_missing_list_is_404(alice: Mastodon) -> None:
    with pytest.raises(MastodonNotFoundError):
        alice.list(999999)
