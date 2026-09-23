"""Extended Mastodon.py contract tests for the remaining Full endpoints."""

from __future__ import annotations

import io

from mastodon import Mastodon


def test_status_edit_and_history(alice: Mastodon) -> None:
    status = alice.status_post("original text")
    edited = alice.status_update(status.id, "edited text")
    assert "edited text" in edited.content
    assert edited.edited_at is not None

    history = alice.status_history(status.id)
    assert len(history) == 2  # one edit → two entries


def test_status_source(alice: Mastodon) -> None:
    status = alice.status_post("source me")
    source = alice.status_source(status.id)
    assert source.text == "source me"


def test_reply_and_context(alice: Mastodon, bob: Mastodon) -> None:
    root = alice.status_post("root post")
    reply = bob.status_reply(root, "a reply")
    assert reply.in_reply_to_id == root.id

    context = alice.status_context(reply.id)
    assert any(s.id == root.id for s in context.ancestors)


def test_poll_create_and_vote(alice: Mastodon, bob: Mastodon) -> None:
    poll = alice.make_poll(["red", "blue"], expires_in=3600)
    status = alice.status_post("favourite color?", poll=poll)
    assert status.poll is not None
    poll_id = status.poll.id

    voted = bob.poll_vote(poll_id, [0])
    assert voted.votes_count == 1
    assert 0 in voted.own_votes


def test_media_upload_and_attach(alice: Mastodon) -> None:
    media = alice.media_post(io.BytesIO(b"\x89PNG\r\n\x1a\n" + b"0" * 64), mime_type="image/png", file_name="x.png")
    assert media.type == "image"
    assert media.url

    status = alice.status_post("with media", media_ids=[media.id])
    assert len(status.media_attachments) == 1
    assert status.media_attachments[0].id == media.id


def test_lists(alice: Mastodon, bob: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id
    created = alice.list_create("besties")
    assert created.title == "besties"

    alice.list_accounts_add(created.id, [bob_id])
    members = alice.list_accounts(created.id)
    assert any(a.id == bob_id for a in members)

    all_lists = alice.lists()
    assert any(lst.id == created.id for lst in all_lists)

    alice.list_delete(created.id)
    assert not any(lst.id == created.id for lst in alice.lists())


def test_scheduled_status(alice: Mastodon) -> None:
    from datetime import datetime, timedelta, timezone

    when = datetime.now(timezone.utc) + timedelta(hours=2)
    scheduled = alice.status_post("later", scheduled_at=when)
    assert scheduled.id

    listing = alice.scheduled_statuses()
    assert any(s.id == scheduled.id for s in listing)

    alice.scheduled_status_delete(scheduled.id)
    assert not any(s.id == scheduled.id for s in alice.scheduled_statuses())


def test_filters_v2(alice: Mastodon) -> None:
    created = alice.create_filter_v2(
        title="spoilers",
        context=["home"],
        filter_action="warn",
        keywords_attributes=[{"keyword": "spoiler", "whole_word": True}],
    )
    assert created.title == "spoilers"
    assert any(k.keyword == "spoiler" for k in created.keywords)

    listing = alice.filters_v2()
    assert any(f.id == created.id for f in listing)


def test_preferences_and_markers(alice: Mastodon) -> None:
    prefs = alice.preferences()
    assert "posting:default:visibility" in prefs

    status = alice.status_post("marker target")
    alice.markers_set(["home"], status.id)
    markers = alice.markers_get(["home"])
    assert markers.home.last_read_id == status.id


def test_block_and_mute(alice: Mastodon, bob: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id

    alice.account_block(bob_id)
    assert any(a.id == bob_id for a in alice.blocks())
    alice.account_unblock(bob_id)
    assert not any(a.id == bob_id for a in alice.blocks())

    alice.account_mute(bob_id)
    assert any(a.id == bob_id for a in alice.mutes())
    alice.account_unmute(bob_id)
    assert not any(a.id == bob_id for a in alice.mutes())


def test_search_v2(alice: Mastodon, bob: Mastodon) -> None:
    bob.status_post("a very searchable phrase about otters")
    results = alice.search("otters")
    assert len(results.statuses) >= 1


def test_update_credentials(alice: Mastodon) -> None:
    updated = alice.account_update_credentials(note="updated bio")
    assert updated.note == "updated bio"


def test_conversations(alice: Mastodon, bob: Mastodon) -> None:
    alice.status_post("@bob secret hello", visibility="direct")
    convos = bob.conversations()
    assert len(convos) >= 1


def test_account_statuses_pagination(alice: Mastodon) -> None:
    posted = [alice.status_post(f"post {i}").id for i in range(5)]
    me = alice.account_verify_credentials()
    statuses = alice.account_statuses(me.id, limit=3)
    assert len(statuses) == 3
    # Newest-first ordering
    assert statuses[0].id == posted[-1]


def test_status_translate_pig_latin(alice: Mastodon) -> None:
    status = alice.status_post("hello translated world")
    translation = alice.status_translate(status.id)
    # The "translation" is a deterministic Pig Latin transform, so it must differ
    # from the source (the old mock echoed content verbatim, which was useless).
    assert "ellohay" in translation.content.lower()
    assert "hello" not in translation.content.lower()
    assert translation.detected_source_language == "en"


def test_status_card_dummy_on_link(alice: Mastodon) -> None:
    status = alice.status_post("check this out https://example.com/article")
    assert status.card is not None
    assert status.card.url == "https://example.com/article"
    assert status.card.provider_name == "mastodon_mock"


def test_status_card_none_without_link(alice: Mastodon) -> None:
    status = alice.status_post("no links here, just text")
    assert status.card is None
