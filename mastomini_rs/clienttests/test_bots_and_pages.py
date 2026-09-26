"""A bot posting with an API key, and a browser following a post's `url`,
against the real desktop binary."""

from __future__ import annotations

import requests
from mastodon import Mastodon

KEYS = "/api/mastomini/v1/me/api_keys"


def test_a_bot_posts_with_an_api_key(household):
    base = household.server.base
    auth = {"Authorization": f"Bearer {household.alice.access_token}"}
    r = requests.post(base + KEYS, headers=auth, timeout=5,
                      data={"name": "Weather bot", "scopes": "read write", "password": "alicepw"})
    assert r.status_code == 200, r.text
    assert r.headers["Cache-Control"] == "no-store"
    key = r.json()["key"]

    bot = Mastodon(access_token=key, api_base_url=base)
    status = bot.status_post("Sunny, 21°")
    assert status["account"]["username"] == "alice"
    assert status["application"]["name"] == "Weather bot"
    # Everyone else sees it like any post of alice's.
    assert household.bob.status(status["id"])["content"] == "<p>Sunny, 21°</p>"

    # Revoked from the account page: the bot is locked out.
    requests.delete(f"{base}{KEYS}/{r.json()['id']}", headers=auth, timeout=5).raise_for_status()
    rejected = requests.get(base + "/api/v1/accounts/verify_credentials",
                            headers={"Authorization": f"Bearer {key}"}, timeout=5)
    assert rejected.status_code == 401


def test_open_in_original_site_with_a_browser_sign_in(household):
    base = household.server.base
    private = household.alice.status_post("for followers", visibility="private")
    household.bob.account_follow(household.alice.me()["id"])

    browser = requests.Session()
    page = browser.get(private["url"], timeout=5)
    assert page.status_code == 404
    signin = browser.post(base + "/web/signin", timeout=5, allow_redirects=False, data={
        "username": "bob", "password": "bobpw", "return": private["url"].split(base, 1)[1]})
    assert signin.status_code == 302, signin.text
    assert "mm_session" in browser.cookies
    page = browser.get(private["url"], timeout=5)
    assert page.status_code == 200
    assert "for followers" in page.text
    assert "Signed in as" in page.text
    # The profile page lists it too, and signing out hides it again.
    assert "for followers" in browser.get(private["account"]["url"], timeout=5).text
    browser.post(base + "/web/signout", timeout=5)
    assert browser.get(private["url"], timeout=5).status_code == 404
