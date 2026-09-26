"""GET-only board load. Never signs in, creates tokens, or follows redirects."""
import os
from pathlib import Path
import random

from locust import HttpUser, constant, task
from requests.adapters import HTTPAdapter

PUBLIC = (
    "/api/v2/instance",
    "/api/v1/instance/rules",
    "/api/v1/custom_emojis",
    "/api/v1/instance/extended_description",
    "/.well-known/oauth-authorization-server",
    "/nodeinfo/2.0",
    "/api/mastomini/v1/version",
    "/api/mastomini/v1/status",
)
AUTHENTICATED = (
    "/api/v1/accounts/verify_credentials",
    "/api/v1/timelines/home?limit=20",
    "/api/v1/timelines/public?limit=20",
    "/api/v1/notifications?limit=20",
    "/api/v1/featured_tags",
)


class NamedHost(HTTPAdapter):
    def init_poolmanager(self, *args, **kwargs):
        name = os.environ.get("MASTOMINI_TLS_NAME", "mastomini.local")
        kwargs.update(server_hostname=name, assert_hostname=name)
        super().init_poolmanager(*args, **kwargs)


def configure(client):
    client.trust_env = False
    client.verify = os.environ.get("MASTOMINI_CA", str(Path(__file__).resolve().parents[1] / "certs/household-ca.crt"))
    client.mount("https://", NamedHost(max_retries=0))
    client.headers["Host"] = os.environ.get("MASTOMINI_TLS_NAME", "mastomini.local")
    token = os.environ.get("MASTOMINI_BENCH_TOKEN", "").strip()
    token_file = os.environ.get("MASTOMINI_BENCH_TOKEN_FILE")
    if token_file:
        token = Path(token_file).read_text(encoding="utf-8").strip()
    if token:
        client.headers["Authorization"] = "Bearer " + token
    original = client.request

    def read_only(method, url, **kwargs):
        if method.upper() != "GET" or url not in PUBLIC + AUTHENTICATED:
            raise ValueError("Load tests permit only allowlisted relative GETs")
        if any(kwargs.get(key) is not None for key in ("data", "json", "files")):
            raise ValueError("Load tests do not send request bodies")
        kwargs["allow_redirects"] = False
        kwargs["timeout"] = 10
        return original("GET", url, **kwargs)

    client.request = read_only
    return PUBLIC + AUTHENTICATED if token else PUBLIC


class ReadOnlyUser(HttpUser):
    wait_time = constant(0)

    def on_start(self):
        self.paths = configure(self.client)

    def on_stop(self):
        self.client.close()

    @task
    def read(self):
        path = random.choice(self.paths)
        with self.client.get(path, catch_response=True) as response:
            if response.status_code != 200:
                response.failure(f"Expected 200, received {response.status_code}")
                return
            try:
                if not isinstance(response.json(), (dict, list)):
                    response.failure("Expected a JSON object or array")
            except ValueError:
                response.failure("Invalid JSON response")
