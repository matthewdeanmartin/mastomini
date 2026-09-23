"""Contract tests for grouped notifications (Mastodon 4.3+; /api/v2/notifications).

Groupable types (favourite/follow/reblog) collapse by target; other types stay
individual. Drives Mastodon.py's `grouped_notifications()` and friends.
"""

from __future__ import annotations

from mastodon import Mastodon


def test_favourites_of_same_status_group_together(alice: Mastodon, bob: Mastodon, carol: Mastodon) -> None:
    status = alice.status_post("group my favourites")
    bob.status_favourite(status.id)
    carol.status_favourite(status.id)

    result = alice.grouped_notifications()
    fav_groups = [g for g in result.notification_groups if g.type == "favourite"]
    assert len(fav_groups) == 1
    group = fav_groups[0]
    assert group.notifications_count == 2
    assert group.status_id == status.id
    # Both actors are represented in the sample + container accounts.
    bob_id = bob.account_verify_credentials().id
    carol_id = carol.account_verify_credentials().id
    assert {str(bob_id), str(carol_id)}.issubset(set(group.sample_account_ids))
    account_ids = {a.id for a in result.accounts}
    assert bob_id in account_ids and carol_id in account_ids


def test_favourites_of_different_statuses_do_not_group(alice: Mastodon, bob: Mastodon) -> None:
    s1 = alice.status_post("first")
    s2 = alice.status_post("second")
    bob.status_favourite(s1.id)
    bob.status_favourite(s2.id)

    result = alice.grouped_notifications()
    fav_groups = [g for g in result.notification_groups if g.type == "favourite"]
    assert len(fav_groups) == 2
    assert all(g.notifications_count == 1 for g in fav_groups)


def test_mentions_are_not_grouped(alice: Mastodon, bob: Mastodon) -> None:
    bob.status_post("@alice one")
    bob.status_post("@alice two")
    result = alice.grouped_notifications()
    mention_groups = [g for g in result.notification_groups if g.type == "mention"]
    # Mentions are never grouped → two distinct groups with unique keys.
    assert len(mention_groups) == 2
    assert len({g.group_key for g in mention_groups}) == 2


def test_grouped_referenced_status_present(alice: Mastodon, bob: Mastodon) -> None:
    status = alice.status_post("referenced status")
    bob.status_favourite(status.id)
    result = alice.grouped_notifications()
    assert any(s.id == status.id for s in result.statuses)


def test_single_group_fetch_and_accounts(alice: Mastodon, bob: Mastodon, carol: Mastodon) -> None:
    status = alice.status_post("fetch one group")
    bob.status_favourite(status.id)
    carol.status_favourite(status.id)

    result = alice.grouped_notifications()
    group = next(g for g in result.notification_groups if g.type == "favourite")

    single = alice.grouped_notification(group.group_key)
    assert any(g.group_key == group.group_key for g in single.notification_groups)

    accounts = alice.grouped_notification_accounts(group.group_key)
    bob_id = bob.account_verify_credentials().id
    carol_id = carol.account_verify_credentials().id
    assert {a.id for a in accounts} == {bob_id, carol_id}


def test_dismiss_grouped_notification(alice: Mastodon, bob: Mastodon) -> None:
    status = alice.status_post("dismiss me")
    bob.status_favourite(status.id)
    result = alice.grouped_notifications()
    group = next(g for g in result.notification_groups if g.type == "favourite")

    alice.dismiss_grouped_notification(group.group_key)

    after = alice.grouped_notifications()
    assert not any(g.group_key == group.group_key for g in after.notification_groups)


def test_grouped_unread_count_counts_groups(alice: Mastodon, bob: Mastodon, carol: Mastodon) -> None:
    status = alice.status_post("count groups")
    bob.status_favourite(status.id)
    carol.status_favourite(status.id)
    # Two favourites on one status → one unread group, not two.
    assert alice.unread_grouped_notifications_count() == 1
