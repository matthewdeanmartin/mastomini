"""HTTPS next to HTTP ("Easy mode", docs/security/https.md), end to end.

Certificates come from the real scripts/certs.sh (into a temporary directory,
never certs/), and the real desktop binary serves both listeners. Clients
verify strictly against the household CA, as a device that installed it from
/trust does. Needs bash and openssl on PATH (Git for Windows has both).
"""

from __future__ import annotations

import hashlib
import os
import shutil
import socket
import ssl
import subprocess
import tempfile
from collections.abc import Iterator
from pathlib import Path

import pytest
import requests
from mastodon import Mastodon

from harness import CRATE, REDIRECT, ServerProcess, free_port, submit_sign_in

SCOPES = ["read", "write"]

# The full path: on Windows a bare "bash" finds WSL's in System32 before
# Git Bash on PATH, and WSL does not pass the environment through.
BASH = shutil.which("bash") or "bash"

pytestmark = pytest.mark.skipif(
    not (shutil.which("bash") and shutil.which("openssl")), reason="needs bash and openssl"
)


class Certs:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.dir = root / "certs"
        self.ca = self.dir / "household-ca.crt"
        self.ca_der = (self.dir / "household-ca.der").read_bytes()


@pytest.fixture(scope="module")
def certs() -> Iterator[Certs]:
    root = Path(tempfile.mkdtemp(prefix="mastomini-certs-"))
    env = dict(
        os.environ,
        MASTOMINI_CA_DIR=str(root / "ca"),
        MASTOMINI_CERT_DIR=str(root / "certs"),
        # Not whatever a developer's .env says.
        MASTOMINI_HOSTNAME="mastomini",
        MASTOMINI_CERT_NAMES="",
        MASTOMINI_CERT_IPS="",
    )
    subprocess.run(
        [BASH, "scripts/certs.sh"], cwd=CRATE, env=env, check=True, capture_output=True
    )
    try:
        yield Certs(root)
    finally:
        shutil.rmtree(root, ignore_errors=True)


@pytest.fixture(scope="module")
def dual(certs: Certs) -> Iterator[tuple[ServerProcess, str]]:
    port = free_port()
    server = ServerProcess(
        {"MASTOMINI_HTTPS_PORT": str(port), "MASTOMINI_TLS_DIR": str(certs.dir)}
    ).start()
    try:
        server.provision("alice", "alicepw")
        yield server, f"https://localhost:{port}"
    finally:
        server.close()


def trusting(certs: Certs) -> requests.Session:
    """A device that installed the household CA (and trusts nothing else)."""
    http = requests.Session()
    http.verify = str(certs.ca)
    return http


def test_benchmark_ip_connection_keeps_hostname_verification(certs: Certs, dual: tuple[ServerProcess, str]) -> None:
    from test_audit_tools import load

    _, https = dual
    tool = load("bench-api")
    ip = https.replace("localhost", "127.0.0.1")
    rows = tool.batch(ip, "/api/v2/instance", 1, 2, "reuse", "", str(certs.ca), 5, "mastomini.local")
    assert all(row["status"] == 200 for row in rows)
    assert all(set(row["server_timing_ms"]) == {"app", "body", "lock"} for row in rows)
    rows = tool.batch(ip, "/api/v2/instance", 1, 2, "reuse", "", str(certs.ca), 5, "wrong.invalid")
    assert all(row["status"] == "SSLError" for row in rows)


def test_the_scripts_certificate_passes_its_own_check(certs: Certs) -> None:
    env = dict(os.environ, MASTOMINI_CERT_DIR=str(certs.dir), MASTOMINI_CERT_HOST="mastomini.local")
    r = subprocess.run(
        [BASH, "scripts/certs-check.sh"], cwd=CRATE, env=env, capture_output=True, text=True
    )
    assert r.returncode == 0, r.stderr
    days = int(r.stdout.split(" days left")[0].rsplit(" ", 1)[1])
    # 820 days in all, started a day early so no device sees tomorrow's date.
    assert 817 <= days <= 819, r.stdout


def test_https_verifies_against_the_household_ca_only(
    certs: Certs, dual: tuple[ServerProcess, str]
) -> None:
    _, https = dual
    status = trusting(certs).get(f"{https}/api/mastomini/v1/status", timeout=10).json()
    assert status["https"] is True
    assert status["secure"] is True
    assert status["mode"] == "easy"
    # By IP too: the certificate names 127.0.0.1.
    ip = https.replace("localhost", "127.0.0.1")
    assert trusting(certs).get(f"{ip}/trust", timeout=10).status_code == 200
    # A device that has not installed the CA refuses the connection.
    with pytest.raises(requests.exceptions.SSLError):
        requests.get(f"{https}/api/v1/instance", timeout=10)


def test_the_certificate_is_only_for_its_own_names(
    certs: Certs, dual: tuple[ServerProcess, str]
) -> None:
    _, https = dual
    port = int(https.rsplit(":", 1)[1])
    context = ssl.create_default_context(cafile=str(certs.ca))
    with socket.create_connection(("127.0.0.1", port), timeout=10) as raw:
        with pytest.raises(ssl.SSLCertVerificationError):
            context.wrap_socket(raw, server_hostname="bank.example.com")


def test_ca_and_trust_page_over_plain_http(certs: Certs, dual: tuple[ServerProcess, str]) -> None:
    server, https = dual
    r = requests.get(server.url("/ca"), timeout=10)
    assert r.status_code == 200
    assert r.content == certs.ca_der
    assert r.headers["Content-Type"] == "application/x-x509-ca-cert"
    page = requests.get(server.url("/trust"), timeout=10).text
    fingerprint = ":".join(f"{b:02X}" for b in hashlib.sha256(certs.ca_der).digest())
    assert fingerprint in page
    assert https in page
    pem = requests.get(server.url("/ca.pem"), timeout=10).text
    assert ssl.PEM_cert_to_DER_cert(pem) == certs.ca_der


def test_a_mastodon_app_signs_in_over_https_and_sees_the_same_household(
    certs: Certs, dual: tuple[ServerProcess, str]
) -> None:
    server, https = dual
    http = trusting(certs)
    client_id, client_secret = Mastodon.create_app(
        "mastomini-tests-https",
        api_base_url=https,
        redirect_uris=REDIRECT,
        scopes=SCOPES,
        session=http,
    )
    client = Mastodon(
        client_id=client_id, client_secret=client_secret, api_base_url=https, session=http
    )
    # No allow_http: this is the path real apps take.
    url = client.auth_request_url(redirect_uris=REDIRECT, scopes=SCOPES)
    code = submit_sign_in(url, "alice", "alicepw", http)
    client.log_in(code=code, redirect_uri=REDIRECT, scopes=SCOPES)
    post = client.status_post("Posted over HTTPS")

    # Easy mode: the same token and data over plain HTTP.
    plain = Mastodon(access_token=client.access_token, api_base_url=server.base)
    assert plain.status(post.id).content == post.content
