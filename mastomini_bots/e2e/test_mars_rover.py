"""Mars commands and authoritative checkpoints through real Mastomini APIs."""
from datetime import datetime, timezone

import requests

from test_good_morning import mastomini, bots  # noqa: F401
from conftest import sign_in


def run_once(bots):
    before = bots.call("GET", "/api/v1/bots/mars_rover").json()["runs"]
    assert bots.call("POST", "/api/v1/bots/mars_rover/run").status_code == 202
    bot = bots.wait(
        lambda: (b := bots.call("GET", "/api/v1/bots/mars_rover").json())["runs"] > before and b,
        "Mars command",
    )
    assert bot["last_run"]["ok"], bot["last_run"]
    return bot["last_run"]["summary"]


def checkpoint(text):
    # Read the compact state from Mastomini's HTML rendering.
    return text.split("MR1:")[1].split("<")[0].split(",")


def test_mars_rover_thread_commands_and_duplicate_polls(mastomini, bots):
    alice = sign_in(mastomini.base, "alice", "alicepw")
    mastomini.add_member(alice.access_token, "mars", "marspw")
    mars = sign_in(mastomini.base, "mars", "marspw")
    mars.account_update_credentials(bot=True)
    key_response = requests.post(
        mastomini.base + "/api/mastomini/v1/me/api_keys", timeout=10,
        headers={"Authorization": f"Bearer {mars.access_token}"},
        data={"name": "Mars rover", "scopes": "read write", "password": "marspw"},
    )
    key_response.raise_for_status()
    bots.token = bots.call("POST", "/api/v1/setup", json={"password": "admin password"}).json()["token"]
    response = bots.call("PUT", "/api/v1/bots/mars_rover", json={
        "instance": mastomini.base, "token": key_response.json()["key"], "enabled": True,
        "settings": {"run": "manual", "zone": "utc", "epoch": datetime.now(timezone.utc).date().isoformat()},
    })
    assert response.status_code == 200, response.text
    assert "landed" in run_once(bots)
    root = mars.account_statuses(mars.me()["id"])[0]
    assert "Mars landing" in root["content"]

    # Copying the state into somebody else's thread does not authorize it.
    fake = alice.status_post("Copied MR1:42", visibility="public")
    alice.status_post("@mars scan", in_reply_to_id=fake["id"])
    assert run_once(bots) == "Mars: 0 commands"

    alice.status_post("@mars scan", in_reply_to_id=root["id"])
    assert run_once(bots) == "Mars: 1 commands"
    log = mars.account_statuses(mars.me()["id"])[0]
    assert "@alice" in log["content"] and "basalt" in log["content"]
    assert int(checkpoint(log["content"])[4], 36) == 92
    assert log["in_reply_to_id"] is not None
    assert run_once(bots) == "Mars: 0 commands"

    alice.status_post("@mars sample", in_reply_to_id=log["id"])
    assert run_once(bots) == "Mars: 1 commands"
    log = mars.account_statuses(mars.me()["id"])[0]
    assert int(checkpoint(log["content"])[4], 36) == 77
    assert int(checkpoint(log["content"])[6], 36) == 1 << 27

    alice.status_post("@mars name discovery Kevin", in_reply_to_id=log["id"])
    assert run_once(bots) == "Mars: 1 commands"
    log = mars.account_statuses(mars.me()["id"])[0]
    assert checkpoint(log["content"])[11] == "Kevin"
    descendants = alice.status_context(root["id"])["descendants"]
    assert any("Kevin" in p["content"] for p in descendants)
