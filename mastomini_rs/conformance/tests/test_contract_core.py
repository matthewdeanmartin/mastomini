"""Mastodon.py-driven contract tests for the core write/read scenarios."""

from __future__ import annotations

from datetime import datetime

import pytest
from mastodon import Mastodon
from mastodon.errors import MastodonNotFoundError


def test_verify_credentials(alice: Mastodon) -> None:
    me = alice.account_verify_credentials()
    assert me.username == "alice"
    assert me.acct == "alice"
    assert isinstance(me.created_at, datetime)
    assert me.source is not None


def test_instance_info(alice: Mastodon) -> None:
    info = alice.instance()
    assert info.version
    assert info.configuration.statuses.max_characters == 500


def test_post_and_read_back(alice: Mastodon) -> None:
    status = alice.status_post("hello world")
    assert status.content == "<p>hello world</p>"
    fetched = alice.status(status.id)
    assert fetched.id == status.id
    assert fetched.account.acct == "alice"


def test_post_then_delete_404s(alice: Mastodon) -> None:
    status = alice.status_post("ephemeral")
    alice.status_delete(status.id)
    with pytest.raises(MastodonNotFoundError):
        alice.status(status.id)


def test_follow_then_timeline(alice: Mastodon, bob: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id

    alice.account_follow(bob_id)
    rel = alice.account_relationships(bob_id)[0]
    assert rel.following is True

    new_status = bob.status_post("hello from bob")
    home = alice.timeline_home()
    assert any(s.id == new_status.id for s in home)


def test_unfollow_removes_from_timeline(alice: Mastodon, bob: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id
    alice.account_follow(bob_id)
    new_status = bob.status_post("transient follow post")
    assert any(s.id == new_status.id for s in alice.timeline_home())

    alice.account_unfollow(bob_id)
    assert not any(s.id == new_status.id for s in alice.timeline_home())


def test_follow_can_hide_reblogs_from_home(alice: Mastodon, bob: Mastodon, carol: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id
    alice.account_follow(bob_id, reblogs=False)
    own_status = bob.status_post("bob original")
    boosted_status = carol.status_post("carol original")
    boost = bob.status_reblog(boosted_status.id)

    home_ids = {status.id for status in alice.timeline_home()}
    assert own_status.id in home_ids
    assert boost.id not in home_ids
    assert alice.account_relationships(bob_id)[0].showing_reblogs is False


def test_remove_follower_does_not_block(alice: Mastodon, bob: Mastodon) -> None:
    alice_id = alice.account_verify_credentials().id
    bob_id = bob.account_verify_credentials().id
    bob.account_follow(alice_id)

    relationship = alice.account_remove_from_followers(bob_id)

    assert relationship.followed_by is False
    assert relationship.blocking is False
    assert bob.account_relationships(alice_id)[0].following is False


def test_follow_generates_notification(alice: Mastodon, bob: Mastodon) -> None:
    alice_id = alice.account_verify_credentials().id
    bob_id = bob.account_verify_credentials().id

    alice.account_follow(bob_id)
    notifs = bob.notifications(types=["follow"])
    assert any(n.account.id == alice_id for n in notifs)


def test_locked_account_follow_request(alice: Mastodon, carol: Mastodon) -> None:
    carol_id = carol.account_verify_credentials().id
    rel = alice.account_follow(carol_id)
    assert rel.requested is True
    assert rel.following is False

    requests = carol.follow_requests()
    alice_id = alice.account_verify_credentials().id
    assert any(a.id == alice_id for a in requests)

    carol.follow_request_authorize(alice_id)
    rel2 = alice.account_relationships(carol_id)[0]
    assert rel2.following is True


def test_favourite_and_notification(alice: Mastodon, bob: Mastodon) -> None:
    alice_id = alice.account_verify_credentials().id
    status = bob.status_post("favourite me")
    alice.status_favourite(status.id)

    refetched = alice.status(status.id)
    assert refetched.favourited is True
    assert refetched.favourites_count == 1

    notifs = bob.notifications(types=["favourite"])
    assert any(n.account.id == alice_id for n in notifs)


def test_reblog(alice: Mastodon, bob: Mastodon) -> None:
    status = bob.status_post("boost me")
    reblog = alice.status_reblog(status.id)
    assert reblog.reblog is not None
    assert reblog.reblog.id == status.id


def test_mentions_and_hashtags(alice: Mastodon, bob: Mastodon) -> None:
    status = bob.status_post("hey @alice check #python")
    assert any(m.acct == "alice" for m in status.mentions)
    assert any(t.name == "python" for t in status.tags)

    tagged = bob.timeline_hashtag("python")
    assert any(s.id == status.id for s in tagged)


def test_bookmark_and_list(alice: Mastodon) -> None:
    status = alice.status_post("bookmark target")
    alice.status_bookmark(status.id)
    bookmarks = alice.bookmarks()
    assert any(s.id == status.id for s in bookmarks)


def test_account_search_and_lookup(alice: Mastodon) -> None:
    results = alice.account_search("bob")
    assert any(a.username == "bob" for a in results)

    looked_up = alice.account_lookup("bob")
    assert looked_up.username == "bob"
