"""Contract tests for tag follow/unfollow and quote revocation / approval policy.

All access goes through Mastodon.py (4.5+ methods). Uses the shared conftest seed.
"""

from __future__ import annotations

import pytest
from mastodon import Mastodon
from mastodon.errors import MastodonAPIError

# --- Tag follow / unfollow ----------------------------------------------------


def test_tag_follow_unfollow_roundtrip(alice: Mastodon) -> None:
    tag = alice.tag_follow("python")
    assert tag.name == "python"
    assert tag.following is True

    followed = {t.name for t in alice.followed_tags()}
    assert "python" in followed

    # Fetching the tag reflects the follow state.
    assert alice.tag("python").following is True

    tag = alice.tag_unfollow("python")
    assert tag.following is False
    assert "python" not in {t.name for t in alice.followed_tags()}


def test_tag_follow_is_idempotent_and_per_account(alice: Mastodon, bob: Mastodon) -> None:
    alice.tag_follow("django")
    alice.tag_follow("django")  # second follow is a no-op
    assert [t.name for t in alice.followed_tags()] == ["django"]

    # bob's follows are independent of alice's.
    assert alice.tag("django").following is True
    assert bob.tag("django").following is False


def test_tag_name_normalized(alice: Mastodon) -> None:
    alice.tag_follow("MixedCase")
    # Hashtags normalize to lowercase, so the followed name is lowercased.
    assert "mixedcase" in {t.name for t in alice.followed_tags()}


# --- Featured tags ------------------------------------------------------------


def test_featured_tag_create_and_list(alice: Mastodon) -> None:
    alice.status_post("a #python post")
    alice.status_post("more #python today")

    ft = alice.featured_tag_create("python")
    assert ft.name == "python"
    # Usage stats are derived from the account's statuses bearing the tag.
    assert int(ft.statuses_count) == 2

    listed = {f.name: f for f in alice.featured_tags()}
    assert "python" in listed
    assert int(listed["python"].statuses_count) == 2

    # The tag now reports itself as featured for the viewer.
    assert alice.tag("python").featuring is True


def test_featured_tag_delete_by_id(alice: Mastodon) -> None:
    ft = alice.featured_tag_create("django")
    assert any(f.name == "django" for f in alice.featured_tags())
    alice.featured_tag_delete(ft.id)
    assert all(f.name != "django" for f in alice.featured_tags())


def test_tag_feature_unfeature_aliases(alice: Mastodon) -> None:
    tag = alice.tag_feature("rust")
    assert tag.name == "rust"
    assert tag.featuring is True
    assert any(f.name == "rust" for f in alice.featured_tags())

    tag = alice.tag_unfeature("rust")
    assert tag.featuring is False
    assert all(f.name != "rust" for f in alice.featured_tags())


def test_featured_tags_visible_to_other_accounts(alice: Mastodon, bob: Mastodon) -> None:
    alice.tag_feature("astronomy")
    alice_id = alice.account_verify_credentials().id
    seen = {f.name for f in bob.account_featured_tags(alice_id)}
    assert "astronomy" in seen


def test_featured_tag_suggestions_exclude_featured(alice: Mastodon) -> None:
    alice.status_post("about #python")
    alice.status_post("about #golang")
    alice.featured_tag_create("python")

    suggested = {f.name for f in alice.featured_tag_suggestions()}
    # golang is used but not featured -> suggested; python is featured -> excluded.
    assert "golang" in suggested
    assert "python" not in suggested


def test_featured_tag_create_is_idempotent(alice: Mastodon) -> None:
    alice.featured_tag_create("once")
    alice.featured_tag_create("once")
    names = [f.name for f in alice.featured_tags()]
    assert names.count("once") == 1


# --- Quote revocation ---------------------------------------------------------


def test_quote_revoke_hides_quoted_status(alice: Mastodon, bob: Mastodon) -> None:
    original = alice.status_post("quotable original")
    quoting = bob.status_post("quoting it", quoted_status_id=original.id)
    assert quoting.quote.state == "accepted"
    assert quoting.quote.quoted_status is not None

    revoked = alice.status_quote_revoke(original.id, quoting.id)
    assert revoked.quote.state == "revoked"
    # A revoked quote no longer exposes the quoted status.
    assert revoked.quote.quoted_status is None

    # The revocation persists on subsequent reads.
    refetched = bob.status(quoting.id)
    assert refetched.quote.state == "revoked"


def test_quote_revoke_requires_ownership_of_quoted(alice: Mastodon, bob: Mastodon) -> None:
    original = alice.status_post("mine")
    quoting = bob.status_post("quoting", quoted_status_id=original.id)
    # bob does not own the quoted status, so cannot revoke.
    with pytest.raises(MastodonAPIError):
        bob.status_quote_revoke(original.id, quoting.id)


# --- Quote approval policy ----------------------------------------------------


def test_update_quote_approval_policy(alice: Mastodon) -> None:
    status = alice.status_post("set a policy on me")
    assert status.quote_approval_policy == "public"

    updated = alice.status_update_quote_approval_policy(status.id, "followers")
    assert updated.quote_approval_policy == "followers"

    # Persisted across reads.
    assert alice.status(status.id).quote_approval_policy == "followers"


def test_quote_policy_forced_to_nobody_for_private(alice: Mastodon) -> None:
    status = alice.status_post("secret", visibility="private")
    updated = alice.status_update_quote_approval_policy(status.id, "public")
    # Private/direct statuses are forced to "nobody" regardless of input.
    assert updated.quote_approval_policy == "nobody"


def test_quote_policy_rejects_invalid_value(alice: Mastodon) -> None:
    status = alice.status_post("policy validation")
    with pytest.raises(MastodonAPIError):
        alice.status_update_quote_approval_policy(status.id, "everyone")
