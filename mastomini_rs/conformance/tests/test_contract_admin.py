"""Contract tests for the admin / moderation API (spec/03-api-coverage.md "admin").

All access goes through Mastodon.py's ``mastodon/admin.py`` methods, per the
project's "if Mastodon.py supports it" contract. Auth is faked: any seeded account
may call admin endpoints (no role enforcement — see spec/00-overview.md non-goals).
"""

from __future__ import annotations

from datetime import datetime, timedelta, timezone

import pytest
from mastodon import Mastodon
from mastodon.errors import MastodonNotFoundError

# --- Accounts -----------------------------------------------------------------


def test_admin_accounts_v2_lists_local(alice: Mastodon) -> None:
    accounts = alice.admin_accounts_v2()
    assert len(accounts) >= 1
    usernames = {a.username for a in accounts}
    # local accounts only by default
    assert "alice" in usernames
    assert "dave" not in usernames  # remote
    one = accounts[0]
    assert one.role is not None
    assert one.account is not None  # nested Account entity


def test_admin_accounts_v2_origin_remote(alice: Mastodon) -> None:
    remote = alice.admin_accounts_v2(origin="remote")
    assert {a.username for a in remote} == {"dave"}


def test_admin_accounts_v1_username_filter(alice: Mastodon) -> None:
    # NB: Mastodon.py types admin_accounts_v1 as returning a single AdminAccount,
    # so list elements arrive as plain dicts rather than entity objects.
    found = alice.admin_accounts_v1(username="ali")
    assert [a["username"] for a in found] == ["alice"]


