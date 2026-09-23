"""Bughunt tests for Mastodon.py-facing server semantics.

These tests intentionally assert real Mastodon behaviour, not whatever the mock
currently happens to do. The mock is most valuable when Mastodon.py consumers
can trust it to reject forbidden writes, keep timelines scoped, and preserve
server-side idempotency rules.
"""

from __future__ import annotations

import pytest
from mastodon import Mastodon
from mastodon.errors import MastodonAPIError, MastodonUnauthorizedError


def test_public_timeline_excludes_unlisted_statuses(alice: Mastodon, bob: Mastodon) -> None:
    unlisted = alice.status_post("this should not be on the public firehose", visibility="unlisted")
    public = alice.status_post("this belongs on the public firehose", visibility="public")

    public_ids = {status.id for status in bob.timeline_public()}

    assert public.id in public_ids
    assert unlisted.id not in public_ids


def test_hashtag_timeline_excludes_unlisted_statuses(alice: Mastodon, bob: Mastodon) -> None:
    unlisted = alice.status_post("quiet tag #contractscope", visibility="unlisted")
    public = alice.status_post("loud tag #contractscope", visibility="public")

    tagged_ids = {status.id for status in bob.timeline_hashtag("contractscope")}

    assert public.id in tagged_ids
    assert unlisted.id not in tagged_ids


def test_account_statuses_do_not_leak_direct_posts_to_non_participants(
    alice: Mastodon,
    bob: Mastodon,
    carol: Mastodon,
) -> None:
    alice_id = alice.account_verify_credentials().id
    direct = alice.status_post("@carol private coordination", visibility="direct")
    public = alice.status_post("profile-visible post", visibility="public")

    bob_visible_ids = {status.id for status in bob.account_statuses(alice_id)}
    carol_visible_ids = {status.id for status in carol.account_statuses(alice_id)}

    assert public.id in bob_visible_ids
    assert direct.id not in bob_visible_ids
    assert direct.id in carol_visible_ids


def test_private_statuses_are_visible_to_followers_but_not_public_profile_viewers(
    alice: Mastodon,
    bob: Mastodon,
    carol: Mastodon,
) -> None:
    alice_id = alice.account_verify_credentials().id
    bob.account_follow(alice_id)
    followers_only = alice.status_post("followers-only status", visibility="private")

    bob_visible_ids = {status.id for status in bob.account_statuses(alice_id)}
    carol_visible_ids = {status.id for status in carol.account_statuses(alice_id)}

    assert followers_only.id in bob_visible_ids
    assert followers_only.id not in carol_visible_ids


def test_status_delete_requires_ownership(alice: Mastodon, bob: Mastodon) -> None:
    status = alice.status_post("only alice may delete this")

    with pytest.raises((MastodonAPIError, MastodonUnauthorizedError)):
        bob.status_delete(status.id)

    assert alice.status(status.id).id == status.id


def test_status_update_requires_ownership(alice: Mastodon, bob: Mastodon) -> None:
    status = alice.status_post("only alice may edit this")

    with pytest.raises((MastodonAPIError, MastodonUnauthorizedError)):
        bob.status_update(status.id, "bob should not be able to edit alice")

    assert alice.status_source(status.id).text == "only alice may edit this"


def test_reblog_is_idempotent_for_one_account(alice: Mastodon, bob: Mastodon) -> None:
    status = alice.status_post("boost once, count once")

    first_boost = bob.status_reblog(status.id)
    second_boost = bob.status_reblog(status.id)
    boosted = alice.status(status.id)

    assert first_boost.id == second_boost.id
    assert boosted.reblogs_count == 1
    assert bob.status(status.id).reblogged is True


def test_following_self_is_rejected(alice: Mastodon) -> None:
    alice_id = alice.account_verify_credentials().id

    with pytest.raises(MastodonAPIError):
        alice.account_follow(alice_id)

    rel = alice.account_relationships(alice_id)[0]
    assert rel.following is False
    assert rel.requested is False
