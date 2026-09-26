"""Safety guard tests run without contacting a board."""
from unittest.mock import Mock

import pytest
from locustfile import PUBLIC, configure


def client():
    fake = Mock()
    fake.headers = {}
    original = fake.request
    configure(fake)
    return fake, original


@pytest.mark.parametrize("method,url,kwargs", [
    ("POST", PUBLIC[0], {}), ("DELETE", PUBLIC[0], {}),
    ("GET", "/api/v1/statuses", {}),
    ("GET", "https://other.example/api/v2/instance", {}),
    ("GET", PUBLIC[0], {"json": {}}),
    ("GET", PUBLIC[0], {"data": b""}),
])
def test_rejects_writes_and_unlisted_destinations(method, url, kwargs):
    fake, original = client()
    with pytest.raises(ValueError):
        fake.request(method, url, **kwargs)
    original.assert_not_called()


def test_get_has_no_redirects_and_a_bounded_timeout():
    fake, original = client()
    fake.request("GET", PUBLIC[0], data=None, json=None, allow_redirects=True)
    original.assert_called_once_with("GET", PUBLIC[0], data=None, json=None,
                                     allow_redirects=False, timeout=10)
