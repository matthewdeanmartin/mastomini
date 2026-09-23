"""Stateful behavior added by the final Phase 1/2 pass."""

from __future__ import annotations

from datetime import datetime, timedelta, timezone

import httpx2 as httpx
import pytest
from mastodon import Mastodon
from mastodon.errors import MastodonAPIError


def test_warn_and_hide_filters_populate_filtered_results(alice: Mastodon, bob: Mastodon) -> None:
    warning = alice.create_filter_v2(
        title="spoilers",
        context=["home"],
        filter_action="warn",
        keywords_attributes=[{"keyword": "dragon", "whole_word": True}],
    )
    warned = bob.status_post("a dragon appears")
    home = alice.timeline_home()
    rendered = next(status for status in home if status.id == warned.id)
    assert rendered.filtered[0].filter.id == warning.id
    assert rendered.filtered[0].keyword_matches == ["dragon"]

    hidden_filter = alice.create_filter_v2(title="hide one", context=["home"], filter_action="hide")
    alice.add_filter_status_v2(hidden_filter, warned.id)
    rendered = next(status for status in alice.timeline_home() if status.id == warned.id)
    assert {match.filter.id for match in rendered.filtered} == {warning.id, hidden_filter.id}
    assert next(match for match in rendered.filtered if match.filter.id == hidden_filter.id).status_matches == [
        str(warned.id)
    ]

    alice.create_filter_v2(
        title="expired",
        context=["home"],
        filter_action="warn",
        expires_in=-1,
        keywords_attributes=[{"keyword": "appears", "whole_word": True}],
    )
    rendered = next(status for status in alice.timeline_home() if status.id == warned.id)
    assert "expired" not in {match.filter.title for match in rendered.filtered}


def test_quote_policy_is_enforced(alice: Mastodon, bob: Mastodon) -> None:
    target = alice.status_post("do not quote", quote_approval_policy="nobody")
    with pytest.raises(MastodonAPIError):
        bob.status_post("trying", quoted_status_id=target.id)

    followers_only = alice.status_post("followers may quote", quote_approval_policy="followers")
    with pytest.raises(MastodonAPIError):
        bob.status_post("not yet", quoted_status_id=followers_only.id)

    alice_id = alice.account_verify_credentials().id
    bob.account_follow(alice_id)
    quoted = bob.status_post("now allowed", quoted_status_id=followers_only.id)
    assert quoted.quote.quoted_status.id == followers_only.id


def test_suggestion_dismissal_persists(live_server: str, alice: Mastodon) -> None:
    suggestions = alice.suggestions_v2()
    target = next(item.account for item in suggestions if item.account.username == "carol")
    response = httpx.delete(
        f"{live_server}/api/v1/suggestions/{target.id}",
        headers={"Authorization": "Bearer alice_token"},
    )
    response.raise_for_status()
    assert target.id not in {item.account.id for item in alice.suggestions_v2()}


def test_notification_policy_filters_accepts_and_overrides(alice: Mastodon, carol: Mastodon) -> None:
    policy = alice.update_notifications_policy(for_not_following="filter")
    assert policy.for_not_following == "filter"

    status = alice.status_post("notification policy target")
    carol.status_favourite(status.id)
    assert alice.notifications() == []

    requests = alice.notification_requests()
    assert len(requests) == 1
    assert requests[0].account.username == "carol"
    assert requests[0].notifications_count == "1"
    assert alice.notifications_policy().summary.pending_notifications_count == 1

    alice.accept_notification_request(requests[0].id)
    assert alice.notifications()[0].account.username == "carol"
    assert alice.notification_requests() == []

    second = alice.status_post("accepted actor remains accepted")
    carol.status_favourite(second.id)
    assert any(notification.status.id == second.id for notification in alice.notifications())


def test_notification_policy_drop_creates_neither_notification_nor_request(alice: Mastodon, carol: Mastodon) -> None:
    alice.update_notifications_policy(for_not_following="drop")
    status = alice.status_post("drop target")
    carol.status_favourite(status.id)
    assert alice.notifications() == []
    assert alice.notification_requests() == []


def test_admin_suspend_blocks_login_and_hides_existing_status(live_server: str, alice: Mastodon, bob: Mastodon) -> None:
    existing = bob.status_post("visible before suspension")
    bob_id = bob.account_verify_credentials().id
    alice.admin_account_moderate(bob_id, action="suspend")

    with pytest.raises(MastodonAPIError):
        bob.status_post("blocked")
    public = httpx.get(f"{live_server}/api/v1/timelines/public").json()
    assert existing.id not in {int(status["id"]) for status in public}

    alice.admin_account_unsuspend(bob_id)
    assert bob.status_post("works again")


def test_admin_sensitive_and_silence_have_public_effects(live_server: str, alice: Mastodon, bob: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id
    alice.admin_account_moderate(bob_id, action="sensitive")
    sensitive = bob.status_post("forced sensitive")
    assert bob.status(sensitive.id).sensitive is True

    alice.admin_account_moderate(bob_id, action="silence")
    anonymous = httpx.get(f"{live_server}/api/v1/timelines/public").json()
    assert sensitive.id not in {int(status["id"]) for status in anonymous}
    # Alice follows Bob, so the limited account remains visible to her.
    assert sensitive.id in {status.id for status in alice.timeline_home()}


def test_rejecting_trend_removes_it_from_public_results(alice: Mastodon, bob: Mastodon) -> None:
    status = bob.status_post("trend review target")
    alice.status_favourite(status.id)
    assert status.id in {item.id for item in alice.trending_statuses()}

    alice.admin_reject_trending_status(status.id)
    assert status.id not in {item.id for item in alice.trending_statuses()}

    alice.admin_approve_trending_status(status.id)
    assert status.id in {item.id for item in alice.trending_statuses()}


def test_scheduled_announcement_visibility(live_server: str) -> None:
    headers = {"Authorization": "Bearer alice_token"}
    future = datetime.now(timezone.utc) + timedelta(days=1)
    response = httpx.post(
        f"{live_server}/api/v1/admin/announcements",
        headers=headers,
        json={"text": "Tomorrow", "starts_at": future.isoformat()},
    )
    response.raise_for_status()
    announcement_id = response.json()["id"]
    public = httpx.get(f"{live_server}/api/v1/announcements", headers=headers).json()
    assert announcement_id not in {item["id"] for item in public}


def test_domain_block_can_reject_reports(alice: Mastodon) -> None:
    remote = alice.account_lookup("dave@remote.example")
    alice.admin_create_domain_block("remote.example", reject_reports=True)
    with pytest.raises(MastodonAPIError):
        alice.report(remote.id, comment="blocked forwarding")


def test_signup_email_block_is_enforced(live_server: str, alice: Mastodon) -> None:
    alice.admin_create_email_domain_block("blocked.example")
    response = httpx.post(
        f"{live_server}/api/v1/accounts",
        headers={"Authorization": "Bearer alice_token"},
        data={
            "username": "blocked_signup",
            "email": "person@blocked.example",
            "password": "not-a-real-password",
            "agreement": "true",
        },
    )
    assert response.status_code == 422


def test_admin_status_filter_is_applied_before_pagination(live_server: str, alice: Mastodon, bob: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id
    alice.admin_account_moderate(bob_id, action="suspend")
    response = httpx.get(
        f"{live_server}/api/v2/admin/accounts",
        headers={"Authorization": "Bearer alice_token"},
        params={"status": "suspended", "limit": 1},
    )
    response.raise_for_status()
    assert [item["id"] for item in response.json()] == [str(bob_id)]
