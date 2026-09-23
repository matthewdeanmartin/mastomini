"""Contract tests for the timeline endpoints (public / hashtag / list)."""

from __future__ import annotations

from mastodon import Mastodon


def test_public_timeline_includes_public_posts(alice: Mastodon, bob: Mastodon) -> None:
    posted = alice.status_post("hello public world", visibility="public")
    timeline = bob.timeline_public()
    assert any(s.id == posted.id for s in timeline)


def test_public_timeline_local_filter(alice: Mastodon) -> None:
    posted = alice.status_post("local only post", visibility="public")
    local = alice.timeline_public(local=True)
    assert any(s.id == posted.id for s in local)


def test_public_timeline_remote_filter_excludes_local(alice: Mastodon) -> None:
    posted = alice.status_post("not remote", visibility="public")
    remote = alice.timeline_public(remote=True)
    # alice is a local account, so her post must not appear in the remote view.
    assert all(s.id != posted.id for s in remote)


def test_hashtag_timeline(alice: Mastodon, bob: Mastodon) -> None:
    posted = alice.status_post("loving #python today", visibility="public")
    tagged = bob.timeline_hashtag("python")
    assert any(s.id == posted.id for s in tagged)


def test_list_timeline(alice: Mastodon, bob: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id
    created = alice.list_create("crew")
    alice.list_accounts_add(created.id, [bob_id])

    bob_post = bob.status_post("post from a list member", visibility="public")
    timeline = alice.timeline_list(created.id)
    assert any(s.id == bob_post.id for s in timeline)
