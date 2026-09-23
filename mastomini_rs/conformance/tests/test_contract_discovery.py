"""Contract tests for discovery / instance-metadata endpoints.

These endpoints used to be bare empty-list stubs; they now return correctly-shaped,
data-derived content (shapes captured from a live mastodon.social). All access goes
through Mastodon.py per the project contract. Uses the shared conftest seed:
alice, bob, carol (local), dave (remote), with alice -> bob following.
"""

from __future__ import annotations

from mastodon import Mastodon

# --- Instance metadata --------------------------------------------------------


def test_instance_activity_shape(alice: Mastodon) -> None:
    activity = alice.instance_activity()
    assert len(activity) == 12
    week = activity[0]
    # Mastodon.py parses these into ints/datetimes; the keys must all be present.
    for key in ("week", "statuses", "logins", "registrations"):
        assert key in week


def test_instance_peers_lists_remote_domains(alice: Mastodon) -> None:
    peers = alice.instance_peers()
    # dave is the seeded "remote" account on remote.example.
    assert "remote.example" in peers


def test_custom_emojis_nonempty_and_shaped(alice: Mastodon) -> None:
    emojis = alice.custom_emojis()
    assert len(emojis) >= 1
    one = emojis[0]
    assert one.shortcode
    assert one.url
    assert one.visible_in_picker is True


def test_translation_languages_map(alice: Mastodon) -> None:
    langs = alice.instance_translation_languages()
    assert "en" in langs
    assert isinstance(langs["en"], list)
    assert "es" in langs["en"]
    # A source language never lists itself as a target.
    assert "en" not in langs["en"]


def test_instance_domain_blocks_reflect_admin_blocks(alice: Mastodon) -> None:
    # No admin blocks seeded -> empty, but correctly typed.
    assert alice.instance_domain_blocks() == []

    alice.admin_create_domain_block("blocked.example", severity="silence", public_comment="spam")
    blocks = alice.instance_domain_blocks()
    match = [b for b in blocks if b.domain == "blocked.example"]
    assert match
    assert match[0].severity == "silence"
    assert match[0].digest  # sha256 hex digest present
    assert match[0].comment == "spam"


# --- Trends -------------------------------------------------------------------


def test_trending_tags_from_local_hashtags(alice: Mastodon) -> None:
    alice.status_post("first #mocktrend post")
    alice.status_post("second #mocktrend and #other")

    tags = alice.trending_tags()
    names = {t.name for t in tags}
    assert "mocktrend" in names
    top = next(t for t in tags if t.name == "mocktrend")
    assert len(top.history) == 7  # 7-day history block


def test_trending_statuses_ranks_by_favourites(alice: Mastodon, bob: Mastodon) -> None:
    popular = alice.status_post("popular post")
    alice.status_post("unpopular post")
    bob.status_favourite(popular.id)

    trending = bob.trending_statuses()
    ids = [s.id for s in trending]
    assert popular.id in ids
    # The favourited status ranks ahead of the unfavourited one.
    assert ids[0] == popular.id


def test_trending_links_empty(alice: Mastodon) -> None:
    assert alice.trending_links() == []


# --- Suggestions / endorsements / tags ----------------------------------------


def test_suggestions_exclude_self_and_followed(alice: Mastodon) -> None:
    suggestions = alice.suggestions_v2()
    usernames = {s.account.username for s in suggestions}
    # alice follows bob and can't be suggested herself; carol (unfollowed) shows up.
    assert "alice" not in usernames
    assert "bob" not in usernames
    assert "carol" in usernames


def test_suggestions_v1_returns_bare_accounts(alice: Mastodon) -> None:
    # v1 parses elements as Account (no Suggestion wrapper).
    suggestions = alice.suggestions_v1()
    usernames = {a.username for a in suggestions}
    assert "carol" in usernames
    assert "alice" not in usernames


def test_endorsements_reflect_pins(alice: Mastodon, bob: Mastodon) -> None:
    assert alice.endorsements() == []
    bob_id = bob.account_verify_credentials().id
    alice.account_follow(bob_id)
    alice.account_pin(bob_id)  # endorse
    endorsed = alice.endorsements()
    assert any(a.username == "bob" for a in endorsed)


def test_followed_tags_reflect_tag_follows(alice: Mastodon) -> None:
    # followed_tags is now backed by real tag follows (see tag_follow).
    assert alice.followed_tags() == []
    alice.tag_follow("rust")
    followed = alice.followed_tags()
    names = {t.name for t in followed}
    assert "rust" in names
    assert all(t.following is True for t in followed)


def test_featured_tags_own_and_by_account(alice: Mastodon, bob: Mastodon) -> None:
    alice.status_post("a #showcase post")
    alice.status_post("another #showcase post")
    alice.tag_feature("showcase")  # featured tags are now an explicit, persisted choice

    own = alice.featured_tags()
    showcase = [ft for ft in own if ft.name == "showcase"]
    assert showcase
    assert int(showcase[0].statuses_count) == 2

    alice_id = alice.account_verify_credentials().id
    by_account = bob.account_featured_tags(alice_id)
    assert any(ft.name == "showcase" for ft in by_account)


# --- Notification policy & requests -------------------------------------------


def test_notifications_policy_has_for_bots(alice: Mastodon) -> None:
    policy = alice.notifications_policy()
    assert policy.for_bots == "accept"
    assert policy.for_not_following == "accept"


def test_notification_requests_empty(alice: Mastodon) -> None:
    assert alice.notification_requests() == []
