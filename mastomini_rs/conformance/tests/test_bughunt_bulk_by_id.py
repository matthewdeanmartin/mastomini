"""Bug-hunting: bulk-by-id endpoints must honor Mastodon.py's ``id[]`` array style.

Mastodon.py serializes list arguments as ``id[]=a&id[]=b``. Endpoints that bound a
plain ``id`` query param silently returned empty results — a real mock-vs-real
divergence. These pin the fixed behavior.
"""

from __future__ import annotations

from mastodon import Mastodon


def test_statuses_bulk_by_id(alice: Mastodon) -> None:
    s1 = alice.status_post("bulk one")
    s2 = alice.status_post("bulk two")
    result = alice.statuses([s1.id, s2.id])
    assert {s.id for s in result} == {s1.id, s2.id}


def test_statuses_bulk_single(alice: Mastodon) -> None:
    s1 = alice.status_post("just one")
    result = alice.statuses([s1.id])
    assert [s.id for s in result] == [s1.id]


def test_accounts_bulk_by_id(alice: Mastodon, bob: Mastodon) -> None:
    alice_id = alice.account_verify_credentials().id
    bob_id = bob.account_verify_credentials().id
    result = alice.accounts([alice_id, bob_id])
    assert {a.id for a in result} == {alice_id, bob_id}


def test_relationships_bulk_list(alice: Mastodon, bob: Mastodon, carol: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id
    carol_id = carol.account_verify_credentials().id
    alice.account_follow(bob_id)

    rels = alice.account_relationships([bob_id, carol_id])
    by_id = {r.id: r for r in rels}
    assert set(by_id) == {bob_id, carol_id}
    assert by_id[bob_id].following is True
    assert by_id[carol_id].following is False


def test_markers_get_filters_requested_timeline(alice: Mastodon) -> None:
    s = alice.status_post("marker source")
    alice.markers_set(["home", "notifications"], [s.id, s.id])

    # Requesting only "home" must not also return the notifications marker.
    only_home = alice.markers_get(["home"])
    assert only_home.home is not None
    assert "notifications" not in only_home


def test_familiar_followers_bulk(alice: Mastodon, bob: Mastodon, carol: Mastodon) -> None:
    # carol follows both alice and bob → carol is a familiar follower of bob (for alice).
    alice_id = alice.account_verify_credentials().id
    bob_id = bob.account_verify_credentials().id
    carol_id = carol.account_verify_credentials().id
    carol.account_follow(alice_id)
    carol.account_follow(bob_id)

    fam = alice.account_familiar_followers([bob_id])
    assert len(fam) == 1
    assert fam[0].id == bob_id
    assert any(a.id == carol_id for a in fam[0].accounts)
