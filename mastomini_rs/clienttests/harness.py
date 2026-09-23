"""Run the real mastomini desktop binary and sign in the way Mastodon apps do.

Sign-in uses the genuine OAuth code flow: the client registers an app,
"opens" /oauth/authorize, submits the HTML sign-in form, follows the redirect
to pick up the code, and exchanges it for a token. There is no test-only
shortcut on the server.
"""

from __future__ import annotations

import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from urllib.parse import parse_qs, urlparse

import requests

CRATE = Path(__file__).resolve().parent.parent
REDIRECT = "urn:ietf:wg:oauth:2.0:oob"
APP_REDIRECT = "mastomini-tests://oauth"


def binary() -> Path:
    exe = "mastomini.exe" if sys.platform == "win32" else "mastomini"
    target = Path(os.environ.get("CARGO_TARGET_DIR", CRATE / "target"))
    path = target / "debug" / exe
    if not path.exists():
        raise RuntimeError(f"{path} missing: run `cargo build` first (make smoke does)")
    return path


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class ServerProcess:
    """One desktop server with its own temporary store."""

    def __init__(self) -> None:
        self.dir = Path(tempfile.mkdtemp(prefix="mastomini-"))
        self.store = self.dir / "test.store"
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        self.proc: subprocess.Popen[bytes] | None = None
        self.log = self.dir / "server.log"

    def start(self) -> "ServerProcess":
        env = dict(
            os.environ,
            MASTOMINI_PORT=str(self.port),
            MASTOMINI_STORE=str(self.store),
            MASTOMINI_PASSWORD_ROUNDS="1000",
        )
        log = self.log.open("ab")
        self.proc = subprocess.Popen([str(binary())], env=env, stdout=log, stderr=log)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                raise RuntimeError(f"server exited early:\n{self.log.read_text()}")
            try:
                requests.get(f"{self.base}/api/v1/instance", timeout=1)
                return self
            except requests.ConnectionError:
                time.sleep(0.05)
        raise RuntimeError("server did not start")

    def stop(self) -> None:
        if self.proc is not None:
            self.proc.terminate()
            self.proc.wait(timeout=10)
            self.proc = None

    def restart(self) -> None:
        self.stop()
        self.start()

    def close(self) -> None:
        self.stop()
        shutil.rmtree(self.dir, ignore_errors=True)

    def url(self, path: str) -> str:
        return self.base + path

    def provision(self, username: str = "alice", password: str = "alicepw") -> None:
        r = requests.post(
            self.url("/api/mastomini/v1/provision"),
            data={"username": username, "password": password, "title": "Test Home"},
            timeout=10,
        )
        r.raise_for_status()

    def add_member(self, admin_token: str, username: str, password: str) -> None:
        r = requests.post(
            self.url("/api/mastomini/v1/admin/members"),
            data={"username": username, "password": password},
            headers={"Authorization": f"Bearer {admin_token}"},
            timeout=10,
        )
        r.raise_for_status()


def submit_sign_in(authorize_url: str, username: str, password: str) -> str:
    """Load the sign-in page, post the form, return the authorization code."""
    page = requests.get(authorize_url, timeout=10)
    page.raise_for_status()
    assert 'name="password"' in page.text, page.text
    query = parse_qs(urlparse(authorize_url).query)
    form = {k: v[0] for k, v in query.items()}
    form.update(username=username, password=password, decision="approve")
    base = authorize_url.split("/oauth/authorize")[0]
    r = requests.post(f"{base}/oauth/authorize", data=form, allow_redirects=False, timeout=10)
    if r.status_code == 302:
        return parse_qs(urlparse(r.headers["Location"]).query)["code"][0]
    # Out-of-band redirect: the code is shown on the page.
    assert r.status_code == 200, r.text
    return r.text.split('<code id="code">')[1].split("</code>")[0]
