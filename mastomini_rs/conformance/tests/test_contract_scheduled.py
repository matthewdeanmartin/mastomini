"""Contract tests for scheduled-status publication (next_phase.md §3 item 1 / §4 P1).

The mock has no background time driver, so scheduled statuses are published
lazily: a ``scheduled_at`` within ~5 minutes publishes immediately (returns a
Status), and a due scheduled status is converted to a real status when the
scheduled list is read.
"""

from __future__ import annotations

from datetime import datetime, timedelta, timezone

import pytest
from mastodon import Mastodon
from mastodon.errors import MastodonAPIError


def test_far_future_schedule_returns_scheduled_status(alice: Mastodon) -> None:
    when = datetime.now(timezone.utc) + timedelta(hours=2)
    scheduled = alice.status_post("future toot", scheduled_at=when)
    # Mastodon.py returns a ScheduledStatus (has scheduled_at, no content).
    assert scheduled.scheduled_at is not None
    assert any(s.id == scheduled.id for s in alice.scheduled_statuses())


def test_near_term_schedule_publishes_immediately(alice: Mastodon) -> None:
    # Under the ~5 minute threshold → published now, returned as a real Status.
    when = datetime.now(timezone.utc) + timedelta(minutes=1)
    posted = alice.status_post("almost now", scheduled_at=when)
    assert posted.content == "<p>almost now</p>"

    # It is a real status, fetchable by id, and not in the scheduled list.
    assert alice.status(posted.id).id == posted.id
    assert not any(s.id == posted.id for s in alice.scheduled_statuses())


def test_due_scheduled_status_is_published_on_list(alice: Mastodon) -> None:
    # Schedule far enough out to be stored, then reschedule into the past to make
    # it due; listing should publish it and drop it from the scheduled list.
    when = datetime.now(timezone.utc) + timedelta(hours=2)
    scheduled = alice.status_post("eventually visible", scheduled_at=when)
    assert any(s.id == scheduled.id for s in alice.scheduled_statuses())

    past = datetime.now(timezone.utc) - timedelta(minutes=1)
    alice.scheduled_status_update(scheduled.id, scheduled_at=past)

    # Reading the scheduled list publishes the due item.
    remaining = alice.scheduled_statuses()
    assert not any(s.id == scheduled.id for s in remaining)

    # The published status now appears on alice's own statuses.
    me = alice.account_verify_credentials()
    contents = [s.content for s in alice.account_statuses(me.id)]
    assert any("eventually visible" in c for c in contents)


def test_scheduled_status_cannot_be_read_by_another_account(alice: Mastodon, bob: Mastodon) -> None:
    scheduled = alice.status_post(
        "alice's private future post", scheduled_at=datetime.now(timezone.utc) + timedelta(hours=2)
    )

    with pytest.raises(MastodonAPIError):
        bob.scheduled_status(scheduled.id)


def test_scheduled_status_cannot_be_rescheduled_by_another_account(alice: Mastodon, bob: Mastodon) -> None:
    original_at = datetime.now(timezone.utc) + timedelta(hours=2)
    scheduled = alice.status_post("alice controls the timing", scheduled_at=original_at)

    with pytest.raises(MastodonAPIError):
        bob.scheduled_status_update(scheduled.id, scheduled_at=original_at + timedelta(hours=1))

    assert alice.scheduled_status(scheduled.id).scheduled_at == scheduled.scheduled_at


def test_scheduled_status_cannot_be_deleted_by_another_account(alice: Mastodon, bob: Mastodon) -> None:
    scheduled = alice.status_post(
        "alice decides whether to cancel", scheduled_at=datetime.now(timezone.utc) + timedelta(hours=2)
    )

    with pytest.raises(MastodonAPIError):
        bob.scheduled_status_delete(scheduled.id)

    assert alice.scheduled_status(scheduled.id).id == scheduled.id
