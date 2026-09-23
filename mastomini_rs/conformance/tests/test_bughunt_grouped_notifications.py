"""Bug-hunting tests for grouped notifications edge cases.

Probes interleaving, reblog/follow grouping, pagination across the grouped
container, and group_key stability — the spots most likely to diverge.
"""

from __future__ import annotations

from mastodon import Mastodon


def test_follow_notifications_group_into_one(alice: Mastodon, bob: Mastodon, carol: Mastodon) -> None:
    # Two different accounts follow alice → Mastodon collapses into one follow group.
    bob.account_follow(alice.account_verify_credentials().id)
    carol.account_follow(alice.account_verify_credentials().id)
    result = alice.grouped_notifications()
    follow_groups = [g for g in result.notification_groups if g.type == "follow"]
    assert len(follow_groups) == 1
    assert follow_groups[0].notifications_count == 2


def test_reblogs_of_same_status_group(alice: Mastodon, bob: Mastodon, carol: Mastodon) -> None:
    status = alice.status_post("boost magnet")
    bob.status_reblog(status.id)
    carol.status_reblog(status.id)
    result = alice.grouped_notifications()
    reblog_groups = [g for g in result.notification_groups if g.type == "reblog"]
    assert len(reblog_groups) == 1
    assert reblog_groups[0].notifications_count == 2
    assert reblog_groups[0].status_id == status.id


def test_favourite_and_reblog_of_same_status_are_separate_groups(alice: Mastodon, bob: Mastodon) -> None:
    status = alice.status_post("fav and boost")
    bob.status_favourite(status.id)
    bob.status_reblog(status.id)
    result = alice.grouped_notifications()
    types = {g.type for g in result.notification_groups}
    # Different types never merge even on the same status.
    assert "favourite" in types and "reblog" in types
    fav = next(g for g in result.notification_groups if g.type == "favourite")
    reb = next(g for g in result.notification_groups if g.type == "reblog")
    assert fav.group_key != reb.group_key


def test_most_recent_notification_id_is_newest_in_group(alice: Mastodon, bob: Mastodon, carol: Mastodon) -> None:
    status = alice.status_post("ordering check")
    bob.status_favourite(status.id)
    carol.status_favourite(status.id)  # carol's is newer
    result = alice.grouped_notifications()
    group = next(g for g in result.notification_groups if g.type == "favourite")
    # most_recent_notification_id must be the highest id (newest) in the group.
    raw = alice.notifications(types=["favourite"])
    newest_id = max(n.id for n in raw)
    assert group.most_recent_notification_id == newest_id


def test_grouped_pagination_limit_counts_underlying_rows(alice: Mastodon, bob: Mastodon) -> None:
    # 5 favourites on distinct statuses → 5 groups; limit=3 should bound the page.
    for i in range(5):
        s = alice.status_post(f"page {i}")
        bob.status_favourite(s.id)
    result = alice.grouped_notifications(limit=3)
    fav_groups = [g for g in result.notification_groups if g.type == "favourite"]
    assert len(fav_groups) <= 3


def test_grouped_types_filter_excludes(alice: Mastodon, bob: Mastodon) -> None:
    status = alice.status_post("filtered grouping")
    bob.status_favourite(status.id)
    bob.status_post("@alice mention too")

    only_favs = alice.grouped_notifications(types=["favourite"])
    assert {g.type for g in only_favs.notification_groups} == {"favourite"}

    no_favs = alice.grouped_notifications(exclude_types=["favourite"])
    assert "favourite" not in {g.type for g in no_favs.notification_groups}


def test_unknown_group_key_404s(alice: Mastodon) -> None:
    from mastodon.errors import MastodonNotFoundError

    try:
        result = alice.grouped_notification("favourite-99999999")
    except MastodonNotFoundError:
        return
    # If it didn't raise, it must at least be empty (no fabricated group).
    assert result.notification_groups == []
