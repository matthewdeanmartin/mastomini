"""Contract tests for the profile directory ordering (next_phase.md §4 P1).

``/api/v1/directory`` supports ``order=active`` (most recently posted first,
the default) and ``order=new`` (most recently created profiles first).
"""

from __future__ import annotations

from mastodon import Mastodon


def test_directory_active_orders_by_recent_activity(alice: Mastodon, bob: Mastodon) -> None:
    # bob posts last, so bob should appear before never-posted accounts under "active".
    alice.status_post("alice posts first")
    bob.status_post("bob posts last")

    directory = alice.directory(order="active", limit=40, local=True)
    accts = [a.acct for a in directory]
    assert "bob" in accts and "alice" in accts

    # Accounts that have posted sort ahead of those that never have (carol, dave).
    bob_idx = accts.index("bob")
    alice_idx = accts.index("alice")
    if "carol" in accts:
        assert bob_idx < accts.index("carol")
        assert alice_idx < accts.index("carol")
    # bob posted after alice → bob ranks ahead of alice.
    assert bob_idx < alice_idx


def test_directory_new_orders_by_creation(alice: Mastodon) -> None:
    directory = alice.directory(order="new", limit=40, local=True)
    accts = [a.acct for a in directory]
    # All seeded local accounts are present; remote dave excluded by local=True.
    assert {"alice", "bob", "carol"}.issubset(set(accts))
    assert "dave@remote.example" not in accts