def test_admin_account_fetch_and_moderation_cycle(alice: Mastodon, bob: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id

    fetched = alice.admin_account(bob_id)
    assert fetched.username == "bob"
    assert fetched.suspended is False

    # Suspend via the moderation action endpoint, then observe + reverse it.
    alice.admin_account_moderate(bob_id, action="suspend")
    assert alice.admin_account(bob_id).suspended is True

    reenabled = alice.admin_account_unsuspend(bob_id)
    assert reenabled.suspended is False

    # Silence / unsilence.
    alice.admin_account_moderate(bob_id, action="silence")
    assert alice.admin_account(bob_id).silenced is True
    assert alice.admin_account_unsilence(bob_id).silenced is False

    # Disable / enable.
    alice.admin_account_moderate(bob_id, action="disable")
    assert alice.admin_account(bob_id).disabled is True
    assert alice.admin_account_enable(bob_id).disabled is False

    # Sensitive / unsensitive.
    alice.admin_account_moderate(bob_id, action="sensitive")
    assert alice.admin_account(bob_id).sensitized is True
    assert alice.admin_account_unsensitive(bob_id).sensitized is False


def test_admin_account_delete(alice: Mastodon, bob: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id
    deleted = alice.admin_account_delete(bob_id)
    assert deleted.username == "bob"
    with pytest.raises(MastodonNotFoundError):
        alice.admin_account(bob_id)


def test_admin_account_delete_removes_their_statuses(alice: Mastodon, bob: Mastodon) -> None:
    # A deleted account's statuses must not survive as orphans: real Mastodon takes
    # the user's content down with them, and serialize_status requires the author
    # to resolve, so a dangling account_id previously crashed any later read (500).
    bob_id = bob.account_verify_credentials().id
    post = bob.status_post("orphan me")

    alice.admin_account_delete(bob_id)

    with pytest.raises(MastodonNotFoundError):
        alice.status(post.id)


# --- Reports ------------------------------------------------------------------


def test_report_then_admin_report_lifecycle(alice: Mastodon, bob: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id
    post = bob.status_post("something reportable")

    report = alice.report(bob_id, status_ids=[post.id], comment="spam", category="spam")
    assert report.comment == "spam"
    assert report.category == "spam"

    # Shows up in the admin (unresolved) queue.
    open_reports = alice.admin_reports()
    assert any(r.id == report.id for r in open_reports)

    full = alice.admin_report(report.id)
    assert full.target_account.username == "bob"
    assert len(full.statuses) == 1

    # Assign to self, then resolve.
    assigned = alice.admin_report_assign(report.id)
    assert assigned.assigned_account.username == "alice"

    resolved = alice.admin_report_resolve(report.id)
    assert resolved.action_taken is True

    # Resolved reports leave the default (unresolved) queue and join the resolved one.
    assert all(r.id != report.id for r in alice.admin_reports())
    assert any(r.id == report.id for r in alice.admin_reports(resolved=True))

    reopened = alice.admin_report_reopen(report.id)
    assert reopened.action_taken is False


# --- Domain blocks ------------------------------------------------------------


def test_admin_domain_block_crud(alice: Mastodon) -> None:
    block = alice.admin_create_domain_block("evil.example", severity="suspend", reject_media=True)
    assert block.domain == "evil.example"
    assert block.severity == "suspend"
    assert block.reject_media is True

    assert any(b.id == block.id for b in alice.admin_domain_blocks())
    assert alice.admin_domain_blocks(id=block.id).domain == "evil.example"

    updated = alice.admin_update_domain_block(block.id, severity="silence")
    assert updated.severity == "silence"

    alice.admin_delete_domain_block(block.id)
    assert all(b.id != block.id for b in alice.admin_domain_blocks())


# --- Domain allows ------------------------------------------------------------


def test_admin_domain_allow_crud(alice: Mastodon) -> None:
    allow = alice.admin_create_domain_allow("friend.example")
    assert allow.domain == "friend.example"
    assert any(a.id == allow.id for a in alice.admin_domain_allows())
    assert alice.admin_domain_allow(allow.id).domain == "friend.example"
    alice.admin_delete_domain_allow(allow.id)
    assert all(a.id != allow.id for a in alice.admin_domain_allows())


# --- Email domain blocks ------------------------------------------------------


def test_admin_email_domain_block_crud(alice: Mastodon) -> None:
    block = alice.admin_create_email_domain_block("spam.example")
    assert block.domain == "spam.example"
    assert any(b.id == block.id for b in alice.admin_email_domain_blocks())
    assert alice.admin_email_domain_block(block.id).domain == "spam.example"
    alice.admin_delete_email_domain_block(block.id)
    assert all(b.id != block.id for b in alice.admin_email_domain_blocks())


# --- Canonical email blocks ---------------------------------------------------


def test_admin_canonical_email_block_crud(alice: Mastodon) -> None:
    block = alice.admin_create_canonical_email_block(email="Bad.Person+spam@example.com")
    assert block.canonical_email_hash

    assert any(b.id == block.id for b in alice.admin_canonical_email_blocks())
    assert alice.admin_canonical_email_block(block.id).id == block.id

    # The test endpoint canonicalizes equivalently (dots/+suffix stripped).
    matches = alice.admin_test_canonical_email_block(email="badperson@example.com")
    assert any(m.id == block.id for m in matches)

    alice.admin_delete_canonical_email_block(block.id)
    assert all(b.id != block.id for b in alice.admin_canonical_email_blocks())


# --- IP blocks ----------------------------------------------------------------


def test_admin_ip_block_crud(alice: Mastodon) -> None:
    block = alice.admin_create_ip_block("192.0.2.0/24", severity="no_access", comment="abuse")
    assert block.ip == "192.0.2.0/24"
    assert block.severity == "no_access"
    assert block.comment == "abuse"

    assert any(b.id == block.id for b in alice.admin_ip_blocks())
    assert alice.admin_ip_block(block.id).ip == "192.0.2.0/24"

    updated = alice.admin_update_ip_block(block.id, comment="updated")
    assert updated.comment == "updated"

    alice.admin_delete_ip_block(block.id)
    assert all(b.id != block.id for b in alice.admin_ip_blocks())


# --- Trends -------------------------------------------------------------------


def test_admin_trending_tags_derived_from_local_usage(alice: Mastodon) -> None:
    # Seed a tagged status so there is a hashtag to trend on.
    alice.status_post("loving the #mockfest today")
    tags = alice.admin_trending_tags()
    assert any(t.name == "mockfest" for t in tags)
    # AdminTag shape carries the moderation flags.
    trended = next(t for t in tags if t.name == "mockfest")
    assert trended.requires_review is False
    assert trended.trendable is True


def test_admin_trending_statuses_ranked_by_favourites(alice: Mastodon, bob: Mastodon) -> None:
    status = alice.status_post("a popular post")
    bob.status_favourite(status.id)
    statuses = alice.admin_trending_statuses()
    assert any(s.id == status.id for s in statuses)


def test_admin_trending_links_are_empty(alice: Mastodon) -> None:
    # Links stay Stub: no preview-card synthesis for trends.
    assert alice.admin_trending_links() == []


# --- Measures / dimensions / retention (shaped stubs) -------------------------


def test_admin_measures_shape(alice: Mastodon) -> None:
    start = datetime.now(timezone.utc) - timedelta(days=5)
    end = datetime.now(timezone.utc)
    measures = alice.admin_measures(start, end, active_users=True, new_users=True)
    keys = {m.key for m in measures}
    assert keys == {"active_users", "new_users"}
    assert all(m.total == "0" for m in measures)


def test_admin_dimensions_shape(alice: Mastodon) -> None:
    start = datetime.now(timezone.utc) - timedelta(days=5)
    end = datetime.now(timezone.utc)
    dims = alice.admin_dimensions(start, end, languages=True)
    assert {d.key for d in dims} == {"languages"}


def test_admin_retention_shape(alice: Mastodon) -> None:
    start = datetime.now(timezone.utc) - timedelta(days=5)
    end = datetime.now(timezone.utc)
    assert alice.admin_retention(start, end) == []
