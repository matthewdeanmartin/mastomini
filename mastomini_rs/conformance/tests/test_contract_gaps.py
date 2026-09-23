"""Contract tests for the §3 "known gaps / sharp edges" (next_phase.md).

Each test pins down behavior that previously had no coverage and was a likely
mock-vs-real divergence point. Covers gap items 2 (reply/mention threading +
CW/visibility inheritance), 3 (``account_statuses(only_media=...)`` scoping),
5 (domain-block relationship surfacing), and 7 (``update_credentials`` fields).
"""

from __future__ import annotations

import io

from mastodon import Mastodon

# --- §3 item 2: reply + mention threading, CW/visibility ---------------------


def test_reply_chain_threads_and_notifies(alice: Mastodon, bob: Mastodon) -> None:
    alice_id = alice.account_verify_credentials().id
    root = alice.status_post("root of the thread")
    reply = bob.status_reply(root, "first reply")
    deep = alice.status_reply(reply, "second reply")

    # in_reply_to chains are wired both ways.
    assert reply.in_reply_to_id == root.id
    assert deep.in_reply_to_id == reply.id

    # Context resolves the full ancestor chain for the deepest node.
    context = bob.status_context(deep.id)
    ancestor_ids = {s.id for s in context.ancestors}
    assert root.id in ancestor_ids
    assert reply.id in ancestor_ids

    # bob's reply mentions nobody explicitly; alice's reply to bob does not auto-
    # mention, but an explicit @mention in a reply still notifies the target.
    mention_reply = bob.status_reply(root, "ping @alice in a reply")
    assert any(m.acct == "alice" for m in mention_reply.mentions)
    notifs = alice.notifications(types=["mention"])
    assert any(n.status and n.status.id == mention_reply.id for n in notifs)
    assert alice_id  # resolved


def test_remote_mention_resolves_against_local_row(alice: Mastodon) -> None:
    # dave@remote.example is seeded as a remote row, so @dave@remote.example resolves.
    status = alice.status_post("hello @dave@remote.example from a reply")
    assert any(m.acct == "dave@remote.example" for m in status.mentions)


def test_reply_can_override_visibility_and_cw(alice: Mastodon, bob: Mastodon) -> None:
    root = alice.status_post("public root")
    reply = bob.status_reply(
        root,
        "sensitive reply",
        spoiler_text="cw: spoiler",
        visibility="unlisted",
    )
    assert reply.spoiler_text == "cw: spoiler"
    assert reply.visibility == "unlisted"
    # The CW/visibility are independent of the (public) root.
    assert root.visibility == "public"


# --- §3 item 3: account_statuses(only_media=...) scoping ---------------------


def test_only_media_scopes_to_target_account(alice: Mastodon, bob: Mastodon) -> None:
    alice_id = alice.account_verify_credentials().id
    bob_id = bob.account_verify_credentials().id

    # alice posts one media status and one text-only status.
    media = alice.media_post(io.BytesIO(b"\x89PNG\r\n\x1a\n" + b"0" * 32), mime_type="image/png", file_name="a.png")
    alice_media_status = alice.status_post("alice with media", media_ids=[media.id])
    alice.status_post("alice text only")

    # bob also posts a media status (must NOT leak into alice's only_media view).
    bob_media = bob.media_post(io.BytesIO(b"\x89PNG\r\n\x1a\n" + b"0" * 32), mime_type="image/png", file_name="b.png")
    bob_media_status = bob.status_post("bob with media", media_ids=[bob_media.id])

    alice_media = alice.account_statuses(alice_id, only_media=True)
    ids = {s.id for s in alice_media}
    assert alice_media_status.id in ids
    assert bob_media_status.id not in ids  # scoped to alice only
    assert all(len(s.media_attachments) >= 1 for s in alice_media)
    assert bob_id  # resolved


# --- §3 item 5: domain block surfaced in relationship ------------------------


def test_domain_block_reflected_in_relationship(alice: Mastodon) -> None:
    dave = alice.account_lookup("dave@remote.example")
    rel_before = alice.account_relationships(dave.id)[0]
    assert rel_before.domain_blocking is False

    alice.domain_block("remote.example")
    assert "remote.example" in alice.domain_blocks()

    rel_after = alice.account_relationships(dave.id)[0]
    assert rel_after.domain_blocking is True

    alice.domain_unblock("remote.example")
    rel_cleared = alice.account_relationships(dave.id)[0]
    assert rel_cleared.domain_blocking is False


# --- §3 item 7: update_credentials fields + avatar ---------------------------


def test_update_credentials_fields_and_avatar(alice: Mastodon) -> None:
    updated = alice.account_update_credentials(
        display_name="Alice Updated",
        fields=[("Website", "https://example.com"), ("Pronouns", "she/her")],
    )
    assert updated.display_name == "Alice Updated"
    field_map = {f.name: f.value for f in updated.fields}
    assert field_map.get("Website") == "https://example.com"
    assert field_map.get("Pronouns") == "she/her"

    with_avatar = alice.account_update_credentials(
        avatar=io.BytesIO(b"\x89PNG\r\n\x1a\n" + b"0" * 32),
        avatar_mime_type="image/png",
    )
    assert with_avatar.avatar
    # Fields set in the prior call persist across a subsequent partial update.
    assert any(f.name == "Website" for f in with_avatar.fields)
