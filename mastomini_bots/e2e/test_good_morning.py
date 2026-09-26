"""End to end on the desktop: a real mastomini server, a real mastomini-bots
server, the admin API, and the good_morning bot posting with an API key.

Run with mastomini's client-test environment (make e2e does):
    uv run --project ../mastomini_rs/clienttests pytest e2e -q
"""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import pytest
import requests

CRATE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(CRATE.parent / "mastomini_rs" / "clienttests"))

from conftest import sign_in  # noqa: E402
from harness import ServerProcess, free_port  # noqa: E402


class Bots:
    def __init__(self) -> None:
        self.dir = Path(tempfile.mkdtemp(prefix="mastobots-"))
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        exe = CRATE / "target" / "debug" / ("mastomini-bots.exe" if sys.platform == "win32" else "mastomini-bots")
        env = dict(os.environ, MASTOBOTS_PORT=str(self.port), MASTOBOTS_STORE=str(self.dir / "bots.json"))
        self.log = (self.dir / "bots.log").open("ab")
        self.proc = subprocess.Popen([str(exe)], env=env, stdout=self.log, stderr=self.log)
        for _ in range(300):
            try:
                requests.get(self.base + "/api/v1/status", timeout=1)
                break
            except requests.ConnectionError:
                time.sleep(0.05)
        self.token = ""

    def call(self, method: str, path: str, **kw) -> requests.Response:
        headers = {"Authorization": f"Bearer {self.token}"} if self.token else {}
        return requests.request(method, self.base + path, headers=headers, timeout=10, **kw)

    def wait(self, check, what: str, seconds: float = 20):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            value = check()
            if value:
                return value
            time.sleep(0.2)
        raise AssertionError(f"timed out waiting for {what}")

    def close(self) -> None:
        self.proc.terminate()
        self.proc.wait(timeout=10)


@pytest.fixture
def mastomini():
    s = ServerProcess().start()
    try:
        s.provision("alice", "alicepw")
        yield s
    finally:
        s.close()


@pytest.fixture
def bots():
    b = Bots()
    try:
        yield b
    finally:
        b.close()


def test_good_morning_posts_through_the_admin_site(mastomini, bots):
    alice = sign_in(mastomini.base, "alice", "alicepw")
    r = requests.post(mastomini.base + "/api/mastomini/v1/me/api_keys", timeout=10,
                      headers={"Authorization": f"Bearer {alice.access_token}"},
                      data={"name": "Good morning bot", "scopes": "read write", "password": "alicepw"})
    r.raise_for_status()
    key = r.json()["key"]

    # First visit: set the admin password.
    assert bots.call("GET", "/api/v1/status").json()["setup_needed"] is True
    bots.token = bots.call("POST", "/api/v1/setup", json={"password": "admin password"}).json()["token"]

    r = bots.call("PUT", "/api/v1/bots/good_morning",
                  json={"instance": mastomini.base, "token": key, "enabled": True})
    assert r.status_code == 200, r.text
    assert key not in r.text
    assert r.json()["next_run_at"]

    assert bots.call("POST", "/api/v1/bots/good_morning/check").status_code == 202
    check = bots.wait(lambda: bots.call("GET", "/api/v1/bots/good_morning").json()["last_check"], "check")
    assert check["ok"], check
    assert check["summary"] == f"The API key works: @alice on {mastomini.base}"

    assert bots.call("POST", "/api/v1/bots/good_morning/run").status_code == 202
    run = bots.wait(lambda: bots.call("GET", "/api/v1/bots/good_morning").json()["last_run"], "run")
    assert run["ok"], run

    post = alice.account_statuses(alice.me()["id"])[0]
    assert post["content"].startswith("<p>Good morning, it is ")
    assert post["application"]["name"] == "Good morning bot"
    assert run["summary"] == f"Posted {post['url']}"
    texts = [e["text"] for e in bots.call("GET", "/api/v1/activity").json()]
    assert "Run requested" in texts


def test_a_bad_key_is_reported_not_retried_forever(mastomini, bots):
    bots.token = bots.call("POST", "/api/v1/setup", json={"password": "admin password"}).json()["token"]
    bots.call("PUT", "/api/v1/bots/good_morning", json={"instance": mastomini.base, "token": "wrong"})
    bots.call("POST", "/api/v1/bots/good_morning/check")
    check = bots.wait(lambda: bots.call("GET", "/api/v1/bots/good_morning").json()["last_check"], "check")
    assert not check["ok"]
    assert check["summary"].startswith("HTTP 401")
