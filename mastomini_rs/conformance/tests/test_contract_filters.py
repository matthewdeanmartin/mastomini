"""Contract tests for the v1 + v2 content-filter endpoints."""

from __future__ import annotations

import pytest
from mastodon import Mastodon
from mastodon.errors import MastodonNotFoundError


def test_filter_v1_crud(alice: Mastodon) -> None:
    created = alice.filter_create(phrase="spoiler", context=["home"], irreversible=False)
    assert created.phrase == "spoiler"
    assert "home" in created.context

    fetched = alice.filter(created.id)
    assert fetched.id == created.id

    assert any(f.id == created.id for f in alice.filters())

    updated = alice.filter_update(created.id, phrase="bigger spoiler", irreversible=True)
    assert updated.phrase == "bigger spoiler"
    assert updated.irreversible is True

    alice.filter_delete(created.id)
    with pytest.raises(MastodonNotFoundError):
        alice.filter(created.id)


def test_filter_v2_crud_with_keywords(alice: Mastodon) -> None:
    created = alice.create_filter_v2(
        title="politics",
        context=["home", "public"],
        filter_action="warn",
        keywords_attributes=[{"keyword": "election", "whole_word": True}],
    )
    assert created.title == "politics"
    assert set(created.context) == {"home", "public"}

    fetched = alice.filter_v2(created.id)
    assert fetched.id == created.id
    assert any(f.id == created.id for f in alice.filters_v2())

    keywords = alice.filter_keywords_v2(created.id)
    assert any(k.keyword == "election" for k in keywords)

    updated = alice.update_filter_v2(created.id, title="news", filter_action="hide")
    assert updated.title == "news"
    assert updated.filter_action == "hide"

    alice.delete_filter_v2(created.id)
    with pytest.raises(MastodonNotFoundError):
        alice.filter_v2(created.id)


def test_filter_not_owned_is_404(alice: Mastodon, bob: Mastodon) -> None:
    alice_filter = alice.create_filter_v2(title="private", context=["home"], filter_action="warn")
    with pytest.raises(MastodonNotFoundError):
        bob.filter_v2(alice_filter.id)


def test_filter_statuses_v2_empty_on_fresh_filter(alice: Mastodon) -> None:
    filt = alice.create_filter_v2(title="empty", context=["home"], filter_action="warn")
    assert alice.filter_statuses_v2(filt.id) == []


def test_filter_statuses_v2_crud(alice: Mastodon) -> None:
    filt = alice.create_filter_v2(title="muted-thread", context=["home"], filter_action="warn")
    status = alice.status_post("a status to filter out")

    attached = alice.add_filter_status_v2(filt.id, status.id)
    assert str(attached.status_id) == str(status.id)

    listed = alice.filter_statuses_v2(filt.id)
    assert [s.id for s in listed] == [attached.id]

    # The attached status also surfaces on the filter itself.
    assert [s.id for s in alice.filter_v2(filt.id).statuses] == [attached.id]

    fetched = alice.filter_status_v2(attached.id)
    assert fetched.id == attached.id
    assert str(fetched.status_id) == str(status.id)

    alice.delete_filter_status_v2(attached.id)
    assert alice.filter_statuses_v2(filt.id) == []
    with pytest.raises(MastodonNotFoundError):
        alice.filter_status_v2(attached.id)


def test_filter_status_not_owned_is_404(alice: Mastodon, bob: Mastodon) -> None:
    filt = alice.create_filter_v2(title="private", context=["home"], filter_action="warn")
    status = alice.status_post("alice's status")
    attached = alice.add_filter_status_v2(filt.id, status.id)
    with pytest.raises(MastodonNotFoundError):
        bob.filter_status_v2(attached.id)
