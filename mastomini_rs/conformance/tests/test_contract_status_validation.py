"""Write-validation contract: the mock 422s what real Mastodon would.

Regression coverage for a divergence found while proving the mock against the
``activist`` bot: the write path used to accept *any* status body — empty text,
or text far longer than the ``max_characters`` the instance advertises — and
return a phantom 200. A consuming bot needs the real failure (422), so this
locks the behaviour in.
"""

from __future__ import annotations

import pytest
from mastodon import Mastodon
from mastodon.errors import MastodonAPIError


def test_empty_status_is_rejected(alice: Mastodon) -> None:
    with pytest.raises(MastodonAPIError) as exc:
        alice.status_post("")
    assert exc.value.args[1] == 422


def test_whitespace_only_status_is_rejected(alice: Mastodon) -> None:
    with pytest.raises(MastodonAPIError) as exc:
        alice.status_post("   \n\t  ")
    assert exc.value.args[1] == 422


def test_over_length_status_is_rejected(alice: Mastodon) -> None:
    # The instance advertises max_characters == 500 (see test_contract_core).
    with pytest.raises(MastodonAPIError) as exc:
        alice.status_post("x" * 501)
    assert exc.value.args[1] == 422


def test_status_at_the_limit_is_accepted(alice: Mastodon) -> None:
    status = alice.status_post("x" * 500)
    assert status.id


def test_valid_status_still_posts(alice: Mastodon) -> None:
    status = alice.status_post("a perfectly ordinary toot")
    assert status.content == "<p>a perfectly ordinary toot</p>"
