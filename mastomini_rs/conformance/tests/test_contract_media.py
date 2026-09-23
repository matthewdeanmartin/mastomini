"""Contract tests for the media GET / PUT update endpoints."""

from __future__ import annotations

import io

import pytest
from mastodon import Mastodon
from mastodon.errors import MastodonNotFoundError

_PNG = b"\x89PNG\r\n\x1a\n" + b"0" * 64


def test_media_get_round_trips(alice: Mastodon) -> None:
    media = alice.media_post(io.BytesIO(_PNG), mime_type="image/png", file_name="x.png")
    fetched = alice.media(media.id)
    assert fetched.id == media.id
    assert fetched.type == "image"


def test_media_update_description_and_focus(alice: Mastodon) -> None:
    media = alice.media_post(io.BytesIO(_PNG), mime_type="image/png", file_name="x.png")
    updated = alice.media_update(media.id, description="a test image", focus=(0.1, -0.2))
    assert updated.description == "a test image"
    assert updated.meta["focus"]["x"] == pytest.approx(0.1)
    assert updated.meta["focus"]["y"] == pytest.approx(-0.2)


def test_media_get_missing_is_404(alice: Mastodon) -> None:
    with pytest.raises(MastodonNotFoundError):
        alice.media(999999)
