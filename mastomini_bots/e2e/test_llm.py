"""The LLM bots end to end, with a real model: a real mastomini, a real
mastomini-bots, and OpenRouter.

Runs only when OPENROUTER_API_KEY is set (make e2e-llm loads it from the
file named by OPENROUTER_ENV_FILE without printing it). Each test makes one
or two small calls to a cheap model; the default is Gemma.
"""

from __future__ import annotations

import os

import pytest
import requests

from test_good_morning import Bots, mastomini, bots  # noqa: F401  (fixtures)
from conftest import sign_in  # noqa: E402

KEY = os.environ.get("OPENROUTER_API_KEY", "")
MODEL = os.environ.get("MASTOBOTS_TEST_MODEL", "google/gemma-4-31b-it")
pytestmark = pytest.mark.skipif(not KEY, reason="OPENROUTER_API_KEY not set")


def bot_account(mastomini, alice_token: str) -> str:
    """A member for the bot, and an API key made the way a person would."""
    mastomini.add_member(alice_token, "helper", "helperpw")
    helper = sign_in(mastomini.base, "helper", "helperpw")
    helper.account_update_credentials(note="A cheerful household helper who loves soup.", bot=True)
    r = requests.post(mastomini.base + "/api/mastomini/v1/me/api_keys", timeout=10,
                      headers={"Authorization": f"Bearer {helper.access_token}"},
                      data={"name": "LLM helper", "scopes": "read write", "password": "helperpw"})
    r.raise_for_status()
    return r.json()["key"]


def setup(bots, mastomini, bot_id: str, key: str, settings: dict) -> None:
    bots.token = bots.call("POST", "/api/v1/setup", json={"password": "admin password"}).json()["token"]
    r = bots.call("PUT", "/api/v1/integrations/openrouter", json={"key": KEY, "model": MODEL})
    assert r.status_code == 200 and KEY not in r.text
    r = bots.call("PUT", f"/api/v1/bots/{bot_id}",
                  json={"instance": mastomini.base, "token": key, "enabled": True, "settings": settings})
    assert r.status_code == 200, r.text


def run_once(bots, bot_id: str) -> dict:
    before = bots.call("GET", f"/api/v1/bots/{bot_id}").json()["runs"]
    assert bots.call("POST", f"/api/v1/bots/{bot_id}/run").status_code == 202
    bot = bots.wait(lambda: (b := bots.call("GET", f"/api/v1/bots/{bot_id}").json())["runs"] > before and b,
                    "run", seconds=120)
    return bot["last_run"]


def test_check_reaches_openrouter(mastomini, bots):
    alice = sign_in(mastomini.base, "alice", "alicepw")
    key = bot_account(mastomini, alice.access_token)
    setup(bots, mastomini, "llm_post", key, {"run": "manual"})
    assert bots.call("POST", "/api/v1/bots/llm_post/check").status_code == 202
    check = bots.wait(lambda: bots.call("GET", "/api/v1/bots/llm_post").json()["last_check"], "check", 120)
    assert check["ok"], check
    assert check["summary"] == f"The API key works: @helper on {mastomini.base}; OpenRouter answers ({MODEL})"


def test_the_bot_answers_a_mention_in_the_thread(mastomini, bots):
    alice = sign_in(mastomini.base, "alice", "alicepw")
    key = bot_account(mastomini, alice.access_token)
    setup(bots, mastomini, "llm_reply", key, {"run": "manual", "max_tokens": "200"})

    # The first run starts from now: older mentions are not answered.
    old = alice.status_post("@helper this is from before the bot started")
    first = run_once(bots, "llm_reply")
    assert first["summary"] == "Watching for mentions from now on", first

    question = alice.status_post("@helper what soup should we make tonight? Answer in one sentence.",
                                 visibility="unlisted")
    run = run_once(bots, "llm_reply")
    assert run["ok"], run
    assert run["summary"] == "Answered 1 mention", run

    replies = alice.status_context(question["id"])["descendants"]
    assert len(replies) == 1, replies
    reply = replies[0]
    assert reply["account"]["acct"] == "helper"
    assert reply["in_reply_to_id"] == question["id"]
    assert reply["visibility"] == "unlisted"  # no more public than the question
    assert reply["content"].startswith("<p>@alice ") or "@alice" in reply["content"]
    assert reply["application"]["name"] == "LLM helper"
    text = reply["content"]
    print(f"\nModel reply ({MODEL}): {ascii(text)}")
    assert alice.status_context(old["id"])["descendants"] == []

    # Nothing new: nothing answered, nothing spent.
    again = run_once(bots, "llm_reply")
    assert again["summary"] == "No new mentions", again


def test_the_bot_writes_a_post_within_the_limit(mastomini, bots):
    alice = sign_in(mastomini.base, "alice", "alicepw")
    key = bot_account(mastomini, alice.access_token)
    setup(bots, mastomini, "llm_post", key, {"run": "manual", "visibility": "unlisted"})
    run = run_once(bots, "llm_post")
    assert run["ok"], run
    post = requests.get(run["summary"].removeprefix("Posted "),
                        timeout=10)
    assert post.status_code == 200
    helper_posts = alice.account_statuses(alice.account_lookup("helper")["id"])
    assert len(helper_posts) == 1
    text = helper_posts[0]["content"]
    print(f"\nModel post ({MODEL}): {ascii(text)}")
    # mastomini's limit is 140 characters; the bot fits its text to it.
    plain = text.removeprefix("<p>").removesuffix("</p>")
    assert len(plain) <= 200  # HTML escaping can lengthen the text a little
    assert helper_posts[0]["visibility"] == "unlisted"
