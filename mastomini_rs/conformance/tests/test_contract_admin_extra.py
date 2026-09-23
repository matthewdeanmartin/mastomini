import httpx2 as httpx
import pytest
from mastodon import Mastodon
from mastodon.errors import MastodonNotFoundError


@pytest.fixture()
def admin_client(live_server: str) -> Mastodon:
    return Mastodon(access_token="alice_token", api_base_url=live_server)


def test_admin_account_lookup_404(admin_client: Mastodon) -> None:
    with pytest.raises(MastodonNotFoundError):
        admin_client.admin_account(123456789)


def test_admin_accounts_filtering(live_server: str) -> None:
    # Test origin=remote via httpx since Mastodon.py might not support all params
    with httpx.Client(base_url=live_server, headers={"Authorization": "Bearer alice_token"}) as client:
        resp = client.get("/api/v2/admin/accounts", params={"origin": "remote"})
        resp.raise_for_status()
        remote = resp.json()
        # Dave is remote in TEST_SEED
        assert any(a["username"] == "dave" for a in remote)
        assert all(a["domain"] is not None for a in remote)

        # Test status=active
        resp = client.get("/api/v2/admin/accounts", params={"status": "active"})
        resp.raise_for_status()
        active = resp.json()
        assert any(a["username"] == "alice" for a in active)


def test_admin_account_action_404(live_server: str) -> None:
    with httpx.Client(base_url=live_server, headers={"Authorization": "Bearer alice_token"}) as client:
        resp = client.post("/api/v1/admin/accounts/123456789/action", json={"type": "none"})
        assert resp.status_code == 404


# --- Admin announcements (mock management surface) ----------------------------
# Mastodon.py has no admin-announcement methods, so these drive the raw endpoints.


def test_admin_announcement_create_publish_delete_cycle(live_server: str) -> None:
    with httpx.Client(base_url=live_server, headers={"Authorization": "Bearer alice_token"}) as client:
        # Create as a draft: it must not appear on the public endpoint yet.
        resp = client.post("/api/v1/admin/announcements", json={"text": "Heads up!", "published": False})
        resp.raise_for_status()
        created = resp.json()
        assert created["published"] is False
        ann_id = created["id"]

        public = client.get("/api/v1/announcements").json()
        assert ann_id not in {a["id"] for a in public}

        # Admin list shows drafts; the public list does not.
        admin_list = client.get("/api/v1/admin/announcements").json()
        assert ann_id in {a["id"] for a in admin_list}

        # Publishing makes it visible publicly.
        resp = client.post(f"/api/v1/admin/announcements/{ann_id}/publish")
        resp.raise_for_status()
        assert resp.json()["published"] is True
        public = client.get("/api/v1/announcements").json()
        assert ann_id in {a["id"] for a in public}

        # Unpublish hides it again.
        client.post(f"/api/v1/admin/announcements/{ann_id}/unpublish").raise_for_status()
        public = client.get("/api/v1/announcements").json()
        assert ann_id not in {a["id"] for a in public}

        # Delete removes it from the admin list.
        client.delete(f"/api/v1/admin/announcements/{ann_id}").raise_for_status()
        admin_list = client.get("/api/v1/admin/announcements").json()
        assert ann_id not in {a["id"] for a in admin_list}


def test_admin_announcement_create_requires_text(live_server: str) -> None:
    with httpx.Client(base_url=live_server, headers={"Authorization": "Bearer alice_token"}) as client:
        resp = client.post("/api/v1/admin/announcements", json={"text": "   "})
        assert resp.status_code == 422


def test_admin_announcement_delete_404(live_server: str) -> None:
    with httpx.Client(base_url=live_server, headers={"Authorization": "Bearer alice_token"}) as client:
        resp = client.delete("/api/v1/admin/announcements/999999999")
        assert resp.status_code == 404
