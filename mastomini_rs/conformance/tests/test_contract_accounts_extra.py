from __future__ import annotations

import pytest
from mastodon import Mastodon
from mastodon.errors import MastodonNotFoundError


def test_account_lookup_404(alice: Mastodon) -> None:
    with pytest.raises(MastodonNotFoundError):
        alice.account_lookup("nonexistent@mock.local")


def test_account_relationships_invalid_id(alice: Mastodon) -> None:
    # Mastodon.py might not let us send literal strings if it expects ints,
    # but we can try to find how it's called.
    # We'll use the underlying session if needed, but let's try via Mastodon.py first.
    # It takes a list of IDs.
    rels = alice.account_relationships(["invalid", 123456789])
    # Invalid should be skipped, nonexistent should return a relationship with all False.
    assert len(rels) == 1
    assert rels[0].id == "123456789"
    assert rels[0].following is False


def test_account_verify_credentials_with_source(alice: Mastodon) -> None:
    me = alice.account_verify_credentials()
    assert "source" in me
    assert "note" in me.source
    assert "fields" in me.source


def test_get_account_404(alice: Mastodon) -> None:
    with pytest.raises(MastodonNotFoundError):
        alice.account(123456789)


def test_account_familiar_followers_json(alice: Mastodon, bob: Mastodon) -> None:
    # Mastodon.py uses JSON for this if use_json=True (default in newer versions)
    bob_id = bob.account_verify_credentials().id
    # familiar_followers returns a list of FamiliarFollower objects
    familiar = alice.account_familiar_followers([bob_id])
    assert len(familiar) == 1
    assert familiar[0].id == str(bob_id)
    assert isinstance(familiar[0].accounts, list)
