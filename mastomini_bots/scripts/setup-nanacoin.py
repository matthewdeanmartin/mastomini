# /// script
# requires-python = ">=3.10"
# dependencies = ["httpx2==2.0.0"]
# ///
"""Create and connect NanaCoin bots through the same worker as the admin wizard.

Run with uv run --no-build scripts/setup-nanacoin.py. No passwords or keys
are accepted as command-line arguments or written to the plan file.
"""
from __future__ import annotations

import argparse
import getpass
import json
import ssl
import time
from pathlib import Path

import httpx2


def default_plan() -> dict:
    strategies = ["band", "lotto", "borrow", "lend", "dca", "rebalance"]
    return {
        "nanacoin": "https://nanacoin.local",
        "mastomini": "https://mastomini.local",
        "bots": [
            {"id": bot, "username": bot, "display_name": "Nana news" if i == 0 else f"Trader {i}",
             "strategy": "" if i == 0 else strategies[i - 1], "initial_grant": False}
            for i, bot in enumerate(["nana_news"] + [f"trader_{n}" for n in range(1, 7)])
        ],
    }


def run(client, plan: dict, *, nana: str, admin: str, replace_read: bool, yes: bool) -> None:
    print(f"NanaCoin: {plan['nanacoin']}\nMastomini: {plan['mastomini']}")
    for row in plan["bots"]:
        print(f"  {row['id']}: {row['username']}, {row['strategy'] or 'news'}, "
              f"initial grant {'yes' if row['initial_grant'] else 'no'}")
    if not yes and input("Create and connect these accounts? [y/N] ").strip().lower() != "y":
        return
    credentials = {
        **plan,
        "nana_username": nana,
        "nana_password": getpass.getpass("Nana password: "),
        "admin_username": admin,
        "admin_password": getpass.getpass("Mastomini admin password: "),
        "replace_news_read_key": replace_read,
    }
    try:
        response = client.post("/api/v1/integrations/nanacoin/setup", json=credentials)
    finally:
        credentials.clear()
    response.raise_for_status()
    previous = None
    deadline = time.monotonic() + 600
    while True:
        result = response.json()
        summary = [(r["username"], r["state"]) for r in result["rows"]]
        if summary != previous:
            for username, state in summary:
                print(f"{username}: {state}")
            previous = summary
        if not result["running"]:
            if result["error"]:
                raise SystemExit(f"Setup paused: {result['error']}\nRun this command again to resume.")
            print("Connected. New bots remain off; traders remain in dry-run.")
            return
        if time.monotonic() > deadline:
            raise SystemExit("Setup is still running. Check the admin wizard before submitting again.")
        time.sleep(1)
        response = client.get("/api/v1/integrations/nanacoin/setup")
        response.raise_for_status()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:8090")
    parser.add_argument("--ca", type=Path, help="Household CA PEM when it is not trusted by this computer")
    parser.add_argument("--plan", type=Path, help="Nonsecret server and bot selections as JSON")
    parser.add_argument("--nana", default="nana")
    parser.add_argument("--admin", help="Mastomini admin username")
    parser.add_argument("--replace-news-read-key", action="store_true")
    parser.add_argument("--yes", action="store_true", help="Accept the displayed account plan")
    args = parser.parse_args()
    from urllib.parse import urlsplit
    parsed = urlsplit(args.url)
    if parsed.scheme != "https" and not (parsed.scheme == "http" and parsed.hostname in {"localhost", "127.0.0.1", "::1"}):
        parser.error("Use HTTPS, or HTTP on localhost")
    verify = ssl.create_default_context(cafile=str(args.ca)) if args.ca else True
    with httpx2.Client(base_url=args.url.rstrip("/"), verify=verify, timeout=10, trust_env=False) as client:
        response = client.post("/api/v1/login", json={"password": getpass.getpass("Bots admin password: ")})
        response.raise_for_status()
        client.headers["Authorization"] = f"Bearer {response.json()['token']}"
        try:
            response = client.get("/api/v1/integrations/nanacoin/setup")
            response.raise_for_status()
            if response.json()["running"]:
                raise SystemExit("Setup is already running; follow its progress in the admin wizard.")
            plan = json.loads(args.plan.read_text()) if args.plan else response.json().get("plan") or default_plan()
            if set(plan) != {"nanacoin", "mastomini", "bots"}:
                raise SystemExit("Plans must contain only nanacoin, mastomini and bots; never credentials.")
            run(client, plan, nana=args.nana, admin=args.admin or input("Mastomini admin username: "),
                replace_read=args.replace_news_read_key, yes=args.yes)
        finally:
            client.post("/api/v1/logout")


if __name__ == "__main__":
    main()
