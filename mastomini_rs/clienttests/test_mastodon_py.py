"""Sprint 3 exit criteria, exercised with Mastodon.py against the real binary.

Two clients signed in through the real OAuth flow follow each other, post,
reply, favourite and boost, see each other on home, get notifications, and
everything except notifications survives a server restart.
"""

from __future__ import annotations

import pytest
from mastodon import MastodonAPIError, MastodonNotFoundError

from conftest import Household


def test_instance_as_mastodon_py_sees_it(household: Household) -> None:
    info = household.alice.instance()
    assert info.title == "Test Home"
    assert info.configuration.statuses.max_characters == 140
    assert household.alice.me().username == "alice"
    assert household.bob.account_verify_credentials().username == "bob"


def test_household_conversation(household: Household) -> None:
    alice, bob = household.alice, household.bob
    alice_id, bob_id = alice.me().id, bob.me().id

    assert alice.account_follow(bob_id).following
    assert bob.account_follow(alice_id).following
    assert alice.account_relationships(bob_id)[0].followed_by

    post = alice.status_post("Pizza tonight? #dinner")
    assert post.tags[0].name == "dinner"
    reply = bob.status_post("@alice yes please", in_reply_to_id=post.id)
    assert reply.in_reply_to_id == post.id
    assert reply.mentions[0].username == "alice"

    fav = bob.status_favourite(post.id)
    assert fav.favourited and fav.favourites_count == 1
    boost = bob.status_reblog(post.id)
    assert boost.reblog.id == post.id

    home = [s.id for s in alice.timeline_home()]
    assert reply.id in home and post.id in home
    assert boost.id in [s.id for s in bob.timeline_home()]
    assert [s.id for s in alice.timeline_hashtag("dinner")] == [post.id]
    context = alice.status_context(post.id)
    assert [s.id for s in context.descendants] == [reply.id]

    kinds = sorted(n.type for n in alice.notifications())
    assert kinds == ["favourite", "follow", "mention", "reblog"]
    assert [n.type for n in bob.notifications()] == ["follow"]

    household.server.restart()
    alice = household.reconnect(alice)
    bob = household.reconnect(bob)

    again = bob.status(post.id)
    assert again.favourited and again.reblogged and again.replies_count == 1
    assert alice.account_relationships(bob_id)[0].following
    assert reply.id in [s.id for s in alice.timeline_home()]
    assert list(alice.notifications()) == [], "notifications are RAM only"


def test_pagination_follows_link_headers(household: Household) -> None:
    alice = household.alice
    ids = [alice.status_post(f"note {i}").id for i in range(7)]
    page = alice.timeline_public(limit=3)
    seen = [s.id for s in page]
    while (page := alice.fetch_next(page)) is not None and len(page) > 0:
        seen.extend(s.id for s in page)
    assert seen == list(reversed(ids))


def test_bookmarks_favourites_pins_and_delete(household: Household) -> None:
    alice, bob = household.alice, household.bob
    post = alice.status_post("keep this")
    bob.status_bookmark(post.id)
    bob.status_favourite(post.id)
    assert [s.id for s in bob.bookmarks()] == [post.id]
    assert [s.id for s in bob.favourites()] == [post.id]
    assert alice.status_pin(post.id).pinned
    assert [s.id for s in alice.account_statuses(alice.me().id, pinned=True)] == [post.id]

    deleted = alice.status_delete(post.id)
    assert deleted.text == "keep this"
    with pytest.raises(MastodonNotFoundError):
        bob.status(post.id)
    assert list(bob.bookmarks()) == []


def test_limits_and_privacy(household: Household) -> None:
    alice, bob = household.alice, household.bob
    with pytest.raises(MastodonAPIError):
        alice.status_post("x" * 141)
    long_link = "https://example.com/" + "a" * 150
    assert alice.status_post(f"read {long_link}").url

    secret = alice.status_post("just for me and nobody else", visibility="direct")
    with pytest.raises(MastodonNotFoundError):
        bob.status(secret.id)
    for_bob = alice.status_post("@bob psst", visibility="direct")
    assert bob.status(for_bob.id).visibility == "direct"


def test_markers_and_search(household: Household) -> None:
    alice = household.alice
    post = alice.status_post("searching for #treasure")
    alice.markers_set(["home"], [post.id])
    assert alice.markers_get(["home"]).home.last_read_id == post.id
    result = alice.search_v2("treasure")
    assert [s.id for s in result.statuses] == [post.id]
    assert result.hashtags[0].name == "treasure"
    assert alice.search_v2("bob", result_type="accounts").accounts[0].username == "bob"
