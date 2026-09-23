"""Contract tests for the spec/openapi_compare_report.md truth-only backlog.

Each test exercises one of the endpoints added to close that backlog
(see spec/03-api-coverage.md and tests/openapi/allowlist.py, which is now
empty of TRUTH_ONLY entries). Endpoints with no Mastodon.py caller are driven
with plain ``requests`` instead.
"""

from __future__ import annotations

import pytest
import requests
from mastodon import Mastodon
from mastodon.errors import MastodonNotFoundError


def test_push_subscription_round_trip(alice: Mastodon) -> None:
    sub = alice.push_subscription_set(
        "https://push.example/endpoint",
        {"pubkey": b"fake-pubkey", "auth": b"fake-auth"},
        follow_events=True,
        favourite_events=False,
    )
    assert sub.endpoint == "https://push.example/endpoint"
    assert sub.alerts.follow is True
    assert sub.alerts.favourite is False

    fetched = alice.push_subscription()
    assert fetched.id == sub.id

    updated = alice.push_subscription_update(follow_events=False)
    assert updated.alerts.follow is False

    alice.push_subscription_delete()
    with pytest.raises(MastodonNotFoundError):
        alice.push_subscription()


def test_filter_keyword_v2_get_and_put(alice: Mastodon) -> None:
    filt = alice.create_filter_v2(title="test filter", context=["home"], filter_action="warn")
    kw = alice.add_filter_keyword_v2(filt, "spoiler", whole_word=True)

    fetched = requests.get(
        f"{alice.api_base_url}/api/v2/filters/keywords/{kw.id}",
        headers={"Authorization": "Bearer alice_token"},
        timeout=10,
    )
    assert fetched.status_code == 200
    assert fetched.json()["keyword"] == "spoiler"

    updated = requests.put(
        f"{alice.api_base_url}/api/v2/filters/keywords/{kw.id}",
        headers={"Authorization": "Bearer alice_token"},
        data={"keyword": "redacted"},
        timeout=10,
    )
    assert updated.status_code == 200
    assert updated.json()["keyword"] == "redacted"


def test_profile_get_and_patch_mirrors_update_credentials(alice: Mastodon) -> None:
    profile = requests.get(
        f"{alice.api_base_url}/api/v1/profile",
        headers={"Authorization": "Bearer alice_token"},
        timeout=10,
    )
    assert profile.status_code == 200
    assert profile.json()["username"] == "alice"

    patched = requests.patch(
        f"{alice.api_base_url}/api/v1/profile",
        headers={"Authorization": "Bearer alice_token"},
        data={"display_name": "Alice Patched"},
        timeout=10,
    )
    assert patched.status_code == 200
    assert patched.json()["display_name"] == "Alice Patched"


def test_conversation_delete(alice: Mastodon, bob: Mastodon) -> None:
    alice.status_post("hey @bob", visibility="direct")
    convo = alice.conversations()[0]
    alice.conversations_read(convo)

    resp = requests.delete(
        f"{alice.api_base_url}/api/v1/conversations/{convo.id}",
        headers={"Authorization": "Bearer alice_token"},
        timeout=10,
    )
    assert resp.status_code == 200
    assert alice.conversations() == []


def test_media_delete(alice: Mastodon) -> None:
    import io

    media = alice.media_post(io.BytesIO(b"\x00\x00\x00"), mime_type="image/png")
    resp = requests.delete(
        f"{alice.api_base_url}/api/v1/media/{media.id}",
        headers={"Authorization": "Bearer alice_token"},
        timeout=10,
    )
    assert resp.status_code == 200
    with pytest.raises(MastodonNotFoundError):
        alice.media(media)


def test_suggestion_dismiss_accepts(alice: Mastodon, bob: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id
    resp = requests.delete(
        f"{alice.api_base_url}/api/v1/suggestions/{bob_id}",
        headers={"Authorization": "Bearer alice_token"},
        timeout=10,
    )
    assert resp.status_code == 200


def test_health_endpoint(live_server: str) -> None:
    resp = requests.get(f"{live_server}/health", timeout=10)
    assert resp.status_code == 200
    assert resp.text == "OK"


def test_timelines_direct(alice: Mastodon, bob: Mastodon) -> None:
    status = alice.status_post("dm to bob", visibility="direct")
    resp = requests.get(
        f"{alice.api_base_url}/api/v1/timelines/direct",
        headers={"Authorization": "Bearer alice_token"},
        timeout=10,
    )
    assert resp.status_code == 200
    assert any(s["id"] == str(status.id) for s in resp.json())


def test_oembed_returns_minimal_shape(live_server: str) -> None:
    resp = requests.get(f"{live_server}/api/oembed", params={"url": "https://example.com/@alice/1"}, timeout=10)
    assert resp.status_code == 200
    body = resp.json()
    assert body["type"] == "link"
    assert body["provider_name"] == "mastodon_mock"


def test_account_identity_proofs_is_empty(alice: Mastodon) -> None:
    alice_id = alice.account_verify_credentials().id
    resp = requests.get(
        f"{alice.api_base_url}/api/v1/accounts/{alice_id}/identity_proofs",
        headers={"Authorization": "Bearer alice_token"},
        timeout=10,
    )
    assert resp.status_code == 200
    assert resp.json() == []


def test_account_endorsements_by_id(alice: Mastodon, bob: Mastodon) -> None:
    bob_id = bob.account_verify_credentials().id
    alice.account_pin(bob_id)
    alice_id = alice.account_verify_credentials().id
    resp = requests.get(
        f"{alice.api_base_url}/api/v1/accounts/{alice_id}/endorsements",
        headers={"Authorization": "Bearer alice_token"},
        timeout=10,
    )
    assert resp.status_code == 200
    assert any(a["id"] == str(bob_id) for a in resp.json())
