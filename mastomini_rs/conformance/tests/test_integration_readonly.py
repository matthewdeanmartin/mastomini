"""Read-only integration tests that run identically against the mock and a real server.

Every assertion here is **backend-agnostic**: it checks *shape* and *invariants*
(types, relationships, response structure) rather than hardcoded usernames or
versions, so the same test passes whether ``mastodon_client`` points at the mock
or at a live Mastodon. All operations are read-only — safe to run against a real
account (see ``conftest.py``).
"""

from __future__ import annotations

from datetime import datetime

import pytest
from mastodon import Mastodon
from mastodon.errors import MastodonNotFoundError


def test_verify_credentials_shape(mastodon_client: Mastodon) -> None:
    me = mastodon_client.account_verify_credentials()
    assert isinstance(me.id, (int, str))
    assert me.username
    assert me.acct
    assert isinstance(me.created_at, datetime)
    # verify_credentials always includes the writeable `source` sub-object.
    assert me.source is not None


def test_instance_info_shape(mastodon_client: Mastodon) -> None:
    info = mastodon_client.instance()
    assert info.version
    # Mastodon's documented default status length; the mock mirrors it.
    assert info.configuration.statuses.max_characters == 500


def test_account_self_lookup_round_trips(mastodon_client: Mastodon) -> None:
    me = mastodon_client.account_verify_credentials()
    fetched = mastodon_client.account(me.id)
    assert fetched.id == me.id
    assert fetched.username == me.username


def test_home_timeline_is_readable_and_well_shaped(mastodon_client: Mastodon) -> None:
    home = mastodon_client.timeline_home(limit=5)
    assert isinstance(home, list)
    for status in home:
        assert status.id
        assert isinstance(status.created_at, datetime)
        # content is HTML-wrapped on both backends.
        assert isinstance(status.content, str)
        assert status.account is not None
        assert status.account.acct


def test_home_timeline_pagination_link_round_trips(mastodon_client: Mastodon) -> None:
    first = mastodon_client.timeline_home(limit=2)
    if len(first) < 2:
        pytest.skip("home timeline too short to exercise pagination")
    nxt = mastodon_client.fetch_next(first)
    # fetch_next returns a (possibly empty) list, never raises, on both backends.
    assert nxt is None or isinstance(nxt, list)
    if nxt:
        assert {s.id for s in first}.isdisjoint({s.id for s in nxt})


def test_fetching_a_known_status_round_trips(mastodon_client: Mastodon) -> None:
    home = mastodon_client.timeline_home(limit=1)
    if not home:
        pytest.skip("no statuses available to refetch")
    original = home[0]
    refetched = mastodon_client.status(original.id)
    assert refetched.id == original.id
    assert refetched.content == original.content


def test_missing_status_raises_not_found(mastodon_client: Mastodon) -> None:
    # An id that does not exist must 404 on both backends.
    with pytest.raises(MastodonNotFoundError):
        mastodon_client.status("0")


def test_notifications_are_readable(mastodon_client: Mastodon) -> None:
    notifs = mastodon_client.notifications(limit=5)
    assert isinstance(notifs, list)
    for n in notifs:
        assert n.id
        assert n.type
        assert n.account is not None


def test_bulk_statuses_by_id_round_trips(mastodon_client: Mastodon) -> None:
    # Validates the id[] array-param fix against both backends: Mastodon.py sends
    # statuses(ids) as id[]=...; the response must contain exactly those statuses.
    home = mastodon_client.timeline_home(limit=3)
    if len(home) < 2:
        pytest.skip("need at least two statuses to bulk-fetch")
    ids = [s.id for s in home[:2]]
    fetched = mastodon_client.statuses(ids)
    assert {s.id for s in fetched} == set(ids)


def test_grouped_notifications_shape(mastodon_client: Mastodon) -> None:
    # Grouped notifications (4.3+) return a container with notification_groups.
    result = mastodon_client.grouped_notifications(limit=5)
    assert isinstance(result.notification_groups, list)
    for group in result.notification_groups:
        assert group.group_key
        assert group.type
        assert group.notifications_count >= 1
