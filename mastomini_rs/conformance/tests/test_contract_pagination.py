"""Mastodon.py-driven pagination round-trip contract tests.

These drive Mastodon.py's ``.fetch_next()`` across a real multi-page result to
prove the ``Link`` header round-trips end to end (spec next_phase.md §3 item 6 /
§4 P0). Covers the three paginated surfaces most consumers touch: home timeline,
account statuses, and notifications.
"""

from __future__ import annotations

from typing import Any

from mastodon import Mastodon


def _drain(client: Mastodon, first_page: list[Any]) -> list[Any]:
    """Collect every item across all pages by following ``fetch_next``."""
    collected: list[Any] = list(first_page)
    page: Any = first_page
    # Guard against an accidental infinite loop if a Link header ever points at itself.
    for _ in range(50):
        page = client.fetch_next(page)
        if not page:
            break
        collected.extend(page)
    return collected


def test_home_timeline_pagination_round_trip(alice: Mastodon, bob: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id
    alice.account_follow(bob_id)

    posted = [bob.status_post(f"home page post {i}").id for i in range(25)]

    first = alice.timeline_home(limit=10)
    assert len(first) == 10

    all_ids = [s.id for s in _drain(alice, first)]
    for sid in posted:
        assert sid in all_ids, f"status {sid} missing from paginated home timeline"
    # No duplicates across page boundaries.
    assert len(all_ids) == len(set(all_ids))


def test_account_statuses_pagination_round_trip(alice: Mastodon) -> None:
    me = alice.account_verify_credentials()
    posted = [alice.status_post(f"acct page post {i}").id for i in range(25)]

    first = alice.account_statuses(me.id, limit=10)
    assert len(first) == 10

    all_ids = [s.id for s in _drain(alice, first)]
    for sid in posted:
        assert sid in all_ids
    assert len(all_ids) == len(set(all_ids))
    # Newest-first overall ordering preserved across pages.
    assert all_ids == sorted(all_ids, key=int, reverse=True)


def test_notifications_pagination_round_trip(alice: Mastodon, bob: Mastodon) -> None:
    alice_id = alice.account_verify_credentials().id

    # Generate 25 favourite notifications for alice from bob.
    for i in range(25):
        status = alice.status_post(f"notif source {i}")
        bob.status_favourite(status.id)

    first = alice.notifications(limit=10)
    assert len(first) == 10

    favs = [n for n in _drain(alice, first) if n.type == "favourite"]
    fav_ids = [n.id for n in favs]
    assert len(fav_ids) >= 25
    assert len(fav_ids) == len(set(fav_ids))
    assert all(n.account.id == bob.account_verify_credentials().id for n in favs)
    assert alice_id  # sanity: alice resolved
